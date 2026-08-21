use pumpkin_data::block_properties::{
    AcaciaShelfLikeProperties, BlockProperties, HorizontalFacing, SideChainPart,
};
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockStateId, HorizontalFacingExt};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::entities::shelf::ShelfBlockEntity;
use crate::block::{
    BlockBehaviour, BlockFuture, GetComparatorOutputArgs, OnNeighborUpdateArgs, OnPlaceArgs,
    OnStateReplacedArgs, PlacedArgs,
};
use crate::entity::EntityBase;
use crate::world::World;
use std::sync::Arc;

type ShelfProperties = AcaciaShelfLikeProperties;

/// `ShelfBlock.getMaxChainLength` (`ShelfBlock.java:272-274`).
const MAX_CHAIN_LENGTH: i32 = 3;

#[pumpkin_block_from_tag("minecraft:wooden_shelves")]
pub struct ShelfBlock;

// --- `SideChainPart` (`SideChainPart.java:27-65`) ---

const fn part_is_connected(part: SideChainPart) -> bool {
    !matches!(part, SideChainPart::Unconnected)
}

const fn part_is_chain_end(part: SideChainPart) -> bool {
    !matches!(part, SideChainPart::Center)
}

fn part_is_connection_towards(part: SideChainPart, end_part: SideChainPart) -> bool {
    matches!(part, SideChainPart::Center) || part == end_part
}

const fn when_connected_to_the_right(part: SideChainPart) -> SideChainPart {
    match part {
        SideChainPart::Unconnected | SideChainPart::Left => SideChainPart::Left,
        SideChainPart::Right | SideChainPart::Center => SideChainPart::Center,
    }
}

const fn when_connected_to_the_left(part: SideChainPart) -> SideChainPart {
    match part {
        SideChainPart::Unconnected | SideChainPart::Right => SideChainPart::Right,
        SideChainPart::Center | SideChainPart::Left => SideChainPart::Center,
    }
}

const fn when_disconnected_from_the_right(part: SideChainPart) -> SideChainPart {
    match part {
        SideChainPart::Unconnected | SideChainPart::Left => SideChainPart::Unconnected,
        SideChainPart::Right | SideChainPart::Center => SideChainPart::Right,
    }
}

const fn when_disconnected_from_the_left(part: SideChainPart) -> SideChainPart {
    match part {
        SideChainPart::Unconnected | SideChainPart::Right => SideChainPart::Unconnected,
        SideChainPart::Center | SideChainPart::Left => SideChainPart::Left,
    }
}

// --- `SideChainPartBlock` (`SideChainPartBlock.java`), whose sole implementor is `ShelfBlock` ---

/// `ShelfBlock.isConnectable` (`ShelfBlock.java:267-270`): a powered wooden shelf.
fn is_connectable(block: &Block, state_id: BlockStateId) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_WOODEN_SHELVES)
        && ShelfProperties::from_state_id(state_id, block).powered
}

/// One side-chain neighbour. `None` data is `SideChainPartBlock.EmptyNeighbor`
/// (`SideChainPartBlock.java:106-121`).
struct Neighbor {
    pos: BlockPos,
    data: Option<(&'static Block, BlockStateId, SideChainPart)>,
}

impl Neighbor {
    const fn is_connectable(&self) -> bool {
        self.data.is_some()
    }

    /// `SideChainPartBlock.Neighbor.isUnconnectableOrChainEnd`.
    fn is_unconnectable_or_chain_end(&self) -> bool {
        self.data.is_none_or(|(_, _, part)| part_is_chain_end(part))
    }

    /// `SideChainPartBlock.Neighbor.connectsTowards`.
    fn connects_towards(&self, end_part: SideChainPart) -> bool {
        self.data
            .is_some_and(|(_, _, part)| part_is_connection_towards(part, end_part))
    }
}

/// `SideChainPartBlock.Neighbors.createNewNeighbor` (`SideChainPartBlock.java:150-154`): only a
/// connectable shelf that faces the same way as the centre counts.
fn read_neighbor(world: &World, pos: BlockPos, facing: HorizontalFacing) -> Neighbor {
    let block = world.get_block(&pos);
    let state_id = world.get_block_state_id(&pos);
    if is_connectable(block, state_id) {
        let props = ShelfProperties::from_state_id(state_id, block);
        if props.facing == facing {
            return Neighbor {
                pos,
                data: Some((block, state_id, props.side_chain)),
            };
        }
    }
    Neighbor { pos, data: None }
}

/// `SideChainPartBlock.Neighbors.left` (`SideChainPartBlock.java:160-162`).
fn left_pos(center: &BlockPos, facing: HorizontalFacing, steps: i32) -> BlockPos {
    center.offset(facing.to_block_direction().rotate_clockwise().to_offset() * steps)
}

/// `SideChainPartBlock.Neighbors.right` (`SideChainPartBlock.java:164-166`).
fn right_pos(center: &BlockPos, facing: HorizontalFacing, steps: i32) -> BlockPos {
    center.offset(
        facing
            .to_block_direction()
            .rotate_counter_clockwise()
            .to_offset()
            * steps,
    )
}

/// `SideChainPartBlock.setPart` (`SideChainPartBlock.java:99-104`).
async fn set_part(world: &Arc<World>, pos: &BlockPos, new_part: SideChainPart) {
    let block = world.get_block(pos);
    let state_id = world.get_block_state_id(pos);
    if !block.has_tag(&tag::Block::MINECRAFT_WOODEN_SHELVES) {
        return;
    }
    let mut props = ShelfProperties::from_state_id(state_id, block);
    if props.side_chain == new_part {
        return;
    }
    props.side_chain = new_part;
    world
        .set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
        .await;
}

/// `SideChainPartBlock.addBlocksConnectingTowards` (`SideChainPartBlock.java:40-53`), reduced to
/// the chain length, which is all `updateSelfAndNeighborsOnPoweringUp` needs.
fn count_blocks_connecting_towards(
    world: &World,
    center: &BlockPos,
    facing: HorizontalFacing,
    towards_left: bool,
    end_part: SideChainPart,
) -> i32 {
    let mut count = 0;
    for steps in 1..MAX_CHAIN_LENGTH {
        let pos = if towards_left {
            left_pos(center, facing, steps)
        } else {
            right_pos(center, facing, steps)
        };
        let neighbor = read_neighbor(world, pos, facing);
        if neighbor.connects_towards(end_part) {
            count += 1;
        }
        if neighbor.is_unconnectable_or_chain_end() {
            break;
        }
    }
    count
}

/// `SideChainPartBlock.getAllBlocksConnectedTo(...).size()` (`SideChainPartBlock.java:26-38`).
fn connected_chain_length(world: &World, pos: &BlockPos) -> i32 {
    let block = world.get_block(pos);
    let state_id = world.get_block_state_id(pos);
    if !is_connectable(block, state_id) {
        return 0;
    }
    let facing = ShelfProperties::from_state_id(state_id, block).facing;
    1 + count_blocks_connecting_towards(world, pos, facing, true, SideChainPart::Left)
        + count_blocks_connecting_towards(world, pos, facing, false, SideChainPart::Right)
}

/// `SideChainPartBlock.canConnect` (`SideChainPartBlock.java:85-87`).
const fn can_connect(new_blocks_to_connect_to: i32, current_chain_length: i32) -> bool {
    new_blocks_to_connect_to > 0
        && current_chain_length + new_blocks_to_connect_to <= MAX_CHAIN_LENGTH
}

/// `SideChainPartBlock.updateNeighborsAfterPoweringDown` (`SideChainPartBlock.java:55-59`).
async fn update_neighbors_after_powering_down(
    world: &Arc<World>,
    pos: &BlockPos,
    facing: HorizontalFacing,
) {
    let left = read_neighbor(world, left_pos(pos, facing, 1), facing);
    if let Some((_, _, part)) = left.data {
        set_part(world, &left.pos, when_disconnected_from_the_right(part)).await;
    }
    let right = read_neighbor(world, right_pos(pos, facing, 1), facing);
    if let Some((_, _, part)) = right.data {
        set_part(world, &right.pos, when_disconnected_from_the_left(part)).await;
    }
}

/// `SideChainPartBlock.isBeingUpdatedByNeighbor` (`SideChainPartBlock.java:89-93`).
fn is_being_updated_by_neighbor(
    state_id: BlockStateId,
    block: &Block,
    old_state_id: BlockStateId,
) -> bool {
    let is_getting_connected =
        part_is_connected(ShelfProperties::from_state_id(state_id, block).side_chain);
    let old_block = Block::from_state_id(old_state_id);
    let has_been_connected_before = is_connectable(old_block, old_state_id)
        && part_is_connected(ShelfProperties::from_state_id(old_state_id, old_block).side_chain);
    is_getting_connected || has_been_connected_before
}

/// `SideChainPartBlock.updateSelfAndNeighborsOnPoweringUp` (`SideChainPartBlock.java:61-83`).
async fn update_self_and_neighbors_on_powering_up(
    world: &Arc<World>,
    pos: &BlockPos,
    block: &Block,
    state_id: BlockStateId,
    old_state_id: BlockStateId,
) {
    if !is_connectable(block, state_id) {
        return;
    }
    if is_being_updated_by_neighbor(state_id, block, old_state_id) {
        return;
    }

    let facing = ShelfProperties::from_state_id(state_id, block).facing;
    let left = read_neighbor(world, left_pos(pos, facing, 1), facing);
    let right = read_neighbor(world, right_pos(pos, facing, 1), facing);

    let existing_chain_on_the_left = if left.is_connectable() {
        connected_chain_length(world, &left.pos)
    } else {
        0
    };
    let existing_chain_on_the_right = if right.is_connectable() {
        connected_chain_length(world, &right.pos)
    } else {
        0
    };

    let mut new_part_for_self = SideChainPart::Unconnected;
    let mut current_chain_length = 1;
    if can_connect(existing_chain_on_the_left, current_chain_length) {
        new_part_for_self = when_connected_to_the_left(new_part_for_self);
        if let Some((_, _, part)) = left.data {
            set_part(world, &left.pos, when_connected_to_the_right(part)).await;
        }
        current_chain_length += existing_chain_on_the_left;
    }
    if can_connect(existing_chain_on_the_right, current_chain_length) {
        new_part_for_self = when_connected_to_the_right(new_part_for_self);
        if let Some((_, _, part)) = right.data {
            set_part(world, &right.pos, when_connected_to_the_left(part)).await;
        }
    }

    set_part(world, pos, new_part_for_self).await;
}

/// `ShelfBlock.onPlace` (`ShelfBlock.java:277-285`).
async fn update_chain(
    world: &Arc<World>,
    pos: &BlockPos,
    block: &Block,
    state_id: BlockStateId,
    old_state_id: BlockStateId,
) {
    let props = ShelfProperties::from_state_id(state_id, block);
    if props.powered {
        update_self_and_neighbors_on_powering_up(world, pos, block, state_id, old_state_id).await;
    } else {
        update_neighbors_after_powering_down(world, pos, props.facing).await;
    }
}

impl BlockBehaviour for ShelfBlock {
    /// `ShelfBlock.getStateForPlacement` (`ShelfBlock.java:126-133`).
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut properties = ShelfProperties::default(args.block);

            // Face in the opposite direction the player is facing
            properties.facing = args.player.get_entity().get_horizontal_facing().opposite();
            properties.waterlogged = args.replacing.water_source();
            properties.powered = block_receives_redstone_power(args.world, args.position).await;

            properties.to_state_id(args.block)
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = ShelfBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(entity));

            update_chain(
                args.world,
                args.position,
                args.block,
                args.state_id,
                args.old_state_id,
            )
            .await;
        })
    }

    /// `ShelfBlock.neighborChanged` (`ShelfBlock.java:107-122`).
    ///
    /// Vanilla reaches the side-chain update through `setBlock`'s `onPlace` callback. Pumpkin only
    /// fires `placed` when the *block* changes, not on a state-only edit, so `update_chain` is
    /// invoked here directly.
    fn on_neighbor_update<'a>(&'a self, args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let mut props = ShelfProperties::from_state_id(state_id, args.block);
            let signal = block_receives_redstone_power(args.world, args.position).await;
            if props.powered == signal {
                return;
            }

            props.powered = signal;
            if !signal {
                props.side_chain = SideChainPart::Unconnected;
            }
            let new_state_id = props.to_state_id(args.block);
            args.world
                .set_block_state(args.position, new_state_id, BlockFlags::NOTIFY_ALL)
                .await;

            args.world.play_sound(
                if signal {
                    Sound::BlockShelfActivate
                } else {
                    Sound::BlockShelfDeactivate
                },
                SoundCategory::Blocks,
                &args.position.to_f64(),
            );

            update_chain(
                args.world,
                args.position,
                args.block,
                new_state_id,
                state_id,
            )
            .await;
        })
    }

    /// `ShelfBlock.affectNeighborsAfterRemoval` (`ShelfBlock.java:101-105`): the surviving
    /// neighbours have to forget the shelf that just went away.
    fn on_state_replaced<'a>(&'a self, args: OnStateReplacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !args.block.has_tag(&tag::Block::MINECRAFT_WOODEN_SHELVES) {
                return;
            }
            let facing = ShelfProperties::from_state_id(args.old_state_id, args.block).facing;
            update_neighbors_after_powering_down(args.world, args.position, facing).await;
        })
    }

    /// `ShelfBlock.getAnalogOutputSignal` (`ShelfBlock.java:314-327`): one bit per occupied slot,
    /// so a full shelf reads 7.
    ///
    /// Vanilla additionally returns 0 unless the querying comparator sits on
    /// `FACING.getOpposite()` (`ShelfBlock.java:322-323`). `GetComparatorOutputArgs` carries no
    /// direction, and the only callers live in `blocks/redstone/comparator.rs`, so that filter is
    /// not reproducible without widening the trait; the signal is emitted on every side here.
    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return Some(0);
            };
            let Some(shelf) = block_entity.as_any().downcast_ref::<ShelfBlockEntity>() else {
                return Some(0);
            };

            let items = shelf.items.read().await;
            let mut signal = 0u8;
            for (slot, item) in items.iter().enumerate() {
                if !item.is_empty() {
                    signal |= 1 << slot;
                }
            }
            Some(signal)
        })
    }
}
