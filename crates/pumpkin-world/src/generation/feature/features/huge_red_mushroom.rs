use pumpkin_data::{
    Block, BlockState,
    block_properties::{BlockProperties, BrownMushroomBlockLikeProperties, is_air},
    tag,
};
use pumpkin_util::{math::position::BlockPos, random::RandomGenerator};

use crate::generation::proto_chunk::GenerationCache;

pub struct HugeRedMushroomFeature;

const FOLIAGE_RADIUS: i32 = 2;

/// Vanilla `HugeRedMushroomFeature.getTreeRadiusForHeight`
/// (`HugeRedMushroomFeature.java:60-69`) for the configured foliage radius from
/// `TreeFeatures.java:365-369`.
const fn get_tree_radius_for_height(tree_height: i32, yo: i32) -> i32 {
    if (yo < tree_height && yo >= tree_height - 3) || yo == tree_height {
        FOLIAGE_RADIUS
    } else {
        0
    }
}

/// Vanilla `HugeRedMushroomFeature.makeCap` (`HugeRedMushroomFeature.java:17-57`) with the
/// generated six-sided mushroom-block state.
fn cap_state(dx: i32, dz: i32, center: i32, up: bool) -> &'static BlockState {
    let mut state = BrownMushroomBlockLikeProperties::default(&Block::RED_MUSHROOM_BLOCK);
    state.down = false;
    state.up = up;
    state.west = dx < -center;
    state.east = dx > center;
    state.north = dz < -center;
    state.south = dz > center;
    BlockState::from_id(state.to_state_id(&Block::RED_MUSHROOM_BLOCK))
}

impl HugeRedMushroomFeature {
    /// Vanilla `AbstractHugeMushroomFeature.isValidPosition`: the ground below
    /// the origin must match `huge_red_mushroom_can_place_on`, and every block
    /// the trunk/cap will occupy must currently be air or leaves, or vanilla
    /// aborts placement entirely rather than overwriting it (e.g. water).
    #[allow(clippy::unused_self)]
    fn is_valid_position<T: GenerationCache>(&self, chunk: &T, pos: BlockPos, height: i32) -> bool {
        let below = GenerationCache::get_block_state(chunk, &pos.down().0).to_block_id();
        if !below.has_tag(tag::Block::MINECRAFT_HUGE_RED_MUSHROOM_CAN_PLACE_ON) {
            return false;
        }

        let check = |dx: i32, dy: i32, dz: i32| -> bool {
            let check_pos = BlockPos::new(pos.0.x + dx, pos.0.y + dy, pos.0.z + dz);
            let state_id = GenerationCache::get_block_state(chunk, &check_pos.0);
            is_air(state_id) || state_id.to_block_id().has_tag(tag::Block::MINECRAFT_LEAVES)
        };

        // Trunk column: dy in 0..height, radius 0.
        for dy in 0..height {
            if !check(0, dy, 0) {
                return false;
            }
        }

        // Vanilla AbstractHugeMushroomFeature checks every radius returned by
        // getTreeRadiusForHeight for dy in 0..=treeHeight.
        for dy in 0..=height {
            let radius = get_tree_radius_for_height(height, dy);
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
        // Vanilla AbstractHugeMushroomFeature.getTreeHeight
        // (AbstractHugeMushroomFeature.java:41-47).
        let tree_height = super::huge_brown_mushroom::mushroom_tree_height(random);

        if !self.is_valid_position(chunk, pos, tree_height) {
            return false;
        }

        // Vanilla `AbstractHugeMushroomFeature.place`: cap first, then trunk.
        // Vanilla HugeRedMushroomFeature.makeCap uses dy = treeHeight - 3 ..= treeHeight.
        for dy in tree_height - 3..=tree_height {
            let radius = if dy < tree_height {
                FOLIAGE_RADIUS
            } else {
                FOLIAGE_RADIUS - 1
            };
            let center = FOLIAGE_RADIUS - 2;
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let x_edge = dx == -radius || dx == radius;
                    let z_edge = dz == -radius || dz == radius;
                    if dy >= tree_height || x_edge != z_edge {
                        let cap_pos = BlockPos::new(pos.0.x + dx, pos.0.y + dy, pos.0.z + dz);
                        chunk.set_block_state(
                            &cap_pos.0,
                            cap_state(dx, dz, center, dy >= tree_height - 1),
                        );
                    }
                }
            }
        }

        let stem_props = BrownMushroomBlockLikeProperties {
            up: false,
            down: false,
            north: true,
            east: true,
            south: true,
            west: true,
        };
        let stem_state = BlockState::from_id(stem_props.to_state_id(&Block::MUSHROOM_STEM));
        for i in 0..tree_height {
            let stem_pos = BlockPos::new(pos.0.x, pos.0.y + i, pos.0.z);
            chunk.set_block_state(&stem_pos.0, stem_state);
        }

        true
    }
}
