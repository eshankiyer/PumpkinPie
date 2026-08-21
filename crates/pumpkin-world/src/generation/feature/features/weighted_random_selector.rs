use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::proto_chunk::GenerationCache;
use crate::{generation::feature::placed_features::PlacedFeatureWrapper, world::WorldPortalExt};

pub struct WeightedFeatureEntry {
    pub feature: PlacedFeatureWrapper,
    pub weight: i32,
}

/// `minecraft:weighted_random_selector`.
///
/// Vanilla `WeightedRandomSelectorFeature.place` draws one entry with
/// `WeightedList.getRandom(random)` and returns `false` when the list is empty
/// (`WeightedRandomSelectorFeature.java:25-26`).
pub struct WeightedRandomFeature {
    pub features: Vec<WeightedFeatureEntry>,
}

/// Resolves a weighted draw to an index.
///
/// Mirrors `WeightedList.Compact.get` (`WeightedList.java:179-186`): the selection is
/// decremented by each entry's weight in list order and the first entry that drives it
/// below zero wins. `WeightedList.Flat` (used when the total weight is under 64) is an
/// expanded lookup table built by the same left-to-right walk, so both selectors agree.
#[must_use]
pub fn weighted_index(weights: &[i32], mut selection: i32) -> Option<usize> {
    for (index, weight) in weights.iter().enumerate() {
        selection -= *weight;
        if selection < 0 {
            return Some(index);
        }
    }
    None
}

impl WeightedRandomFeature {
    /// Sum of all entry weights, i.e. `WeightedRandom.getTotalWeight`
    /// (`WeightedList.java:27`).
    #[must_use]
    pub fn total_weight(&self) -> i32 {
        self.features.iter().map(|entry| entry.weight).sum()
    }

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
        let total = self.total_weight();
        // `WeightedList` reports itself empty when the total weight is zero, and
        // `getRandom` then yields `Optional.empty()` -> `orElse(false)`
        // (`WeightedList.java:28-31,76-81`).
        if total <= 0 {
            return false;
        }
        let selection = random.next_bounded_i32(total);
        let weights: Vec<i32> = self.features.iter().map(|entry| entry.weight).collect();
        let Some(index) = weighted_index(&weights, selection) else {
            return false;
        };
        self.features[index].feature.get().is_some_and(|feature| {
            feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::weighted_index;

    /// The four sulfur-spring entries weigh 200/90/20/5, so the cumulative boundaries
    /// fall at 200, 290, 310 and 315
    /// (`assets/datapacks/26_2/data/minecraft/worldgen/configured_feature/sulfur_spring.json`).
    #[test]
    fn weighted_index_matches_cumulative_boundaries() {
        let weights = [200, 90, 20, 5];
        assert_eq!(weighted_index(&weights, 0), Some(0));
        assert_eq!(weighted_index(&weights, 199), Some(0));
        assert_eq!(weighted_index(&weights, 200), Some(1));
        assert_eq!(weighted_index(&weights, 289), Some(1));
        assert_eq!(weighted_index(&weights, 290), Some(2));
        assert_eq!(weighted_index(&weights, 309), Some(2));
        assert_eq!(weighted_index(&weights, 310), Some(3));
        assert_eq!(weighted_index(&weights, 314), Some(3));
        // A selection at or past the total weight has no owner; vanilla throws here and
        // is unreachable because `nextInt(totalWeight)` is exclusive.
        assert_eq!(weighted_index(&weights, 315), None);
    }

    #[test]
    fn weighted_index_skips_zero_weight_entries() {
        let weights = [0, 3, 0, 1];
        assert_eq!(weighted_index(&weights, 0), Some(1));
        assert_eq!(weighted_index(&weights, 2), Some(1));
        assert_eq!(weighted_index(&weights, 3), Some(3));
    }

    #[test]
    fn weighted_index_on_empty_list_is_none() {
        assert_eq!(weighted_index(&[], 0), None);
    }
}
