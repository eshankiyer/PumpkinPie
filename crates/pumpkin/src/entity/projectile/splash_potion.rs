use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::{
    entity::{Entity, EntityBase, EntityBaseFuture, NBTStorage, projectile::ThrownItemEntity},
    server::Server,
};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{Block, BlockId};
use pumpkin_protocol::java::client::play::CWorldEvent;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{position::BlockPos, vector2::to_chunk_pos};
use pumpkin_world::world::BlockFlags;
use tokio::sync::RwLock;

const GRAVITY: f64 = 0.05;

pub struct SplashPotionEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
}

impl SplashPotionEntity {
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
            item_stack: RwLock::new(ItemStack::new(1, &pumpkin_data::item::Item::SPLASH_POTION)),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        thrown.entity.set_velocity(Vector3::new(0.0, 0.1, 0.0));
        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(1, &pumpkin_data::item::Item::SPLASH_POTION)),
        }
    }

    pub async fn set_item_stack(&self, item_stack: ItemStack) {
        let mut write = self.item_stack.write().await;
        *write = item_stack;
    }
}

impl NBTStorage for SplashPotionEntity {}

fn is_water_potion(stack: &ItemStack) -> bool {
    stack
        .get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
        .and_then(|pc| pc.potion_id)
        == Some(pumpkin_data::potion::Potion::WATER.id as i32)
}

/// Extinguishes fire (including soul fire) at the hit position and its four horizontal neighbors.
async fn extinguish_fire(world: &Arc<crate::world::World>, hit_pos: Vector3<f64>) {
    let air_state_id = Block::AIR.default_state.id;

    let neighbors = [
        hit_pos,
        Vector3::new(hit_pos.x + 1.0, hit_pos.y, hit_pos.z),
        Vector3::new(hit_pos.x - 1.0, hit_pos.y, hit_pos.z),
        Vector3::new(hit_pos.x, hit_pos.y, hit_pos.z + 1.0),
        Vector3::new(hit_pos.x, hit_pos.y, hit_pos.z - 1.0),
    ];

    for p in neighbors {
        let pos = BlockPos(Vector3::new(
            p.x.floor() as i32,
            p.y.floor() as i32,
            p.z.floor() as i32,
        ));
        let state_id = world.get_block_state_id(&pos);
        let raw_block_id = state_id.to_block_id();
        if raw_block_id == BlockId::FIRE || raw_block_id == BlockId::SOUL_FIRE {
            world
                .set_block_state(&pos, air_state_id, BlockFlags::NOTIFY_ALL)
                .await;
        }
    }
}

pub(crate) async fn extinguish_fire_if_water_potion(
    world: &Arc<crate::world::World>,
    hit_pos: Vector3<f64>,
    stack: &ItemStack,
) {
    if is_water_potion(stack) {
        extinguish_fire(world, hit_pos).await;
    }
}

/// `AbstractThrownPotion.onHitAsWater` (`AbstractThrownPotion.java:87-106`). A thrown water
/// bottle -- splash or lingering -- searches its own bounding box inflated by (4, 2, 4) for
/// living entities that are either water sensitive or on fire, and for every one closer than
/// `SPLASH_RANGE_SQ` (16.0) deals a point of indirect magic damage if it is water sensitive and
/// extinguishes it if it is burning. Axolotls in the same box are rehydrated
/// (`Axolotl.rehydrate`, `Axolotl.java:270-273`) regardless of distance.
pub(crate) async fn apply_water_potion_entity_effects(
    potion_base: &dyn EntityBase,
    owner: Option<&dyn EntityBase>,
    stack: &ItemStack,
) {
    if !is_water_potion(stack) {
        return;
    }

    let potion = potion_base.get_entity();
    let world = potion.world.load();
    let aabb = potion.bounding_box.load().expand(4.0, 2.0, 4.0);
    let potion_pos = potion.pos.load();

    let mut candidates = world.get_entities_at_box(&aabb);
    for player in world.get_players_at_box(&aabb) {
        candidates.push(player as Arc<dyn EntityBase>);
    }

    for candidate in candidates {
        let target = candidate.get_entity();
        if target.entity_id == potion.entity_id {
            continue;
        }

        // `Axolotl.rehydrate`: +1800 air, capped at the axolotl's 6000 maximum. Vanilla runs this
        // over the whole inflated box, with no distance test.
        if target.entity_type.id == EntityType::AXOLOTL.id
            && let Some(living) = candidate.get_living_entity()
        {
            let max_air = living.max_air_supply();
            let new_air =
                (living.air_supply.load(std::sync::atomic::Ordering::Relaxed) + 1800).min(max_air);
            if living
                .air_supply
                .swap(new_air, std::sync::atomic::Ordering::Relaxed)
                != new_air
            {
                living.send_air_supply();
            }
        }

        if candidate.get_living_entity().is_none() {
            continue;
        }

        let on_fire = target.fire_ticks.load(std::sync::atomic::Ordering::Relaxed) > 0;
        let sensitive = candidate.is_sensitive_to_water();
        if !sensitive && !on_fire {
            continue;
        }

        let delta = target.pos.load() - potion_pos;
        if delta.length_squared() >= 16.0 {
            continue;
        }

        if sensitive {
            candidate
                .damage_with_context(
                    candidate.as_ref(),
                    1.0,
                    pumpkin_data::damage::DamageType::INDIRECT_MAGIC,
                    None,
                    Some(potion_base),
                    owner,
                )
                .await;
        }

        if on_fire && target.is_alive() {
            target.extinguish();
        }
    }
}

impl EntityBase for SplashPotionEntity {
    fn init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            let stack = self.item_stack.read().await;

            // Sync the item stack
            entity.send_meta_data(
                &[pumpkin_protocol::java::client::play::Metadata::new(
                    pumpkin_data::tracked_data::splash_potion::ITEM_STACK,
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
            let owner = self
                .thrown
                .owner_id
                .and_then(|id| world.get_entity_by_id(id));
            apply_water_potion_entity_effects(self, owner.as_deref(), &stack).await;

            let effects = crate::item::potion::PotionContents::read_potion_effects(&stack);

            // PotionContents.java:113-119 and 126-144 define custom-color precedence and the
            // weighted opaque fallback used by AbstractThrownPotion.java:81-82.
            let color = stack
                .get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
                .and_then(|pc| pc.custom_color)
                .unwrap_or_else(|| {
                    crate::item::potion::PotionContents::get_color_or(&effects, -13_083_194)
                });

            // Play splash particles
            let has_instant = effects.iter().any(|(e, _, _, _, _, _)| {
                e.id == pumpkin_data::effect::StatusEffect::INSTANT_DAMAGE.id
                    || e.id == pumpkin_data::effect::StatusEffect::INSTANT_HEALTH.id
            });
            let event_id = if has_instant { 2007 } else { 2002 };

            // Convert hit_pos to BlockPos
            let block_pos = BlockPos(Vector3::new(
                hit_pos.x.floor() as i32,
                hit_pos.y.floor() as i32,
                hit_pos.z.floor() as i32,
            ));
            world.broadcast_to_chunk(
                to_chunk_pos(&Vector2::new(block_pos.0.x, block_pos.0.z)),
                &CWorldEvent::new(event_id, block_pos, color, false),
            );

            // If no effects, just splash (like water bottles)
            if effects.is_empty() {
                return;
            }

            let this_entity = self.get_entity();
            let potion_aabb = this_entity
                .bounding_box
                .load()
                .shift(hit_pos - this_entity.pos.load());
            let effect_aabb = potion_aabb.expand(4.0, 2.0, 4.0);

            // Gather entity and player candidates
            let mut candidates = world.get_entities_at_box(&effect_aabb);
            let players = world.get_players_at_box(&effect_aabb);
            for p in players {
                candidates.push(p.clone() as Arc<dyn EntityBase>);
            }

            let margin = ((this_entity.age.load(std::sync::atomic::Ordering::Relaxed) as f32 - 2.0)
                / 20.0)
                .clamp(0.0, 0.3) as f64;

            for cand in candidates {
                let cand_clone = cand.clone();
                let effs_clone: Vec<_> = effects.clone();
                tokio::spawn(async move {
                    // `ThrownSplashPotion.hitBlock`: an entity that is not affected by potions,
                    // such as an armour stand, is skipped before the range test.
                    if let Some(living) = cand_clone
                        .get_living_entity()
                        .filter(|living| living.is_affected_by_potions())
                    {
                        let target_aabb = cand_clone
                            .get_entity()
                            .bounding_box
                            .load()
                            .expand_all(margin);
                        let dist_sq = potion_aabb.squared_distance_to_box(&target_aabb);
                        if dist_sq >= 16.0 {
                            return;
                        }

                        // Distance scaling
                        let scale = (1.0f32 - (dist_sq.sqrt() as f32 / 4.0)).max(0.0);

                        crate::item::potion::PotionContents::apply_effects_to(
                            living,
                            effs_clone,
                            scale,
                            crate::item::potion::PotionApplicationSource::Normal,
                        )
                        .await;
                    }
                });
            }
        })
    }
}
