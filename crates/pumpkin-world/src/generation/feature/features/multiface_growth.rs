use pumpkin_data::block_properties::{BlockProperties, GlowLichenLikeProperties};
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState};
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::proto_chunk::GenerationCache;

/// `MultifaceBlock.canAttachTo`: vanilla checks `isFaceFull` against the neighbour's
/// block-support shape and collision shape. This codebase has no per-direction shape
/// query available to `pumpkin-world` (only the coarser `BlockState::is_side_solid`,
/// itself `isFaceSturdy()` in Java), so that's used here as a known simplification —
/// the same substitution `pumpkin/src/block/blocks/abstract_multiface.rs` documents
/// for the runtime (non-worldgen) multiface framework.
const fn can_attach_to(
    neighbour_state: &BlockState,
    direction_towards_neighbour: BlockDirection,
) -> bool {
    neighbour_state.is_side_solid(direction_towards_neighbour.opposite())
}

pub struct MultifaceGrowthConfiguration {
    pub place_block: BlockId,
    pub search_range: i32,
    pub can_place_on_floor: bool,
    pub can_place_on_ceiling: bool,
    pub can_place_on_wall: bool,
    pub chance_of_spreading: f32,
    pub can_be_placed_on: Vec<BlockId>,
    valid_directions: Vec<BlockDirection>,
}

impl MultifaceGrowthConfiguration {
    /// Mirrors `MultifaceGrowthConfiguration`'s constructor direction list
    /// (`net/minecraft/world/level/levelgen/feature/configurations/MultifaceGrowthConfiguration.java:46-64`).
    #[must_use]
    pub fn new(
        place_block: BlockId,
        search_range: i32,
        can_place_on_floor: bool,
        can_place_on_ceiling: bool,
        can_place_on_wall: bool,
        chance_of_spreading: f32,
        can_be_placed_on: Vec<BlockId>,
    ) -> Self {
        let mut valid_directions = Vec::with_capacity(6);
        if can_place_on_ceiling {
            valid_directions.push(BlockDirection::Up);
        }
        if can_place_on_floor {
            valid_directions.push(BlockDirection::Down);
        }
        if can_place_on_wall {
            valid_directions.extend([
                BlockDirection::North,
                BlockDirection::East,
                BlockDirection::South,
                BlockDirection::West,
            ]);
        }
        Self {
            place_block,
            search_range,
            can_place_on_floor,
            can_place_on_ceiling,
            can_place_on_wall,
            chance_of_spreading,
            can_be_placed_on,
            valid_directions,
        }
    }

    /// Ports `getShuffledDirectionsExcept` from
    /// `net/minecraft/world/level/levelgen/feature/configurations/MultifaceGrowthConfiguration.java:75-77`.
    #[must_use]
    pub fn get_shuffled_directions_except(
        &self,
        random: &mut RandomGenerator,
        exclude_direction: BlockDirection,
    ) -> Vec<BlockDirection> {
        let mut directions: Vec<_> = self
            .valid_directions
            .iter()
            .copied()
            .filter(|direction| *direction != exclude_direction)
            .collect();
        shuffle(&mut directions, random);
        directions
    }

    /// Ports `getShuffledDirections` from
    /// `net/minecraft/world/level/levelgen/feature/configurations/MultifaceGrowthConfiguration.java:79-81`.
    #[must_use]
    pub fn get_shuffled_directions(&self, random: &mut RandomGenerator) -> Vec<BlockDirection> {
        let mut directions = self.valid_directions.clone();
        shuffle(&mut directions, random);
        directions
    }
}

pub struct MultifaceGrowthFeature {
    pub configuration: MultifaceGrowthConfiguration,
}

impl MultifaceGrowthFeature {
    /// Ports `MultifaceGrowthFeature.place` from
    /// `net/minecraft/world/level/levelgen/feature/MultifaceGrowthFeature.java:20-55`.
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        _min_y: i8,
        _height: u16,
        _feature: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let config = &self.configuration;
        if !matches!(
            config.place_block,
            BlockId::GLOW_LICHEN | BlockId::SCULK_VEIN
        ) {
            return false;
        }
        let origin_state = GenerationCache::get_block_state(chunk, &pos.0);
        if !(chunk.is_air(&pos.0) || origin_state.to_block_id() == BlockId::WATER) {
            return false;
        }

        let directions = config.get_shuffled_directions(random);
        if place_growth_if_possible(chunk, pos, origin_state, config, &directions) {
            return true;
        }

        for search_direction in directions {
            let placement_directions =
                config.get_shuffled_directions_except(random, search_direction.opposite());
            for _ in 0..config.search_range {
                let search_pos = pos.offset(search_direction.to_offset());
                let search_state = GenerationCache::get_block_state(chunk, &search_pos.0);
                if !is_air_or_water(search_state)
                    && search_state.to_block_id() != config.place_block
                {
                    break;
                }
                if place_growth_if_possible(
                    chunk,
                    search_pos,
                    search_state,
                    config,
                    &placement_directions,
                ) {
                    return true;
                }
            }
        }
        false
    }
}

/// Ports `MultifaceGrowthFeature.placeGrowthIfPossible` from
/// `net/minecraft/world/level/levelgen/feature/MultifaceGrowthFeature.java:58-87`.
fn place_growth_if_possible<T: GenerationCache>(
    chunk: &mut T,
    pos: BlockPos,
    old_state: pumpkin_data::BlockStateId,
    config: &MultifaceGrowthConfiguration,
    placement_directions: &[BlockDirection],
) -> bool {
    for &placement_direction in placement_directions {
        let neighbour = pos.offset(placement_direction.to_offset());
        let neighbour_state = GenerationCache::get_block_state(chunk, &neighbour.0);
        if config
            .can_be_placed_on
            .contains(&neighbour_state.to_block_id())
        {
            let block = Block::from_id(config.place_block);
            let mut props = if old_state.to_block_id() == config.place_block {
                GlowLichenLikeProperties::from_state_id(old_state, block)
            } else {
                GlowLichenLikeProperties::default(block)
            };
            set_face(&mut props, placement_direction);
            chunk.set_block_state(&pos.0, BlockState::from_id(props.to_state_id(block)));
            return true;
        }
    }
    false
}

const fn set_face(props: &mut GlowLichenLikeProperties, direction: BlockDirection) {
    match direction {
        BlockDirection::Down => props.r#down = true,
        BlockDirection::Up => props.r#up = true,
        BlockDirection::North => props.r#north = true,
        BlockDirection::South => props.r#south = true,
        BlockDirection::West => props.r#west = true,
        BlockDirection::East => props.r#east = true,
    }
}

fn is_air_or_water(state: pumpkin_data::BlockStateId) -> bool {
    let block = state.to_block_id();
    block == BlockId::AIR || block == BlockId::WATER
}

fn shuffle<T>(values: &mut [T], random: &mut RandomGenerator) {
    let mut length = values.len();
    while length > 1 {
        let swap_to = random.next_bounded_i32(length as i32) as usize;
        values.swap(length - 1, swap_to);
        length -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::can_attach_to;
    use pumpkin_data::{Block, BlockDirection, BlockState};

    #[test]
    fn attaches_to_a_sturdy_face() {
        let stone = BlockState::from_id(Block::STONE.default_state.id);
        // Growth sits above the stone block, attaching to stone's up-facing side.
        assert!(can_attach_to(stone, BlockDirection::Down));
    }

    #[test]
    fn does_not_attach_to_air() {
        let air = BlockState::from_id(Block::AIR.default_state.id);
        assert!(!can_attach_to(air, BlockDirection::Down));
    }

    #[test]
    fn does_not_attach_to_a_non_full_block() {
        // Bamboo has no full/sturdy face on any side - growth must not be able to
        // attach to it, unlike the old "any non-air neighbour" check.
        let bamboo = BlockState::from_id(Block::BAMBOO.default_state.id);
        for direction in BlockDirection::all() {
            assert!(!can_attach_to(bamboo, direction));
        }
    }
}
