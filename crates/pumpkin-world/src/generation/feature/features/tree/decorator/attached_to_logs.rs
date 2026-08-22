use pumpkin_data::BlockDirection;
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::feature::java_set::vanilla_hash_set_order;
use crate::generation::proto_chunk::GenerationCache;
use crate::{generation::block_state_provider::BlockStateProvider, world::WorldPortalExt};

pub struct AttachedToLogsTreeDecorator {
    pub probability: f32,
    pub block_provider: BlockStateProvider,
    pub directions: Vec<BlockDirection>,
}

impl AttachedToLogsTreeDecorator {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        log_positions: &[BlockPos],
    ) {
        let mut sorted = vanilla_hash_set_order(log_positions);
        sorted.sort_by_key(|pos| pos.0.y);
        let mut shuffled = sorted;
        for i in (1..shuffled.len()).rev() {
            let j = random.next_bounded_i32(i as i32 + 1) as usize;
            shuffled.swap(i, j);
        }
        for pos in shuffled {
            let direction =
                self.directions[random.next_bounded_i32(self.directions.len() as i32) as usize];
            let pos = pos.offset(direction.to_offset());
            if random.next_f32() > self.probability
                || !GenerationCache::get_block_state(chunk, &pos.0)
                    .to_state()
                    .is_air()
            {
                continue;
            }
            chunk.set_block_state(
                &pos.0,
                self.block_provider.get(random, pos, chunk, block_registry),
            );
        }
    }
}
