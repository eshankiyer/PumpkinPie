use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, PinkPetalsLikeProperties};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockId, tag};
use pumpkin_world::world::BlockFlags;

use crate::block::blocks::plant::PlantBlockBase;

use crate::block::{
    BlockBehaviour, BlockMetadata, BonemealArgs, CanPlaceAtArgs, CanUpdateAtArgs,
    GetStateForNeighborUpdateArgs, OnPlaceArgs,
};

use super::segmented::Segmented;

type FlowerbedProperties = pumpkin_data::block_properties::PinkPetalsLikeProperties;

pub struct FlowerbedBlock;

impl BlockMetadata for FlowerbedBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::PINK_PETALS, BlockId::WILDFLOWERS].into()
    }
}

impl BlockBehaviour for FlowerbedBlock {
    /// `FlowerBedBlock.isValidBonemealTarget` (`FlowerBedBlock.java:84-87`) always succeeds.
    fn is_valid_bonemeal_target(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    /// `FlowerBedBlock.isBonemealSuccess` (`FlowerBedBlock.java:89-92`) always succeeds.
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    /// `FlowerBedBlock.performBonemeal` (`FlowerBedBlock.java:94-102`) adds one segment up to
    /// four, then drops one flower-bed item when the block is already full.
    fn perform_bonemeal(&self, args: BonemealArgs<'_>) {
        let mut props = PinkPetalsLikeProperties::from_state_id(args.state_id, args.block);
        if props.flower_amount < 4 {
            props.flower_amount += 1;
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_LISTENERS,
            );
        } else if let Some(item) = Item::from_id(args.block.item_id) {
            args.world
                .drop_stack(args.position, ItemStack::new(1, item));
        }
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        let block_below = args.block_accessor.get_block(&args.position.down());
        block_below.has_tag(&tag::Block::MINECRAFT_DIRT) || block_below == &Block::FARMLAND
    }

    fn can_update_at(&self, args: CanUpdateAtArgs<'_>) -> bool {
        Segmented::can_update_at(self, args)
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        Segmented::on_place(self, args)
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        <Self as PlantBlockBase>::get_state_for_neighbor_update(
            self,
            args.world,
            args.position,
            args.state_id,
        )
    }
}

impl PlantBlockBase for FlowerbedBlock {}

impl Segmented for FlowerbedBlock {
    type Properties = FlowerbedProperties;
}
