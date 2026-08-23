use pumpkin_data::block_properties::{BlockProperties, TurtleEggLikeProperties};
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockDirection, BlockStateId, tag};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::{RngExt, rng};
use std::sync::atomic::Ordering::Relaxed;

use crate::block::{
    BlockBehaviour, BlockFuture, BlockIsReplacing, BrokenArgs, CanPlaceAtArgs, CanUpdateAtArgs,
    GetStateForNeighborUpdateArgs, OnEntityStepArgs, OnLandedUponArgs, OnPlaceArgs,
    OnScheduledTickArgs, RandomTickArgs,
};
use crate::entity::EntityBase;
use crate::world::game_event::{GameEventContext, emit_game_event};

type TurtleEggProperties = TurtleEggLikeProperties;

#[pumpkin_block("minecraft:turtle_egg")]
pub struct TurtleEggBlock;

impl BlockBehaviour for TurtleEggBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if args.player.get_entity().pose.load() != EntityPose::Crouching
                && let BlockIsReplacing::Itself(state_id) = args.replacing
            {
                let mut properties = TurtleEggProperties::from_state_id(state_id, args.block);
                if properties.eggs < 4 {
                    properties.eggs += 1;
                }
                return properties.to_state_id(args.block);
            }

            let properties = TurtleEggProperties::default(args.block);
            properties.to_state_id(args.block)
        })
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_place_at(args.block_accessor, args.position)
    }

    fn can_update_at(&self, args: CanUpdateAtArgs<'_>) -> bool {
        let b = BlockAccessor::get_block(args.world, args.position);
        args.player.get_entity().pose.load() != EntityPose::Crouching
            && TurtleEggProperties::from_state_id(args.state_id, args.block).eggs < 4
            && args.block.id == b.id
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if !can_place_at(args.world, args.position) {
                args.world
                    .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
            }
            args.state_id
        })
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !can_place_at(args.world.as_ref(), args.position) {
                args.world
                    .break_block(args.position, None, BlockFlags::empty())
                    .await;
            }
        })
    }

    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // Turtle eggs can only hatch when placed on sand
            if !args
                .world
                .get_block(&args.position.down())
                .has_tag(&tag::Block::MINECRAFT_SAND)
            {
                return;
            }

            let state_id = args.world.get_block_state_id(args.position);
            let mut props = TurtleEggProperties::from_state_id(state_id, args.block);

            if props.hatch < 2 {
                props.hatch += 1;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;

                args.world.play_sound(
                    Sound::EntityTurtleEggCrack,
                    SoundCategory::Blocks,
                    &args.position.to_f64(),
                );
            } else {
                args.world
                    .break_block(args.position, None, BlockFlags::SKIP_DROPS)
                    .await;

                args.world.play_sound(
                    Sound::EntityTurtleEggHatch,
                    SoundCategory::Blocks,
                    &args.position.to_f64(),
                );
            }
        })
    }

    fn on_landed_upon<'a>(&'a self, args: OnLandedUponArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if let Some(living) = args.entity.get_living_entity() {
                living
                    .handle_fall_damage(args.entity, args.fall_distance, 1.0)
                    .await;
            }

            // Vanilla `fallOn` (TurtleEggBlock.java:65-71): falling onto the egg (zombies are
            // immune) rolls against randomness 3.
            if args.entity.get_entity().entity_type.id != EntityType::ZOMBIE.id {
                let block = args.world.get_block(args.position);
                destroy_egg(args.world, block, args.position, args.entity, 3).await;
            }
        })
    }

    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // Vanilla `playerDestroy` (TurtleEggBlock.java:142-152): after the normal break
            // (loot handled by the engine), the cluster loses exactly one egg - mining a
            // 4-egg block leaves a 3-egg block behind. `registry.broken` is only invoked
            // from the player-mining path (`entity/player.rs`), matching vanilla's context.
            decrease_eggs(args.world, args.block, args.position).await;
        })
    }

    /// Vanilla `stepOn` (TurtleEggBlock.java:56-62): anything not stepping carefully
    /// (`isSteppingCarefully` == shift-key-down, `Entity.java:2681-2683`) can crush an egg,
    /// rolled each tick against randomness 100.
    fn on_entity_step<'a>(&'a self, args: OnEntityStepArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !args.entity.get_entity().sneaking.load(Relaxed) {
                destroy_egg(args.world, args.block, args.position, args.entity, 100).await;
            }
        })
    }
}

/// Vanilla `canDestroyEgg` (TurtleEggBlock.java:177-183) + `destroyEgg` (:73-80): turtles and
/// bats never crush eggs; non-living entities never do; living entities need to be players or
/// mob griefing enabled; then a `random.nextInt(randomness) == 0` roll.
async fn destroy_egg(
    world: &std::sync::Arc<crate::world::World>,
    block: &Block,
    position: &BlockPos,
    entity: &dyn EntityBase,
    randomness: i32,
) {
    let entity_ref = entity.get_entity();
    if entity_ref.entity_type.id == EntityType::TURTLE.id
        || entity_ref.entity_type.id == EntityType::BAT.id
    {
        return;
    }

    if entity.get_living_entity().is_none() {
        return;
    }

    if entity.get_player().is_none() && !world.level_info.load().game_rules.mob_griefing {
        return;
    }

    if rng().random_range(0..randomness) != 0 {
        return;
    }

    decrease_eggs(world, block, position).await;
}

/// Vanilla `decreaseEggs` (TurtleEggBlock.java:82-92): `TURTLE_EGG_BREAK` at volume 0.7 and
/// pitch 0.9-1.1; the last egg pops the block (no drops), otherwise the state keeps the
/// remaining eggs (flag 2 = `NOTIFY_LISTENERS`), firing `BLOCK_DESTROY` plus level event 2001
/// for the break particles. Pumpkin's `GameEventContext` has no block-state variant, so the
/// event carries no source (same documented simplification as `jukebox.rs`).
async fn decrease_eggs(
    world: &std::sync::Arc<crate::world::World>,
    block: &Block,
    position: &BlockPos,
) {
    world.play_sound_raw(
        Sound::EntityTurtleEggBreak as u16,
        SoundCategory::Blocks,
        &position.to_f64(),
        0.7,
        0.9 + rng().random::<f32>() * 0.2,
    );

    let state_id = world.get_block_state_id(position);
    let mut props = TurtleEggProperties::from_state_id(state_id, block);
    if props.eggs <= 1 {
        world
            .break_block(position, None, BlockFlags::SKIP_DROPS)
            .await;
    } else {
        props.eggs -= 1;
        world
            .set_block_state(
                position,
                props.to_state_id(block),
                BlockFlags::NOTIFY_LISTENERS,
            )
            .await;

        emit_game_event(
            world,
            GameEvent::BlockDestroy,
            position.to_centered_f64(),
            GameEventContext::none(),
        )
        .await;
        world.sync_world_event(
            WorldEvent::ParticlesDestroyBlock,
            *position,
            i32::from(state_id.as_u16()),
        );
    }
}

fn can_place_at(block_accessor: &dyn BlockAccessor, position: &BlockPos) -> bool {
    let (support_block, state) = block_accessor.get_block_and_state(&position.down());
    support_block.has_tag(&tag::Block::MINECRAFT_SAND) || state.is_center_solid(BlockDirection::Up)
}
