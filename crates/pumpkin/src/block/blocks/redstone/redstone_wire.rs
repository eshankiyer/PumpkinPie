use std::sync::Arc;

use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{
    BlockProperties, EastRedstone, HorizontalFacing, NorthRedstone, ObserverLikeProperties,
    RedstoneWireLikeProperties, RepeaterLikeProperties, SouthRedstone, WestRedstone,
};
use pumpkin_data::block_rotation::{Mirror, Rotation};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, BlockState, HorizontalFacingExt};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::{BlockAccessor, BlockFlags};

use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BrokenArgs, CanPlaceAtArgs, GetRedstonePowerArgs,
    GetStateForNeighborUpdateArgs, NormalUseArgs, OnNeighborUpdateArgs, OnPlaceArgs, PlacedArgs,
    PrepareArgs,
};
use crate::world::World;

use super::get_redstone_power_no_dust;

type RedstoneWireProperties = RedstoneWireLikeProperties;

#[pumpkin_block("minecraft:redstone_wire")]
pub struct RedstoneWireBlock;

impl BlockBehaviour for RedstoneWireBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_survive(args.block_accessor, args.position)
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut wire = RedstoneWireProperties::default(args.block);
        wire.power = calculate_target_strength(args.world, args.position);
        let wire = get_connection_state(args.world, wire, args.position);
        if is_dot(wire) { make_cross(wire.power).to_state_id(args.block) } else { wire.to_state_id(args.block) }
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        update_power_strength(args.world, args.position);
        for direction in [BlockDirection::Up, BlockDirection::Down] {
            let neighbor_pos = args.position.offset(direction.to_offset());
            update_neighbors_at(args.world, &neighbor_pos, &Block::REDSTONE_WIRE);
        }
        update_neighbors_of_neighboring_wires(args.world, args.position);
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        for direction in BlockDirection::all() {
            let neighbor_pos = args.position.offset(direction.to_offset());
            update_neighbors_at(args.world, &neighbor_pos, &Block::REDSTONE_WIRE);
        }
        update_neighbors_of_neighboring_wires(args.world, args.position);
    }

    fn on_state_replaced(&self, args: crate::block::OnStateReplacedArgs<'_>) {
        // RedStoneWireBlock.affectNeighborsAfterRemoval (vanilla:
        // RedStoneWireBlock.java:315-325) skips piston moves and updates the surrounding network.
        if !args.moved {
            update_neighbors_at(args.world, args.position, &Block::REDSTONE_WIRE);
            update_neighbors_of_neighboring_wires(args.world, args.position);
        }
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if args.direction == BlockDirection::Down {
            let (below_block, below_state) = args.world.get_block_and_state(args.neighbor_position);
            if !can_survive_on(below_block, below_state) { return Block::AIR.default_state.id; }
            return args.state_id;
        }
        let wire = RedstoneWireProperties::from_state_id(args.state_id, args.block);
        if args.direction == BlockDirection::Up {
            return get_connection_state(args.world, wire, args.position).to_state_id(args.block);
        }
        let can_connect_up = !args.world.get_block_state(&args.position.up()).is_solid_block();
        let side_connection = get_connecting_side(args.world, args.position, args.direction, can_connect_up);
        let Some(horizontal) = args.direction.to_horizontal_facing() else { return args.state_id; };
        let current_side = get_side_connection(wire, horizontal);
        if side_connection.is_connected() == current_side.is_connected() && !is_cross(wire) {
            let mut new_wire = wire;
            set_side_connection(&mut new_wire, horizontal, side_connection);
            new_wire.to_state_id(args.block)
        } else {
            let mut cross = make_cross(wire.power);
            set_side_connection(&mut cross, horizontal, side_connection);
            get_connection_state(args.world, cross, args.position).to_state_id(args.block)
        }
    }

    fn prepare(&self, args: PrepareArgs<'_>) {
        let wire = RedstoneWireProperties::from_state_id(args.state_id, args.block);
        for direction in BlockDirection::horizontal() {
            if is_side_connected_prop(wire, direction) {
                let dir_pos = args.position.offset(direction.to_offset());
                if args.world.get_block(&dir_pos) != &Block::REDSTONE_WIRE {
                    let down = dir_pos.down();
                    if args.world.get_block(&down) == &Block::REDSTONE_WIRE {
                        args.world.replace_with_state_for_neighbor_update(
                            &down, direction.opposite().to_block_direction(), args.flags,
                        );
                    }
                    let up = dir_pos.up();
                    if args.world.get_block(&up) == &Block::REDSTONE_WIRE {
                        args.world.replace_with_state_for_neighbor_update(
                            &up, direction.opposite().to_block_direction(), args.flags,
                        );
                    }
                }
            }
        }
        // Preserve the fork's corner propagation to torches and pistons.
        for side in BlockDirection::all() {
            let side_pos = args.position.offset(side.to_offset());
            for direction in BlockDirection::all() {
                if direction.opposite() == side { continue; }
                let target = side_pos.offset(direction.to_offset());
                let target_block = args.world.get_block(&target);
                if target_block == &Block::REDSTONE_WALL_TORCH
                    || target_block == &Block::PISTON
                    || target_block == &Block::STICKY_PISTON
                {
                    args.world.update_neighbor(&target, &Block::REDSTONE_WIRE);
                }
            }
        }
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        // RedStoneWireBlock.useWithoutItem (vanilla: RedStoneWireBlock.java:490-506) does not
        // toggle the shape when the player may not build.
        if !args.player.abilities.lock().unwrap_or_else(std::sync::PoisonError::into_inner).allow_modify_world {
            return BlockActionResult::Pass;
        }
        let state = args.world.get_block_state(args.position);
        let wire = RedstoneWireProperties::from_state_id(state.id, args.block);
        if is_cross(wire) || is_dot(wire) {
            let mut new_wire = if is_cross(wire) {
                RedstoneWireProperties::default(&Block::REDSTONE_WIRE)
            } else { make_cross(wire.power) };
            new_wire.power = wire.power;
            new_wire = get_connection_state(args.world, new_wire, args.position);
            if wire != new_wire {
                args.world.set_block_state(args.position, new_wire.to_state_id(&Block::REDSTONE_WIRE), BlockFlags::NOTIFY_ALL);
                for direction in BlockDirection::horizontal() {
                    let relative = args.position.offset(direction.to_offset());
                    if is_side_connected_prop(wire, direction) != is_side_connected_prop(new_wire, direction)
                        && args.world.get_block_state(&relative).is_solid_block()
                    {
                        args.world.update_neighbors(&relative, Some(direction.opposite().to_block_direction()));
                    }
                }
                return BlockActionResult::Success;
            }
        }
        BlockActionResult::Pass
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        if can_survive(args.world.as_ref(), args.position) {
            update_power_strength(args.world, args.position);
        } else {
            args.world.break_block(args.position, None, BlockFlags::NOTIFY_ALL);
        }
    }

    fn get_weak_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8 {
        let wire = RedstoneWireProperties::from_state_id(args.state.id, args.block);
        if wire.power == 0 || args.direction == BlockDirection::Down { return 0; }
        if args.direction == BlockDirection::Up { return wire.power; }
        args.direction.opposite().to_horizontal_facing()
            .filter(|f| is_side_connected_prop(wire, *f)).map_or(0, |_| wire.power)
    }

    fn get_strong_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8 {
        self.get_weak_redstone_power(args)
    }

    fn rotate(&self, block: &Block, state_id: BlockStateId, rotation: Rotation) -> &'static BlockState {
        let wire = RedstoneWireProperties::from_state_id(state_id, block);
        BlockState::from_id(rotate_wire(wire, rotation).to_state_id(block))
    }

    fn mirror(&self, block: &Block, state_id: BlockStateId, mirror: Mirror) -> &'static BlockState {
        let wire = RedstoneWireProperties::from_state_id(state_id, block);
        BlockState::from_id(mirror_wire(wire, mirror).to_state_id(block))
    }
}

fn mirror_wire(wire: RedstoneWireProperties, mirror: Mirror) -> RedstoneWireProperties {
    match mirror {
        Mirror::LeftRight => RedstoneWireProperties {
            north: wire.south.to_wire_connection().to_north(),
            south: wire.north.to_wire_connection().to_south(), ..wire
        },
        Mirror::FrontBack => RedstoneWireProperties {
            east: wire.west.to_wire_connection().to_east(),
            west: wire.east.to_wire_connection().to_west(), ..wire
        },
        Mirror::None => wire,
    }
}

fn rotate_wire(wire: RedstoneWireProperties, rotation: Rotation) -> RedstoneWireProperties {
    match rotation {
        Rotation::Rotate180 => RedstoneWireProperties {
            north: wire.south.to_wire_connection().to_north(), east: wire.west.to_wire_connection().to_east(),
            south: wire.north.to_wire_connection().to_south(), west: wire.east.to_wire_connection().to_west(), power: wire.power,
        },
        Rotation::CounterClockwise90 => RedstoneWireProperties {
            north: wire.east.to_wire_connection().to_north(), east: wire.south.to_wire_connection().to_east(),
            south: wire.west.to_wire_connection().to_south(), west: wire.north.to_wire_connection().to_west(), power: wire.power,
        },
        Rotation::Clockwise90 => RedstoneWireProperties {
            north: wire.west.to_wire_connection().to_north(), east: wire.north.to_wire_connection().to_east(),
            south: wire.east.to_wire_connection().to_south(), west: wire.south.to_wire_connection().to_west(), power: wire.power,
        },
        Rotation::None => wire,
    }
}

pub fn update_power_strength(world: &Arc<World>, pos: &BlockPos) {
    let (block, state) = world.get_block_and_state(pos);
    if block != &Block::REDSTONE_WIRE { return; }
    let mut wire = RedstoneWireProperties::from_state_id(state.id, block);
    let target = calculate_target_strength(world, pos);
    if wire.power != target {
        wire.power = target;
        world.set_block_state(pos, wire.to_state_id(&Block::REDSTONE_WIRE), BlockFlags::empty());
        let mut positions = Vec::with_capacity(7);
        positions.push(*pos);
        for direction in BlockDirection::all() { positions.push(pos.offset(direction.to_offset())); }
        for position in positions { update_neighbors_at(world, &position, &Block::REDSTONE_WIRE); }
    }
}

fn calculate_target_strength(world: &World, pos: &BlockPos) -> u8 {
    let block_signal = get_block_signal(world, pos);
    if block_signal == 15 { return 15; }
    block_signal.max(get_incoming_wire_signal(world, pos))
}

fn get_block_signal(world: &World, pos: &BlockPos) -> u8 {
    let mut max_signal = 0;
    for side in BlockDirection::all() {
        let neighbor_pos = pos.offset(side.to_offset());
        let (block, state) = world.get_block_and_state(&neighbor_pos);
        let signal = get_redstone_power_no_dust(block, state, world, neighbor_pos, side);
        if signal == 15 { return 15; }
        max_signal = max_signal.max(signal);
    }
    max_signal
}

fn get_incoming_wire_signal(world: &World, pos: &BlockPos) -> u8 {
    let mut max_signal = 0;
    let up_pos = pos.up();
    let up_conductor = world.get_block_state(&up_pos).is_solid_block();
    for direction in BlockDirection::horizontal() {
        let neighbor_pos = pos.offset(direction.to_offset());
        let (neighbor_block, neighbor_state) = world.get_block_and_state(&neighbor_pos);
        if neighbor_block == &Block::REDSTONE_WIRE {
            max_signal = max_signal.max(RedstoneWireProperties::from_state_id(neighbor_state.id, neighbor_block).power);
        }
        if !up_conductor && neighbor_state.is_solid_block() {
            let (block, state) = world.get_block_and_state(&neighbor_pos.up());
            if block == &Block::REDSTONE_WIRE {
                max_signal = max_signal.max(RedstoneWireProperties::from_state_id(state.id, block).power);
            }
        } else if !neighbor_state.is_solid_block() {
            let (block, state) = world.get_block_and_state(&neighbor_pos.down());
            if block == &Block::REDSTONE_WIRE {
                max_signal = max_signal.max(RedstoneWireProperties::from_state_id(state.id, block).power);
            }
        }
    }
    max_signal.saturating_sub(1)
}

pub fn update_neighbors_at(world: &Arc<World>, pos: &BlockPos, source_block: &Block) {
    for direction in BlockDirection::update_order() {
        world.update_neighbor(&pos.offset(direction.to_offset()), source_block);
    }
}

fn check_corner_change_at(world: &Arc<World>, pos: &BlockPos) {
    if world.get_block(pos) == &Block::REDSTONE_WIRE {
        update_neighbors_at(world, pos, &Block::REDSTONE_WIRE);
        for direction in BlockDirection::all() { update_neighbors_at(world, &pos.offset(direction.to_offset()), &Block::REDSTONE_WIRE); }
    }
}

fn update_neighbors_of_neighboring_wires(world: &Arc<World>, pos: &BlockPos) {
    for direction in BlockDirection::horizontal() {
        let neighbor = pos.offset(direction.to_offset());
        check_corner_change_at(world, &neighbor);
        let target = neighbor;
        if world.get_block_state(&target).is_solid_block() { check_corner_change_at(world, &target.up()); }
        else { check_corner_change_at(world, &target.down()); }
    }
}

pub fn get_connection_state(world: &World, state: RedstoneWireProperties, pos: &BlockPos) -> RedstoneWireProperties {
    let was_dot = is_dot(state);
    let mut new_state = get_missing_connections(world, RedstoneWireProperties { power: state.power, ..RedstoneWireProperties::default(&Block::REDSTONE_WIRE) }, pos);
    if was_dot && is_dot(new_state) { return new_state; }
    let ns = !new_state.north.to_wire_connection().is_connected();
    let ss = !new_state.south.to_wire_connection().is_connected();
    let es = !new_state.east.to_wire_connection().is_connected();
    let ws = !new_state.west.to_wire_connection().is_connected();
    if ws && ns && ss { new_state.west = WestRedstone::Side; }
    if es && ns && ss { new_state.east = EastRedstone::Side; }
    if ns && es && ws { new_state.north = NorthRedstone::Side; }
    if ss && es && ws { new_state.south = SouthRedstone::Side; }
    new_state
}

fn get_missing_connections(world: &World, mut state: RedstoneWireProperties, pos: &BlockPos) -> RedstoneWireProperties {
    let can_connect_up = !world.get_block_state(&pos.up()).is_solid_block();
    for direction in BlockDirection::horizontal() {
        if !is_side_connected_prop(state, direction) {
            let connection = get_connecting_side(world, pos, direction.to_block_direction(), can_connect_up);
            set_side_connection(&mut state, direction, connection);
        }
    }
    state
}

fn get_connecting_side(world: &World, pos: &BlockPos, direction: BlockDirection, can_connect_up: bool) -> WireConnection {
    let relative_pos = pos.offset(direction.to_offset());
    let (relative_block, relative_state) = world.get_block_and_state(&relative_pos);
    if can_connect_up {
        let placeable_above = is_trapdoor(relative_block) || can_survive_on(relative_block, relative_state);
        let (above_block, above_state) = world.get_block_and_state(&relative_pos.up());
        if placeable_above && should_connect_to(world, above_block, above_state, None) {
            return if relative_state.is_side_solid(direction.opposite()) { WireConnection::Up } else { WireConnection::Side };
        }
    }
    let relative_connected = should_connect_to(world, relative_block, relative_state, Some(direction));
    let (below_block, below_state) = world.get_block_and_state(&relative_pos.down());
    let below_connected = should_connect_to(world, below_block, below_state, None);
    if !relative_connected && (relative_state.is_solid_block() || !below_connected) { WireConnection::None } else { WireConnection::Side }
}

fn should_connect_to(world: &World, block: &Block, state: &BlockState, direction: Option<BlockDirection>) -> bool {
    if block == &Block::REDSTONE_WIRE { return true; }
    if block == &Block::REPEATER {
        let facing = RepeaterLikeProperties::from_state_id(state.id, block).facing.to_block_direction();
        return direction.is_some_and(|dir| facing == dir || facing == dir.opposite());
    }
    if block == &Block::OBSERVER {
        let facing = ObserverLikeProperties::from_state_id(state.id, block).facing;
        return direction.is_some_and(|dir| dir.to_facing() == facing);
    }
    direction.is_some_and(|dir| world.block_registry.emits_redstone_power(block, state, dir))
}

fn is_trapdoor(block: &Block) -> bool { block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_TRAPDOORS) }

fn can_survive(world: &dyn BlockAccessor, pos: &BlockPos) -> bool {
    let below = pos.down();
    let (block, state) = world.get_block_and_state(&below);
    can_survive_on(block, state)
}

fn can_survive_on(block: &Block, state: &BlockState) -> bool {
    state.is_side_solid(BlockDirection::Up) || block == &Block::HOPPER
}

#[must_use]
pub fn is_dot(wire: RedstoneWireProperties) -> bool {
    !wire.north.to_wire_connection().is_connected() && !wire.south.to_wire_connection().is_connected()
        && !wire.east.to_wire_connection().is_connected() && !wire.west.to_wire_connection().is_connected()
}

#[must_use]
pub fn is_cross(wire: RedstoneWireProperties) -> bool {
    wire.north.to_wire_connection().is_connected() && wire.south.to_wire_connection().is_connected()
        && wire.east.to_wire_connection().is_connected() && wire.west.to_wire_connection().is_connected()
}

#[must_use]
pub const fn make_cross(power: u8) -> RedstoneWireProperties {
    RedstoneWireProperties { north: NorthRedstone::Side, south: SouthRedstone::Side, east: EastRedstone::Side, west: WestRedstone::Side, power }
}

fn get_side_connection(wire: RedstoneWireProperties, direction: HorizontalFacing) -> WireConnection {
    match direction {
        HorizontalFacing::North => wire.north.to_wire_connection(), HorizontalFacing::South => wire.south.to_wire_connection(),
        HorizontalFacing::East => wire.east.to_wire_connection(), HorizontalFacing::West => wire.west.to_wire_connection(),
    }
}

const fn set_side_connection(wire: &mut RedstoneWireProperties, direction: HorizontalFacing, connection: WireConnection) {
    match direction {
        HorizontalFacing::North => wire.north = connection.to_north(), HorizontalFacing::South => wire.south = connection.to_south(),
        HorizontalFacing::East => wire.east = connection.to_east(), HorizontalFacing::West => wire.west = connection.to_west(),
    }
}

fn is_side_connected_prop(wire: RedstoneWireProperties, direction: HorizontalFacing) -> bool { get_side_connection(wire, direction).is_connected() }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireConnection { Up, Side, None }

impl WireConnection {
    #[must_use] pub const fn is_connected(self) -> bool { !matches!(self, Self::None) }
    pub const fn to_north(self) -> NorthRedstone { match self { Self::Up => NorthRedstone::Up, Self::Side => NorthRedstone::Side, Self::None => NorthRedstone::None } }
    pub const fn to_south(self) -> SouthRedstone { match self { Self::Up => SouthRedstone::Up, Self::Side => SouthRedstone::Side, Self::None => SouthRedstone::None } }
    pub const fn to_east(self) -> EastRedstone { match self { Self::Up => EastRedstone::Up, Self::Side => EastRedstone::Side, Self::None => EastRedstone::None } }
    pub const fn to_west(self) -> WestRedstone { match self { Self::Up => WestRedstone::Up, Self::Side => WestRedstone::Side, Self::None => WestRedstone::None } }
}

pub trait CardinalWireConnectionExt { fn to_wire_connection(&self) -> WireConnection; }
impl CardinalWireConnectionExt for NorthRedstone { fn to_wire_connection(&self) -> WireConnection { match self { Self::Side => WireConnection::Side, Self::Up => WireConnection::Up, Self::None => WireConnection::None } } }
impl CardinalWireConnectionExt for SouthRedstone { fn to_wire_connection(&self) -> WireConnection { match self { Self::Side => WireConnection::Side, Self::Up => WireConnection::Up, Self::None => WireConnection::None } } }
impl CardinalWireConnectionExt for EastRedstone { fn to_wire_connection(&self) -> WireConnection { match self { Self::Side => WireConnection::Side, Self::Up => WireConnection::Up, Self::None => WireConnection::None } } }
impl CardinalWireConnectionExt for WestRedstone { fn to_wire_connection(&self) -> WireConnection { match self { Self::Side => WireConnection::Side, Self::Up => WireConnection::Up, Self::None => WireConnection::None } } }

#[cfg(test)]
mod rotate_mirror_tests {
    use super::*;

    const fn wire(
        north: NorthRedstone,
        south: SouthRedstone,
        east: EastRedstone,
        west: WestRedstone,
    ) -> RedstoneWireProperties {
        RedstoneWireProperties {
            north,
            south,
            east,
            west,
            power: 0,
        }
    }

    #[test]
    fn rotate_none_is_identity() {
        let w = wire(
            NorthRedstone::Side,
            SouthRedstone::None,
            EastRedstone::Up,
            WestRedstone::None,
        );
        assert_eq!(rotate_wire(w, Rotation::None), w);
    }

    #[test]
    fn rotate_clockwise_90_permutes_sides() {
        let w = wire(
            NorthRedstone::Side,
            SouthRedstone::None,
            EastRedstone::Up,
            WestRedstone::None,
        );
        let rotated = rotate_wire(w, Rotation::Clockwise90);
        assert_eq!(
            rotated.north,
            WestRedstone::None.to_wire_connection().to_north()
        );
        assert_eq!(
            rotated.east,
            NorthRedstone::Side.to_wire_connection().to_east()
        );
        assert_eq!(
            rotated.south,
            EastRedstone::Up.to_wire_connection().to_south()
        );
        assert_eq!(
            rotated.west,
            SouthRedstone::None.to_wire_connection().to_west()
        );
    }

    #[test]
    fn rotate_counter_clockwise_90_is_inverse_of_clockwise_90() {
        let w = wire(
            NorthRedstone::Side,
            SouthRedstone::Up,
            EastRedstone::None,
            WestRedstone::Side,
        );
        let round_tripped = rotate_wire(
            rotate_wire(w, Rotation::Clockwise90),
            Rotation::CounterClockwise90,
        );
        assert_eq!(round_tripped, w);
    }

    #[test]
    fn rotate_180_twice_is_identity() {
        let w = wire(
            NorthRedstone::Side,
            SouthRedstone::Up,
            EastRedstone::None,
            WestRedstone::Side,
        );
        let twice = rotate_wire(rotate_wire(w, Rotation::Rotate180), Rotation::Rotate180);
        assert_eq!(twice, w);
    }

    #[test]
    fn mirror_none_is_identity() {
        let w = wire(
            NorthRedstone::Side,
            SouthRedstone::None,
            EastRedstone::Up,
            WestRedstone::None,
        );
        assert_eq!(mirror_wire(w, Mirror::None), w);
    }

    #[test]
    fn mirror_left_right_swaps_north_south_only() {
        let w = wire(
            NorthRedstone::Side,
            SouthRedstone::None,
            EastRedstone::Up,
            WestRedstone::None,
        );
        let mirrored = mirror_wire(w, Mirror::LeftRight);
        assert_eq!(
            mirrored.north,
            SouthRedstone::None.to_wire_connection().to_north()
        );
        assert_eq!(
            mirrored.south,
            NorthRedstone::Side.to_wire_connection().to_south()
        );
        assert_eq!(mirrored.east, w.east);
        assert_eq!(mirrored.west, w.west);
    }

    #[test]
    fn mirror_front_back_swaps_east_west_only() {
        let w = wire(
            NorthRedstone::Side,
            SouthRedstone::None,
            EastRedstone::Up,
            WestRedstone::None,
        );
        let mirrored = mirror_wire(w, Mirror::FrontBack);
        assert_eq!(
            mirrored.east,
            WestRedstone::None.to_wire_connection().to_east()
        );
        assert_eq!(
            mirrored.west,
            EastRedstone::Up.to_wire_connection().to_west()
        );
        assert_eq!(mirrored.north, w.north);
        assert_eq!(mirrored.south, w.south);
    }

    #[test]
    fn mirror_applied_twice_is_identity() {
        let w = wire(
            NorthRedstone::Side,
            SouthRedstone::Up,
            EastRedstone::None,
            WestRedstone::Side,
        );
        assert_eq!(
            mirror_wire(mirror_wire(w, Mirror::LeftRight), Mirror::LeftRight),
            w
        );
        assert_eq!(
            mirror_wire(mirror_wire(w, Mirror::FrontBack), Mirror::FrontBack),
            w
        );
    }
}
