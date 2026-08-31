//! `SculkShriekerBlockEntity` (`world/level/block/entity/SculkShriekerBlockEntity.java`).
//!
//! The warning-level escalation, `canRespond` gating and warden summon are ported here;
//! the per-player half lives in `crate::entity::mob::warden::warden_spawn_tracker`.
//!
//! The shrieker's `VibrationSystem.Data` and ticker are modeled locally because this codebase's
//! flat game-event registry still dispatches listeners synchronously; the shrieker queues its
//! candidate and delivers it from the block-entity tick (`VibrationSystem.java:123-180,278-361`).

use super::BlockEntity;
use pumpkin_data::block_properties::{BlockProperties, SculkShriekerLikeProperties};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{BlockId, entity::EntityType};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

use crate::block::blocks::sculk::sculk_shrieker::ShriekerListener;
use crate::entity::mob::warden::{WardenEntity, apply_darkness_around};
use crate::entity::player::Player;
use crate::world::World;
use crate::world::game_event::vibration::{VibrationInfo, VibrationSelector};

/// `SculkShriekerBlockEntity.WARNING_SOUND_RADIUS` (line 42).
const WARNING_SOUND_RADIUS: i32 = 10;
/// `SculkShriekerBlockEntity.WARDEN_SPAWN_ATTEMPTS` (line 43).
const WARDEN_SPAWN_ATTEMPTS: i32 = 20;
/// `SculkShriekerBlockEntity.WARDEN_SPAWN_RANGE_XZ` (line 44).
const WARDEN_SPAWN_RANGE_XZ: i32 = 5;
/// `SculkShriekerBlockEntity.WARDEN_SPAWN_RANGE_Y` (line 45).
const WARDEN_SPAWN_RANGE_Y: i32 = 6;
/// `SculkShriekerBlockEntity.DARKNESS_RADIUS` (line 46).
const DARKNESS_RADIUS: f64 = 40.0;
/// `SculkShriekerBlockEntity.SHRIEKING_TICKS` (line 47).
const SHRIEKING_TICKS: u8 = 90;

/// `SculkShriekerBlockEntity.SOUND_BY_LEVEL` (lines 48-53).
const fn sound_by_level(warning_level: i32) -> Option<Sound> {
    match warning_level {
        1 => Some(Sound::EntityWardenNearbyClose),
        2 => Some(Sound::EntityWardenNearbyCloser),
        3 => Some(Sound::EntityWardenNearbyClosest),
        4 => Some(Sound::EntityWardenListeningAngry),
        _ => None,
    }
}

/// Mirrors `VibrationSystem.Data` and `VibrationSystem.Ticker`: candidates are selected by the
/// current tick, then travel for `floor(distance)` ticks before delivery
/// (`VibrationSystem.java:123-180,278-361`).
struct VibrationData {
    selector: VibrationSelector,
    current_vibration: Option<VibrationInfo>,
    travel_time: u32,
    tick: u64,
}

impl Default for VibrationData {
    fn default() -> Self {
        Self {
            selector: VibrationSelector::new(),
            current_vibration: None,
            travel_time: 0,
            tick: 0,
        }
    }
}

pub struct SculkShriekerBlockEntity {
    pub position: BlockPos,
    pub warning_level: Mutex<i32>,
    /// Guards the lazy `GameEventListener` registration performed on the first tick, exactly
    /// as `SculkCatalystBlockEntity` does (this codebase's listener registry is flat and
    /// per-world, so a block entity loaded from disk never runs the block's `placed` hook).
    listener_registered: AtomicBool,
    /// Mirrors the block state's `shrieking` and `can_summon` while a shriek is in flight.
    ///
    /// `preRemoveSideEffects` (lines 133-138) reads the state being removed, but this
    /// codebase calls `on_block_replaced` *after* the new state is written, so by then the
    /// shrieker's own properties are gone. These two flags preserve exactly what that hook
    /// and `canRespond` need.
    shrieking_flag: AtomicBool,
    can_summon_flag: AtomicBool,
    /// Vanilla stores this as the block entity's `VibrationSystem.Data`
    /// (`SculkShriekerBlockEntity.java:56-58`).
    vibration_data: Mutex<VibrationData>,
    /// Mirrors `VibrationSystem.User.onDataChanged`, which calls `setChanged` after the
    /// listener data changes (`SculkShriekerBlockEntity.java:215-218`).
    dirty: AtomicBool,
}

impl BlockEntity for SculkShriekerBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    /// `loadAdditional` (lines 74-79).
    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let warning_level = nbt.get_int("warning_level").unwrap_or(0);
        Self {
            position,
            warning_level: Mutex::new(warning_level),
            listener_registered: AtomicBool::new(false),
            shrieking_flag: AtomicBool::new(false),
            can_summon_flag: AtomicBool::new(false),
            vibration_data: Mutex::new(VibrationData::default()),
            dirty: AtomicBool::new(false),
        }
    }

    /// Exposes the dirty state set by vanilla `onDataChanged` (`SculkShriekerBlockEntity.java:215-218`).
    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Release);
    }

    /// `saveAdditional` (lines 81-86).
    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            nbt.put_int("warning_level", *self.warning_level.lock().await);
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        nbt.put_int("warning_level", *self.warning_level.try_lock().ok()?);
        Some(nbt)
    }

    fn tick<'a>(&'a self, world: &'a Arc<World>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.ensure_listener_registered(world).await;
            self.tick_vibration(world).await;
        })
    }

    /// `preRemoveSideEffects` (lines 133-138): a shrieker broken mid-shriek still responds.
    fn on_block_replaced<'a>(
        self: Arc<Self>,
        world: Arc<World>,
        position: BlockPos,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            if self.shrieking_flag.load(Ordering::Acquire) {
                self.try_respond(&world).await;
            }
            world.unregister_game_event_listener_at(&position).await;
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl SculkShriekerBlockEntity {
    pub const ID: &'static str = "minecraft:sculk_shrieker";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            warning_level: Mutex::new(0),
            listener_registered: AtomicBool::new(false),
            shrieking_flag: AtomicBool::new(false),
            can_summon_flag: AtomicBool::new(false),
            vibration_data: Mutex::new(VibrationData::default()),
            dirty: AtomicBool::new(false),
        }
    }

    /// Implements vanilla `onDataChanged` through the existing block-entity dirty path
    /// (`SculkShriekerBlockEntity.java:215-218`).
    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// Returns the `Data.currentVibration != null` state checked by the vanilla listener before
    /// accepting another event (`VibrationSystem.java:210-218`).
    pub(crate) async fn has_current_vibration(&self) -> bool {
        self.vibration_data.lock().await.current_vibration.is_some()
    }

    /// Queues a vibration for the block entity's tick, matching `VibrationSystem.Listener`
    /// delivery through `VibrationSystem.Ticker` (`VibrationSystem.java:300-315,342-361`).
    pub(crate) async fn queue_vibration(
        &self,
        source_position: Vector3<f64>,
        source_entity: uuid::Uuid,
        event: &GameEvent,
    ) {
        let listener_position = self.position.to_centered_f64();
        let distance = (source_position - listener_position).length() as f32;
        let mut data = self.vibration_data.lock().await;
        let tick = data.tick;
        data.selector.add_candidate(
            VibrationInfo {
                frequency: crate::world::game_event::vibration_frequency(event),
                distance,
                pos: source_position,
                source_entity: Some(source_entity),
                projectile_owner: None,
            },
            tick,
        );
    }

    async fn tick_vibration(&self, world: &Arc<World>) {
        let (due_source, data_changed) = {
            let mut data = self.vibration_data.lock().await;
            data.tick = data.tick.saturating_add(1);
            let mut data_changed = if data.current_vibration.is_none()
                && let Some(vibration) = data.selector.chosen_candidate(data.tick)
            {
                data.travel_time = vibration_travel_time(vibration.distance);
                data.current_vibration = Some(vibration);
                data.selector.start_over();
                true
            } else {
                false
            };
            if data.current_vibration.is_some() {
                data_changed |= data.travel_time > 0;
                data.travel_time = data.travel_time.saturating_sub(1);
            }
            let due_source = (data.travel_time == 0)
                .then(|| {
                    data.current_vibration
                        .as_ref()
                        .and_then(|v| v.source_entity)
                })
                .flatten();
            (due_source, data_changed)
        };

        // Mirrors `VibrationSystem.Ticker` calling `onDataChanged` after selection and travel
        // progress (`VibrationSystem.java:285-295,300-313`).
        if data_changed {
            self.mark_dirty();
        }

        let Some(source_entity) = due_source else {
            return;
        };
        if !adjacent_chunks_are_ticking(world, self.position) {
            return;
        }
        let source = {
            let mut data = self.vibration_data.lock().await;
            data.current_vibration.take().and_then(|vibration| {
                (vibration.source_entity == Some(source_entity)).then_some(vibration)
            })
        };
        if source.is_some() {
            // `receiveVibration` clears the current vibration after delivery and then invokes
            // `onDataChanged` (`VibrationSystem.java:342-360`).
            self.mark_dirty();
        }
        if let Some(player) = world.get_player_by_uuid(source_entity)
            && source.is_some()
        {
            // `onReceiveVibration` resolves the source entity before calling `tryShriek`
            // (`SculkShriekerBlockEntity.java:204-213`).
            self.try_shriek(world, &player).await;
        }
    }

    async fn ensure_listener_registered(&self, world: &Arc<World>) {
        if self.listener_registered.swap(true, Ordering::AcqRel) {
            return;
        }
        let already_present = world
            .game_event_listeners
            .lock()
            .await
            .iter()
            .any(|listener| {
                matches!(
                    listener.listener_source(),
                    crate::world::game_event::PositionSource::Block(pos) if pos == self.position
                )
            });
        if !already_present {
            world
                .register_game_event_listener(Arc::new(ShriekerListener { pos: self.position }))
                .await;
        }
    }

    fn shrieking_at(world: &Arc<World>, position: &BlockPos) -> bool {
        let (block, state) = world.get_block_and_state(position);
        block.id == BlockId::SCULK_SHRIEKER
            && SculkShriekerLikeProperties::from_state_id(state.id, block).shrieking
    }

    /// `canRespond` (lines 127-131). Falls back to the cached `can_summon` when the block
    /// itself is already gone - see `can_summon_flag`.
    fn can_respond(&self, world: &Arc<World>) -> bool {
        let (block, state) = world.get_block_and_state(&self.position);
        let can_summon = if block.id == BlockId::SCULK_SHRIEKER {
            let can_summon = SculkShriekerLikeProperties::from_state_id(state.id, block).can_summon;
            self.can_summon_flag.store(can_summon, Ordering::Release);
            can_summon
        } else {
            self.can_summon_flag.load(Ordering::Acquire)
        };

        let info = world.level_info.load();
        can_summon && info.difficulty != Difficulty::Peaceful && info.game_rules.spawn_wardens
    }

    /// `tryShriek` (lines 100-110). Note the ordering: `warningLevel` is cleared first, and
    /// when the shrieker *can* respond but no warning could be issued (a warden is already
    /// nearby, or a nearby player is inside the 200-tick cooldown) nothing happens at all -
    /// not even a shriek.
    pub async fn try_shriek(&self, world: &Arc<World>, player: &Arc<Player>) {
        if Self::shrieking_at(world, &self.position) {
            return;
        }
        *self.warning_level.lock().await = 0;
        if !self.can_respond(world) || self.try_to_warn(world, player).await {
            self.shriek(world, player).await;
        }
    }

    /// `tryToWarn` (lines 112-116).
    async fn try_to_warn(&self, world: &Arc<World>, player: &Arc<Player>) -> bool {
        match crate::entity::mob::warden::warden_spawn_tracker::try_warn(
            world,
            &self.position,
            player,
        )
        .await
        {
            Some(warning_level) => {
                *self.warning_level.lock().await = warning_level;
                true
            }
            None => false,
        }
    }

    /// `shriek` (lines 118-125).
    async fn shriek(&self, world: &Arc<World>, source: &Arc<Player>) {
        let (block, state) = world.get_block_and_state(&self.position);
        if block.id != BlockId::SCULK_SHRIEKER {
            return;
        }
        let mut props = SculkShriekerLikeProperties::from_state_id(state.id, block);
        props.shrieking = true;
        self.shrieking_flag.store(true, Ordering::Release);
        world
            .set_block_state(
                &self.position,
                props.to_state_id(block),
                BlockFlags::NOTIFY_LISTENERS,
            )
            .await;
        world.schedule_block_tick(block, self.position, SHRIEKING_TICKS, TickPriority::Normal);
        world.sync_world_event(WorldEvent::ParticlesSculkShriek, self.position, 0);

        let source: Arc<dyn crate::entity::EntityBase> = source.clone();
        crate::world::game_event::emit_game_event(
            world,
            GameEvent::Shriek,
            self.position.to_centered_f64(),
            crate::world::game_event::GameEventContext::of_entity(source),
        )
        .await;
    }

    /// `tryRespond` (lines 140-148): runs when the 90-tick shriek ends, not when it starts.
    pub async fn try_respond(&self, world: &Arc<World>) {
        self.shrieking_flag.store(false, Ordering::Release);
        if !self.can_respond(world) {
            return;
        }
        let warning_level = *self.warning_level.lock().await;
        if warning_level <= 0 {
            return;
        }

        if !self.try_summon_warden(world, warning_level).await {
            self.play_warden_reply_sound(world, warning_level);
        }

        apply_darkness_around(world, self.position.to_centered_f64(), DARKNESS_RADIUS).await;
    }

    /// `playWardenReplySound` (lines 150-160).
    fn play_warden_reply_sound(&self, world: &Arc<World>, warning_level: i32) {
        let Some(sound) = sound_by_level(warning_level) else {
            return;
        };
        let offset = || rand::random_range(-WARNING_SOUND_RADIUS..=WARNING_SOUND_RADIUS);
        let position = Vector3::new(
            f64::from(self.position.0.x + offset()),
            f64::from(self.position.0.y + offset()),
            f64::from(self.position.0.z + offset()),
        );
        world.play_sound_fine(sound, SoundCategory::Hostile, &position, 5.0, 1.0);
    }

    /// `trySummonWarden` (lines 162-169): `SpawnUtil.trySpawnMob(WARDEN, TRIGGERED, level,
    /// pos, 20, 5, 6, ON_TOP_OF_COLLIDER, false)`.
    async fn try_summon_warden(&self, world: &Arc<World>, warning_level: i32) -> bool {
        if warning_level < crate::entity::mob::warden::warden_spawn_tracker::MAX_WARNING_LEVEL {
            return false;
        }
        let Some(spawn_pos) = find_warden_spawn_position(world, self.position) else {
            return false;
        };

        let position = Vector3::new(
            f64::from(spawn_pos.0.x) + 0.5,
            f64::from(spawn_pos.0.y),
            f64::from(spawn_pos.0.z) + 0.5,
        );
        // `EntityType.create` + `finalizeSpawn` with `EntitySpawnReason.TRIGGERED`
        // (Warden.java:480-492). Built directly rather than through `entity::type::from_type`
        // so the emerge can be started before the entity is handed out as `dyn EntityBase`.
        let warden = WardenEntity::new(crate::entity::Entity::from_uuid(
            uuid::Uuid::new_v4(),
            world.clone(),
            position,
            &EntityType::WARDEN,
        ));
        // Started after the spawn so the pose metadata broadcast has an audience: nothing is
        // tracking the entity until `spawn_entity` returns.
        world.spawn_entity(warden.clone()).await;
        warden.start_emerging();
        true
    }
}

/// `VibrationSystem.Ticker.areAdjacentChunksTicking` (`VibrationSystem.java:363-374`).
fn adjacent_chunks_are_ticking(world: &World, position: BlockPos) -> bool {
    let center = position.chunk_position();
    (-1..=1).all(|dx| {
        (-1..=1).all(|dz| {
            let chunk = pumpkin_util::math::vector2::Vector2::new(center.x + dx, center.y + dz);
            world.active_chunks.load().contains(&chunk) && world.level.is_chunk_loaded(&chunk)
        })
    })
}

/// `VibrationSystem.User.calculateTravelTimeInTicks` (`VibrationSystem.java:401-403`).
#[must_use]
const fn vibration_travel_time(distance: f32) -> u32 {
    distance.floor().max(0.0) as u32
}

/// `SpawnUtil.trySpawnMob` + `moveToPossibleSpawnPosition` with `Strategy.ON_TOP_OF_COLLIDER`
/// (`util/SpawnUtil.java:19-101`), specialised to the shrieker's arguments.
///
/// `checkCollisions` is `false` for this call site, so vanilla's `noCollision` pre-check is
/// skipped; `Warden.checkSpawnObstruction` (Warden.java:151-153) still applies and is the
/// three-block clearance check below.
fn find_warden_spawn_position(world: &Arc<World>, start: BlockPos) -> Option<BlockPos> {
    for _ in 0..WARDEN_SPAWN_ATTEMPTS {
        let dx = rand::random_range(-WARDEN_SPAWN_RANGE_XZ..=WARDEN_SPAWN_RANGE_XZ);
        let dz = rand::random_range(-WARDEN_SPAWN_RANGE_XZ..=WARDEN_SPAWN_RANGE_XZ);
        let mut search = BlockPos::new(
            start.0.x + dx,
            start.0.y + WARDEN_SPAWN_RANGE_Y,
            start.0.z + dz,
        );

        // `moveToPossibleSpawnPosition`: walk down looking for a full-faced collider with an
        // empty-collision block above it, then step back up onto that block.
        let mut above_state = world.get_block_state(&search);
        let mut found = None;
        for _ in 0..=(WARDEN_SPAWN_RANGE_Y * 2) {
            search = search.down();
            let current_state = world.get_block_state(&search);
            let above_empty = above_state.get_block_collision_shapes().next().is_none();
            if above_empty && current_state.is_side_solid(pumpkin_data::BlockDirection::Up) {
                found = Some(search.up());
                break;
            }
            above_state = current_state;
        }

        let Some(spawn_pos) = found else {
            continue;
        };
        if warden_fits(world, spawn_pos) {
            return Some(spawn_pos);
        }
    }
    None
}

/// `Warden.checkSpawnObstruction` (Warden.java:151-153): the warden's 0.9 x 2.9 box must be
/// free of collision. Checked here as the three block cells it spans.
fn warden_fits(world: &Arc<World>, pos: BlockPos) -> bool {
    (0..3).all(|dy| {
        world
            .get_block_state(&BlockPos::new(pos.0.x, pos.0.y + dy, pos.0.z))
            .get_block_collision_shapes()
            .next()
            .is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::{SculkShriekerBlockEntity, vibration_travel_time};
    use crate::block::entities::BlockEntity;
    use pumpkin_util::math::position::BlockPos;

    #[test]
    fn vibration_travel_time_uses_vanilla_flooring() {
        // `calculateTravelTimeInTicks` uses `Mth.floor` (`VibrationSystem.java:401-403`).
        assert_eq!(vibration_travel_time(3.9), 3);
        assert_eq!(vibration_travel_time(0.9), 0);
    }

    #[test]
    fn vibration_data_change_marks_shrieker_dirty_until_cleared() {
        let shrieker = SculkShriekerBlockEntity::new(BlockPos::ZERO);
        assert!(!shrieker.is_dirty());

        // `onDataChanged` calls `setChanged` (`SculkShriekerBlockEntity.java:215-218`).
        shrieker.mark_dirty();
        assert!(shrieker.is_dirty());

        shrieker.clear_dirty();
        assert!(!shrieker.is_dirty());
    }
}
