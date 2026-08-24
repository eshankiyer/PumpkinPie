use std::sync::Arc;

use pumpkin_data::block_properties::{
    BlockProperties, CaveVinesLikeProperties, CaveVinesPlantLikeProperties,
};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockId, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, BonemealArgs, NormalUseArgs, RandomTickArgs,
    registry::BlockActionResult,
};
use crate::world::World;

/// `CaveVinesBlock`'s `growPerTickProbability`, passed to the `GrowingPlantHeadBlock`
/// constructor: `super(properties, Direction.DOWN, SHAPE, false, 0.1)`.
const GROW_PER_TICK_PROBABILITY: f64 = 0.1;

/// `CaveVinesBlock.CHANCE_OF_BERRIES_ON_GROWTH`, rolled in `getGrowIntoState` for the
/// newly grown segment: `random.nextFloat() < 0.11F`.
const CHANCE_OF_BERRIES_ON_GROWTH: f32 = 0.11;

/// Vanilla's `GrowingPlantHeadBlock.MAX_AGE`.
const MAX_AGE: u8 = 25;

/// Growth/spread `random_tick` for cave vines (glow berries).
///
/// Only `CAVE_VINES` (the head block) grows on a random tick. `CAVE_VINES_PLANT` (the
/// body block, `CaveVinesPlantBlock`) never gets `.randomTicks()` in vanilla's
/// `Blocks.java` registration and `GrowingPlantBodyBlock` never overrides `randomTick`,
/// so the body segment is inert here — matching vanilla.
pub struct CaveVinesBlock;

impl BlockMetadata for CaveVinesBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::CAVE_VINES, BlockId::CAVE_VINES_PLANT].into()
    }
}

impl BlockBehaviour for CaveVinesBlock {
    /// `CaveVinesBlock.isValidBonemealTarget` and `CaveVinesPlantBlock.isValidBonemealTarget`
    /// (`CaveVinesBlock.java:77-79`, `CaveVinesPlantBlock.java:59-62`).
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        has_berries(args.block, args.state_id).is_some_and(|berries| !berries)
    }

    /// Both vanilla cave-vine classes return true from `isBonemealSuccess`
    /// (`CaveVinesBlock.java:81-84`, `CaveVinesPlantBlock.java:64-67`).
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    /// `CaveVinesBlock.performBonemeal` and `CaveVinesPlantBlock.performBonemeal`
    /// (`CaveVinesBlock.java:86-89`, `CaveVinesPlantBlock.java:69-72`) set BERRIES directly;
    /// this is deliberately not the inherited stem-growth operation.
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let Some(false) = has_berries(args.block, args.state_id) else {
                return;
            };
            let state_id = set_berries(args.block, args.state_id, true);
            args.world
                .set_block_state(args.position, state_id, BlockFlags::NOTIFY_LISTENERS)
                .await;
        })
    }

    /// `CaveVines.useWithoutItem` delegates to `CaveVines.use`
    /// (`CaveVinesBlock.java:63-68`, `CaveVinesPlantBlock.java:47-52`).
    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let Some(true) = has_berries(args.block, state_id) else {
                return BlockActionResult::Pass;
            };

            // `CaveVines.use` (`CaveVines.java:23-45`) drops the harvest loot, plays the
            // pick-berries sound, clears BERRIES, and emits BLOCK_CHANGE with the new state.
            args.world
                .drop_stack(
                    args.position,
                    ItemStack::new(1, &pumpkin_data::item::Item::GLOW_BERRIES),
                )
                .await;
            let pitch = rand::rng().random_range(0.8f32..1.2f32);
            args.world.play_sound_fine(
                Sound::BlockCaveVinesPickBerries,
                SoundCategory::Blocks,
                &args.position.to_centered_f64(),
                1.0,
                pitch,
            );

            let new_state_id = set_berries(args.block, state_id, false);
            args.world
                .set_block_state(args.position, new_state_id, BlockFlags::NOTIFY_LISTENERS)
                .await;
            crate::world::game_event::emit_game_event(
                args.world,
                GameEvent::BlockChange,
                args.position.to_centered_f64(),
                crate::world::game_event::GameEventContext::of_entity_with_block_state(
                    args.player.clone(),
                    new_state_id,
                ),
            )
            .await;

            BlockActionResult::Success
        })
    }

    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.block != &Block::CAVE_VINES {
                return;
            }
            grow(args.world.clone(), args.position).await;
        })
    }
}

fn has_berries(block: &Block, state_id: BlockStateId) -> Option<bool> {
    if block == &Block::CAVE_VINES {
        Some(CaveVinesLikeProperties::from_state_id(state_id, block).berries)
    } else if block == &Block::CAVE_VINES_PLANT {
        Some(CaveVinesPlantLikeProperties::from_state_id(state_id, block).berries)
    } else {
        None
    }
}

fn set_berries(block: &Block, state_id: BlockStateId, berries: bool) -> BlockStateId {
    if block == &Block::CAVE_VINES {
        let mut props = CaveVinesLikeProperties::from_state_id(state_id, block);
        props.berries = berries;
        props.to_state_id(block)
    } else if block == &Block::CAVE_VINES_PLANT {
        CaveVinesPlantLikeProperties { berries }.to_state_id(block)
    } else {
        state_id
    }
}

/// Mirrors `GrowingPlantHeadBlock.randomTick` + `CaveVinesBlock.getGrowIntoState`.
///
/// Vanilla:
/// ```java
/// if (state.getValue(AGE) < 25 && random.nextDouble() < this.growPerTickProbability) {
///    BlockPos growthPos = pos.relative(this.growthDirection); // DOWN
///    if (this.canGrowInto(level.getBlockState(growthPos))) {  // isAir()
///       level.setBlockAndUpdate(growthPos, this.getGrowIntoState(state, level.getRandom()));
///    }
/// }
/// ```
/// `setBlockAndUpdate` at `growthPos` cascades a neighbor update back onto the old head
/// at `pos`, which `GrowingPlantHeadBlock.updateShape` turns into the body block via
/// `updateBodyAfterConvertedFromHead`, which `CaveVinesBlock` overrides to carry the old
/// head's berries onto the new body state. Both writes are applied explicitly below
/// since pumpkin has no registered neighbor-update chain for cave vines to rely on.
async fn grow(world: Arc<World>, pos: &BlockPos) {
    let (block, state_id) = world.get_block_and_state_id(pos);
    if block != &Block::CAVE_VINES {
        return;
    }
    let props = CaveVinesLikeProperties::from_state_id(state_id, block);
    if !should_attempt_growth(props.age, rand::rng().random::<f64>()) {
        return;
    }

    let grow_pos = pos.down();
    if !world.get_block_state(&grow_pos).is_air() {
        return;
    }

    let new_age = next_age(props.age);
    let new_berries = rand::rng().random::<f32>() < CHANCE_OF_BERRIES_ON_GROWTH;
    let new_head_props = CaveVinesLikeProperties {
        age: new_age,
        berries: new_berries,
    };
    world
        .set_block_state(
            &grow_pos,
            new_head_props.to_state_id(&Block::CAVE_VINES),
            BlockFlags::NOTIFY_NEIGHBORS,
        )
        .await;

    // Old head converts into the body block, carrying over its own (pre-growth) berries.
    let new_body_props = CaveVinesPlantLikeProperties {
        berries: props.berries,
    };
    world
        .set_block_state(
            pos,
            new_body_props.to_state_id(&Block::CAVE_VINES_PLANT),
            BlockFlags::NOTIFY_NEIGHBORS,
        )
        .await;
}

/// Pure growth gate mirroring `state.getValue(AGE) < 25 && random.nextDouble() < growPerTickProbability`.
const fn should_attempt_growth(age: u8, roll: f64) -> bool {
    age < MAX_AGE && roll < GROW_PER_TICK_PROBABILITY
}

/// Mirrors `BlockState#cycle(AGE)` used by `getGrowIntoState`. Only ever called when
/// `age < MAX_AGE` (see `should_attempt_growth`), so the wrap-to-0 branch is unreachable
/// in practice; it is kept to faithfully mirror `cycle`'s general wraparound semantics.
const fn next_age(age: u8) -> u8 {
    if age >= MAX_AGE { 0 } else { age + 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_growing_at_max_age() {
        assert!(!should_attempt_growth(MAX_AGE, 0.0));
    }

    #[test]
    fn growth_gate_respects_probability() {
        assert!(should_attempt_growth(0, 0.0));
        assert!(should_attempt_growth(
            0,
            GROW_PER_TICK_PROBABILITY - f64::EPSILON
        ));
        assert!(!should_attempt_growth(0, GROW_PER_TICK_PROBABILITY));
        assert!(!should_attempt_growth(0, 1.0));
    }

    #[test]
    fn next_age_increments_then_wraps() {
        assert_eq!(next_age(0), 1);
        assert_eq!(next_age(24), 25);
        assert_eq!(next_age(25), 0);
    }
}
