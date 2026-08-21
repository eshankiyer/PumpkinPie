// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;

use crate::block::entities::calibrated_sculk_sensor::CalibratedSculkSensorBlockEntity;
use crate::block::entities::sculk_sensor::SculkSensorBlockEntity;
use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, EmitsRedstonePowerArgs, GetComparatorOutputArgs,
    GetRedstonePowerArgs, OnPlaceArgs, OnScheduledTickArgs, OnStateReplacedArgs, PlacedArgs,
};
use crate::world::World;
use crate::world::game_event::{
    GameEventContext, GameEventFuture, GameEventListener, PositionSource,
    redstone_strength_for_distance,
};
use pumpkin_data::block_properties::{
    BlockProperties, CalibratedSculkSensorLikeProperties, HorizontalFacing,
    SculkSensorLikeProperties, SculkSensorPhase,
};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::{Block, BlockDirection, BlockId, BlockStateId, HorizontalFacingExt};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

pub struct SculkSensorBlock;

// SculkSensorBlockEntity.VibrationUser.LISTENER_RANGE = 8.
const LISTENER_RANGE: i32 = 8;

struct SculkSensorListener {
    pos: BlockPos,
}

impl GameEventListener for SculkSensorListener {
    fn listener_source(&self) -> PositionSource {
        PositionSource::Block(self.pos)
    }

    fn listener_radius(&self) -> i32 {
        LISTENER_RANGE
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
            let power = redstone_strength_for_distance(distance, LISTENER_RANGE);
            let frequency = crate::world::game_event::vibration_frequency(event);

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

impl SculkSensorBlock {
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

                if calibrated_freq > 0 && calibrated_freq != power {
                    return;
                }

                if let Some(be) = world.get_block_entity(pos)
                    && let Some(cal_be) = be
                        .as_any()
                        .downcast_ref::<CalibratedSculkSensorBlockEntity>()
                {
                    *cal_be.last_vibration_frequency.lock().await = power as i32;
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
                }))
                .await;
        })
    }

    fn on_state_replaced<'a>(&'a self, args: OnStateReplacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world
                .unregister_game_event_listener_at(args.position)
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
