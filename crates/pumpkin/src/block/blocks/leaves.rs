use crate::block::{
    BlockBehaviour, BlockFuture, BonemealArgs, GetStateForNeighborUpdateArgs, OnPlaceArgs,
    OnScheduledTickArgs, RandomTickArgs,
};
use pumpkin_data::block_properties::{
    BlockProperties, MangrovePropaguleLikeProperties, OakLeavesLikeProperties,
};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockStateId};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

use crate::world::World;

type LeavesProperties = OakLeavesLikeProperties;

const MAX_DISTANCE: u8 = 7;

#[pumpkin_block_from_tag("minecraft:leaves")]
pub struct LeavesBlock;

fn distance_from_state(block: &'static Block, state_id: BlockStateId) -> Option<u8> {
    if block.has_tag(&tag::Block::MINECRAFT_LOGS) {
        return Some(0);
    }
    if !block.has_tag(&tag::Block::MINECRAFT_LEAVES)
        || !LeavesProperties::handles_block_id(block.id)
    {
        return None;
    }
    Some(LeavesProperties::from_state_id(state_id, block).distance)
}

fn compute_distance(world: &World, position: &BlockPos) -> u8 {
    let mut distance = MAX_DISTANCE;
    for direction in BlockDirection::all() {
        let neighbor = position.offset(direction.to_offset());
        let (block, state) = world.get_block_and_state(&neighbor);
        if let Some(neighbor_distance) = distance_from_state(block, state.id) {
            distance = distance.min(neighbor_distance.saturating_add(1));
            if distance == 1 {
                break;
            }
        }
    }
    distance
}

impl BlockBehaviour for LeavesBlock {
    /// `MangroveLeavesBlock.isValidBonemealTarget` (`MangroveLeavesBlock.java:32-35`): the
    /// block below must be empty and inside the build height.
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        if args.block != &Block::MANGROVE_LEAVES {
            return false;
        }
        let below = args.position.down();
        args.world.is_in_height_limit(below.0.y)
            && args.world.is_loaded(&below)
            && args.world.get_block_state(&below).is_air()
    }

    /// `MangroveLeavesBlock.isBonemealSuccess` (`MangroveLeavesBlock.java:37-39`) always
    /// succeeds.
    fn is_bonemeal_success(&self, args: BonemealArgs<'_>) -> bool {
        args.block == &Block::MANGROVE_LEAVES
    }

    /// `MangroveLeavesBlock.performBonemeal` (`MangroveLeavesBlock.java:41-43`) places
    /// `MangrovePropaguleBlock.createNewHangingPropagule` (`MangrovePropaguleBlock.java:142-144`)
    /// below the leaves.
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.block != &Block::MANGROVE_LEAVES {
                return;
            }
            let mut props = MangrovePropaguleLikeProperties::default(&Block::MANGROVE_PROPAGULE);
            props.hanging = true;
            args.world
                .set_block_state(
                    &args.position.down(),
                    props.to_state_id(&Block::MANGROVE_PROPAGULE),
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
        })
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = LeavesProperties::default(args.block);
            props.persistent = true;
            props.distance = compute_distance(args.world, args.position);
            props.waterlogged = args.replacing.water_source();
            props.to_state_id(args.block)
        })
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
            args.state_id
        })
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let mut props = LeavesProperties::from_state_id(state_id, args.block);
            let distance = compute_distance(args.world, args.position);
            if props.distance != distance {
                props.distance = distance;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
            }
        })
    }

    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let props = LeavesProperties::from_state_id(state_id, args.block);
            if !props.persistent && props.distance >= MAX_DISTANCE {
                args.world
                    .break_block(args.position, None, BlockFlags::NOTIFY_ALL)
                    .await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::distance_from_state;
    use pumpkin_data::Block;

    #[test]
    fn distance_sources_match_vanilla_leaf_rules() {
        assert_eq!(
            distance_from_state(&Block::OAK_LOG, Block::OAK_LOG.default_state.id),
            Some(0)
        );
        assert_eq!(
            distance_from_state(&Block::OAK_LEAVES, Block::OAK_LEAVES.default_state.id),
            Some(7)
        );
        assert_eq!(
            distance_from_state(&Block::STONE, Block::STONE.default_state.id),
            None
        );
    }
}
