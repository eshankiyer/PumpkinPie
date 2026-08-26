use std::sync::Arc;

use crate::block::blocks::plant::PlantBlockBase;
use crate::block::blocks::plant::big_dripleaf::{can_grow_into, can_plant_dripleaf_on_top};
use crate::block::{
    BlockBehaviour, BlockFuture, BonemealArgs, BrokenArgs, CanPlaceAtArgs,
    GetStateForNeighborUpdateArgs,
};
use crate::world::World;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{
    BigDripleafLikeProperties, BlockProperties, LadderLikeProperties,
};
use pumpkin_data::{Block, BlockDirection};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::{BlockAccessor, BlockFlags};

#[pumpkin_block("minecraft:big_dripleaf_stem")]
pub struct BigDripleafStemBlock;

pub type BigDripleafStemLikeProperties = LadderLikeProperties;

impl BlockBehaviour for BigDripleafStemBlock {
    /// Vanilla `GrowingPlantBodyBlock.isValidBonemealTarget`
    /// (`BigDripleafStemBlock.java:104-108`): only the connected top leaf can
    /// grow, and its block above must be replaceable.
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        let Some((head_pos, _)) = crate::block::blocks::plant::connected_plant_head(
            args.world,
            args.position,
            &Block::BIG_DRIPLEAF,
            &Block::BIG_DRIPLEAF_STEM,
            BlockDirection::Up,
        ) else {
            return false;
        };
        can_grow_into(args.world, &head_pos.up())
    }

    /// Vanilla `BigDripleafStemBlock.isBonemealSuccess`
    /// (`BigDripleafStemBlock.java:110-113`).
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    /// Vanilla `BigDripleafStemBlock.performBonemeal`
    /// (`BigDripleafStemBlock.java:115-125`): convert the connected top leaf
    /// to a stem and place a new leaf above it, preserving facing and
    /// waterlogging.
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let Some((head_pos, head_state_id)) = crate::block::blocks::plant::connected_plant_head(
                args.world,
                args.position,
                &Block::BIG_DRIPLEAF,
                &Block::BIG_DRIPLEAF_STEM,
                BlockDirection::Up,
            ) else {
                return;
            };

            let stem_props = BigDripleafStemLikeProperties::from_state_id(
                args.state_id,
                &Block::BIG_DRIPLEAF_STEM,
            );
            let head_props =
                BigDripleafLikeProperties::from_state_id(head_state_id, &Block::BIG_DRIPLEAF);
            let mut new_stem_props =
                BigDripleafStemLikeProperties::default(&Block::BIG_DRIPLEAF_STEM);
            new_stem_props.facing = stem_props.facing;
            new_stem_props.waterlogged = head_props.waterlogged;

            args.world
                .set_block_state(
                    &head_pos,
                    new_stem_props.to_state_id(&Block::BIG_DRIPLEAF_STEM),
                    BlockFlags::NOTIFY_ALL,
                )
                .await;

            let new_leaf_pos = head_pos.up();
            let mut new_leaf_props = BigDripleafLikeProperties::default(&Block::BIG_DRIPLEAF);
            new_leaf_props.facing = stem_props.facing;
            new_leaf_props.waterlogged = args.world.get_block(&new_leaf_pos) == &Block::WATER;
            args.world
                .set_block_state(
                    &new_leaf_pos,
                    new_leaf_props.to_state_id(&Block::BIG_DRIPLEAF),
                    BlockFlags::NOTIFY_ALL,
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
    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move { handle_big_dripleaf_breaking(args.world, args.position).await })
    }
}
impl PlantBlockBase for BigDripleafStemBlock {
    fn can_plant_on_top(&self, block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        let support_block = block_accessor.get_block(pos);
        can_plant_dripleaf_on_top(support_block)
    }

    #[allow(clippy::unused_async_trait_impl)]
    async fn get_state_for_neighbor_update(
        &self,
        block_accessor: &dyn BlockAccessor,
        block_pos: &BlockPos,
        block_state: BlockStateId,
    ) -> BlockStateId {
        if !<Self as PlantBlockBase>::can_place_at(self, block_accessor, block_pos) {
            let block = block_accessor.get_block(block_pos);

            let dripleaf_stem_props =
                BigDripleafStemLikeProperties::from_state_id(block_state, block);
            if dripleaf_stem_props.waterlogged {
                return Block::WATER.default_state.id;
            }
            return Block::AIR.default_state.id;
        }
        block_state
    }
}
pub async fn handle_big_dripleaf_breaking(world: &Arc<World>, position: &BlockPos) {
    let support_pos = position.down();
    let (support_block, support_state_id) = world.get_block_and_state_id(&support_pos);
    if support_block == &Block::BIG_DRIPLEAF_STEM {
        let dripleaf_stem_props =
            BigDripleafStemLikeProperties::from_state_id(support_state_id, support_block);

        let mut dripleaf_props = BigDripleafLikeProperties::default(&Block::BIG_DRIPLEAF);
        dripleaf_props.facing = dripleaf_stem_props.facing;
        dripleaf_props.waterlogged = dripleaf_stem_props.waterlogged;
        world
            .set_block_state(
                &support_pos,
                dripleaf_props.to_state_id(&Block::BIG_DRIPLEAF),
                BlockFlags::empty(),
            )
            .await;
    }
}
