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

pub struct TrunkVineTreeDecorator;

impl TrunkVineTreeDecorator {
    pub fn generate<T: GenerationCache>(
        chunk: &mut T,
        random: &mut RandomGenerator,
        log_positions: &[BlockPos],
        decorations: &mut Vec<BlockPos>,
    ) {
        // Vanilla iterates `context.logs()`: the trunk HashSet, stable-sorted by Y.
        let mut logs = vanilla_hash_set_order(log_positions);
        logs.sort_by_key(|pos| pos.0.y);
        for pos in &logs {
            if random.next_bounded_i32(3) > 0 && chunk.is_air(&pos.west().0) {
                let mut vine = VineLikeProperties::default(&Block::VINE);
                vine.east = true;
                decorations.push(pos.west());
                chunk.set_block_state(
                    &pos.west().0,
                    BlockState::from_id(vine.to_state_id(&Block::VINE)),
                );
            }

            if random.next_bounded_i32(3) > 0 && chunk.is_air(&pos.east().0) {
                let mut vine = VineLikeProperties::default(&Block::VINE);
                vine.west = true;
                decorations.push(pos.east());
                chunk.set_block_state(
                    &pos.east().0,
                    BlockState::from_id(vine.to_state_id(&Block::VINE)),
                );
            }

            if random.next_bounded_i32(3) > 0 && chunk.is_air(&pos.north().0) {
                let mut vine = VineLikeProperties::default(&Block::VINE);
                vine.south = true;
                decorations.push(pos.north());
                chunk.set_block_state(
                    &pos.north().0,
                    BlockState::from_id(vine.to_state_id(&Block::VINE)),
                );
            }

            if random.next_bounded_i32(3) > 0 && chunk.is_air(&pos.south().0) {
                let mut vine = VineLikeProperties::default(&Block::VINE);
                vine.north = true;
                decorations.push(pos.south());
                chunk.set_block_state(
                    &pos.south().0,
                    BlockState::from_id(vine.to_state_id(&Block::VINE)),
                );
            }
        }
    }
}
