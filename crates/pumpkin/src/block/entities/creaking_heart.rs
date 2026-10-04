use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};
use std::sync::{Mutex, PoisonError, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::Block;
use pumpkin_data::block_properties::{
    BlockProperties, CreakingHeartLikeProperties, CreakingHeartState,
};
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::GameMode;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use uuid::Uuid;

use crate::block::blocks::creaking_heart::{creaking_active, has_required_logs};
use crate::entity::mob::creaking::CreakingEntity;
use crate::entity::{Entity, EntityBase};
use crate::world::World;
use crate::world::game_event::{GameEventContext, emit_game_event};

use super::BlockEntity;

/// `net.minecraft.world.level.block.entity.CreakingHeartBlockEntity`.
///
/// The protector lifecycle (`spawnProtector`, `getCreakingProtector`, `removeProtector`,
/// `computeAnalogOutputSignal`) is ported. `creakingHurt`'s trail particles and resin spread
/// and the emitter-driven hurt sounds are not: only the hurt sound is played.
pub struct CreakingHeartBlockEntity {
    pub position: BlockPos,
    /// `creakingInfo`, reduced to the persisted UUID arm (`Either.right`).
    pub creaking_uuid: AtomicCell<Option<Uuid>>,
    /// `ticksExisted`: ticks since the UUID was last bound by `setCreakingInfo(UUID)`.
    pub ticks_existed: AtomicI64,
    /// The `Either.left` arm of `creakingInfo`: the resolved live protector.
    live_protector: Mutex<Option<Weak<dyn EntityBase>>>,
    pub ticker: AtomicI32,
    pub output_signal: AtomicI32,
}

impl BlockEntity for CreakingHeartBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        // `CreakingHeartBlockEntity.loadAdditional` (`CreakingHeartBlockEntity.java:348-352`)
        // restores the UUID through setCreakingInfo.
        let creaking_uuid = nbt.get_int_array("creaking").and_then(uuid_from_int_array);
        let entity = Self::new(position);
        entity.set_creaking_uuid(creaking_uuid);
        entity
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let creaking_uuid = self.creaking_uuid.load();
            if let Some(uuid) = creaking_uuid {
                nbt.put("creaking", uuid_to_int_array(uuid));
            }
        })
    }

    /// `CreakingHeartBlockEntity.serverTick`, minus the protector spawn/upkeep branch.
    /// Vanilla's `CreakingHeartBlock.getTicker` returns null while the heart is UPROOTED,
    /// so an uprooted heart does not tick here either.
    fn tick<'a>(&'a self, world: &'a Arc<World>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let (block, state) = world.get_block_and_state(&self.position);
            if block.id != Block::CREAKING_HEART.id {
                return;
            }
            let mut props = CreakingHeartLikeProperties::from_state_id(state.id, block);
            if props.creaking_heart_state == CreakingHeartState::Uprooted {
                return;
            }

            // `entity.ticksExisted++` (`CreakingHeartBlockEntity.java:72`).
            self.ticks_existed.fetch_add(1, Ordering::Relaxed);

            let computed = self.compute_analog_output_signal(world);
            if self.output_signal.swap(computed, Ordering::Relaxed) != computed {
                world.update_comparators(&self.position, block).await;
            }

            // `if (entity.ticker-- < 0)`: post-decrement, so the body runs on the tick the
            // pre-decrement value is negative. Reseeds to `nextInt(5) + 20`.
            if self.ticker.fetch_sub(1, Ordering::Relaxed) >= 0 {
                return;
            }
            self.ticker
                .store(20 + rand::rng().random_range(0..5), Ordering::Relaxed);

            // updateCreakingState: an uprooted-eligible heart (no logs, no bound creaking)
            // goes UPROOTED, otherwise it tracks the creaking_active environment attribute.
            let new_state = if has_required_logs(world, &self.position, props.axis)
                || self.creaking_uuid.load().is_some()
            {
                if creaking_active(world).await {
                    CreakingHeartState::Awake
                } else {
                    CreakingHeartState::Dormant
                }
            } else {
                CreakingHeartState::Uprooted
            };

            if new_state != props.creaking_heart_state {
                props.creaking_heart_state = new_state;
                world
                    .set_block_state(
                        &self.position,
                        props.to_state_id(block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
                if new_state == CreakingHeartState::Uprooted {
                    return;
                }
            }

            if self.creaking_uuid.load().is_none() {
                // `serverLevel.isSpawningMonsters()` and a non-peaceful difficulty; the nearest
                // player search does not filter out creative players, only spectators.
                if new_state == CreakingHeartState::Awake
                    && world.should_spawn_monsters()
                    && world
                        .get_closest_player_where(
                            self.position.to_f64(),
                            PLAYER_DETECTION_RANGE,
                            |player| player.gamemode.load() != GameMode::Spectator,
                        )
                        .is_some()
                    && let Some(creaking) = self.spawn_protector(world).await
                {
                    creaking.make_sound(Sound::EntityCreakingSpawn);
                    world.play_sound(
                        Sound::BlockCreakingHeartSpawn,
                        SoundCategory::Blocks,
                        &self.position.to_centered_f64(),
                    );
                }
            } else if let Some(entity) = self.get_creaking_protector(world)
                && let Some(creaking) = entity.cast_any().downcast_ref::<CreakingEntity>()
            {
                let players = world
                    .get_nearby_players(entity.get_entity().pos.load(), PLAYER_DETECTION_RANGE)
                    .into_iter()
                    .filter(|player| player.gamemode.load() != GameMode::Spectator)
                    .collect::<Vec<_>>();
                if (!creaking_active(world).await && !creaking.mob_entity.is_persistence_required())
                    || self.distance_to_creaking(&entity) > DISTANCE_CREAKING_TOO_FAR
                    || creaking.player_is_stuck_in_you(&players)
                {
                    self.remove_protector(world, None).await;
                }
            }
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        // getUpdateTag == saveCustomOnly.
        let mut nbt = NbtCompound::new();
        let creaking_uuid = self.creaking_uuid.load();
        if let Some(uuid) = creaking_uuid {
            nbt.put("creaking", uuid_to_int_array(uuid));
        }
        Some(nbt)
    }

    /// `CreakingHeartBlockEntity.preRemoveSideEffects` (`CreakingHeartBlockEntity.java:309-312`).
    fn pre_remove_side_effects<'a>(
        self: Arc<Self>,
        world: Arc<World>,
        _position: BlockPos,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            self.remove_protector_on_removal(&world).await;
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl CreakingHeartBlockEntity {
    pub const ID: &'static str = "minecraft:creaking_heart";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            creaking_uuid: AtomicCell::new(None),
            ticks_existed: AtomicI64::new(0),
            live_protector: Mutex::new(None),
            ticker: AtomicI32::new(0),
            output_signal: AtomicI32::new(0),
        }
    }

    /// `setCreakingInfo(UUID)` (`CreakingHeartBlockEntity.java:163-167`): also restarts the
    /// grace period counted by `ticks_existed`.
    pub fn set_creaking_uuid(&self, uuid: Option<Uuid>) {
        self.creaking_uuid.store(uuid);
        self.ticks_existed.store(0, Ordering::Relaxed);
    }

    /// `clearCreakingInfo` (`CreakingHeartBlockEntity.java:153-156`).
    fn clear_creaking_info(&self) {
        self.creaking_uuid.store(None);
        *self
            .live_protector
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// `getCreakingProtector` (`CreakingHeartBlockEntity.java:169-198`): the live creaking, found
    /// by UUID when it is not already resolved; an unresolved binding is dropped once
    /// `ticks_existed` reaches the 30 tick grace period.
    fn get_creaking_protector(&self, world: &Arc<World>) -> Option<Arc<dyn EntityBase>> {
        let uuid = self.creaking_uuid.load()?;
        let cached = self
            .live_protector
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade);
        let cached_present = cached.is_some();
        if let Some(creaking) = cached
            && creaking.get_entity().entity_uuid == uuid
            && !creaking.get_entity().is_removed()
        {
            return Some(creaking);
        }
        if cached_present {
            // Vanilla calls `setCreakingInfo(uuid)` when the cached protector was removed.
            self.ticks_existed.store(0, Ordering::Relaxed);
        }

        if let Some(entity) = world.get_entity_by_uuid(uuid)
            && entity.cast_any().is::<CreakingEntity>()
        {
            *self
                .live_protector
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(Arc::downgrade(&entity));
            return Some(entity);
        }
        if self.ticks_existed.load(Ordering::Relaxed) >= TICKS_GRACE_PERIOD {
            self.clear_creaking_info();
        }
        None
    }

    /// `distanceToCreaking` (`CreakingHeartBlockEntity.java:149-151`): 0 without a protector.
    fn distance_to_creaking(&self, creaking: &Arc<dyn EntityBase>) -> f64 {
        let pos = creaking.get_entity().pos.load();
        let bottom_center = self.position.to_centered_f64();
        let dx = pos.x - bottom_center.x;
        let dz = pos.z - bottom_center.z;
        let dy = pos.y - f64::from(self.position.0.y);
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// `spawnProtector` (`CreakingHeartBlockEntity.java:200-214`) over
    /// `SpawnUtil.trySpawnMob(CREAKING, SPAWNER, level, pos, 5, 16, 8,
    /// ON_TOP_OF_COLLIDER_NO_LEAVES, true)` (`SpawnUtil.java:19-75`).
    async fn spawn_protector(&self, world: &Arc<World>) -> Option<Arc<CreakingEntity>> {
        let dimensions = EntityDimensions::new(
            EntityType::CREAKING.dimension[0],
            EntityType::CREAKING.dimension[1],
            0.0,
        );
        for _ in 0..SPAWN_ATTEMPTS {
            let dx = rand::random_range(-SPAWN_RANGE_XZ..=SPAWN_RANGE_XZ);
            let dz = rand::random_range(-SPAWN_RANGE_XZ..=SPAWN_RANGE_XZ);
            let mut search = BlockPos::new(
                self.position.0.x + dx,
                self.position.0.y + SPAWN_RANGE_Y,
                self.position.0.z + dz,
            );
            if !world
                .worldborder
                .lock()
                .await
                .contains_block(search.0.x, search.0.z)
            {
                continue;
            }

            // `moveToPossibleSpawnPosition`.
            let mut above_state = world.get_block_state(&search);
            let mut found = None;
            for _ in 0..=(SPAWN_RANGE_Y * 2) {
                search = search.down();
                let (block, current_state) = world.get_block_and_state(&search);
                if above_state.get_block_collision_shapes().next().is_none()
                    && !block.has_tag(&tag::Block::MINECRAFT_LEAVES)
                    && current_state.is_side_solid(pumpkin_data::BlockDirection::Up)
                {
                    found = Some(search.up());
                    break;
                }
                above_state = current_state;
            }
            let Some(spawn_pos) = found else {
                continue;
            };

            // `level.noCollision(spawnAABB)` and `Mob.checkSpawnObstruction`
            // (`Mob.java:821-823`): no block collision, no liquid, no collidable entity.
            let x = f64::from(spawn_pos.0.x) + 0.5;
            let z = f64::from(spawn_pos.0.z) + 0.5;
            let bounding_box =
                BoundingBox::new_from_pos(x, f64::from(spawn_pos.0.y), z, &dimensions);
            if !world.is_space_empty(bounding_box)
                || contains_any_liquid(world, &bounding_box)
                || world
                    .get_all_at_box(&bounding_box.expand_all(1.0e-7))
                    .iter()
                    .any(|entity| !entity.is_spectator() && entity.can_be_collided_with())
            {
                continue;
            }

            let creaking = CreakingEntity::new(Entity::from_uuid(
                Uuid::new_v4(),
                world.clone(),
                Vector3::new(x, f64::from(spawn_pos.0.y), z),
                &EntityType::CREAKING,
            ));
            // Bound before the creaking is added: a heart-bound creaking whose heart does not
            // report it as protector is killed on its first tick.
            creaking.set_transient(self.position);
            self.set_bound_creaking(creaking.get_entity().entity_uuid);
            let creaking_base: Arc<dyn EntityBase> = creaking.clone();
            world.spawn_entity(creaking_base.clone()).await;
            emit_game_event(
                world,
                GameEvent::EntityPlace,
                creaking.get_entity().pos.load(),
                GameEventContext::of_entity(creaking_base.clone()),
            )
            .await;
            world.send_entity_status(creaking.get_entity(), EntityStatus::Poof, None);
            return Some(creaking);
        }
        None
    }

    /// `setCreakingInfo(Creaking)` (`CreakingHeartBlockEntity.java:158-161`): no grace reset.
    fn set_bound_creaking(&self, uuid: Uuid) {
        self.creaking_uuid.store(Some(uuid));
        *self
            .live_protector
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// `getAnalogOutputSignal`: the value cached by the last tick, not a fresh compute.
    pub fn get_analog_output_signal(&self) -> u8 {
        self.output_signal.load(Ordering::Relaxed).clamp(0, 15) as u8
    }

    /// `computeAnalogOutputSignal` (`CreakingHeartBlockEntity.java:338-346`): 0 without a live
    /// protector, else by its distance from the heart.
    fn compute_analog_output_signal(&self, world: &Arc<World>) -> i32 {
        if self.creaking_uuid.load().is_none() {
            return 0;
        }
        self.get_creaking_protector(world).map_or(0, |creaking| {
            analog_signal_for_distance(self.distance_to_creaking(&creaking))
        })
    }

    /// `CreakingHeartBlockEntity.isProtector`: whether this is the creaking whose UUID the heart
    /// holds (the UUID is what identifies the live protector).
    pub fn is_protector(&self, creaking_uuid: Uuid) -> bool {
        self.creaking_uuid.load() == Some(creaking_uuid)
    }

    /// `CreakingHeartBlockEntity.removeProtector(null)`
    /// (`CreakingHeartBlockEntity.java:309-327`): removal without a damage source tears down
    /// the bound creaking and clears the persisted binding atomically.
    pub async fn remove_protector_on_removal(&self, world: &Arc<World>) {
        self.remove_protector(world, None).await;
    }

    /// `CreakingHeartBlockEntity.removeProtector` (`CreakingHeartBlockEntity.java:314-327`):
    /// damage removes the binding, starts the creaking death effects, and sets health to zero;
    /// removal without damage tears the protector down immediately.
    pub async fn remove_protector(&self, world: &Arc<World>, damage_type: Option<DamageType>) {
        let Some(uuid) = self.creaking_uuid.swap(None) else {
            return;
        };
        if let Some(entity) = world.get_entity_by_uuid(uuid)
            && let Some(creaking) = entity.cast_any().downcast_ref::<CreakingEntity>()
        {
            if damage_type.is_some() {
                creaking.creaking_death_effects();
                creaking.mob_entity.living_entity.set_health(0.0);
            } else {
                creaking.tear_down().await;
            }
        }
    }

    /// `CreakingHeartBlockEntity.removeProtector(damageSource)`
    /// (`CreakingHeartBlockEntity.java:314-327`): a player break uses the death
    /// effects path before the heart block is removed.
    pub fn remove_protector_after_player_attack(&self, world: &Arc<World>) {
        let Some(uuid) = self.creaking_uuid.swap(None) else {
            return;
        };
        if let Some(entity) = world.get_entity_by_uuid(uuid)
            && let Some(creaking) = entity.cast_any().downcast_ref::<CreakingEntity>()
        {
            creaking.creaking_death_effects();
            creaking.mob_entity.living_entity.health.store(0.0);
        }
    }

    /// `CreakingHeartBlockEntity.creakingHurt`. The particle/resin-spread effects this drives
    /// in vanilla are deliberately not ported here (same scope cut as this file's existing
    /// doc comment already calls out for `spawnProtector`/`removeProtector`/`spreadResin`);
    /// only the hurt sound is played.
    pub fn creaking_hurt(&self, world: &Arc<World>) {
        world.play_sound(
            Sound::BlockCreakingHeartHurt,
            SoundCategory::Blocks,
            &self.position.to_f64(),
        );
    }
}

const PLAYER_DETECTION_RANGE: f64 = 32.0;
const DISTANCE_CREAKING_TOO_FAR: f64 = 34.0;
const SPAWN_RANGE_XZ: i32 = 16;
const SPAWN_RANGE_Y: i32 = 8;
const SPAWN_ATTEMPTS: i32 = 5;
const TICKS_GRACE_PERIOD: i64 = 30;

/// `computeAnalogOutputSignal` for a protector `distance` blocks from the heart
/// (`CreakingHeartBlockEntity.java:341-343`).
fn analog_signal_for_distance(distance: f64) -> i32 {
    let scaled = distance.clamp(0.0, 32.0) / 32.0;
    15 - (scaled * 15.0).floor() as i32
}

/// `LevelReader::containsAnyLiquid` (`LevelReader.java:140-161`).
pub(super) fn contains_any_liquid(world: &World, bounding_box: &BoundingBox) -> bool {
    for x in bounding_box.min.x.floor() as i32..bounding_box.max.x.ceil() as i32 {
        for y in bounding_box.min.y.floor() as i32..bounding_box.max.y.ceil() as i32 {
            for z in bounding_box.min.z.floor() as i32..bounding_box.max.z.ceil() as i32 {
                if !world
                    .get_fluid_and_fluid_state(&BlockPos::new(x, y, z))
                    .1
                    .is_empty
                {
                    return true;
                }
            }
        }
    }
    false
}

/// `UUIDUtil.CODEC`: four big-endian ints, most-significant first.
const fn uuid_from_int_array(values: &[i32]) -> Option<Uuid> {
    let &[a, b, c, d] = values else { return None };
    Some(Uuid::from_u128(
        ((a as u32 as u128) << 96)
            | ((b as u32 as u128) << 64)
            | ((c as u32 as u128) << 32)
            | (d as u32 as u128),
    ))
}

fn uuid_to_int_array(u: Uuid) -> NbtTag {
    let v = u.as_u128();
    NbtTag::IntArray(vec![
        (v >> 96) as i32,
        ((v >> 64) & 0xFFFF_FFFF) as i32,
        ((v >> 32) & 0xFFFF_FFFF) as i32,
        (v & 0xFFFF_FFFF) as i32,
    ])
}

#[cfg(test)]
mod tests {
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_util::math::position::BlockPos;
    use uuid::Uuid;

    use super::{
        BlockEntity, CreakingHeartBlockEntity, analog_signal_for_distance, uuid_to_int_array,
    };

    #[test]
    fn analog_signal_scales_with_distance() {
        // `15 - floor(clamp(d, 0, 32) / 32 * 15)` (`CreakingHeartBlockEntity.java:341-343`).
        assert_eq!(analog_signal_for_distance(0.0), 15);
        assert_eq!(analog_signal_for_distance(16.0), 8);
        assert_eq!(analog_signal_for_distance(32.0), 0);
        assert_eq!(analog_signal_for_distance(100.0), 0);
    }

    #[test]
    fn load_restores_creaking_uuid() {
        // `CreakingHeartBlockEntity.loadAdditional`/`saveAdditional`
        // (`CreakingHeartBlockEntity.java:348-359`) round-trip the creaking UUID.
        let uuid = Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff);
        let mut nbt = NbtCompound::new();
        nbt.put("creaking", uuid_to_int_array(uuid));

        let entity = CreakingHeartBlockEntity::from_nbt(&nbt, BlockPos::new(1, 2, 3));

        assert_eq!(entity.creaking_uuid.load(), Some(uuid));
    }
}
