use std::sync::Arc;

use crate::block::blocks::copper_weathering;
use crate::block::{
    BlockBehaviour, EmitsRedstonePowerArgs, GetRedstonePowerArgs, OnPlaceArgs, OnScheduledTickArgs,
    OnStateReplacedArgs, PlacedArgs, RandomTickArgs,
};
use crate::world::World;
use pumpkin_data::block_properties::{BlockProperties, LightningRodLikeProperties};
use pumpkin_data::{BlockStateId, FacingExt};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

/// `LightningRodBlock` (`net/minecraft/world/level/block/LightningRodBlock.java:26`).
///
/// `Blocks.java:5303-5315` registers this as a `WeatheringCopperCollection`, so all eight ids
/// exist: weathering and waxed variants extend or use `LightningRodBlock`. `LightningBolt.java:70`
/// instance-checks the common behavior, and `PoiTypes.java:54-58` collects every variant into the
/// `lightning_rod` POI. `ServerLevel.java:552` uses the same `minecraft:lightning_rods` set.
#[pumpkin_block_from_tag("minecraft:lightning_rods")]
pub struct LightningRodBlock;

impl LightningRodBlock {
    pub fn trigger(world: &Arc<World>, pos: &BlockPos) {
        let (block, state_id) = world.get_block_and_state_id(pos);
        let mut props = LightningRodLikeProperties::from_state_id(state_id, block);
        if !props.powered {
            props.powered = true;
            world.set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
            Self::update_neighbors(world, pos, props);
            world.schedule_block_tick(block, *pos, 8, TickPriority::Normal);
        }
    }

    fn update_neighbors(world: &Arc<World>, pos: &BlockPos, props: LightningRodLikeProperties) {
        world.update_neighbors(pos, None);
        let attached_pos = pos.offset(props.facing.opposite().to_block_direction().to_offset());
        world.update_neighbors(&attached_pos, None);
    }

    fn update_attached_neighbor(
        world: &Arc<World>,
        pos: &BlockPos,
        props: LightningRodLikeProperties,
    ) {
        let attached_pos = pos.offset(props.facing.opposite().to_block_direction().to_offset());
        world.update_neighbors(&attached_pos, None);
    }
}

impl BlockBehaviour for LightningRodBlock {
    /// `LightningRodBlock#getStateForPlacement` (LightningRodBlock.java:45-49).
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = LightningRodLikeProperties::default(args.block);
        props.facing = args.direction.to_facing().opposite();
        props.waterlogged = args.replacing.water_source();
        props.to_state_id(args.block)
    }

    /// `LightningRodBlock.onPlace` (LightningRodBlock.java:97-103): restore the reset tick for
    /// a powered state that arrives without one (piston move, setblock, or chunk edit).
    fn placed(&self, args: PlacedArgs<'_>) {
        if pumpkin_data::Block::from_state_id(args.old_state_id) == args.block {
            return;
        }
        let props = LightningRodLikeProperties::from_state_id(args.state_id, args.block);
        if props.powered
            && !args
                .world
                .is_block_tick_scheduled(args.position, args.block)
        {
            args.world
                .schedule_block_tick(args.block, *args.position, 8, TickPriority::Normal);
        }
    }

    fn emits_redstone_power(&self, _args: EmitsRedstonePowerArgs<'_>) -> bool {
        true
    }
    fn get_weak_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8 {
        let props = LightningRodLikeProperties::from_state_id(args.state.id, args.block);
        if props.powered { 15 } else { 0 }
    }
    fn get_strong_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8 {
        let props = LightningRodLikeProperties::from_state_id(args.state.id, args.block);
        if props.powered && props.facing.to_block_direction() == args.direction {
            15
        } else {
            0
        }
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let state = args.world.get_block_state(args.position);
        let mut props = LightningRodLikeProperties::from_state_id(state.id, args.block);
        if props.powered {
            props.powered = false;
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_ALL,
            );
            Self::update_neighbors(args.world, args.position, props);
        }
    }

    /// `LightningRodBlock.affectNeighborsAfterRemoval` (`LightningRodBlock.java:112-116`).
    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>) {
        let props = LightningRodLikeProperties::from_state_id(args.old_state_id, args.block);
        if props.powered {
            Self::update_attached_neighbor(args.world, args.position, props);
        }
    }

    /// Only non-waxed rods oxidize; the collection contains the four oxidation stages and the
    /// tag registration supplies the common behavior to all variants (`Blocks.java:5306-5307`).
    fn random_tick(&self, args: RandomTickArgs<'_>) {
        let oxidation_stages = [
            &pumpkin_data::Block::LIGHTNING_ROD,
            &pumpkin_data::Block::EXPOSED_LIGHTNING_ROD,
            &pumpkin_data::Block::WEATHERED_LIGHTNING_ROD,
            &pumpkin_data::Block::OXIDIZED_LIGHTNING_ROD,
        ];
        let current_state_id = args.world.get_block_state_id(args.position);
        let current_props = LightningRodLikeProperties::from_state_id(current_state_id, args.block);
        copper_weathering::try_oxidize_copper(
            args.world,
            args.position,
            args.block,
            &oxidation_stages,
            |next_block| {
                let mut new_props = LightningRodLikeProperties::default(next_block);
                new_props.facing = current_props.facing;
                new_props.powered = current_props.powered;
                new_props.to_state_id(next_block)
            },
        );
    }
}
