use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::entity::projectile::splash_potion::extinguish_fire_if_water_potion;
use crate::{
    entity::{Entity, EntityBase, EntityBaseFuture, NBTStorage, projectile::ThrownItemEntity},
    server::Server,
};
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_protocol::java::client::play::CWorldEvent;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::{Vector2, to_chunk_pos};
use pumpkin_util::math::vector3::Vector3;
use tokio::sync::RwLock;
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

    pub async fn set_item_stack(&self, item_stack: ItemStack) {
        let mut write = self.item_stack.write().await;
        *write = item_stack;
    }
}

impl NBTStorage for LingeringPotionEntity {}

impl EntityBase for LingeringPotionEntity {
    fn init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            let stack = self.item_stack.read().await;

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
        })
    }

    fn tick<'a>(
        &'a self,
        caller: &'a Arc<dyn EntityBase>,
        server: &'a Server,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move { self.thrown.process_tick(caller, server).await })
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: crate::entity::projectile::ProjectileHit) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let world = self.get_entity().world.load();
            let hit_pos = hit.hit_pos();

            // Only extinguish fire for plain water potions
            let stack = self.item_stack.read().await.clone();
            extinguish_fire_if_water_potion(&world, hit_pos, &stack).await;
            // `AbstractThrownPotion.onHit` (`AbstractThrownPotion.java:70-85`) runs the water
            // branch for the lingering potion too, so a lingering water bottle also burns
            // endermen, douses burning mobs and rehydrates axolotls.
            let owner = self
                .thrown
                .owner_id
                .and_then(|id| world.get_entity_by_id(id));
            crate::entity::projectile::splash_potion::apply_water_potion_entity_effects(
                self,
                owner.as_deref(),
                &stack,
            )
            .await;

            // Play impact particles
            world.send_entity_status(self.get_entity(), EntityStatus::Death, None);

            // Read stored item stack and compute potion effects
            let stack = self.item_stack.read().await.clone();
            let effects = crate::item::potion::PotionContents::read_potion_effects(&stack);

            // Play splash/break particles & sound. `AbstractThrownPotion.onHit`
            // (`AbstractThrownPotion.java:81-82`) emits the level event unconditionally, before
            // the effect branch, so a plain water bottle still splashes.
            // PotionContents.java:113-119 and 126-144 define custom-color precedence and the
            // weighted opaque fallback used by AbstractThrownPotion.java:81-82.
            let color = stack
                .get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
                .and_then(|pc| pc.custom_color)
                .unwrap_or_else(|| {
                    crate::item::potion::PotionContents::get_color_or(&effects, -13_083_194)
                });

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
            if effects.is_empty() {
                return;
            }

            // Spawn and configure an `AreaEffectCloud` entity
            let cloud_entity = crate::entity::Entity::from_uuid(
                Uuid::new_v4(),
                world.clone(),
                hit_pos,
                &pumpkin_data::entity::EntityType::AREA_EFFECT_CLOUD,
            );
            let cloud = crate::entity::area_effect_cloud::AreaEffectCloudEntity::create(
                cloud_entity,
                stack.clone(),
                effects.clone(),
                600,
                3.0,
                20,
                10,
                -0.5,
                0,
                -3.0 / 600.0,
            );

            world.spawn_entity(cloud).await;
        })
    }
}
