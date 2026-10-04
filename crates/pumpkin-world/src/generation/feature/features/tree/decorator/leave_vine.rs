use crate::generation::feature::java_set::vanilla_hash_set_order;
use crate::generation::proto_chunk::GenerationCache;
use pumpkin_data::{
    Block, BlockState,
    block_properties::{BlockProperties, VineLikeProperties},
};
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

pub struct LeavesVineTreeDecorator {
    pub probability: f32,
}

impl LeavesVineTreeDecorator {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        foliage_positions: &[BlockPos],
        decorations: &mut Vec<BlockPos>,
    ) {
        // Vanilla iterates `context.leaves()`: the foliage HashSet, stable-sorted by Y.
        let mut leaves = vanilla_hash_set_order(foliage_positions);
        leaves.sort_by_key(|pos| pos.0.y);
        for pos in &leaves {
            if random.next_f32() < self.probability {
                let target = pos.west();
                if chunk.is_air(&target.0) {
                    Self::place_vines(chunk, decorations, target, |vine| vine.east = true);
                }
            }
            if random.next_f32() < self.probability {
                let target = pos.east();
                if chunk.is_air(&target.0) {
                    Self::place_vines(chunk, decorations, target, |vine| vine.west = true);
                }
            }
            if random.next_f32() < self.probability {
                let target = pos.north();
                if chunk.is_air(&target.0) {
                    Self::place_vines(chunk, decorations, target, |vine| vine.south = true);
                }
            }
            if random.next_f32() < self.probability {
                let target = pos.south();
                if chunk.is_air(&target.0) {
                    Self::place_vines(chunk, decorations, target, |vine| vine.north = true);
                }
            }
        }
    }

    fn place_vines<T: GenerationCache>(
        chunk: &mut T,
        decorations: &mut Vec<BlockPos>,
        start: BlockPos,
        configure_face: impl Fn(&mut VineLikeProperties),
    ) {
        let mut vine = VineLikeProperties::default(&Block::VINE);
        configure_face(&mut vine);
        let state = BlockState::from_id(vine.to_state_id(&Block::VINE));
        decorations.push(start);
        chunk.set_block_state(&start.0, state);

        let mut current = start.down();
        for _ in 0..4 {
            if !chunk.is_air(&current.0) {
                break;
            }
            decorations.push(current);
            chunk.set_block_state(&current.0, state);
            current = current.down();
        }
    }
}
