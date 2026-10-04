use super::physics;
use crate::block::fluid::flowing_trait::FlowingFluid;
use crate::world::World;
use pumpkin_data::{
    Block, BlockDirection, BlockState, BlockStateId,
    fluid::{Fluid, FluidProperties},
};
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

const HORIZONTAL: [BlockDirection; 4] = [
    BlockDirection::North,
    BlockDirection::South,
    BlockDirection::West,
    BlockDirection::East,
];

/// `FlowingFluid.canPassThroughWall` (`FlowingFluid.java:198-252`).
///
/// A full collision cube on either side blocks the fluid, and so does a sturdy face on the
/// shared side (`Shapes.mergedFaceOccludes`), as for a top slab seen from above.
#[must_use]
pub const fn can_pass_through_wall(
    direction: BlockDirection,
    from_state: &BlockState,
    to_state: &BlockState,
) -> bool {
    !(from_state.is_full_cube()
        || to_state.is_full_cube()
        || from_state.is_side_solid(direction)
        || to_state.is_side_solid(direction.opposite()))
}

/// `FlowingFluid.canHoldFluid` (`FlowingFluid.java:409-430`): `canHoldAnyFluid` excludes
/// bubble columns, and `canHoldSpecificFluid` sends a waterloggable block to
/// `SimpleWaterloggedBlock.canPlaceLiquid`, which accepts only water. The rest is Pumpkin's
/// replaceability approximation.
fn can_hold_fluid(state: &BlockState, block: &Block, fluid: &Fluid) -> bool {
    if block == &Block::BUBBLE_COLUMN {
        return false;
    }
    if !fluid.matches_type(&Fluid::WATER) && block.with_waterlogged(state.id).is_some() {
        return false;
    }
    physics::can_be_replaced(state, block, fluid)
}

/// `FlowingFluid.isWaterHole` (`FlowingFluid.java:310-318`): the fluid at `pos` may drop into
/// the block below. Same-type fluid below (any level, or waterlogged) always counts as a hole.
#[must_use]
pub fn is_water_hole(world: &World, fluid: &Fluid, pos: &BlockPos) -> bool {
    let state = world.get_block_state(pos);
    let below_pos = pos.down();
    let below_state = world.get_block_state(&below_pos);
    if !can_pass_through_wall(BlockDirection::Down, state, below_state) {
        return false;
    }
    let (below_fluid, _) = world.get_fluid_and_fluid_state(&below_pos);
    below_fluid.matches_type(fluid)
        || can_hold_fluid(below_state, Block::from_state_id(below_state.id), fluid)
}

/// `FlowingFluid.canMaybePassThrough` (`FlowingFluid.java:334-346`): the target is not a source
/// of this fluid, can hold some fluid, and no wall separates it from `from_state`.
fn can_maybe_pass_through(
    world: &World,
    fluid: &Fluid,
    from_state: &BlockState,
    direction: BlockDirection,
    pos: &BlockPos,
) -> bool {
    let (pos_fluid, pos_fluid_state) = world.get_fluid_and_fluid_state(pos);
    if pos_fluid.matches_type(fluid) && pos_fluid_state.is_source {
        return false;
    }
    let state = world.get_block_state(pos);
    if !can_pass_through_wall(direction, from_state, state) {
        return false;
    }
    let block = Block::from_state_id(state.id);
    // A waterlogged block is a water source (handled above for water) and cannot take lava.
    if block.is_waterlogged(state.id) {
        return false;
    }
    // Air and water/lava blocks: `LiquidBlock` neither blocks motion nor is excluded.
    if Fluid::from_state_id(state.id).is_some() {
        return true;
    }
    can_hold_fluid(state, block, fluid)
}

/// Determines valid spread directions for fluid flow, ported from `FlowingFluid.getSpread`
/// (`FlowingFluid.java:368-407`).
///
/// - Holes (downward flow opportunities) get distance 0 priority
/// - All directions with equal minimum distance are returned
/// - Returns up to 4 directions with their computed fluid states
pub async fn get_spread<T: FlowingFluid + Sync + ?Sized>(
    fluid_impl: &T,
    world: &Arc<World>,
    fluid: &Fluid,
    block_pos: &BlockPos,
) -> ([(BlockDirection, BlockStateId); 4], usize) {
    let mut min_dist = 1000;
    let mut result = [(BlockDirection::North, BlockStateId::default()); 4];
    let mut result_count = 0;
    let state = world.get_block_state(block_pos);
    let slope_find_distance = fluid_impl.get_max_flow_distance(world);

    for direction in HORIZONTAL {
        let side_pos = block_pos.offset(direction.to_offset());
        if !can_maybe_pass_through(world, fluid, state, direction, &side_pos) {
            continue;
        }

        // Skip if no valid fluid state for this position
        let Some(new_fluid_props) = fluid_impl.get_new_liquid(world, fluid, &side_pos).await else {
            continue;
        };

        // Holes get distance 0
        let slope_dist = if is_water_hole(world, fluid, &side_pos) {
            0
        } else {
            get_slope_distance(
                world,
                fluid,
                side_pos,
                1,
                direction.opposite(),
                slope_find_distance,
            )
        };

        // Clear results if we find a shorter path
        if slope_dist < min_dist {
            result_count = 0;
        }

        // Add all directions with equal minimum distance. The minimum is lowered even when the
        // side's fluid refuses the new fluid (water never spreads sideways into water).
        if slope_dist <= min_dist {
            let (side_fluid, side_fluid_state) = world.get_fluid_and_fluid_state(&side_pos);
            if physics::can_be_replaced_with(side_fluid, side_fluid_state, fluid, direction)
                && result_count < 4
            {
                result[result_count] = (direction, new_fluid_props.to_state_id(fluid));
                result_count += 1;
            }

            min_dist = slope_dist;
        }
    }
    (result, result_count)
}

/// `FlowingFluid.getSlopeDistance` (`FlowingFluid.java:282-308`): the number of horizontal
/// steps past `pos` to the nearest hole, searching at most `slope_find_distance` steps deep.
///
/// # Returns
/// `pass` at the first hole found, or 1000 if no hole is reachable
#[must_use]
pub fn get_slope_distance(
    world: &World,
    fluid: &Fluid,
    pos: BlockPos,
    pass: i32,
    from: BlockDirection,
    slope_find_distance: i32,
) -> i32 {
    let mut lowest = 1000;
    let state = world.get_block_state(&pos);

    for direction in HORIZONTAL {
        if direction == from {
            continue;
        }
        let test_pos = pos.offset(direction.to_offset());
        if !can_maybe_pass_through(world, fluid, state, direction, &test_pos) {
            continue;
        }
        if is_water_hole(world, fluid, &test_pos) {
            return pass;
        }
        if pass < slope_find_distance {
            lowest = lowest.min(get_slope_distance(
                world,
                fluid,
                test_pos,
                pass + 1,
                direction.opposite(),
                slope_find_distance,
            ));
        }
    }

    lowest
}

#[cfg(test)]
mod tests {
    use super::can_pass_through_wall;
    use pumpkin_data::{Block, BlockDirection};

    #[test]
    fn wall_blocks_full_cubes() {
        let air = Block::AIR.default_state;
        let stone = Block::STONE.default_state;
        assert!(can_pass_through_wall(BlockDirection::Down, air, air));
        assert!(!can_pass_through_wall(BlockDirection::Down, air, stone));
        assert!(!can_pass_through_wall(BlockDirection::North, stone, air));
    }
}
