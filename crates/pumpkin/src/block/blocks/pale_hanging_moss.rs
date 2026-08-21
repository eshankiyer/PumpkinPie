use pumpkin_data::block_properties::{BlockProperties, PaleHangingMossLikeProperties};
use pumpkin_data::{Block, BlockDirection, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::{BlockAccessor, BlockFlags};

use crate::block::blocks::abstract_multiface::can_attach_to;
use crate::block::{
    BlockBehaviour, BlockFuture, BonemealArgs, CanPlaceAtArgs, GetStateForNeighborUpdateArgs,
    OnScheduledTickArgs,
};

/// `HangingMossBlock` (`net/minecraft/world/level/block/HangingMossBlock.java:23`).
///
/// Pale hanging moss hangs from a downward-attachable face or from another moss segment,
/// breaks when that support goes away, and grows one segment downward on bonemeal.
/// `animateTick` (HangingMossBlock.java:44-52) is client-side ambient sound only and has
/// no server-side effect, so it is deliberately not ported.
#[pumpkin_block("minecraft:pale_hanging_moss")]
pub struct PaleHangingMossBlock;

/// `HangingMossBlock#canStayAtPosition` (HangingMossBlock.java:64-68):
/// `MultifaceBlock.canAttachTo(level, Direction.UP, above, aboveState) || aboveState.is(this)`.
fn can_stay_at_position(accessor: &dyn BlockAccessor, position: &BlockPos) -> bool {
    let above = position.up();
    let (above_block, above_state) = accessor.get_block_and_state(&above);
    can_attach_to(above_state, BlockDirection::Up) || above_block == &Block::PALE_HANGING_MOSS
}

/// `HangingMossBlock#getTip` (HangingMossBlock.java:110-120): walk down while the column is
/// still this block, then step back up to the last moss block.
fn get_tip(accessor: &dyn BlockAccessor, position: &BlockPos) -> BlockPos {
    let mut cursor = *position;
    loop {
        let next = cursor.down();
        if accessor.get_block(&next) == &Block::PALE_HANGING_MOSS {
            cursor = next;
        } else {
            return cursor;
        }
    }
}

impl BlockBehaviour for PaleHangingMossBlock {
    /// `HangingMossBlock#canSurvive` (HangingMossBlock.java:59-62).
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_stay_at_position(args.block_accessor, args.position)
    }

    /// `HangingMossBlock#updateShape` (HangingMossBlock.java:70-86): schedules a 1-tick
    /// self-check when support is gone, and always recomputes `TIP` from the block below.
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if !can_stay_at_position(args.world, args.position) {
                args.world
                    .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
            }
            let mut props = PaleHangingMossLikeProperties::from_state_id(args.state_id, args.block);
            props.tip = args.world.get_block(&args.position.down()) != &Block::PALE_HANGING_MOSS;
            props.to_state_id(args.block)
        })
    }

    /// `HangingMossBlock#tick` (HangingMossBlock.java:88-93): `destroyBlock(pos, true)`, i.e.
    /// break with drops.
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !can_stay_at_position(args.world.as_ref(), args.position) {
                args.world
                    .break_block(args.position, None, BlockFlags::empty())
                    .await;
            }
        })
    }

    /// `HangingMossBlock#isValidBonemealTarget` (HangingMossBlock.java:100-104).
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        let grow_pos = get_tip(args.world.as_ref(), args.position).down();
        args.world.get_block_state(&grow_pos).is_air()
            && args.world.is_in_height_limit(grow_pos.0.y)
    }

    /// `HangingMossBlock#performBonemeal` (HangingMossBlock.java:127-133).
    ///
    /// Vanilla writes only the new tip; the resulting neighbour-shape cascade is what flips the
    /// segment above from `TIP=true` to `TIP=false` through `updateShape`. `set_block_state` with
    /// `NOTIFY_ALL` is this codebase's `setBlockAndUpdate` and runs that same cascade
    /// (`World::set_block_state` -> `BlockRegistry::update_neighbors`), so one write suffices here
    /// too.
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let grow_pos = get_tip(args.world.as_ref(), args.position).down();
            if !args.world.get_block_state(&grow_pos).is_air() {
                return;
            }
            let mut props = PaleHangingMossLikeProperties::from_state_id(args.state_id, args.block);
            props.tip = true;
            args.world
                .set_block_state(
                    &grow_pos,
                    props.to_state_id(args.block),
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::BlockState;
    use std::collections::HashMap;

    struct FakeAccessor {
        states: HashMap<BlockPos, &'static BlockState>,
    }

    impl FakeAccessor {
        fn new() -> Self {
            Self {
                states: HashMap::new(),
            }
        }

        fn with(mut self, pos: BlockPos, state: &'static BlockState) -> Self {
            self.states.insert(pos, state);
            self
        }
    }

    impl BlockAccessor for FakeAccessor {
        fn get_block(&self, position: &BlockPos) -> &'static Block {
            Block::from_state_id(self.get_block_state(position).id)
        }

        fn get_block_state(&self, position: &BlockPos) -> &'static BlockState {
            self.states
                .get(position)
                .copied()
                .unwrap_or(Block::AIR.default_state)
        }

        fn get_block_state_id(&self, position: &BlockPos) -> BlockStateId {
            self.get_block_state(position).id
        }

        fn get_block_and_state(
            &self,
            position: &BlockPos,
        ) -> (&'static Block, &'static BlockState) {
            let state = self.get_block_state(position);
            (Block::from_state_id(state.id), state)
        }

        fn get_fluid(&self, _position: &BlockPos) -> pumpkin_data::fluid::Fluid {
            pumpkin_data::fluid::Fluid::EMPTY
        }
    }

    /// `HangingMossBlock` declares exactly one property, `TIP`
    /// (HangingMossBlock.java:96-98), and defaults it to true (HangingMossBlock.java:36).
    #[test]
    fn pale_hanging_moss_has_only_tip_and_defaults_true() {
        assert_eq!(Block::PALE_HANGING_MOSS.states.len(), 2);
        let props = PaleHangingMossLikeProperties::from_state_id(
            Block::PALE_HANGING_MOSS.default_state.id,
            &Block::PALE_HANGING_MOSS,
        );
        assert!(props.tip);
    }

    #[test]
    fn survives_under_a_solid_block_and_under_more_moss_but_not_in_air() {
        let pos = BlockPos::new(0, 10, 0);

        assert!(can_stay_at_position(
            &FakeAccessor::new().with(pos.up(), Block::STONE.default_state),
            &pos
        ));
        assert!(can_stay_at_position(
            &FakeAccessor::new().with(pos.up(), Block::PALE_HANGING_MOSS.default_state),
            &pos
        ));
        assert!(!can_stay_at_position(&FakeAccessor::new(), &pos));
    }

    #[test]
    fn get_tip_walks_to_the_bottom_of_the_column() {
        let top = BlockPos::new(0, 10, 0);
        let accessor = FakeAccessor::new()
            .with(top, Block::PALE_HANGING_MOSS.default_state)
            .with(top.down(), Block::PALE_HANGING_MOSS.default_state)
            .with(top.down().down(), Block::PALE_HANGING_MOSS.default_state);

        assert_eq!(get_tip(&accessor, &top), top.down().down());
        assert_eq!(get_tip(&accessor, &top.down()), top.down().down());
        // A lone segment is its own tip.
        assert_eq!(get_tip(&FakeAccessor::new(), &top), top);
    }
}
