use crate::block::{
    BlockBehaviour, BlockFuture, BonemealArgs, GetStateForNeighborUpdateArgs, OnPlaceArgs,
    OnScheduledTickArgs, RandomTickArgs,
};
use pumpkin_data::block_properties::{
    BlockProperties, MangrovePropaguleLikeProperties, OakLeavesLikeProperties,
};
use pumpkin_data::fluid::Fluid;
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
    // `LeavesBlock.getOptionalDistanceAt` (`LeavesBlock.java:126-135`) treats every block in
    // `PREVENTS_NEARBY_LEAF_DECAY` as a distance-zero source, not only blocks in `logs`.
    if block.has_tag(&tag::Block::MINECRAFT_PREVENTS_NEARBY_LEAF_DECAY) {
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

// `LeavesBlock.updateShape` (`LeavesBlock.java:103-105`) schedules a one-tick update unless the
// neighbor's distance produces the current leaf distance.
fn should_schedule_distance_tick(current_distance: u8, neighbor_distance: Option<u8>) -> bool {
    let distance_from_neighbor = neighbor_distance.unwrap_or(MAX_DISTANCE).saturating_add(1);
    distance_from_neighbor != 1 || current_distance != distance_from_neighbor
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
            // `LeavesBlock.updateShape` (`LeavesBlock.java:89-108`) schedules water for
            // waterlogged leaves and only schedules the leaf tick when its distance changes.
            let props = LeavesProperties::from_state_id(args.state_id, args.block);
            if props.waterlogged {
                args.world.schedule_fluid_tick(
                    &Fluid::WATER,
                    *args.position,
                    Fluid::WATER.flow_speed as u8,
                    TickPriority::Normal,
                );
            }

            let neighbor_block = Block::from_state_id(args.neighbor_state_id);
            let neighbor_distance = distance_from_state(neighbor_block, args.neighbor_state_id);
            if should_schedule_distance_tick(props.distance, neighbor_distance) {
                args.world
                    .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
            }
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
    use super::{distance_from_state, should_schedule_distance_tick};
    use pumpkin_data::Block;

    #[test]
    fn distance_sources_match_vanilla_leaf_rules() {
        assert_eq!(
            distance_from_state(&Block::OAK_LOG, Block::OAK_LOG.default_state.id),
            Some(0)
        );
        // `LeavesBlock.getOptionalDistanceAt` (`LeavesBlock.java:130-135`) also covers wood
        // blocks and stripped wood through the vanilla prevention tag.
        assert_eq!(
            distance_from_state(&Block::OAK_WOOD, Block::OAK_WOOD.default_state.id),
            Some(0)
        );
        assert_eq!(
            distance_from_state(&Block::CRIMSON_STEM, Block::CRIMSON_STEM.default_state.id),
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

    #[test]
    fn neighbor_tick_schedule_matches_vanilla_distance_check() {
        // `LeavesBlock.updateShape` (`LeavesBlock.java:103-105`) skips a tick only when the
        // neighbor distance produces the current leaf distance of one.
        assert!(!should_schedule_distance_tick(1, Some(0)));
        assert!(should_schedule_distance_tick(3, Some(2)));
        assert!(should_schedule_distance_tick(3, Some(1)));
        assert!(should_schedule_distance_tick(7, None));
    }
}
