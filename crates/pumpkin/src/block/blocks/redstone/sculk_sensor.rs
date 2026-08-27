// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;

use crate::block::entities::calibrated_sculk_sensor::CalibratedSculkSensorBlockEntity;
use crate::block::entities::sculk_sensor::SculkSensorBlockEntity;
use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, BrokenArgs, EmitsRedstonePowerArgs,
    GetComparatorOutputArgs, GetRedstonePowerArgs, OnEntityStepArgs, OnPlaceArgs,
    OnScheduledTickArgs, OnStateReplacedArgs, PlacedArgs,
};
use crate::entity::boss::ender_dragon::Vector3Ext;
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::world::World;
use crate::world::game_event::{
    GameEventContext, GameEventFuture, GameEventListener, PositionSource,
    redstone_strength_for_distance,
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
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

pub struct SculkSensorBlock;

// SculkSensorBlockEntity.VibrationUser.LISTENER_RANGE = 8.
const LISTENER_RANGE: i32 = 8;

struct SculkSensorListener {
    pos: BlockPos,
    radius: i32,
}

impl GameEventListener for SculkSensorListener {
    fn listener_source(&self) -> PositionSource {
        PositionSource::Block(self.pos)
    }

    fn listener_radius(&self) -> i32 {
        self.radius
    }

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

            // SculkSensorBlockEntity.VibrationUser.canReceiveVibration: a block_destroy
            // or block_place at the sensor's own position is ignored (the sensor's own
            // placement/removal must not self-trigger it).
            let event_pos = BlockPos::new(
                source_position.x.floor() as i32,
                source_position.y.floor() as i32,
                source_position.z.floor() as i32,
            );
            if event_pos == self.pos
                && matches!(event, GameEvent::BlockDestroy | GameEvent::BlockPlace)
            {
                return false;
            }

            let listener_pos = PositionSource::Block(self.pos)
                .get_position(world)
                .expect("block position source always resolves");
            let distance = (listener_pos - source_position).length() as f32;
            let power = redstone_strength_for_distance(distance, self.radius);
            let frequency = crate::world::game_event::vibration_frequency(event);
            // Vanilla `SculkSensorBlockEntity.VibrationUser.canReceiveVibration`
            // (`SculkSensorBlockEntity.java:102-107`) rejects events with no vibration
            // frequency before delegating to the sensor activation check.
            if frequency == 0 {
                return false;
            }

            let _ = context;
            SculkSensorBlock::trigger(world, &self.pos, block, power, frequency).await;
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
    /// `RESONATE_<frequency>` game event and plays the resonating sound at the frequency's
    /// pitch bend (`RESONANCE_PITCH_BEND`, `SculkSensorBlock.java:53-59`, pitch via
    /// `NoteBlock.getPitchFromNote`, `NoteBlock.java:143-145`).
    pub async fn try_resonate_vibration(world: &Arc<World>, pos: &BlockPos, frequency: i32) {
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
                GameEventContext::none(),
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

    pub async fn trigger(
        world: &Arc<World>,
        pos: &BlockPos,
        block: &Block,
        power: u8,
        frequency: i32,
    ) {
        if block.id == BlockId::SCULK_SENSOR {
            let state = world.get_block_state(pos);
            let mut props = SculkSensorLikeProperties::from_state_id(state.id, block);
            if props.sculk_sensor_phase == SculkSensorPhase::Inactive {
                if let Some(be) = world.get_block_entity(pos)
                    && let Some(sensor_be) = be.as_any().downcast_ref::<SculkSensorBlockEntity>()
                {
                    *sensor_be.last_vibration_frequency.lock().await = power as i32;
                }

                props.sculk_sensor_phase = SculkSensorPhase::Active;
                props.power = power;
                world
                    .set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
                    .await;
                world.update_neighbors(pos, None).await;
                world.schedule_block_tick(block, *pos, 30, TickPriority::Normal);
                if let Some(block_entity) = world.get_block_entity(pos)
                    && let Some(sculk_sensor) = block_entity
                        .as_any()
                        .downcast_ref::<crate::block::entities::sculk_sensor::SculkSensorBlockEntity>()
                {
                    sculk_sensor.set_last_vibration_frequency(frequency).await;
                }
                Self::try_resonate_vibration(world, pos, frequency).await;
            }
        } else if block.id == BlockId::CALIBRATED_SCULK_SENSOR {
            let state = world.get_block_state(pos);
            let mut props = CalibratedSculkSensorLikeProperties::from_state_id(state.id, block);
            if props.sculk_sensor_phase == SculkSensorPhase::Inactive {
                let back_dir = horizontal_facing_to_dir(props.facing).opposite();
                let back_pos = pos.offset(back_dir.to_offset());
                let back_state = world.get_block_state(&back_pos);
                let back_block = Block::from_state_id(back_state.id);

                let calibrated_freq = world
                    .block_registry
                    .get_weak_redstone_power(back_block, world, &back_pos, back_state, back_dir)
                    .await;

                // Vanilla `CalibratedSculkSensorBlockEntity.VibrationUser.canReceiveVibration`
                // (`CalibratedSculkSensorBlockEntity.java:35-40`) compares the back signal to
                // the event frequency, not to the distance-derived redstone power.
                if calibrated_freq > 0 && i32::from(calibrated_freq) != frequency {
                    return;
                }

                if let Some(be) = world.get_block_entity(pos)
                    && let Some(cal_be) = be
                        .as_any()
                        .downcast_ref::<CalibratedSculkSensorBlockEntity>()
                {
                    *cal_be.last_vibration_frequency.lock().await = frequency;
                }

                props.sculk_sensor_phase = SculkSensorPhase::Active;
                props.power = power;
                world
                    .set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
                    .await;
                world.update_neighbors(pos, None).await;
                // CalibratedSculkSensorBlock overrides getActiveTicks() to 10.
                world.schedule_block_tick(block, *pos, 10, TickPriority::Normal);
                if let Some(block_entity) = world.get_block_entity(pos)
                    && let Some(calibrated) = block_entity
                        .as_any()
                        .downcast_ref::<crate::block::entities::calibrated_sculk_sensor::CalibratedSculkSensorBlockEntity>()
                {
                    calibrated.set_last_vibration_frequency(frequency).await;
                }
                // The calibrated sensor extends `SculkSensorBlock` in vanilla and inherits
                // `activate`, so it resonates adjacent amethyst the same way.
                Self::try_resonate_vibration(world, pos, frequency).await;
            }
        }
    }
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
            args.world
                .register_game_event_listener(Arc::new(SculkSensorListener {
                    pos: *args.position,
                    radius: if args.block.id == BlockId::CALIBRATED_SCULK_SENSOR {
                        CalibratedSculkSensorBlockEntity::LISTENER_RADIUS
                    } else {
                        LISTENER_RANGE
                    },
                }))
                .await;
        })
    }

    /// Vanilla `SculkSensorBlock.spawnAfterBreak` (`SculkSensorBlock.java:289-294`): breaking
    /// a sensor with drops enabled pops 5 experience (`tryDropExperience(ConstantInt.of(5))`).
    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !matches!(
                args.player.gamemode.load(),
                GameMode::Creative | GameMode::Spectator
            ) {
                ExperienceOrbEntity::spawn(args.world, args.position.to_centered_f64(), 5).await;
            }
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
    /// vibration at the sensor, i.e. triggers it regardless of distance/occlusion.
    fn on_entity_step<'a>(&'a self, args: OnEntityStepArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.entity.get_entity().entity_type == &EntityType::WARDEN {
                return;
            }
            let phase = if args.block.id == BlockId::SCULK_SENSOR {
                SculkSensorLikeProperties::from_state_id(args.state.id, args.block)
                    .sculk_sensor_phase
            } else {
                CalibratedSculkSensorLikeProperties::from_state_id(args.state.id, args.block)
                    .sculk_sensor_phase
            };
            if phase != SculkSensorPhase::Inactive {
                return;
            }
            let listener_pos = args.position.to_centered_f64();
            let distance = listener_pos
                .distance_squared(args.entity.get_entity().pos.load())
                .sqrt();
            let listener_radius = if args.block.id == BlockId::CALIBRATED_SCULK_SENSOR {
                CalibratedSculkSensorBlockEntity::LISTENER_RADIUS
            } else {
                LISTENER_RANGE
            };
            let power = redstone_strength_for_distance(distance as f32, listener_radius);
            Self::trigger(
                args.world,
                args.position,
                args.block,
                power,
                crate::world::game_event::vibration_frequency(&GameEvent::Step),
            )
            .await;
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
                    }
                    SculkSensorPhase::Inactive => {}
                }
            }
        })
    }
}
