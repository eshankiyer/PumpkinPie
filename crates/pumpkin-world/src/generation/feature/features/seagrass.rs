use crate::generation::proto_chunk::GenerationCache;
use pumpkin_data::{
    Block, BlockDirection, BlockState,
    block_properties::{BlockProperties, DoubleBlockHalf, TallSeagrassLikeProperties},
    fluid::Fluid,
    tag::{Block as BlockTag, Taggable},
};
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

pub struct SeagrassFeature {
    pub probability: f32,
}

impl SeagrassFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        _min_y: i8,
        _height: u16,
        _feature: pumpkin_data::placed_feature::PlacedFeature, // This placed feature
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let x = random.next_bounded_i32(8) - random.next_bounded_i32(8);
        let z = random.next_bounded_i32(8) - random.next_bounded_i32(8);
        let y = chunk.ocean_floor_height_exclusive(pos.0.x + x, pos.0.z + z);
        let top_pos = BlockPos::new(pos.0.x + x, y, pos.0.z + z);
        let top_state = GenerationCache::get_block_state(chunk, &top_pos.0).to_state();
        if top_state.id.to_block_id() == Block::WATER {
            let tall = random.next_f64() < self.probability as f64;
            if can_survive_on(chunk, &top_pos) && (!tall || is_full_water_state(top_state.id)) {
                if tall {
                    let tall_pos = top_pos.up();
                    if GenerationCache::get_block_state(chunk, &tall_pos.0).to_block_id()
                        == Block::WATER
                    {
                        let mut props = TallSeagrassLikeProperties::default(&Block::TALL_SEAGRASS);
                        props.half = DoubleBlockHalf::Upper;
                        chunk.set_block_state(&top_pos.0, Block::TALL_SEAGRASS.default_state);
                        chunk.set_block_state(
                            &tall_pos.0,
                            BlockState::from_id(props.to_state_id(&Block::TALL_SEAGRASS)),
                        );
                    }
                } else {
                    chunk.set_block_state(&top_pos.0, Block::SEAGRASS.default_state);
                }
                // Vanilla sets placedAny after the tall branch, even when its upper-water
                // check did not place the second block. Preserve that return value exactly.
                return true;
            }
        }
        false
    }
}

fn can_survive_on<T: GenerationCache>(chunk: &T, pos: &BlockPos) -> bool {
    let support_state = GenerationCache::get_block_state(chunk, &pos.down().0).to_state();
    can_survive_on_state(support_state)
}

#[cfg(test)]
mod tests {
    use super::{can_survive_on_state, is_full_water_state};
    use pumpkin_data::{Block, BlockStateId};

    #[test]
    fn seagrass_support_matches_vanilla_tag_and_shape_rules() {
        assert!(can_survive_on_state(&Block::SAND.default_state));
        assert!(!can_survive_on_state(&Block::MAGMA_BLOCK.default_state));
        assert!(!can_survive_on_state(&Block::WATER.default_state));
    }

    #[test]
    fn seagrass_requires_full_water_for_tall_variant() {
        assert!(is_full_water_state(Block::WATER.default_state.id));
        assert!(is_full_water_state(BlockStateId::new(94).unwrap()));
        assert!(!is_full_water_state(BlockStateId::new(87).unwrap()));
    }
}

fn is_full_water_state(state_id: pumpkin_data::BlockStateId) -> bool {
    Fluid::from_state_id(state_id).is_some_and(|fluid| {
        fluid.name == "water"
            || fluid
                .properties(state_id)
                .to_props()
                .into_iter()
                .any(|(name, value)| name == "level" && value == "8")
    })
}

fn can_survive_on_state(state: &'static BlockState) -> bool {
    let support_block = Block::from_state_id(state.id);
    state.is_side_solid(BlockDirection::Up)
        && !support_block.has_tag(&BlockTag::MINECRAFT_CANNOT_SUPPORT_SEAGRASS)
}
