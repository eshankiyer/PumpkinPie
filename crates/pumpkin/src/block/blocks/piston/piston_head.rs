use pumpkin_data::block_properties::BlockProperties;
use pumpkin_data::{Block, FacingExt};
use pumpkin_macros::pumpkin_block;
use pumpkin_world::world::BlockFlags;

use crate::block::{BlockBehaviour, BlockFuture};
use crate::block::{BrokenArgs, OnNeighborUpdateArgs};

use super::piston::PistonProps;

pub(crate) type PistonHeadProperties = pumpkin_data::block_properties::PistonHeadLikeProperties;

#[pumpkin_block("minecraft:piston_head")]
pub struct PistonHeadBlock;

impl BlockBehaviour for PistonHeadBlock {
    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let props = PistonHeadProperties::from_state_id(args.state.id, &Block::PISTON_HEAD);
            let pos = args
                .position
                .offset(props.facing.opposite().to_block_direction().to_offset());
            let (new_block, new_state) = args.world.get_block_and_state_id(&pos);
            if &Block::PISTON == new_block || &Block::STICKY_PISTON == new_block {
                let props = PistonProps::from_state_id(new_state, new_block);
                if props.extended {
                    args.world
                        .break_block(&pos, None, BlockFlags::empty())
                        .await;
                }
            }
        })
    }
    fn on_neighbor_update<'a>(&'a self, args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // `PistonHeadBlock.neighborChanged` relays EVERY update the head receives to the base
            // behind it, which is the only way a change two blocks from the base — the classic
            // quasi-connectivity position beside the head — ever reaches `checkIfExtend`. Pumpkin
            // relayed only for an upward head whose neighbour above had stopped being a redstone
            // block, so a horizontal piston powered that way stayed stuck.
            let head_state_id = args.world.get_block_state_id(args.position);
            let head_props =
                PistonHeadProperties::from_state_id(head_state_id, &Block::PISTON_HEAD);
            let piston_pos = args.position.offset(
                head_props
                    .facing
                    .opposite()
                    .to_block_direction()
                    .to_offset(),
            );

            // `PistonHeadBlock.canSurvive`: the head only counts while its base is still an
            // extended piston facing it, or a moving piston mid-animation.
            let base_block = args.world.get_block(&piston_pos);
            let base_survives =
                if base_block == &Block::PISTON || base_block == &Block::STICKY_PISTON {
                    let base_state_id = args.world.get_block_state_id(&piston_pos);
                    let base_props = PistonProps::from_state_id(base_state_id, base_block);
                    base_props.extended && base_props.facing == head_props.facing
                } else if base_block == &Block::MOVING_PISTON {
                    let base_state_id = args.world.get_block_state_id(&piston_pos);
                    let base_props =
                        PistonHeadProperties::from_state_id(base_state_id, &Block::MOVING_PISTON);
                    base_props.facing == head_props.facing
                } else {
                    false
                };

            if base_survives {
                args.world
                    .update_neighbor(&piston_pos, args.source_block)
                    .await;
            }
        })
    }
}
