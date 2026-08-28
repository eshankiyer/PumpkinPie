use pumpkin_data::configured_feature::ConfiguredFeature;
use pumpkin_data::{Block, BlockDirection, BlockId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::xoroshiro128::Xoroshiro;
use pumpkin_world::lighting::light_dampening_into;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use std::sync::Arc;

use crate::block::{BlockBehaviour, BlockFuture, BlockMetadata, BonemealArgs, RandomTickArgs};
use crate::world::World;
use crate::world::feature_placer::place_configured_feature;

/// Above this light dampening, nylium is considered covered and reverts to netherrack.
const MAX_LIGHT_LEVEL: u8 = 15;

/// `NyliumBlock`: nylium dies back to netherrack once the block above it blocks all light.
///
/// Bone meal grows nether vegetation on it by placing configured features
/// (`NyliumBlock.performBonemeal`, `NyliumBlock.java:56-71`).
pub struct NyliumBlock;

impl BlockMetadata for NyliumBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::CRIMSON_NYLIUM, BlockId::WARPED_NYLIUM].into()
    }
}

impl BlockBehaviour for NyliumBlock {
    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if can_be_nylium(args.block, args.world, args.position) {
                return;
            }
            args.world
                .set_block_state(
                    args.position,
                    Block::NETHERRACK.default_state.id,
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
        })
    }

    /// `NyliumBlock.isValidBonemealTarget` (`NyliumBlock.java:46-49`): air above, inside the
    /// build height. The loaded check is additional; an unloaded read reports air.
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        let above = args.position.up();
        args.world.is_in_build_limit(above)
            && args.world.is_loaded(&above)
            && args.world.get_block_state(&above).is_air()
    }

    /// `NyliumBlock.performBonemeal` (`NyliumBlock.java:56-71`).
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let above = args.position.up();
            let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::rng().random()));
            if args.block == &Block::CRIMSON_NYLIUM {
                place(
                    args.world,
                    ConfiguredFeature::CrimsonForestVegetationBonemeal,
                    above,
                    &mut random,
                )
                .await;
            } else if args.block == &Block::WARPED_NYLIUM {
                place(
                    args.world,
                    ConfiguredFeature::WarpedForestVegetationBonemeal,
                    above,
                    &mut random,
                )
                .await;
                place(
                    args.world,
                    ConfiguredFeature::NetherSproutsBonemeal,
                    above,
                    &mut random,
                )
                .await;
                if rand::rng().random_range(0..8) == 0 {
                    place(
                        args.world,
                        ConfiguredFeature::TwistingVinesBonemeal,
                        above,
                        &mut random,
                    )
                    .await;
                }
            }
        })
    }
}

/// `NyliumBlock.place` (`NyliumBlock.java:73-84`): the build-height guard each placement repeats.
async fn place(
    world: &Arc<World>,
    feature: ConfiguredFeature,
    pos: BlockPos,
    random: &mut RandomGenerator,
) {
    if world.is_in_build_limit(pos) {
        place_configured_feature(world, feature, pos, random).await;
    }
}

fn can_be_nylium(
    block: &Block,
    world: &crate::world::World,
    position: &pumpkin_util::math::position::BlockPos,
) -> bool {
    let above_state = world.get_block_state(&position.up());
    light_dampening_into(
        block.default_state,
        above_state,
        BlockDirection::Up,
        above_state.opacity,
    ) < MAX_LIGHT_LEVEL
}
