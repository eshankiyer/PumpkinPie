use pumpkin_data::{
    Block, BlockDirection, BlockState,
    block_properties::{BlockProperties, CreakingHeartLikeProperties, CreakingHeartState},
    tag,
};
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::feature::java_set::vanilla_hash_set_order;
use crate::generation::proto_chunk::GenerationCache;

/// Vanilla `CreakingHeartDecorator` (`CreakingHeartDecorator.java:33-60`).
pub struct CreakingHeartTreeDecorator {
    pub probability: f32,
}

impl CreakingHeartTreeDecorator {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        log_positions: &[BlockPos],
    ) {
        if log_positions.is_empty() || random.next_f32() >= self.probability {
            return;
        }
        let mut logs = vanilla_hash_set_order(log_positions);
        logs.sort_by_key(|pos| pos.0.y);
        // Util.shuffle: Fisher-Yates from the back.
        for i in (1..logs.len()).rev() {
            let j = random.next_bounded_i32(i as i32 + 1) as usize;
            logs.swap(i, j);
        }
        let target = logs.into_iter().find(|pos| {
            BlockDirection::all().iter().all(|dir| {
                GenerationCache::get_block_state(chunk, &pos.offset(dir.to_offset()).0)
                    .to_block_id()
                    .has_tag(tag::Block::MINECRAFT_LOGS)
            })
        });
        let Some(target) = target else {
            return;
        };
        let mut props = CreakingHeartLikeProperties::default(&Block::CREAKING_HEART);
        props.creaking_heart_state = CreakingHeartState::Dormant;
        props.natural = true;
        chunk.set_block_state(
            &target.0,
            BlockState::from_id(props.to_state_id(&Block::CREAKING_HEART)),
        );
    }
}
