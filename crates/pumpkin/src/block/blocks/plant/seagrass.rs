use pumpkin_data::{
    Block, BlockDirection, BlockState, BlockStateId,
    tag::{self, Taggable},
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockAccessor;

use crate::block::{
    BlockBehaviour, BlockFuture, BonemealArgs, CanPlaceAtArgs, GetStateForNeighborUpdateArgs,
    blocks::plant::PlantBlockBase,
};
use pumpkin_data::block_properties::{
    BlockProperties, DoubleBlockHalf, TallSeagrassLikeProperties,
};
use pumpkin_world::world::BlockFlags;
#[pumpkin_block("minecraft:seagrass")]
pub struct SeaGrassBlock;
impl BlockBehaviour for SeaGrassBlock {
    /// `SeagrassBlock.isValidBonemealTarget` (`SeagrassBlock.java:75-78`): `state.is(Blocks.WATER)`
    /// matches any water block state (source or flowing), not just the source state.
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        args.world.get_block(&args.position.up()) == &Block::WATER
    }

    /// `SeagrassBlock.isBonemealSuccess` (`SeagrassBlock.java:80-83`) always succeeds.
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    /// `SeagrassBlock.performBonemeal` (`SeagrassBlock.java:90-97`) replaces the short
    /// seagrass with a lower and upper tall-seagrass half.
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let lower = Block::TALL_SEAGRASS.default_state.id;
            let mut upper = TallSeagrassLikeProperties::default(&Block::TALL_SEAGRASS);
            upper.half = DoubleBlockHalf::Upper;
            args.world
                .set_block_state(args.position, lower, BlockFlags::NOTIFY_NEIGHBORS)
                .await;
            args.world
                .set_block_state(
                    &args.position.up(),
                    upper.to_state_id(&Block::TALL_SEAGRASS),
                    BlockFlags::NOTIFY_NEIGHBORS,
                )
                .await;
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
}

impl PlantBlockBase for SeaGrassBlock {
    fn can_plant_on_top(
        &self,
        block_accessor: &dyn pumpkin_world::world::BlockAccessor,
        pos: &pumpkin_util::math::position::BlockPos,
    ) -> bool {
        let (support_block, support_block_state) = block_accessor.get_block_and_state(pos);
        let replacing_block = block_accessor.get_block(&pos.up());
        if replacing_block != &Block::WATER && replacing_block != &Block::SEAGRASS {
            return false;
        }
        if supports_seagrass(support_block, support_block_state) {
            return true;
        }
        false
    }
    #[allow(clippy::unused_async_trait_impl)]
    async fn get_state_for_neighbor_update(
        &self,
        block_accessor: &dyn BlockAccessor,
        block_pos: &BlockPos,
        block_state: BlockStateId,
    ) -> BlockStateId {
        if !<Self as PlantBlockBase>::can_place_at(self, block_accessor, block_pos) {
            return Block::WATER.default_state.id;
        }
        block_state
    }
}
#[must_use]
pub fn supports_seagrass(support_block: &Block, support_block_state: &BlockState) -> bool {
    support_block_state.is_side_solid(BlockDirection::Up)
        && !support_block.has_tag(&tag::Block::MINECRAFT_CANNOT_SUPPORT_SEAGRASS)
}
