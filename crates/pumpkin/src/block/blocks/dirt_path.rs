use crate::block::BlockBehaviour;
use crate::block::BlockFuture;
use crate::block::CanPlaceAtArgs;
use crate::block::GetStateForNeighborUpdateArgs;
use crate::block::OnPlaceArgs;
use crate::block::OnScheduledTickArgs;
use crate::block::blocks::farmland::turn_to_dirt;
use crate::block::push_entities_up;
use pumpkin_data::Block;
use pumpkin_data::BlockDirection;
use pumpkin_data::BlockStateId;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockAccessor;

#[pumpkin_block("minecraft:dirt_path")]
pub struct DirtPathBlock;

impl BlockBehaviour for DirtPathBlock {
    /// `DirtPathBlock.tick` is `FarmlandBlock.turnToDirt(null, state, level, pos)`
    /// (`DirtPathBlock.java:62-64`).
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            turn_to_dirt(args.world, args.position, None).await;
        })
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            // `getStateForPlacement` (`DirtPathBlock.java:36-41`) lifts entities out of the
            // dirt that replaces a path which cannot survive here.
            if !can_place_at(args.world, args.position) {
                push_entities_up(
                    &args.player.world(),
                    args.block.default_state,
                    Block::DIRT.default_state,
                    args.position,
                )
                .await;
                return Block::DIRT.default_state.id;
            }

            args.block.default_state.id
        })
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if args.direction == BlockDirection::Up && !can_place_at(args.world, args.position) {
                args.world
                    .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
            }
            args.state_id
        })
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_place_at(args.block_accessor, args.position)
    }
}

fn can_place_at(world: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
    let (block, state) = world.get_block_and_state(&block_pos.up());
    !state.is_solid() || block.has_tag(&tag::Block::C_FENCE_GATES)
}
