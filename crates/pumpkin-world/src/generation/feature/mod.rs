pub mod configured_features;
// So first we go trough all the placed features and check if we should place a feature
// somewhere using `placed_features`. Then if we want to place a feature we place it
// using the `configured_features`, there is the logic for how we are going to place the
// feature.
pub mod placed_features;

mod features;
pub mod java_set;
mod size;

/// Proof that [`GenerationCache`] is implementable without a [`ProtoChunk`].
///
/// The runtime-placement blocker recorded for this repo was that `Feature::generate` needs a
/// cache whose contract includes `get_center_chunk_mut() -> &mut ProtoChunk`, which a live
/// `World` cannot supply. That method now lives on `ProtoChunkCache`, and no feature calls it -
/// vanilla features take a `WorldGenLevel`, which `ServerLevel` implements too. `FlatWorld` is
/// exactly the shape a live-world adapter would take: a flat block store, no chunks.
#[cfg(test)]
mod runtime_placement {
    use pumpkin_data::Block;
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_util::math::vector3::Vector3;
    use pumpkin_util::random::{RandomGenerator, xoroshiro128::Xoroshiro};

    use super::features::void_start_platform::VoidStartPlatformFeature;
    use crate::generation::proto_chunk::GenerationCache;
    use crate::generation::proto_chunk::test_cache::FlatWorld;

    #[test]
    fn a_feature_places_through_a_chunkless_cache() {
        let mut world = FlatWorld::default();
        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(1));
        let origin = BlockPos::new(100, 70, -33);

        let placed = VoidStartPlatformFeature.generate(
            &mut world,
            -64,
            384,
            pumpkin_data::placed_feature::PlacedFeature::Acacia,
            &mut random,
            origin,
        );

        assert!(placed);
        assert_eq!(world.blocks.len(), 9);
        for dx in -1..=1 {
            for dz in -1..=1 {
                let pos = Vector3::new(origin.0.x + dx, origin.0.y, origin.0.z + dz);
                assert_eq!(
                    GenerationCache::get_block_state(&world, &pos),
                    Block::OBSIDIAN.default_state.id,
                    "expected obsidian at {pos:?}"
                );
            }
        }
    }
}
