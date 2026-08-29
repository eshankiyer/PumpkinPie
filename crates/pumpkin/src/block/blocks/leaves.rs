use pumpkin_data::{
    Block, BlockDirection, BlockStateId,
    block_properties::{BlockProperties, MangrovePropaguleLikeProperties, OakLeavesLikeProperties},
    tag,
    tag::Taggable,
};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::{
    tick::TickPriority,
    world::{BlockAccessor, BlockFlags},
};

use crate::block::{
    BlockBehaviour, BonemealArgs, GetStateForNeighborUpdateArgs, OnPlaceArgs, OnScheduledTickArgs,
    RandomTickArgs,
};
use crate::world::World;

pub const DECAY_DISTANCE: u8 = 7;

#[pumpkin_block_from_tag("minecraft:leaves")]
pub struct LeavesBlock;

/// Fork-side helper retained for the leaf-distance calculation: logs are the zero-distance
/// sources, supported leaves contribute their stored distance, and unrelated blocks contribute
/// nothing.
fn distance_from_state(block: &'static Block, state_id: BlockStateId) -> Option<u8> {
    if block.has_tag(&tag::Block::MINECRAFT_LOGS) {
        return Some(0);
    }
    if !block.has_tag(&tag::Block::MINECRAFT_LEAVES)
        || !OakLeavesLikeProperties::handles_block_id(block.id)
    {
        return None;
    }
    Some(OakLeavesLikeProperties::from_state_id(state_id, block).distance)
}

#[must_use]
pub fn get_distance_at(block: &Block, state_id: BlockStateId) -> u8 {
    if block.has_tag(&tag::Block::MINECRAFT_PREVENTS_NEARBY_LEAF_DECAY)
        || block.has_tag(&tag::Block::MINECRAFT_LOGS)
    {
        0
    } else if block.has_tag(&tag::Block::MINECRAFT_LEAVES) {
        OakLeavesLikeProperties::from_state_id(state_id, block).distance
    } else {
        DECAY_DISTANCE
    }
}

#[must_use]
pub fn update_distance(
    world: &dyn BlockAccessor,
    pos: &BlockPos,
    mut props: OakLeavesLikeProperties,
) -> OakLeavesLikeProperties {
    let mut min_distance = DECAY_DISTANCE;
    for direction in BlockDirection::all() {
        let neighbor_pos = pos.offset(direction.to_offset());
        let (neighbor_block, neighbor_state) = world.get_block_and_state(&neighbor_pos);
        let dist = get_distance_at(neighbor_block, neighbor_state.id).saturating_add(1);
        min_distance = min_distance.min(dist);
        if min_distance == 1 {
            break;
        }
    }
    props.distance = min_distance.min(DECAY_DISTANCE);
    props
}

impl BlockBehaviour for LeavesBlock {
    /// `MangroveLeavesBlock.isValidBonemealTarget` (`MangroveLeavesBlock.java:32-35`).
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        if args.block != &Block::MANGROVE_LEAVES {
            return false;
        }
        let below = args.position.down();
        args.world.is_in_height_limit(below.0.y)
            && args.world.is_loaded(&below)
            && args.world.get_block_state(&below).is_air()
    }

    /// `MangroveLeavesBlock.isBonemealSuccess` (`MangroveLeavesBlock.java:37-39`) always succeeds.
    fn is_bonemeal_success(&self, args: BonemealArgs<'_>) -> bool {
        args.block == &Block::MANGROVE_LEAVES
    }

    /// `MangroveLeavesBlock.performBonemeal` (`MangroveLeavesBlock.java:41-43`) places
    /// `MangrovePropaguleBlock.createNewHangingPropagule` (`MangrovePropaguleBlock.java:142-144`)
    /// below the leaves.
    fn perform_bonemeal(&self, args: BonemealArgs<'_>) {
        if args.block != &Block::MANGROVE_LEAVES {
            return;
        }
        let mut props = MangrovePropaguleLikeProperties::default(&Block::MANGROVE_PROPAGULE);
        props.hanging = true;
        args.world.set_block_state(
            &args.position.down(),
            props.to_state_id(&Block::MANGROVE_PROPAGULE),
            BlockFlags::NOTIFY_ALL,
        );
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = OakLeavesLikeProperties::default(args.block);
        props.persistent = true;
        props.waterlogged = args.replacing.water_source();
        props = update_distance(args.world, args.position, props);
        props.to_state_id(args.block)
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        let current_props = OakLeavesLikeProperties::from_state_id(args.state_id, args.block);
        if current_props.waterlogged {
            args.world.schedule_fluid_tick(
                &pumpkin_data::fluid::Fluid::WATER,
                *args.position,
                pumpkin_data::fluid::Fluid::WATER.flow_speed as u8,
                TickPriority::Normal,
            );
        }
        let neighbor_block = args.world.get_block(args.neighbor_position);
        let distance_from_neighbor =
            get_distance_at(neighbor_block, args.neighbor_state_id).saturating_add(1);
        if distance_from_neighbor != 1 || current_props.distance != distance_from_neighbor {
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
        }
        args.state_id
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let state_id = args.world.get_block_state_id(args.position);
        let props = OakLeavesLikeProperties::from_state_id(state_id, args.block);
        let updated_props = update_distance(&**args.world, args.position, props);
        let new_state_id = updated_props.to_state_id(args.block);
        if new_state_id != state_id {
            args.world.set_block_state(
                args.position,
                new_state_id,
                BlockFlags::NOTIFY_ALL,
            );
        }
    }

    fn random_tick(&self, args: RandomTickArgs<'_>) {
        let state_id = args.world.get_block_state_id(args.position);
        let props = OakLeavesLikeProperties::from_state_id(state_id, args.block);
        if !props.persistent && props.distance == DECAY_DISTANCE {
            args.world
                .break_block(args.position, None, BlockFlags::empty());
        }
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
