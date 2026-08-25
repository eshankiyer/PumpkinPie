use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, BonemealArgs, CanPlaceAtArgs,
    GetStateForNeighborUpdateArgs,
};
use crate::world::feature_placer::place_configured_feature;
use pumpkin_data::BlockStateId;
use pumpkin_data::configured_feature::ConfiguredFeature;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockId, tag};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::{RandomGenerator, xoroshiro128::Xoroshiro};
use pumpkin_world::world::BlockAccessor;
use rand::RngExt;
pub struct FungusBlock;

impl BlockMetadata for FungusBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::CRIMSON_FUNGUS, BlockId::WARPED_FUNGUS].into()
    }
}

/// Vanilla gives every fungus its own support tag, chosen from the fungus being placed
/// and not from the block it is standing on.
fn has_support(block_accessor: &dyn BlockAccessor, fungus: &Block, position: &BlockPos) -> bool {
    let ground = block_accessor.get_block(&position.down());
    if fungus == &Block::WARPED_FUNGUS {
        ground.has_tag(&tag::Block::MINECRAFT_SUPPORTS_WARPED_FUNGUS)
    } else {
        ground.has_tag(&tag::Block::MINECRAFT_SUPPORTS_CRIMSON_FUNGUS)
    }
}

impl BlockBehaviour for FungusBlock {
    /// `NetherFungusBlock.mayPlaceOn` (`NetherFungusBlock.java:61-64`) accepts only the
    /// fungus-specific support tag.
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        has_support(args.block_accessor, args.block, args.position)
    }

    /// `NetherFungusBlock.isValidBonemealTarget` (`NetherFungusBlock.java:70-74`) requires the
    /// configured nylium below and one block of build height above the fungus.
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        let required = if args.block == &Block::WARPED_FUNGUS {
            &Block::WARPED_NYLIUM
        } else {
            &Block::CRIMSON_NYLIUM
        };
        args.world.get_block(&args.position.down()) == required
            && args.world.is_in_build_limit(args.position.up())
    }

    /// `NetherFungusBlock.isBonemealSuccess` (`NetherFungusBlock.java:76-79`) succeeds 40% of
    /// the time.
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        rand::rng().random::<f32>() < 0.4
    }

    /// `NetherFungusBlock.performBonemeal` (`NetherFungusBlock.java:81-84`) places the configured
    /// planted crimson or warped huge-fungus feature at the fungus position.
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let feature = if args.block == &Block::WARPED_FUNGUS {
                ConfiguredFeature::WarpedFungusPlanted
            } else {
                ConfiguredFeature::CrimsonFungusPlanted
            };
            let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::rng().random()));
            place_configured_feature(args.world, feature, *args.position, &mut random).await;
        })
    }
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if has_support(args.world, args.block, args.position) {
                args.state_id
            } else {
                Block::AIR.default_state.id
            }
        })
    }
}
