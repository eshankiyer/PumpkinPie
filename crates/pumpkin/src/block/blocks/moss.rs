use pumpkin_data::configured_feature::ConfiguredFeature;
use pumpkin_data::{Block, BlockId};
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::xoroshiro128::Xoroshiro;
use rand::RngExt;

use crate::block::{BlockBehaviour, BlockFuture, BlockMetadata, BonemealArgs};
use crate::world::feature_placer::place_configured_feature;

/// Moss and pale moss: bone meal places a configured feature, not blocks.
///
/// Vanilla registers them as
/// `BonemealableFeaturePlacerBlock(CaveFeatures.MOSS_PATCH_BONEMEAL)` (`Blocks.java:5429-5432`)
/// and `BonemealableFeaturePlacerBlock(VegetationFeatures.PALE_MOSS_PATCH_BONEMEAL)`
/// (`Blocks.java:5638-5641`).
pub struct MossBlock;

impl BlockMetadata for MossBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::MOSS_BLOCK, BlockId::PALE_MOSS_BLOCK].into()
    }
}

impl BlockBehaviour for MossBlock {
    /// `BonemealableFeaturePlacerBlock.isValidBonemealTarget`
    /// (`BonemealableFeaturePlacerBlock.java:33-36`): the block above must be air.
    ///
    /// The loaded check is additional: an unloaded read reports air here, which would let bone
    /// meal be consumed for a placement that can never commit.
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        let above = args.position.up();
        args.world.is_loaded(&above) && args.world.get_block_state(&above).is_air()
    }

    /// `BonemealableFeaturePlacerBlock.performBonemeal`
    /// (`BonemealableFeaturePlacerBlock.java:43-49`): place the configured feature at `pos.above()`.
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let feature = if args.block == &Block::PALE_MOSS_BLOCK {
                ConfiguredFeature::PaleMossPatchBonemeal
            } else {
                ConfiguredFeature::MossPatchBonemeal
            };
            let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::rng().random()));
            place_configured_feature(args.world, feature, args.position.up(), &mut random).await;
        })
    }
}
