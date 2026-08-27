use pumpkin_data::Block;
use pumpkin_data::block_properties::{BlockProperties, BrownMushroomBlockLikeProperties, is_air};
use pumpkin_data::tag;
use pumpkin_util::{math::position::BlockPos, random::RandomGenerator, random::RandomImpl};

use crate::generation::proto_chunk::GenerationCache;

pub struct HugeBrownMushroomFeature;

const FOLIAGE_RADIUS: i32 = 3;

/// Vanilla `HugeBrownMushroomFeature.getTreeRadiusForHeight`
/// (`HugeBrownMushroomFeature.java:58-61`) for the configured foliage radius from
/// `TreeFeatures.java:350-359`.
const fn get_tree_radius_for_height(yo: i32) -> i32 {
    if yo <= 3 { 0 } else { FOLIAGE_RADIUS }
}

impl HugeBrownMushroomFeature {
    /// Vanilla `AbstractHugeMushroomFeature.isValidPosition`: the ground below
    /// the origin must match `huge_brown_mushroom_can_place_on`, and every
    /// block the trunk/cap will occupy must currently be air or leaves, or
    /// vanilla aborts placement entirely rather than overwriting it (e.g. water).
    #[allow(clippy::unused_self)]
    fn is_valid_position<T: GenerationCache>(&self, chunk: &T, pos: BlockPos, height: i32) -> bool {
        let below = GenerationCache::get_block_state(chunk, &pos.down().0).to_block_id();
        if !below.has_tag(tag::Block::MINECRAFT_HUGE_BROWN_MUSHROOM_CAN_PLACE_ON) {
            return false;
        }

        let check = |dx: i32, dy: i32, dz: i32| -> bool {
            let check_pos = BlockPos::new(pos.0.x + dx, pos.0.y + dy, pos.0.z + dz);
            let state_id = GenerationCache::get_block_state(chunk, &check_pos.0);
            is_air(state_id) || state_id.to_block_id().has_tag(tag::Block::MINECRAFT_LEAVES)
        };

        // Vanilla AbstractHugeMushroomFeature checks every radius returned by
        // getTreeRadiusForHeight for dy in 0..=treeHeight.
        for dy in 0..=height {
            let radius = get_tree_radius_for_height(dy);
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if !check(dx, dy, dz) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Vanilla `HugeBrownMushroomFeature.makeCap` (`HugeBrownMushroomFeature.java:17-56`)
    /// with the configured radius and brown cap provider from `TreeFeatures.java:350-359`.
    fn make_cap<T: GenerationCache>(chunk: &mut T, pos: BlockPos, height: i32) {
        let radius = FOLIAGE_RADIUS;
        let cap_y = pos.0.y + height;
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let min_x = dx == -radius;
                let max_x = dx == radius;
                let min_z = dz == -radius;
                let max_z = dz == radius;
                let x_edge = min_x || max_x;
                let z_edge = min_z || max_z;
                if !x_edge || !z_edge {
                    let west = min_x || (z_edge && dx == 1 - radius);
                    let east = max_x || (z_edge && dx == radius - 1);
                    let north = min_z || (x_edge && dz == 1 - radius);
                    let south = max_z || (x_edge && dz == radius - 1);
                    let mut state =
                        BrownMushroomBlockLikeProperties::default(&Block::BROWN_MUSHROOM_BLOCK);
                    state.up = true;
                    state.down = false;
                    state.west = west;
                    state.east = east;
                    state.north = north;
                    state.south = south;
                    let cap_pos = BlockPos::new(pos.0.x + dx, cap_y, pos.0.z + dz);
                    chunk.set_block_state(
                        &cap_pos.0,
                        pumpkin_data::BlockState::from_id(
                            state.to_state_id(&Block::BROWN_MUSHROOM_BLOCK),
                        ),
                    );
                }
            }
        }
    }

    #[allow(clippy::unused_self)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        _min_y: i8,
        _height: u16,
        _feature: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let height = random.next_bounded_i32(3) + 4;

        if !self.is_valid_position(chunk, pos, height) {
            return false;
        }

        for i in 0..height {
            let stem_pos = BlockPos::new(pos.0.x, pos.0.y + i, pos.0.z);
            chunk.set_block_state(&stem_pos.0, Block::MUSHROOM_STEM.default_state);
        }

        Self::make_cap(chunk, pos, height);
        true
    }
}
