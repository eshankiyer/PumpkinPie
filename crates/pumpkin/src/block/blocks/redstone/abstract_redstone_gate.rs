use std::sync::Arc;

use pumpkin_data::{
    Block, BlockDirection, BlockState, BlockStateId, HorizontalFacingExt,
    block_properties::{
        BlockProperties, ComparatorLikeProperties, HorizontalFacing, RedstoneWireLikeProperties,
        RepeaterLikeProperties,
    },
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::{
    tick::TickPriority,
    world::{BlockAccessor, BlockFlags},
};

use crate::{
    block::{GetRedstonePowerArgs, OnNeighborUpdateArgs, OnStateReplacedArgs, PlayerPlacedArgs},
    entity::player::Player,
    world::World,
};

use super::{get_redstone_power, is_diode};

pub trait RedstoneGateBlockProperties {
    fn is_powered(&self) -> bool;
    fn get_facing(&self) -> HorizontalFacing;
    fn set_facing(&mut self, facing: HorizontalFacing);
}

pub trait RedstoneGateBlock<T: Send + Sync + BlockProperties + RedstoneGateBlockProperties> {
    fn can_place_at(&self, world: &dyn BlockAccessor, pos: BlockPos) -> bool
    where
        Self: Send + Sync,
    {
        let under_pos = pos.down();
        let under_state = world.get_block_state(&under_pos);
        self.can_place_above(world, under_pos, under_state)
    }

    fn can_place_above(
        &self,
        _world: &dyn BlockAccessor,
        _pos: BlockPos,
        state: &BlockState,
    ) -> bool {
        state.is_side_solid(BlockDirection::Up)
    }

    fn get_weak_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8
    where
        Self: Send + Sync,
    {
        let props = T::from_state_id(args.state.id, args.block);
        if props.is_powered() && props.get_facing().to_block_direction() == args.direction {
            self.get_output_level(args.world, *args.position)
        } else {
            0
        }
    }

    fn get_strong_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8
    where
        Self: Send + Sync,
    {
        self.get_weak_redstone_power(args)
    }

    fn get_output_level(&self, world: &World, pos: BlockPos) -> u8;

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>)
    where
        Self: Send + Sync,
    {
        let state = args.world.get_block_state(args.position);
        if RedstoneGateBlock::can_place_at(self, args.world.as_ref(), *args.position) {
            self.update_powered(args.world, *args.position, state, args.block);
            return;
        }
        // `DiodeBlock.neighborChanged` (DiodeBlock.java:77-79) calls `dropResources` before
        // `removeBlock`, so a repeater or comparator whose support is broken drops as an item.
        // Writing AIR straight into the world skipped the drop and destroyed the gate.
        args.world
            .break_block(args.position, None, BlockFlags::NOTIFY_ALL);
        for dir in BlockDirection::all() {
            args.world
                .update_neighbor(&args.position.offset(dir.to_offset()), args.source_block);
        }
    }

    fn update_powered(&self, world: &World, pos: BlockPos, state: &BlockState, block: &Block);

    fn has_power(&self, world: &World, pos: BlockPos, state: &BlockState, block: &Block) -> bool
    where
        Self: Send + Sync,
    {
        self.get_power(world, pos, state, block) > 0
    }

    fn get_power(&self, world: &World, pos: BlockPos, state: &BlockState, block: &Block) -> u8
    where
        Self: Send + Sync,
    {
        get_power::<T>(world, pos, state.id, block)
    }

    fn get_max_input_level_sides(
        &self,
        world: &World,
        pos: BlockPos,
        state_id: BlockStateId,
        block: &Block,
        only_gate: bool,
    ) -> u8 {
        let props = T::from_state_id(state_id, block);
        let facing = props.get_facing();

        let power_left = get_power_on_side(world, &pos, facing.rotate_clockwise(), only_gate);
        let power_right =
            get_power_on_side(world, &pos, facing.rotate_counter_clockwise(), only_gate);

        std::cmp::max(power_left, power_right)
    }

    fn update_target(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        state_id: BlockStateId,
        block: &Block,
    ) {
        let props = T::from_state_id(state_id, block);
        let facing = props.get_facing();
        let front_pos = pos.offset(facing.opposite().to_offset());
        world.update_neighbor(&front_pos, block);
        world.update_neighbors(&front_pos, Some(facing.to_block_direction()));
    }

    fn on_place(&self, player: &Player, block: &Block) -> BlockStateId {
        let mut props = T::default(block);
        let dir = player
            .living_entity
            .entity
            .get_horizontal_facing()
            .opposite();
        props.set_facing(dir);

        props.to_state_id(block)
    }

    fn player_placed(&self, args: PlayerPlacedArgs<'_>)
    where
        Self: Send + Sync,
    {
        if RedstoneGateBlock::has_power(
            self,
            args.world,
            *args.position,
            BlockState::from_id(args.state_id),
            args.block,
        ) {
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
        }
    }

    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>)
    where
        Self: Send + Sync,
    {
        // `DiodeBlock.affectNeighborsAfterRemoval` (DiodeBlock.java:174-178) skips the
        // front-neighbour update only when a piston moved the gate. `args.block` is always
        // the block the OLD state belongs to, so the extra identity comparison this guard
        // used to carry was a tautology that made the whole method dead: a broken repeater
        // or comparator left the block in front of it holding stale power.
        if args.moved {
            return;
        }
        RedstoneGateBlock::update_target(
            self,
            args.world,
            *args.position,
            args.old_state_id,
            args.block,
        );
    }

    fn is_target_not_aligned(
        &self,
        world: &dyn BlockAccessor,
        pos: BlockPos,
        state: &BlockState,
        block: &Block,
    ) -> bool {
        let props = T::from_state_id(state.id, block);
        let facing = props.get_facing().opposite();
        let (target_block, target_state) =
            world.get_block_and_state(&pos.offset(facing.to_offset()));
        if target_block == &Block::COMPARATOR {
            let props = ComparatorLikeProperties::from_state_id(target_state.id, target_block);
            props.facing != facing
        } else if target_block == &Block::REPEATER {
            let props = RepeaterLikeProperties::from_state_id(target_state.id, target_block);
            props.facing != facing
        } else {
            false
        }
    }

    fn get_update_delay_internal(&self, state_id: BlockStateId, block: &Block) -> u8;
}

pub fn get_power<T: BlockProperties + RedstoneGateBlockProperties + Send>(
    world: &World,
    pos: BlockPos,
    state_id: BlockStateId,
    block: &Block,
) -> u8 {
    let props = T::from_state_id(state_id, block);
    let facing = props.get_facing();
    let source_pos = pos.offset(facing.to_offset());
    let (source_block, source_state) = world.get_block_and_state(&source_pos);
    let source_level = get_redstone_power(
        source_block,
        source_state,
        world,
        &source_pos,
        facing.to_block_direction(),
    );
    if source_level >= 15 {
        source_level
    } else {
        source_level.max(if source_block == &Block::REDSTONE_WIRE {
            let props = RedstoneWireLikeProperties::from_state_id(source_state.id, source_block);
            props.power
        } else {
            0
        })
    }
}

pub fn get_power_on_side(
    world: &World,
    pos: &BlockPos,
    side: HorizontalFacing,
    only_gate: bool,
) -> u8 {
    let side_pos = pos.offset(side.to_block_direction().to_offset());
    let (side_block, side_state) = world.get_block_and_state(&side_pos);

    // `SignalGetter.getControlInputSignal`: a redstone block reads as 15 and wire as its power,
    // and everything else contributes its DIRECT signal, not its weak one. A redstone torch beside
    // a comparator emits weak 15 sideways but direct 0, so reading the weak signal here let it
    // feed the side input when vanilla ignores it.
    if only_gate {
        if !is_diode(side_block) {
            return 0;
        }
    } else if side_block == &Block::REDSTONE_BLOCK {
        return 15;
    } else if side_block == &Block::REDSTONE_WIRE {
        return RedstoneWireLikeProperties::from_state_id(side_state.id, side_block).power;
    }

    world.block_registry.get_strong_redstone_power(
        side_block,
        world,
        &side_pos,
        side_state,
        side.to_block_direction(),
    )
}
