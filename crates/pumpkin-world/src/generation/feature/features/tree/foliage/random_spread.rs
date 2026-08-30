use pumpkin_data::BlockState;
use pumpkin_util::{
    math::{int_provider::IntProvider, position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};

use super::FoliagePlacer;
use crate::generation::feature::features::tree::TreeNode;
use crate::generation::proto_chunk::GenerationCache;

pub struct RandomSpreadFoliagePlacer {
    pub foliage_height: IntProvider,
    pub leaf_placement_attempts: i32,
}

impl RandomSpreadFoliagePlacer {
    #[expect(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        node: &TreeNode,
        foliage_height: i32,
        radius: i32,
        _offset: i32,
        foliage_provider: &BlockState,
    ) -> Vec<BlockPos> {
        let mut foliage_positions = Vec::new();
        for _ in 0..self.leaf_placement_attempts {
            let pos = BlockPos(node.center.0.add(&Vector3::new(
                random.next_bounded_i32(radius) - random.next_bounded_i32(radius),
                random.next_bounded_i32(foliage_height) - random.next_bounded_i32(foliage_height),
                random.next_bounded_i32(radius) - random.next_bounded_i32(radius),
            )));
            if FoliagePlacer::place_foliage_block(chunk, pos, foliage_provider) {
                foliage_positions.push(pos);
            }
        }
        foliage_positions
    }
    // TODO: getRandomRadius
    pub fn get_random_height(&self, random: &mut RandomGenerator, _trunk_height: i32) -> i32 {
        self.foliage_height.get(random)
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_util::{
        math::int_provider::IntProvider,
        random::{RandomGenerator, legacy_rand::LegacyRand},
    };

    use super::RandomSpreadFoliagePlacer;

    #[test]
    fn foliage_height_uses_the_configured_provider() {
        // `RandomSpreadFoliagePlacer.foliageHeight` (`RandomSpreadFoliagePlacer.java:64-67`)
        // samples the placer field and does not use the tree height.
        let placer = RandomSpreadFoliagePlacer {
            foliage_height: IntProvider::Constant(7),
            leaf_placement_attempts: 0,
        };
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));

        assert_eq!(placer.get_random_height(&mut random, 32), 7);
    }
}
