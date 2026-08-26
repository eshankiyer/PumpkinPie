use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::BlockProperties;
use pumpkin_macros::pumpkin_block;

use crate::block::{BlockBehaviour, BlockFuture, GetComparatorOutputArgs, OnPlaceArgs};
use crate::entity::EntityBase;

type EndPortalFrameProperties = pumpkin_data::block_properties::EndPortalFrameLikeProperties;

#[pumpkin_block("minecraft:end_portal_frame")]
pub struct EndPortalFrameBlock;

impl BlockBehaviour for EndPortalFrameBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut end_portal_frame_props = EndPortalFrameProperties::default(args.block);
            end_portal_frame_props.facing =
                args.player.get_entity().get_horizontal_facing().opposite();

            end_portal_frame_props.to_state_id(args.block)
        })
    }

    /// `EndPortalFrameBlock.hasAnalogOutputSignal` and `getAnalogOutputSignal`
    /// (EndPortalFrameBlock.java:59-67): an eye-filled frame emits comparator level 15.
    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            let props = EndPortalFrameProperties::from_state_id(args.state.id, args.block);
            Some(if props.eye { 15 } else { 0 })
        })
    }
}
