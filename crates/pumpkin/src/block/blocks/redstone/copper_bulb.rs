use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::blocks::weathering_copper::{
    ChangeOverTimeBlock, WeatherState, WeatheringCopper, change_over_time, get_chance_modifier,
    get_first, get_next, get_previous, get_weather_state,
};
use crate::block::{
    BlockBehaviour, BlockMetadata, GetComparatorOutputArgs, OnNeighborUpdateArgs, OnPlaceArgs,
    PlacedArgs, RandomTickArgs,
};
use pumpkin_data::BlockId;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::BlockProperties;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_world::world::BlockFlags;

type CopperBulbLikeProperties = pumpkin_data::block_properties::CopperBulbLikeProperties;

pub struct CopperBulbBlock;

impl ChangeOverTimeBlock<WeatherState> for CopperBulbBlock {
    fn get_age(&self, block: &pumpkin_data::Block) -> Option<WeatherState> {
        get_weather_state(block)
    }

    fn get_chance_modifier(&self, age: WeatherState) -> f32 {
        get_chance_modifier(age)
    }

    fn get_next(&self, block: &pumpkin_data::Block) -> Option<&'static pumpkin_data::Block> {
        get_next(block)
    }

    fn get_previous(&self, block: &pumpkin_data::Block) -> Option<&'static pumpkin_data::Block> {
        get_previous(block)
    }

    fn get_first(&self, block: &pumpkin_data::Block) -> Option<&'static pumpkin_data::Block> {
        get_first(block)
    }
}

impl WeatheringCopper for CopperBulbBlock {}

impl BlockMetadata for CopperBulbBlock {
    fn ids() -> Box<[BlockId]> {
        [
            BlockId::COPPER_BULB,
            BlockId::EXPOSED_COPPER_BULB,
            BlockId::WEATHERED_COPPER_BULB,
            BlockId::OXIDIZED_COPPER_BULB,
            BlockId::WAXED_COPPER_BULB,
            BlockId::WAXED_EXPOSED_COPPER_BULB,
            BlockId::WAXED_WEATHERED_COPPER_BULB,
            BlockId::WAXED_OXIDIZED_COPPER_BULB,
        ]
        .into()
    }
}

impl BlockBehaviour for CopperBulbBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = CopperBulbLikeProperties::default(args.block);
        let is_receiving_power = block_receives_redstone_power(args.world, args.position);
        if is_receiving_power {
            props.lit = true;
            args.world.play_block_sound(
                Sound::BlockCopperBulbTurnOn,
                SoundCategory::Blocks,
                *args.position,
            );
            props.powered = true;
        }
        props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        // `CopperBulbBlock.onPlace` (CopperBulbBlock.java:148-152) runs `checkAndFlip` on every
        // arrival, not just a player placement, so a bulb pushed into a powered spot by a
        // piston or written by /setblock still flips. `on_place` below only covers the
        // player-placement path.
        if pumpkin_data::Block::from_state_id(args.old_state_id) == args.block {
            return;
        }
        Self::check_and_flip(args.world, args.block, args.position);
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        Self::check_and_flip(args.world, args.block, args.position);
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        let props = CopperBulbLikeProperties::from_state_id(args.state.id, args.block);
        Some(if props.lit { 15 } else { 0 })
    }

    fn random_tick(&self, args: RandomTickArgs<'_>) {
        change_over_time(args.world, args.position, args.block);
    }
}

impl CopperBulbBlock {
    /// `CopperBulbBlock.checkAndFlip` (CopperBulbBlock.java:163-174).
    fn check_and_flip(
        world: &std::sync::Arc<crate::world::World>,
        block: &pumpkin_data::Block,
        position: &pumpkin_util::math::position::BlockPos,
    ) {
        let state = world.get_block_state(position);
        let mut props = CopperBulbLikeProperties::from_state_id(state.id, block);
        let signal = block_receives_redstone_power(world, position);
        if props.powered == signal {
            return;
        }
        if !props.powered {
            props.lit = !props.lit;
            world.play_block_sound(
                if props.lit {
                    Sound::BlockCopperBulbTurnOn
                } else {
                    Sound::BlockCopperBulbTurnOff
                },
                SoundCategory::Blocks,
                *position,
            );
        }
        props.powered = signal;
        world.set_block_state(position, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
    }
}
