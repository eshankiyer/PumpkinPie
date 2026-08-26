use pumpkin_data::{BlockStateId, item::Item, item_stack::ItemStack};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockAccessor;

use crate::block::blocks::plant::PlantBlockBase;
use crate::block::blocks::plant::crop::CropBlockBase;
use crate::block::blocks::plant::crop::ravager_destroy_crop;
use crate::block::{
    BlockBehaviour, BlockFuture, CanPlaceAtArgs, GetCloneItemStackArgs,
    GetStateForNeighborUpdateArgs, OnEntityCollisionArgs, RandomTickArgs,
};

#[pumpkin_block("minecraft:wheat")]
pub struct WheatBlock;

impl BlockBehaviour for WheatBlock {
    fn on_entity_collision<'a>(&'a self, args: OnEntityCollisionArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move { ravager_destroy_crop(args.world, args.position, args.entity).await })
    }

    /// `CropBlock.getBaseSeedId` (`CropBlock.java:164-166`) supplies wheat seeds to
    /// `CropBlock.getCloneItemStack` (`CropBlock.java:169-170`).
    fn get_clone_item_stack(&self, _args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        Some(crate::block::blocks::plant::crop::clone_seed_stack(
            &Item::WHEAT_SEEDS,
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
            <Self as CropBlockBase>::random_tick(self, args.world, args.position).await;
        })
    }
}

impl PlantBlockBase for WheatBlock {
    /// `CropBlock.canSurvive` (`CropBlock.java:145-147`).
    fn can_place_at(&self, block_accessor: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
        <Self as CropBlockBase>::crop_can_survive(self, block_accessor, block_pos)
    }
}

impl CropBlockBase for WheatBlock {}
