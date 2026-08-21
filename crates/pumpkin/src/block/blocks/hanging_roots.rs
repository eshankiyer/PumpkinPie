use pumpkin_data::block_properties::{BlockProperties, MangroveRootsLikeProperties};
use pumpkin_data::{BlockDirection, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockAccessor;

use crate::block::{
    BlockBehaviour, BlockFuture, CanPlaceAtArgs, GetStateForNeighborUpdateArgs, OnPlaceArgs,
};

/// `HangingRootsBlock` (`net/minecraft/world/level/block/HangingRootsBlock.java:22`).
///
/// A hanging plant with exactly two behaviours: it needs a downward-sturdy face above it, and it
/// is waterloggable. Both were missing entirely, so hanging roots could be placed in mid-air and
/// stayed there after the block above was mined.
#[pumpkin_block("minecraft:hanging_roots")]
pub struct HangingRootsBlock;

/// `HangingRootsBlock#canSurvive` (HangingRootsBlock.java:58-63): the block above must present a
/// sturdy DOWN face.
fn can_survive(world: &dyn BlockAccessor, position: &BlockPos) -> bool {
    world
        .get_block_state(&position.up())
        .is_side_solid(BlockDirection::Down)
}

impl BlockBehaviour for HangingRootsBlock {
    /// `HangingRootsBlock#getStateForPlacement` (HangingRootsBlock.java:47-56).
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = MangroveRootsLikeProperties::default(args.block);
            props.waterlogged = args.replacing.water_source();
            props.to_state_id(args.block)
        })
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_survive(args.block_accessor, args.position)
    }

    /// `HangingRootsBlock#updateShape` (HangingRootsBlock.java:70-90): losing its support breaks
    /// the roots immediately rather than scheduling a tick.
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if args.direction == BlockDirection::Up && !can_survive(args.world, args.position) {
                return BlockStateId::AIR;
            }
            args.state_id
        })
    }
}

#[cfg(test)]
mod test {
    use pumpkin_data::block_properties::{BlockProperties, MangroveRootsLikeProperties};
    use pumpkin_data::{Block, BlockDirection};

    /// Vanilla's support test is `isFaceSturdy(..., Direction.DOWN)` on the block above, so a full
    /// block holds the roots up and air does not.
    #[test]
    fn support_face_test_separates_stone_from_air() {
        assert!(
            Block::STONE
                .default_state
                .is_side_solid(BlockDirection::Down)
        );
        assert!(!Block::AIR.default_state.is_side_solid(BlockDirection::Down));
    }

    /// `HangingRootsBlock` declares exactly one property, `WATERLOGGED`
    /// (HangingRootsBlock.java:37-40), so it has two states and defaults to dry.
    #[test]
    fn hanging_roots_is_waterloggable_and_defaults_dry() {
        assert_eq!(Block::HANGING_ROOTS.states.len(), 2);
        let props = MangroveRootsLikeProperties::from_state_id(
            Block::HANGING_ROOTS.default_state.id,
            &Block::HANGING_ROOTS,
        );
        assert!(!props.waterlogged);
    }
}
