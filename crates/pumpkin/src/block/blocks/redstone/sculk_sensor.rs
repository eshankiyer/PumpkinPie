// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;

use crate::block::entities::BlockEntity;
use crate::block::entities::calibrated_sculk_sensor::CalibratedSculkSensorBlockEntity;
use crate::block::entities::sculk_sensor::SculkSensorBlockEntity;
use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, BrokenArgs, EmitsRedstonePowerArgs,
    GetComparatorOutputArgs, GetRedstonePowerArgs, OnEntityStepArgs, OnPlaceArgs,
    OnScheduledTickArgs, OnStateReplacedArgs, PlacedArgs,
};
use crate::entity::EntityBase;
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::world::World;
use crate::world::game_event::vibration::{VibrationData, VibrationInfo, is_valid_vibration};
use crate::world::game_event::{
    GameEventContext, GameEventFuture, GameEventListener, PositionSource,
    redstone_strength_for_distance, vibration_frequency,
};
use pumpkin_data::block_properties::{
    BlockProperties, CalibratedSculkSensorLikeProperties, HorizontalFacing,
    SculkSensorLikeProperties, SculkSensorPhase,
};
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, BlockId, BlockStateId, HorizontalFacingExt};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use rand::{RngExt, rng};
use rustc_hash::FxHashSet;
use tokio::sync::Mutex;
use uuid::Uuid;

pub struct SculkSensorBlock;

struct SculkSensorListener {
    pos: BlockPos,
    radius: i32,
}

/// `VibrationSystem.Ticker.areAdjacentChunksTicking` (`VibrationSystem.java:342-374`).
fn adjacent_chunks_are_ticking(world: &World, position: BlockPos) -> bool {
    let center = position.chunk_position();
    let active_chunks = world.active_chunks.load();
    adjacent_chunks_are_ticking_in_sets(center, &active_chunks, |chunk| {
        world.level.is_chunk_loaded(chunk)
    })
}

fn adjacent_chunks_are_ticking_in_sets(
    center: pumpkin_util::math::vector2::Vector2<i32>,
    active_chunks: &FxHashSet<pumpkin_util::math::vector2::Vector2<i32>>,
    is_loaded: impl Fn(&pumpkin_util::math::vector2::Vector2<i32>) -> bool,
) -> bool {
    (-1..=1).all(|dx| {
        (-1..=1).all(|dz| {
            let chunk = pumpkin_util::math::vector2::Vector2::new(center.x + dx, center.y + dz);
            active_chunks.contains(&chunk) && is_loaded(&chunk)
        })
    })
}

/// Re-registers a sensor loaded from disk in the world's flat game-event registry.
///
/// Vanilla constructs the vibration user/data/listener together in the block entity
/// constructor (`SculkSensorBlockEntity.java:25-30`), and its listener exposes the block
/// position and radius (`SculkSensorBlockEntity.java:72-94`).
pub async fn ensure_listener_registered(world: &Arc<World>, pos: &BlockPos) {
    let (block, _) = world.get_block_and_state(pos);
    let radius = match block.id {
        BlockId::SCULK_SENSOR => SculkSensorBlockEntity::LISTENER_RADIUS,
        BlockId::CALIBRATED_SCULK_SENSOR => CalibratedSculkSensorBlockEntity::LISTENER_RADIUS,
        _ => return,
    };

    let already_registered = world
        .game_event_listeners
        .lock()
        .await
        .iter()
        .any(|listener| {
            matches!(
                listener.listener_source(),
                PositionSource::Block(listener_pos) if listener_pos == *pos
            )
        });
    if !already_registered {
        world
            .register_game_event_listener(Arc::new(SculkSensorListener { pos: *pos, radius }))
            .await;
    }
}

/// The `VibrationSystem.Data` of either sensor's block entity.
fn vibration_data_of(block_entity: &dyn BlockEntity) -> Option<&Mutex<VibrationData>> {
    let block_entity = block_entity.as_any();
    if let Some(sensor) = block_entity.downcast_ref::<SculkSensorBlockEntity>() {
        return Some(&sensor.vibration_data);
    }
    block_entity
        .downcast_ref::<CalibratedSculkSensorBlockEntity>()
        .map(|calibrated| &calibrated.vibration_data)
}

/// `Level.getGameTime`, which stamps the selector's candidates.
async fn current_game_time(world: &World) -> u64 {
    u64::try_from(world.get_world_age().await).unwrap_or(0)
}

/// `VibrationSystem.Listener.distanceBetweenInBlocks` (`VibrationSystem.java:258-260`):
/// `Vec3i.distSqr` sums the squares as doubles.
fn distance_between_in_blocks(origin: BlockPos, dest: BlockPos) -> f32 {
    let dx = f64::from(origin.0.x) - f64::from(dest.0.x);
    let dy = f64::from(origin.0.y) - f64::from(dest.0.y);
    let dz = f64::from(origin.0.z) - f64::from(dest.0.z);
    (dx * dx + dy * dy + dz * dz).sqrt() as f32
}

/// `SculkSensorBlockEntity.VibrationUser.canReceiveVibration`
/// (`SculkSensorBlockEntity.java:101-108`) with the calibrated sensor's back-signal filter
/// (`CalibratedSculkSensorBlockEntity.java:34-46`). `event_pos` is
/// `BlockPos.containing(sourcePosition)`.
async fn can_receive_vibration(
    world: &Arc<World>,
    pos: &BlockPos,
    event_pos: &BlockPos,
    event: &GameEvent,
) -> bool {
    let (block, state) = world.get_block_and_state(pos);
    let frequency = vibration_frequency(event);
    let phase = if block.id == BlockId::SCULK_SENSOR {
        SculkSensorLikeProperties::from_state_id(state.id, block).sculk_sensor_phase
    } else if block.id == BlockId::CALIBRATED_SCULK_SENSOR {
        let props = CalibratedSculkSensorLikeProperties::from_state_id(state.id, block);
        let back_dir = horizontal_facing_to_dir(props.facing).opposite();
        let back_pos = pos.offset(back_dir.to_offset());
        let back_state = world.get_block_state(&back_pos);
        let back_block = Block::from_state_id(back_state.id);
        // `CalibratedSculkSensorBlockEntity.getBackSignal` is `Level.getSignal`
        // (`SignalGetter.java:65-68`): the weak signal, or for a conductive block also the
        // strong power directed at it, which `get_redstone_power` reads.
        let comparison_type =
            super::get_redstone_power(back_block, back_state, world, &back_pos, back_dir).await;
        if comparison_type > 0 && i32::from(comparison_type) != frequency {
            return false;
        }
        props.sculk_sensor_phase
    } else {
        return false;
    };

    // A block_destroy or block_place at the sensor's own position is ignored (the sensor's own
    // placement/removal must not self-trigger it).
    if event_pos == pos && matches!(event, GameEvent::BlockDestroy | GameEvent::BlockPlace) {
        return false;
    }
    // `SculkSensorBlock.canActivate`.
    frequency != 0 && phase == SculkSensorPhase::Inactive
}

/// `VibrationSystem.Ticker.tick` (`VibrationSystem.java:278-298`) for a sculk sensor.
///
/// The sensor's `VibrationUser` requires adjacent chunks to be ticking and has listener radius
/// `radius`. Returns whether vanilla would call `onDataChanged`, i.e. `setChanged`.
///
/// Vanilla also streams a `VibrationParticleOption` to clients when a vibration is selected
/// (`VibrationSystem.java:308-310`) and after a reload (`tryReloadVibrationParticle`); the
/// packet payload is protocol-version specific and purely visual, so it is not sent.
pub async fn tick_vibration(
    world: &Arc<World>,
    position: &BlockPos,
    vibration_data: &Mutex<VibrationData>,
    radius: i32,
) -> bool {
    {
        let data = vibration_data.lock().await;
        if data.current_vibration.is_none() && data.selector.is_empty() {
            return false;
        }
    }
    let game_time = current_game_time(world).await;
    let (selected, mut has_changed, due) = {
        let mut data = vibration_data.lock().await;
        let selected = data.current_vibration.is_none() && data.select_and_schedule(game_time);
        let Some(current) = data.current_vibration else {
            return selected;
        };
        let has_changed = data.travel_time > 0;
        data.travel_time = data.travel_time.saturating_sub(1);
        (
            selected,
            has_changed,
            (data.travel_time == 0).then_some(current),
        )
    };
    if let Some(current) = due {
        has_changed = receive_vibration(world, position, vibration_data, &current, radius).await;
    }
    selected || has_changed
}

/// `VibrationSystem.Ticker.receiveVibration` (`VibrationSystem.java:342-361`) and the sensor's
/// `onReceiveVibration` (`SculkSensorBlockEntity.java:110-128`). The vibration stays in
/// flight while an adjacent chunk is not ticking. The data lock is not held across the
/// activation, which can emit resonance events that reach this sensor's own listener.
async fn receive_vibration(
    world: &Arc<World>,
    position: &BlockPos,
    vibration_data: &Mutex<VibrationData>,
    vibration: &VibrationInfo,
    radius: i32,
) -> bool {
    if !adjacent_chunks_are_ticking(world, *position) {
        return false;
    }
    let origin = BlockPos::floored_v(vibration.pos);
    let power =
        redstone_strength_for_distance(distance_between_in_blocks(origin, *position), radius);
    let (block, _) = world.get_block_and_state(position);
    // `currentVibration.getEntity(serverLevel)`; the projectile owner is only read by the warden.
    let source_entity = vibration
        .source_entity
        .and_then(|uuid| entity_or_player_by_uuid(world, uuid));
    SculkSensorBlock::trigger(
        world,
        position,
        block,
        power,
        vibration.frequency,
        source_entity,
    )
    .await;
    vibration_data.lock().await.current_vibration = None;
    true
}

/// `ServerLevel.getEntity(uuid)`, which also finds players.
fn entity_or_player_by_uuid(world: &World, uuid: Uuid) -> Option<Arc<dyn EntityBase>> {
    if let Some(player) = world.get_player_by_uuid(uuid) {
        return Some(player);
    }
    world.get_entity_by_uuid(uuid)
}

impl GameEventListener for SculkSensorListener {
    fn listener_source(&self) -> PositionSource {
        PositionSource::Block(self.pos)
    }

    fn listener_radius(&self) -> i32 {
        self.radius
    }

    /// `VibrationSystem.Listener.handleGameEvent` (`VibrationSystem.java:209-237`): the event
    /// is not acted on here, only queued as a candidate for the block entity's ticker.
    /// `isOccluded` already ran in `emit_game_event`.
    fn handle_game_event<'a>(
        &'a self,
        world: &'a Arc<World>,
        event: &'a GameEvent,
        context: &'a GameEventContext,
        source_position: Vector3<f64>,
    ) -> GameEventFuture<'a> {
        Box::pin(async move {
            let (block, _) = world.get_block_and_state(&self.pos);
            if block.id != BlockId::SCULK_SENSOR && block.id != BlockId::CALIBRATED_SCULK_SENSOR {
                return false;
            }
            let Some(block_entity) = world.get_block_entity(&self.pos) else {
                return false;
            };
            let Some(vibration_data) = vibration_data_of(block_entity.as_ref()) else {
                return false;
            };

            let in_flight = vibration_data.lock().await.current_vibration.is_some();
            if in_flight {
                return false;
            }
            // `canTriggerAvoidVibration` is true for sculk sensors
            // (`SculkSensorBlockEntity.java:96-99`).
            if !is_valid_vibration(event, context, true).await {
                return false;
            }
            let Some(destination) = PositionSource::Block(self.pos).get_position(world) else {
                return false;
            };
            let event_pos = BlockPos::floored_v(source_position);
            if !can_receive_vibration(world, &self.pos, &event_pos, event).await {
                return false;
            }

            // `VibrationInfo(event, distance, origin, context.sourceEntity())`. The projectile
            // owner is only read by the warden, so it is not resolved here.
            let vibration = VibrationInfo {
                frequency: vibration_frequency(event),
                distance: (destination - source_position).length() as f32,
                pos: source_position,
                source_entity: context
                    .source_entity
                    .as_ref()
                    .map(|entity| entity.get_entity().entity_uuid),
                projectile_owner: None,
            };
            let game_time = current_game_time(world).await;
            vibration_data
                .lock()
                .await
                .selector
                .add_candidate(vibration, game_time);
            true
        })
    }
}

impl BlockMetadata for SculkSensorBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::SCULK_SENSOR, BlockId::CALIBRATED_SCULK_SENSOR].into()
    }
}

const fn horizontal_facing_to_dir(facing: HorizontalFacing) -> BlockDirection {
    match facing {
        HorizontalFacing::North => BlockDirection::North,
        HorizontalFacing::South => BlockDirection::South,
        HorizontalFacing::West => BlockDirection::West,
        HorizontalFacing::East => BlockDirection::East,
    }
}

/// Vanilla `VibrationSystem.getResonanceEventByFrequency`: frequency N maps to
/// `RESONATE_N` (1-15).
const fn resonance_event_by_frequency(frequency: i32) -> GameEvent {
    match frequency {
        1 => GameEvent::Resonate1,
        2 => GameEvent::Resonate2,
        3 => GameEvent::Resonate3,
        4 => GameEvent::Resonate4,
        5 => GameEvent::Resonate5,
        6 => GameEvent::Resonate6,
        7 => GameEvent::Resonate7,
        8 => GameEvent::Resonate8,
        9 => GameEvent::Resonate9,
        10 => GameEvent::Resonate10,
        11 => GameEvent::Resonate11,
        12 => GameEvent::Resonate12,
        13 => GameEvent::Resonate13,
        14 => GameEvent::Resonate14,
        _ => GameEvent::Resonate15,
    }
}

/// Vanilla `SculkSensorBlock.RESONANCE_PITCH_BEND` (`SculkSensorBlock.java:53-59`).
///
/// `NoteBlock.getPitchFromNote(toneMap[frequency])`, where
/// `getPitchFromNote(note) = 2^((note - 12) / 12)` (`NoteBlock.java:143-145`).
#[must_use]
pub fn resonance_pitch_bend(frequency: i32) -> f32 {
    const TONE_MAP: [i32; 16] = [0, 0, 2, 4, 6, 7, 9, 10, 12, 14, 15, 18, 19, 21, 22, 24];
    let index = frequency.clamp(0, 15) as usize;
    f32::powf(2.0, (TONE_MAP[index] - 12) as f32 / 12.0)
}

impl SculkSensorBlock {
    /// Vanilla `SculkSensorBlock.tryResonateVibration` (`SculkSensorBlock.java:233-243`):
    /// every adjacent `minecraft:vibration_resonators` block (amethyst) re-emits the
    /// `RESONATE_<frequency>` game event, with the source entity and that block's state as its
    /// context (`GameEvent.Context.of(sourceEntity, blockState)`), and plays the resonating
    /// sound at the frequency's pitch bend (`RESONANCE_PITCH_BEND`, `SculkSensorBlock.java:53-59`,
    /// pitch via `NoteBlock.getPitchFromNote`, `NoteBlock.java:143-145`).
    pub async fn try_resonate_vibration(
        world: &Arc<World>,
        pos: &BlockPos,
        frequency: i32,
        source_entity: Option<&Arc<dyn EntityBase>>,
    ) {
        for direction in BlockDirection::all() {
            let relative_pos = pos.offset(direction.to_offset());
            let neighbor_state = world.get_block_state(&relative_pos);
            let neighbor_block = Block::from_state_id(neighbor_state.id);
            if !neighbor_block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_VIBRATION_RESONATORS) {
                continue;
            }
            crate::world::game_event::emit_game_event(
                world,
                resonance_event_by_frequency(frequency),
                relative_pos.to_centered_f64(),
                GameEventContext {
                    source_entity: source_entity.cloned(),
                    affected_block_state: Some(neighbor_state.id),
                },
            )
            .await;
            world.play_sound_fine(
                Sound::BlockAmethystBlockResonate,
                SoundCategory::Blocks,
                &relative_pos.to_centered_f64(),
                1.0,
                resonance_pitch_bend(frequency),
            );
        }
    }

    /// The tail of `SculkSensorBlock.activate` (`SculkSensorBlock.java:206-231`) after the
    /// state change and resonance: the `SCULK_SENSOR_TENDRILS_CLICKING` game event, which
    /// sculk shriekers and wardens listen to, and the clicking sound unless waterlogged.
    async fn emit_tendrils_clicking(
        world: &Arc<World>,
        pos: &BlockPos,
        source_entity: Option<Arc<dyn EntityBase>>,
        waterlogged: bool,
    ) {
        let context =
            source_entity.map_or_else(GameEventContext::none, GameEventContext::of_entity);
        crate::world::game_event::emit_game_event(
            world,
            GameEvent::SculkSensorTendrilsClicking,
            pos.to_centered_f64(),
            context,
        )
        .await;
        if !waterlogged {
            world.play_sound_fine(
                Sound::BlockSculkSensorClicking,
                SoundCategory::Blocks,
                &pos.to_centered_f64(),
                1.0,
                rng().random::<f32>().mul_add(0.2, 0.8),
            );
        }
    }

    /// `SculkSensorBlock.activate` (`SculkSensorBlock.java:206-231`) as reached from
    /// `SculkSensorBlockEntity.VibrationUser.onReceiveVibration`
    /// (`SculkSensorBlockEntity.java:119-127`): the frequency is recorded before the state
    /// changes so neighbouring comparators read it.
    pub async fn trigger(
        world: &Arc<World>,
        pos: &BlockPos,
        block: &Block,
        power: u8,
        frequency: i32,
        source_entity: Option<Arc<dyn EntityBase>>,
    ) {
        if block.id == BlockId::SCULK_SENSOR {
            let state = world.get_block_state(pos);
            let mut props = SculkSensorLikeProperties::from_state_id(state.id, block);
            if props.sculk_sensor_phase == SculkSensorPhase::Inactive {
                if let Some(block_entity) = world.get_block_entity(pos)
                    && let Some(sculk_sensor) = block_entity
                        .as_any()
                        .downcast_ref::<crate::block::entities::sculk_sensor::SculkSensorBlockEntity>()
                {
                    sculk_sensor.set_last_vibration_frequency(frequency).await;
                }

                props.sculk_sensor_phase = SculkSensorPhase::Active;
                props.power = power;
                world
                    .set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
                    .await;
                world.update_neighbors(pos, None).await;
                world.schedule_block_tick(block, *pos, 30, TickPriority::Normal);
                Self::try_resonate_vibration(world, pos, frequency, source_entity.as_ref()).await;
                Self::emit_tendrils_clicking(world, pos, source_entity, props.waterlogged).await;
            }
        } else if block.id == BlockId::CALIBRATED_SCULK_SENSOR {
            let state = world.get_block_state(pos);
            let mut props = CalibratedSculkSensorLikeProperties::from_state_id(state.id, block);
            if props.sculk_sensor_phase == SculkSensorPhase::Inactive {
                if let Some(block_entity) = world.get_block_entity(pos)
                    && let Some(calibrated) = block_entity
                        .as_any()
                        .downcast_ref::<crate::block::entities::calibrated_sculk_sensor::CalibratedSculkSensorBlockEntity>()
                {
                    calibrated.set_last_vibration_frequency(frequency).await;
                }

                props.sculk_sensor_phase = SculkSensorPhase::Active;
                props.power = power;
                world
                    .set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
                    .await;
                world.update_neighbors(pos, None).await;
                // CalibratedSculkSensorBlock overrides getActiveTicks() to 10.
                world.schedule_block_tick(block, *pos, 10, TickPriority::Normal);
                // The calibrated sensor extends `SculkSensorBlock` in vanilla and inherits
                // `activate`, so it resonates adjacent amethyst and clicks the same way.
                Self::try_resonate_vibration(world, pos, frequency, source_entity.as_ref()).await;
                Self::emit_tendrils_clicking(world, pos, source_entity, props.waterlogged).await;
            }
        }
    }
}

/// `SculkSensorBlock.tick` (`SculkSensorBlock.java:84-95`): the COOLDOWN -> INACTIVE step
/// plays `SCULK_CLICKING_STOP` at the block position unless the sensor is waterlogged.
fn play_clicking_stop(world: &World, pos: &BlockPos) {
    world.play_sound_fine(
        Sound::BlockSculkSensorClickingStop,
        SoundCategory::Blocks,
        &pos.to_centered_f64(),
        1.0,
        rng().random::<f32>().mul_add(0.2, 0.8),
    );
}

impl BlockBehaviour for SculkSensorBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if args.block.id == BlockId::CALIBRATED_SCULK_SENSOR {
                let mut props = CalibratedSculkSensorLikeProperties::default(args.block);
                props.facing = args.player.living_entity.entity.get_horizontal_facing();
                props.to_state_id(args.block)
            } else {
                let props = SculkSensorLikeProperties::default(args.block);
                props.to_state_id(args.block)
            }
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.block.id == BlockId::CALIBRATED_SCULK_SENSOR {
                let entity = CalibratedSculkSensorBlockEntity::new(*args.position);
                args.world.add_block_entity(Arc::new(entity));
            } else if args.block.id == BlockId::SCULK_SENSOR {
                let entity = SculkSensorBlockEntity::new(*args.position);
                args.world.add_block_entity(Arc::new(entity));
            }
            ensure_listener_registered(args.world, args.position).await;
        })
    }

    /// Vanilla `SculkSensorBlock.spawnAfterBreak` (`SculkSensorBlock.java:289-294`): breaking
    /// a sensor with drops enabled pops 5 experience (`tryDropExperience(ConstantInt.of(5))`).
    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let tool = args.player.inventory().held_item().await;
            if !crate::block::blocks::sculk::sculk_catalyst::should_drop_experience(
                args.drop_experience,
                args.world.level_info.load().game_rules.block_drops,
                args.player.gamemode.load(),
                tool.get_enchantment_level(&pumpkin_data::Enchantment::SILK_TOUCH) > 0,
            ) {
                return;
            }
            ExperienceOrbEntity::spawn(args.world, args.position.to_centered_f64(), 5).await;
        })
    }

    fn on_state_replaced<'a>(&'a self, args: OnStateReplacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world
                .unregister_game_event_listener_at(args.position)
                .await;
            // Vanilla `SculkSensorBlock.affectNeighborsAfterRemoval`
            // (`SculkSensorBlock.java:121-125`): a sensor broken while ACTIVE re-notifies
            // its own and the block below's neighbors so stale redstone power clears.
            let phase = if args.block.id == BlockId::SCULK_SENSOR {
                SculkSensorLikeProperties::from_state_id(args.old_state_id, args.block)
                    .sculk_sensor_phase
            } else {
                CalibratedSculkSensorLikeProperties::from_state_id(args.old_state_id, args.block)
                    .sculk_sensor_phase
            };
            if phase == SculkSensorPhase::Active {
                args.world.update_neighbors(args.position, None).await;
                args.world
                    .update_neighbors(&args.position.down(), None)
                    .await;
            }
        })
    }

    /// Vanilla `SculkSensorBlock.stepOn` (`SculkSensorBlock.java:98-109`): an entity (other
    /// than the warden) walking on top of an INACTIVE sensor force-schedules a STEP
    /// vibration at the sensor (`forceScheduleVibration`, `VibrationSystem.java:239-245`):
    /// it skips the in-flight, validity and occlusion checks of an ordinary event but is
    /// still a candidate delivered by the sensor's ticker.
    fn on_entity_step<'a>(&'a self, args: OnEntityStepArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.entity.get_entity().entity_type == &EntityType::WARDEN {
                return;
            }
            let event = GameEvent::Step;
            if !can_receive_vibration(args.world, args.position, args.position, &event).await {
                return;
            }
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return;
            };
            let Some(vibration_data) = vibration_data_of(block_entity.as_ref()) else {
                return;
            };
            let entity = args.entity.get_entity();
            let origin = entity.pos.load();
            let vibration = VibrationInfo {
                frequency: vibration_frequency(&event),
                distance: (args.position.to_centered_f64() - origin).length() as f32,
                pos: origin,
                source_entity: Some(entity.entity_uuid),
                projectile_owner: None,
            };
            let game_time = current_game_time(args.world).await;
            vibration_data
                .lock()
                .await
                .selector
                .add_candidate(vibration, game_time);
        })
    }

    fn emits_redstone_power<'a>(
        &'a self,
        _args: EmitsRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, bool> {
        Box::pin(async move { true })
    }

    fn get_weak_redstone_power<'a>(
        &'a self,
        args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(async move {
            if args.block.id == BlockId::SCULK_SENSOR {
                let props = SculkSensorLikeProperties::from_state_id(args.state.id, args.block);
                if props.sculk_sensor_phase == SculkSensorPhase::Active {
                    props.power
                } else {
                    0
                }
            } else if args.block.id == BlockId::CALIBRATED_SCULK_SENSOR {
                let props =
                    CalibratedSculkSensorLikeProperties::from_state_id(args.state.id, args.block);
                if props.sculk_sensor_phase == SculkSensorPhase::Active
                    && args.direction != props.facing.to_block_direction()
                {
                    props.power
                } else {
                    0
                }
            } else {
                0
            }
        })
    }

    /// Vanilla `SculkSensorBlock.getDirectSignal` (`SculkSensorBlock.java:183-185`): the
    /// sensor only propagates strong power out of its top face
    /// (`direction == UP ? state.getSignal(...) : 0`); inherited by the calibrated sensor.
    fn get_strong_redstone_power<'a>(
        &'a self,
        args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(async move {
            if args.direction == BlockDirection::Up {
                self.get_weak_redstone_power(args).await
            } else {
                0
            }
        })
    }

    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            let be = args.world.get_block_entity(args.position)?;
            if let Some(sensor_be) = be.as_any().downcast_ref::<SculkSensorBlockEntity>() {
                return Some(*sensor_be.last_vibration_frequency.lock().await as u8);
            }
            if let Some(cal_be) = be
                .as_any()
                .downcast_ref::<CalibratedSculkSensorBlockEntity>()
            {
                return Some(*cal_be.last_vibration_frequency.lock().await as u8);
            }
            None
        })
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            if args.block.id == BlockId::SCULK_SENSOR {
                let mut props = SculkSensorLikeProperties::from_state_id(state.id, args.block);
                match props.sculk_sensor_phase {
                    SculkSensorPhase::Active => {
                        props.sculk_sensor_phase = SculkSensorPhase::Cooldown;
                        props.power = 0;
                        args.world
                            .set_block_state(
                                args.position,
                                props.to_state_id(args.block),
                                BlockFlags::NOTIFY_ALL,
                            )
                            .await;
                        args.world.schedule_block_tick(
                            args.block,
                            *args.position,
                            10,
                            TickPriority::Normal,
                        );
                        args.world.update_neighbors(args.position, None).await;
                    }
                    SculkSensorPhase::Cooldown => {
                        props.sculk_sensor_phase = SculkSensorPhase::Inactive;
                        props.power = 0;
                        args.world
                            .set_block_state(
                                args.position,
                                props.to_state_id(args.block),
                                BlockFlags::NOTIFY_ALL,
                            )
                            .await;
                        args.world.update_neighbors(args.position, None).await;
                        if !props.waterlogged {
                            play_clicking_stop(args.world, args.position);
                        }
                    }
                    SculkSensorPhase::Inactive => {}
                }
            } else if args.block.id == BlockId::CALIBRATED_SCULK_SENSOR {
                let mut props =
                    CalibratedSculkSensorLikeProperties::from_state_id(state.id, args.block);
                match props.sculk_sensor_phase {
                    SculkSensorPhase::Active => {
                        props.sculk_sensor_phase = SculkSensorPhase::Cooldown;
                        props.power = 0;
                        args.world
                            .set_block_state(
                                args.position,
                                props.to_state_id(args.block),
                                BlockFlags::NOTIFY_ALL,
                            )
                            .await;
                        args.world.schedule_block_tick(
                            args.block,
                            *args.position,
                            10,
                            TickPriority::Normal,
                        );
                        args.world.update_neighbors(args.position, None).await;
                    }
                    SculkSensorPhase::Cooldown => {
                        props.sculk_sensor_phase = SculkSensorPhase::Inactive;
                        props.power = 0;
                        args.world
                            .set_block_state(
                                args.position,
                                props.to_state_id(args.block),
                                BlockFlags::NOTIFY_ALL,
                            )
                            .await;
                        args.world.update_neighbors(args.position, None).await;
                        if !props.waterlogged {
                            play_clicking_stop(args.world, args.position);
                        }
                    }
                    SculkSensorPhase::Inactive => {}
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{adjacent_chunks_are_ticking_in_sets, distance_between_in_blocks};
    use pumpkin_util::math::position::BlockPos;
    use rustc_hash::FxHashSet;

    #[test]
    fn receiving_distance_is_the_euclidean_block_distance() {
        // `distanceBetweenInBlocks` is `sqrt(origin.distSqr(dest))` (`VibrationSystem.java:258-260`).
        let origin = BlockPos::new(0, 0, 0);
        assert!((distance_between_in_blocks(origin, BlockPos::new(3, 4, 0)) - 5.0).abs() < 1.0e-6);
        assert!(distance_between_in_blocks(origin, origin).abs() < 1.0e-6);
        assert!(
            (distance_between_in_blocks(BlockPos::new(-2, 1, 5), BlockPos::new(-2, 1, -3)) - 8.0)
                .abs()
                < 1.0e-6
        );
    }

    #[test]
    fn sensor_requires_all_adjacent_chunks_to_be_active_and_loaded() {
        // `VibrationSystem.Ticker.areAdjacentChunksTicking` checks the full 3x3 neighborhood
        // (`VibrationSystem.java:363-374`).
        let center = pumpkin_util::math::vector2::Vector2::new(0, 0);
        let mut active = FxHashSet::default();
        let mut loaded = FxHashSet::default();
        for dx in -1..=1 {
            for dz in -1..=1 {
                let chunk = pumpkin_util::math::vector2::Vector2::new(dx, dz);
                active.insert(chunk);
                loaded.insert(chunk);
            }
        }

        assert!(adjacent_chunks_are_ticking_in_sets(
            center,
            &active,
            |chunk| { loaded.contains(chunk) }
        ));
        loaded.remove(&pumpkin_util::math::vector2::Vector2::new(1, 1));
        assert!(!adjacent_chunks_are_ticking_in_sets(
            center,
            &active,
            |chunk| { loaded.contains(chunk) }
        ));
    }
}
