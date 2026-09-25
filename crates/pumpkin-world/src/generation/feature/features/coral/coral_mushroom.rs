use crate::{generation::proto_chunk::GenerationCache, world::WorldPortalExt};
use pumpkin_data::tag;
use pumpkin_util::{
    math::{position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};

use super::CoralFeature;

pub struct CoralMushroomFeature;

impl CoralMushroomFeature {
    #[allow(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        _min_y: i8,
        _height: u16,
        _feature: pumpkin_data::placed_feature::PlacedFeature, // This placed feature
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        // First lets get a random coral
        let block = CoralFeature::get_random_tag_entry(tag::Block::MINECRAFT_CORAL_BLOCKS, random);

        let i = random.next_bounded_i32(3) + 3;
        let j = random.next_bounded_i32(3) + 3;
        let k = random.next_bounded_i32(3) + 3;
        let l = random.next_bounded_i32(3) + 1;

        for m in 0..=j {
            for n in 0..=i {
                for o in 0..=k {
                    // Vanilla CoralMushroomFeature.placeFeature sets the mutable position
                    // from the untouched origin on every iteration (CoralMushroomFeature.java:22-28).
                    let candidate = pos.offset(Vector3::new(m, n, o)).down_height(l);

                    let condition_a = (m != 0 && m != j) || (n != 0 && n != i);
                    let condition_b = (o != 0 && o != k) || (n != 0 && n != i);
                    let condition_c = (m != 0 && m != j) || (o != 0 && o != k);
                    let condition_d = m == 0 || m == j || n == 0 || n == i || o == 0 || o == k;

                    // Vanilla CoralMushroomFeature.placeFeature draws nextFloat only after
                    // all four shape checks pass (CoralMushroomFeature.java:29-34).
                    if condition_a
                        && condition_b
                        && condition_c
                        && condition_d
                        && random.next_f32() >= 0.1f32
                        && CoralFeature::generate_coral_piece(
                            chunk,
                            block_registry,
                            random,
                            block,
                            candidate,
                        )
                    {}
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_position_is_origin_relative_not_self_fed() {
        // This test covers the origin-relative assignment in CoralMushroomFeature.java:22-28.
        let origin = BlockPos::new(100, 64, -200);
        let (m, n, o, l) = (2, 1, 3, 2);

        let candidate = origin.offset(Vector3::new(m, n, o)).down_height(l);

        assert_eq!(candidate.0.x, origin.0.x + m);
        assert_eq!(candidate.0.y, origin.0.y + n - l);
        assert_eq!(candidate.0.z, origin.0.z + o);
    }
}
