use std::sync::Arc;

use pumpkin_data::block_properties::{BlockProperties, TurtleEggLikeProperties};
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockDirection, BlockState, BlockStateId, tag};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::{RngExt, rng};
use std::sync::atomic::Ordering::Relaxed;
use uuid::Uuid;

use crate::block::{
    BlockBehaviour, BlockIsReplacing, BrokenArgs, CanPlaceAtArgs, CanUpdateAtArgs,
    GetStateForNeighborUpdateArgs, OnEntityStepArgs, OnLandedUponArgs, OnPlaceArgs,
    OnScheduledTickArgs, RandomTickArgs,
};
use crate::entity::EntityBase;
use crate::entity::r#type::from_type;
use crate::world::World;
use crate::world::game_event::{GameEventContext, emit_game_event};

type TurtleEggProperties = TurtleEggLikeProperties;

#[pumpkin_block("minecraft:turtle_egg")]
pub struct TurtleEggBlock;

impl TurtleEggBlock {
    /// Vanilla `TurtleEggBlock.isSand`.
    #[must_use]
    pub fn is_sand(world: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        world.get_block(pos).has_tag(&tag::Block::MINECRAFT_SAND)
    }

    /// Vanilla `TurtleEggBlock.onSand`: the block below is in `#minecraft:sand`.
    #[must_use]
    pub fn on_sand(world: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        Self::is_sand(world, &pos.down())
    }
}

impl BlockBehaviour for TurtleEggBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
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

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if !can_place_at(args.world, args.position) {
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
        }
        args.state_id
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        if !can_place_at(args.world.as_ref(), args.position) {
            args.world
                .break_block(args.position, None, BlockFlags::empty());
        }
    }

    /// Vanilla `TurtleEggBlock.randomTick` (`TurtleEggBlock.java:89-113`). The
    /// `TURTLE_EGG_HATCH_CHANCE` environment-attribute roll (`shouldUpdateHatchLevel`) is not
    /// modelled; every random tick on sand advances the egg.
    fn random_tick(&self, args: RandomTickArgs<'_>) {
        if !Self::on_sand(args.world.as_ref(), args.position) {
            return;
        }

        let state = args.world.get_block_state(args.position);
        let mut props = TurtleEggProperties::from_state_id(state.id, args.block);

        if props.hatch < 2 {
            args.world.play_sound_raw(
                Sound::EntityTurtleEggCrack as u16,
                SoundCategory::Blocks,
                &args.position.to_f64(),
                0.7,
                0.9 + rng().random::<f32>() * 0.2,
            );
            props.hatch += 1;
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_LISTENERS,
            );
            emit_game_event(
                args.world,
                GameEvent::BlockChange,
                args.position.to_centered_f64(),
                GameEventContext::none(),
            );
        } else {
            args.world.play_sound_raw(
                Sound::EntityTurtleEggHatch as u16,
                SoundCategory::Blocks,
                &args.position.to_f64(),
                0.7,
                0.9 + rng().random::<f32>() * 0.2,
            );
            args.world
                .break_block(args.position, None, BlockFlags::SKIP_DROPS);
            emit_game_event(
                args.world,
                GameEvent::BlockDestroy,
                args.position.to_centered_f64(),
                GameEventContext::none(),
            );

            // One baby turtle per egg, homed on the nest (`Turtle` picks up its home
            // position from where it spawns), with a destroy-block particle burst each.
            for i in 0..props.eggs {
                args.world.sync_world_event(
                    WorldEvent::ParticlesDestroyBlock,
                    *args.position,
                    i32::from(state.id.as_u16()),
                );
                let spawn_pos = Vector3::new(
                    f64::from(args.position.0.x) + 0.3 + f64::from(i) * 0.2,
                    f64::from(args.position.0.y),
                    f64::from(args.position.0.z) + 0.3,
                );
                let turtle = from_type(&EntityType::TURTLE, spawn_pos, args.world, Uuid::new_v4());
                turtle.get_entity().set_age(-24000);
                args.world.spawn_entity(turtle);
            }
        }
    }

    fn on_landed_upon(&self, args: OnLandedUponArgs<'_>) {
        if let Some(living) = args.entity.get_living_entity() {
            living.handle_fall_damage(args.entity, args.fall_distance, 1.0);
        }

        // Vanilla `fallOn` (TurtleEggBlock.java:65-71): falling onto the egg (zombies are
        // immune) rolls against randomness 3.
        if args.entity.get_entity().entity_type.id != EntityType::ZOMBIE.id {
            let (block, state) = args.world.get_block_and_state(args.position);
            destroy_egg(args.world, block, state, args.position, args.entity, 3);
        }
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        // Vanilla `playerDestroy` (TurtleEggBlock.java:141-152) receives the original state
        // after the normal break, so a multi-egg cluster leaves one fewer egg behind.
        decrease_eggs(args.world, args.block, args.state, args.position);
    }

    /// Vanilla `stepOn` (TurtleEggBlock.java:56-62): anything not stepping carefully
    /// (`isSteppingCarefully` == shift-key-down, `Entity.java:2681-2683`) can crush an egg,
    /// rolled each tick against randomness 100.
    fn on_entity_step(&self, args: OnEntityStepArgs<'_>) {
        if !args.entity.get_entity().sneaking.load(Relaxed) {
            destroy_egg(
                args.world,
                args.block,
                args.state,
                args.position,
                args.entity,
                100,
            );
        }
    }
}

/// Vanilla `canDestroyEgg` (TurtleEggBlock.java:177-183) + `destroyEgg` (:73-80): turtles and
/// bats never crush eggs; non-living entities never do; living entities need to be players or
/// mob griefing enabled; then a `random.nextInt(randomness) == 0` roll.
fn destroy_egg(
    world: &Arc<World>,
    block: &Block,
    state: &BlockState,
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

    decrease_eggs(world, block, state, position);
}

/// Vanilla `decreaseEggs` (TurtleEggBlock.java:82-92): `TURTLE_EGG_BREAK` at volume 0.7 and
/// pitch 0.9-1.1; the last egg pops the block (no drops), otherwise the state keeps the
/// remaining eggs (flag 2 = `NOTIFY_LISTENERS`), firing `BLOCK_DESTROY` plus level event 2001
/// for the break particles. Pumpkin's `GameEventContext` has no block-state variant, so the
/// event carries no source (same documented simplification as `jukebox.rs`).
fn decrease_eggs(world: &Arc<World>, block: &Block, state: &BlockState, position: &BlockPos) {
    world.play_sound_raw(
        Sound::EntityTurtleEggBreak as u16,
        SoundCategory::Blocks,
        &position.to_f64(),
        0.7,
        0.9 + rng().random::<f32>() * 0.2,
    );

    if let Some(state_id) = decreased_egg_state(state.id, block) {
        world.set_block_state(position, state_id, BlockFlags::NOTIFY_LISTENERS);

        emit_game_event(
            world,
            GameEvent::BlockDestroy,
            position.to_centered_f64(),
            GameEventContext::none(),
        );
        world.sync_world_event(
            WorldEvent::ParticlesDestroyBlock,
            *position,
            i32::from(state.id.as_u16()),
        );
    } else {
        world.break_block(position, None, BlockFlags::SKIP_DROPS);
    }
}

/// Vanilla `decreaseEggs` uses the state passed by `playerDestroy`, not the now-air world state
/// (`TurtleEggBlock.java:82-90, 141-152`).
fn decreased_egg_state(state_id: BlockStateId, block: &Block) -> Option<BlockStateId> {
    let mut properties = TurtleEggProperties::from_state_id(state_id, block);
    if properties.eggs <= 1 {
        None
    } else {
        properties.eggs -= 1;
        Some(properties.to_state_id(block))
    }
}

fn can_place_at(block_accessor: &dyn BlockAccessor, position: &BlockPos) -> bool {
    let (support_block, state) = block_accessor.get_block_and_state(&position.down());
    support_block.has_tag(&tag::Block::MINECRAFT_SAND) || state.is_center_solid(BlockDirection::Up)
}

#[cfg(test)]
mod tests {
    use super::{TurtleEggProperties, decreased_egg_state};
    use pumpkin_data::Block;
    use pumpkin_data::block_properties::BlockProperties;

    /// `decreaseEggs` keeps a multi-egg state with one fewer egg and removes a single egg
    /// (`TurtleEggBlock.java:82-90`).
    #[test]
    fn decreased_egg_state_matches_vanilla() {
        let mut properties = TurtleEggProperties::default(&Block::TURTLE_EGG);
        properties.eggs = 4;
        let four_eggs = properties.to_state_id(&Block::TURTLE_EGG);

        let three_eggs = decreased_egg_state(four_eggs, &Block::TURTLE_EGG).unwrap();
        assert_eq!(
            TurtleEggProperties::from_state_id(three_eggs, &Block::TURTLE_EGG).eggs,
            3
        );

        let one_egg =
            TurtleEggProperties::default(&Block::TURTLE_EGG).to_state_id(&Block::TURTLE_EGG);
        assert!(decreased_egg_state(one_egg, &Block::TURTLE_EGG).is_none());
    }
}
