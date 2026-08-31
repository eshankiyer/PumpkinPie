use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, DriedGhastLikeProperties, HorizontalFacing};
use pumpkin_data::entity::EntityType;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use uuid::Uuid;

use crate::block::{
    BlockBehaviour, BlockFuture, GetStateForNeighborUpdateArgs, OnPlaceArgs, OnScheduledTickArgs,
    PlayerPlacedArgs, RandomTickArgs,
};
use crate::entity::EntityBase;

/// `DriedGhastBlock.MAX_HYDRATION_LEVEL` (`DriedGhastBlock.java:40`).
const MAX_HYDRATION_LEVEL: u8 = 3;
/// `DriedGhastBlock.HYDRATION_TICK_DELAY` (`DriedGhastBlock.java:43`).
const HYDRATION_TICK_DELAY: i64 = 5000;

#[pumpkin_block("minecraft:dried_ghast")]
pub struct DriedGhastBlock;

/// `Direction.getYRot`.
const fn direction_yaw(facing: HorizontalFacing) -> f32 {
    match facing {
        HorizontalFacing::South => 0.0,
        HorizontalFacing::West => 90.0,
        HorizontalFacing::North => 180.0,
        HorizontalFacing::East => 270.0,
    }
}

impl BlockBehaviour for DriedGhastBlock {
    /// `DriedGhastBlock.getStateForPlacement` (`DriedGhastBlock.java:168-173`).
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = DriedGhastLikeProperties::default(args.block);
            props.waterlogged = args.replacing.water_source();
            props.facing = args.player.get_entity().get_horizontal_facing().opposite();
            props.to_state_id(args.block)
        })
    }

    /// `DriedGhastBlock.updateShape` (`DriedGhastBlock.java:61-77`).
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let props = DriedGhastLikeProperties::from_state_id(args.state_id, args.block);
            if props.waterlogged {
                args.world.schedule_fluid_tick(
                    &Fluid::WATER,
                    *args.position,
                    Fluid::WATER.flow_speed as u8,
                    TickPriority::Normal,
                );
            }
            args.state_id
        })
    }

    /// `DriedGhastBlock.randomTick` (`DriedGhastBlock.java:161-166`): arm the hydration timer, but
    /// never stack a second one on top of a pending tick.
    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let props = DriedGhastLikeProperties::from_state_id(state_id, args.block);
            if (props.waterlogged || props.hydration > 0)
                && !args
                    .world
                    .is_block_tick_scheduled(args.position, args.block)
            {
                args.world.schedule_block_tick_long(
                    args.block,
                    *args.position,
                    HYDRATION_TICK_DELAY,
                    TickPriority::Normal,
                );
            }
        })
    }

    /// `DriedGhastBlock.tick` (`DriedGhastBlock.java:92-113`).
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let mut props = DriedGhastLikeProperties::from_state_id(state_id, args.block);

            if !props.waterlogged {
                // Out of water the block dries back out one hydration level at a time.
                if props.hydration > 0 {
                    props.hydration -= 1;
                    args.world
                        .set_block_state(
                            args.position,
                            props.to_state_id(args.block),
                            BlockFlags::NOTIFY_LISTENERS,
                        )
                        .await;
                    // DriedGhastBlock.tick emits BLOCK_CHANGE after drying
                    // (DriedGhastBlock.java:93-101).
                    crate::world::game_event::emit_game_event(
                        args.world,
                        GameEvent::BlockChange,
                        args.position.to_centered_f64(),
                        crate::world::game_event::GameEventContext {
                            source_entity: None,
                            affected_block_state: Some(state_id),
                        },
                    )
                    .await;
                }
                return;
            }

            // `tickWaterlogged` (`DriedGhastBlock.java:105-113`).
            if props.hydration < MAX_HYDRATION_LEVEL {
                args.world.play_sound(
                    Sound::BlockDriedGhastTransition,
                    SoundCategory::Blocks,
                    &args.position.to_f64(),
                );
                props.hydration += 1;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_LISTENERS,
                    )
                    .await;
                // DriedGhastBlock.tickWaterlogged emits BLOCK_CHANGE after each
                // hydration step (DriedGhastBlock.java:105-109).
                crate::world::game_event::emit_game_event(
                    args.world,
                    GameEvent::BlockChange,
                    args.position.to_centered_f64(),
                    crate::world::game_event::GameEventContext {
                        source_entity: None,
                        affected_block_state: Some(state_id),
                    },
                )
                .await;
                return;
            }

            // `spawnGhastling` (`DriedGhastBlock.java:115-127`).
            args.world
                .set_block_state(
                    args.position,
                    pumpkin_data::Block::AIR.default_state.id,
                    BlockFlags::NOTIFY_ALL,
                )
                .await;

            let yaw = direction_yaw(props.facing);
            let spawn_at = Vector3::new(
                f64::from(args.position.0.x) + 0.5,
                f64::from(args.position.0.y),
                f64::from(args.position.0.z) + 0.5,
            );
            let ghastling = crate::entity::r#type::from_type(
                &EntityType::HAPPY_GHAST,
                spawn_at,
                args.world,
                Uuid::new_v4(),
            );
            ghastling.get_entity().set_age(-24000);
            ghastling.get_entity().set_rotation(yaw, 0.0);
            args.world.spawn_entity(ghastling).await;
            args.world.play_sound(
                Sound::EntityGhastlingSpawn,
                SoundCategory::Blocks,
                &spawn_at,
            );
        })
    }

    /// `DriedGhastBlock.setPlacedBy` (`DriedGhastBlock.java:195-201`).
    fn player_placed<'a>(&'a self, args: PlayerPlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let props = DriedGhastLikeProperties::from_state_id(args.state_id, args.block);
            let sound = if props.waterlogged {
                Sound::BlockDriedGhastPlaceInWater
            } else {
                Sound::BlockDriedGhastPlace
            };
            args.world
                .play_sound(sound, SoundCategory::Blocks, &args.position.to_f64());
        })
    }
}
