use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use crate::entity::projectile::splash_potion::extinguish_fire_if_water_potion;
use crate::{
    entity::{Entity, EntityBase, projectile::ThrownItemEntity},
    server::Server,
};
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::java::client::play::CWorldEvent;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::{Vector2, to_chunk_pos};
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

const GRAVITY: f64 = 0.05;

pub struct LingeringPotionEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
}

impl LingeringPotionEntity {
    pub fn new(entity: Entity) -> Self {
        entity.set_velocity(Vector3::new(0.0, 0.1, 0.0));
        let thrown = ThrownItemEntity {
            entity,
            owner_id: None,
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: GRAVITY,
        };

        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(
                1,
                &pumpkin_data::item::Item::LINGERING_POTION,
            )),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        thrown.entity.set_velocity(Vector3::new(0.0, 0.1, 0.0));
        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(
                1,
                &pumpkin_data::item::Item::LINGERING_POTION,
            )),
        }
    }

    pub fn set_item_stack(&self, item_stack: ItemStack) {
        let mut write = self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *write = item_stack;
    }
}

impl EntityBase for LingeringPotionEntity {
    fn init_data_tracker(&self) {
        let entity = self.get_entity();
        let stack = self
            .item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Sync the item stack so the client renders the correct potion type
        entity.send_meta_data(
            &[pumpkin_protocol::java::client::play::Metadata::new(
                pumpkin_data::tracked_data::lingering_potion::ITEM_STACK,
                &pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer::from(
                    stack.clone(),
                ),
            )],
            None,
        );
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.thrown.process_tick(caller);
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: crate::entity::projectile::ProjectileHit) {
        let world = self.get_entity().world.load();
        let hit_pos = hit.hit_pos();

        // Read stored item stack and compute potion effects
        let stack = self
            .item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        // Play impact particles
        world.send_entity_status(
            self.get_entity(),
            EntityStatus::Death,
            Some(ActorEventID::Death),
        );

        // `AbstractThrownPotion.onHit` (`AbstractThrownPotion.java:70-85`) runs the water
        // branch for lingering potions too, including entity effects before potion-cloud setup.
        let owner = self
            .thrown
            .owner_id
            .and_then(|id| world.get_entity_by_id(id));
        extinguish_fire_if_water_potion(&world, hit_pos, &stack);
        crate::entity::projectile::splash_potion::apply_water_potion_entity_effects(
            self,
            owner.as_deref(),
            &stack,
        );

        let effects = crate::item::potion::PotionContents::read_potion_effects(&stack);

        // If no effects, just splash (like water bottles); the water branch above has already
        // extinguished fire and applied water-sensitive entity effects.
        if effects.is_empty() {
            return;
        }

        // Play splash/break particles & sound. `AbstractThrownPotion.onHit`
        // (`AbstractThrownPotion.java:81-82`) emits the level event unconditionally, before
        // the effect branch, so a plain water bottle still splashes.
        let mut color = 0x385dc6; // default water-like color
        if let Some(pc) =
            stack.get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
        {
            if let Some(c) = pc.custom_color {
                color = c;
            } else if !effects.is_empty() {
                let mut r_sum = 0.0;
                let mut g_sum = 0.0;
                let mut b_sum = 0.0;
                let count = effects.len() as f32;
                for (eff, _, _, _, _, _) in &effects {
                    let c = eff.color;
                    r_sum += ((c >> 16) & 0xFF) as f32;
                    g_sum += ((c >> 8) & 0xFF) as f32;
                    b_sum += (c & 0xFF) as f32;
                }
                let r = (r_sum / count) as i32;
                let g = (g_sum / count) as i32;
                let b = (b_sum / count) as i32;
                color = (r << 16) | (g << 8) | b;
            }
        } else if !effects.is_empty() {
            let mut r_sum = 0.0;
            let mut g_sum = 0.0;
            let mut b_sum = 0.0;
            let count = effects.len() as f32;
            for (eff, _, _, _, _, _) in &effects {
                let c = eff.color;
                r_sum += ((c >> 16) & 0xFF) as f32;
                g_sum += ((c >> 8) & 0xFF) as f32;
                b_sum += (c & 0xFF) as f32;
            }
            let r = (r_sum / count) as i32;
            let g = (g_sum / count) as i32;
            let b = (b_sum / count) as i32;
            color = (r << 16) | (g << 8) | b;
        }

        let has_instant = effects.iter().any(|(e, _, _, _, _, _)| {
            e.id == pumpkin_data::effect::StatusEffect::INSTANT_DAMAGE.id
                || e.id == pumpkin_data::effect::StatusEffect::INSTANT_HEALTH.id
        });
        let event_id = if has_instant { 2007 } else { 2002 };
        let block_pos = BlockPos(Vector3::new(
            hit_pos.x.floor() as i32,
            hit_pos.y.floor() as i32,
            hit_pos.z.floor() as i32,
        ));
        let chunk_pos = to_chunk_pos(&Vector2::new(block_pos.0.x, block_pos.0.z));
        world.broadcast_to_chunk(
            chunk_pos,
            &CWorldEvent::new(event_id, block_pos, color, false),
        );

        // `ThrownLingeringPotion.onHitAsPotion` is only reached for a potion that actually
        // has effects (`AbstractThrownPotion.java:77-79`), so a water bottle leaves no cloud.
        let cloud_entity = crate::entity::Entity::from_uuid(
            Uuid::new_v4(),
            world.clone(),
            hit_pos,
            &pumpkin_data::entity::EntityType::AREA_EFFECT_CLOUD,
        );
        let cloud = crate::entity::area_effect_cloud::AreaEffectCloudEntity::create(
            cloud_entity,
            stack,
            effects,
            600,
            3.0,
            20,
            10,
            -0.5,
            0,
        );

        world.spawn_entity(cloud);
    }
}
