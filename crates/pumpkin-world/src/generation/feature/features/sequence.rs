use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::RandomGenerator;

use crate::generation::proto_chunk::GenerationCache;
use crate::{generation::feature::placed_features::PlacedFeatureWrapper, world::WorldPortalExt};

/// `minecraft:sequence`.
///
/// Vanilla `SequenceFeature.place` iterates the configured placed features in order and
/// short-circuits with `false` the moment one of them fails, otherwise returns `true`
/// (`SequenceFeature.java:15-21`).
pub struct SequenceFeature {
    pub features: Vec<PlacedFeatureWrapper>,
}

impl SequenceFeature {
    #[expect(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        min_y: i8,
        height: u16,
        feature_name: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        for wrapper in &self.features {
            // A `Holder<PlacedFeature>` that cannot be resolved is treated as a failed
            // placement, matching the short-circuit in `SequenceFeature.java:16-18`.
            let Some(feature) = wrapper.get() else {
                return false;
            };
            if !feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ) {
                return false;
            }
        }
        true
    }
}
