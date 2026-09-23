use pumpkin_data::{BlockStateId, item::Item, item_stack::ItemStack};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockAccessor;

use crate::block::blocks::plant::PlantBlockBase;
use crate::block::blocks::plant::crop::CropBlockBase;
use crate::block::blocks::plant::crop::ravager_destroy_crop;
use crate::block::{
    BlockBehaviour, CanPlaceAtArgs, GetCloneItemStackArgs, GetStateForNeighborUpdateArgs,
    OnEntityCollisionArgs, RandomTickArgs,
};

#[pumpkin_block("minecraft:carrots")]
pub struct CarrotBlock;

impl BlockBehaviour for CarrotBlock {
    fn on_entity_collision(&self, args: OnEntityCollisionArgs<'_>) {
        ravager_destroy_crop(args.world, args.position, args.entity)
    }

    /// `CarrotBlock.getBaseSeedId` (`CarrotBlock.java:27-29`) supplies carrots to the inherited
    /// `CropBlock.getCloneItemStack` (`CropBlock.java:169-170`).
    fn get_clone_item_stack(&self, _args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        Some(crate::block::blocks::plant::crop::clone_seed_stack(
            &Item::CARROT,
        ))
    }

    fn is_valid_bonemeal_target(&self, args: crate::block::BonemealArgs<'_>) -> bool {
        <Self as CropBlockBase>::is_valid_bonemeal_target(self, args.world, args.position)
    }

    fn perform_bonemeal(&self, args: crate::block::BonemealArgs<'_>) {
        <Self as CropBlockBase>::perform_bonemeal(self, args.world, args.position);
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        <Self as PlantBlockBase>::can_place_at(self, args.block_accessor, args.position)
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

    fn random_tick(&self, args: RandomTickArgs<'_>) {
        <Self as CropBlockBase>::random_tick(self, args.world, args.position);
    }
}

impl PlantBlockBase for CarrotBlock {
    /// `CropBlock.canSurvive` (`CropBlock.java:145-147`).
    fn can_place_at(&self, block_accessor: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
        <Self as CropBlockBase>::crop_can_survive(self, block_accessor, block_pos)
    }
}

impl CropBlockBase for CarrotBlock {}
