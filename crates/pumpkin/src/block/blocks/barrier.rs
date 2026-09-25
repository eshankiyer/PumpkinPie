use crate::block::{BlockBehaviour, GetStateForNeighborUpdateArgs, OnPlaceArgs};
use pumpkin_data::block_properties::{
    BlockProperties, MangroveRootsLikeProperties as BarrierLikeProperties,
};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_world::tick::TickPriority;

#[pumpkin_block("minecraft:barrier")]
pub struct BarrierBlock;

/// `BarrierBlock.canPlaceLiquid` (`BarrierBlock.java:92-95`) allows water only when the user is a
/// creative player and the barrier is currently dry. Dispenser placement passes `false` here.
pub(crate) fn can_place_liquid(state_id: BlockStateId, user_is_creative: bool) -> bool {
    user_is_creative && !BarrierLikeProperties::from_state_id(state_id, &Block::BARRIER).waterlogged
}

/// `BarrierBlock.pickupBlock` (`BarrierBlock.java:87-90`) exposes its water only to creative
/// players. The caller reaches this helper only after confirming the state is waterlogged.
pub(crate) const fn can_pickup_liquid(user_is_creative: bool) -> bool {
    user_is_creative
}

impl BlockBehaviour for BarrierBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = BarrierLikeProperties::default(args.block);
        props.waterlogged = args.replacing.water_source();
        props.to_state_id(args.block)
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        let props = BarrierLikeProperties::from_state_id(args.state_id, args.block);
        if props.waterlogged {
            args.world.schedule_fluid_tick(
                &Fluid::WATER,
                *args.position,
                Fluid::WATER.flow_speed as u8,
                TickPriority::Normal,
            );
        }
        props.to_state_id(args.block)
    }
}
