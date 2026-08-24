use pumpkin_data::block_properties::BlockProperties;
use pumpkin_data::{Block, FacingExt};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::block::{
    BlockBehaviour, BlockFuture, BrokenArgs, OnNeighborUpdateArgs, OnStateReplacedArgs,
    PlayerWillDestroyArgs,
};
use crate::world::World;

use super::piston::PistonProps;

pub(crate) type PistonHeadProperties = pumpkin_data::block_properties::PistonHeadLikeProperties;

#[pumpkin_block("minecraft:piston_head")]
pub struct PistonHeadBlock;

impl PistonHeadBlock {
    /// `PistonHeadBlock.isFittingBase` (`PistonHeadBlock.java:64-67`): the base must be the
    /// specific piston variant matching the head's own `type` property, not either piston kind.
    fn fitting_base(world: &World, position: &BlockPos, head: PistonHeadProperties) -> bool {
        let expected_base = if head.r#type == pumpkin_data::block_properties::PistonType::Normal {
            &Block::PISTON
        } else {
            &Block::STICKY_PISTON
        };

        let base_pos = position.offset(head.facing.opposite().to_block_direction().to_offset());
        let (base_block, base_state_id) = world.get_block_and_state_id(&base_pos);
        if base_block != expected_base {
            return false;
        }

        let base = PistonProps::from_state_id(base_state_id, base_block);
        base.extended && base.facing == head.facing
    }
}

impl BlockBehaviour for PistonHeadBlock {
    /// `PistonHeadBlock.playerWillDestroy` (`PistonHeadBlock.java:70-78`): creative removal
    /// destroys a fitting base without drops before the head itself is destroyed.
    fn player_will_destroy<'a>(&'a self, args: PlayerWillDestroyArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.player.gamemode.load() != GameMode::Creative {
                return;
            }

            let head = PistonHeadProperties::from_state_id(args.state.id, &Block::PISTON_HEAD);
            if Self::fitting_base(args.world, args.position, head) {
                let base_pos = args
                    .position
                    .offset(head.facing.opposite().to_block_direction().to_offset());
                args.world
                    .break_block(
                        &base_pos,
                        None,
                        BlockFlags::SKIP_DROPS | BlockFlags::NOTIFY_NEIGHBORS,
                    )
                    .await;
            }
        })
    }

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

    /// `PistonHeadBlock.affectNeighborsAfterRemoval` (`PistonHeadBlock.java:82-87`): remove the
    /// fitting base with drops whenever the head disappears outside the player pre-destroy path.
    fn on_state_replaced<'a>(&'a self, args: OnStateReplacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let head = PistonHeadProperties::from_state_id(args.old_state_id, &Block::PISTON_HEAD);
            if Self::fitting_base(args.world, args.position, head) {
                let base_pos = args
                    .position
                    .offset(head.facing.opposite().to_block_direction().to_offset());
                args.world
                    .break_block(&base_pos, None, BlockFlags::NOTIFY_NEIGHBORS)
                    .await;
            }
        })
    }
}
