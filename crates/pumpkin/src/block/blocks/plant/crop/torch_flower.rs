use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, TorchflowerCropLikeProperties};
use pumpkin_data::{Block, item::Item, item_stack::ItemStack};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockAccessor;
use rand::RngExt;

use crate::block::blocks::plant::PlantBlockBase;
use crate::block::blocks::plant::crop::CropBlockBase;
use crate::block::blocks::plant::crop::ravager_destroy_crop;
use crate::block::{
    BlockBehaviour, BlockFuture, CanPlaceAtArgs, GetCloneItemStackArgs,
    GetStateForNeighborUpdateArgs, OnEntityCollisionArgs, RandomTickArgs,
};

type TorchFlowerProperties = TorchflowerCropLikeProperties;

#[pumpkin_block("minecraft:torchflower_crop")]
pub struct TorchFlowerBlock;

impl BlockBehaviour for TorchFlowerBlock {
    fn on_entity_collision<'a>(&'a self, args: OnEntityCollisionArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move { ravager_destroy_crop(args.world, args.position, args.entity).await })
    }

    /// `TorchflowerCropBlock.getBaseSeedId` (`TorchflowerCropBlock.java:56-58`) supplies
    /// torchflower seeds to the inherited `CropBlock.getCloneItemStack` (`CropBlock.java:169-170`).
    fn get_clone_item_stack(&self, _args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        Some(crate::block::blocks::plant::crop::clone_seed_stack(
            &Item::TORCHFLOWER_SEEDS,
        ))
    }

    fn is_valid_bonemeal_target(&self, args: crate::block::BonemealArgs<'_>) -> bool {
        <Self as CropBlockBase>::is_valid_bonemeal_target(self, args.world, args.position)
    }

    fn perform_bonemeal<'a>(&'a self, args: crate::block::BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            <Self as CropBlockBase>::perform_bonemeal(self, args.world, args.position).await;
        })
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        <Self as PlantBlockBase>::can_place_at(self, args.block_accessor, args.position)
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            <Self as PlantBlockBase>::get_state_for_neighbor_update(
                self,
                args.world,
                args.position,
                args.state_id,
            )
            .await
        })
    }

    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if rand::rng().random_range(0..2) != 0 {
                <Self as CropBlockBase>::random_tick(self, args.world, args.position).await;
            }
        })
    }
}

impl PlantBlockBase for TorchFlowerBlock {
    /// `CropBlock.canSurvive` (`CropBlock.java:145-147`).
    fn can_place_at(&self, block_accessor: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
        <Self as CropBlockBase>::crop_can_survive(self, block_accessor, block_pos)
    }
}

impl CropBlockBase for TorchFlowerBlock {
    fn bonemeal_age_increase(&self) -> i32 {
        1
    }

    fn max_age(&self) -> i32 {
        2
    }

    fn get_age(&self, state: BlockStateId, block: &Block) -> i32 {
        let props = TorchFlowerProperties::from_state_id(state, block);
        i32::from(props.age)
    }

    fn state_with_age(&self, block: &Block, state: BlockStateId, age: i32) -> BlockStateId {
        if age == 1 {
            let mut properties = TorchFlowerProperties::from_state_id(state, block);
            properties.age = 1;
            properties.to_state_id(block)
        } else {
            Block::TORCHFLOWER.default_state.id
        }
    }
}
