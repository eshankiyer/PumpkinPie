use pumpkin_data::{
    Block, BlockDirection, BlockState,
    block_properties::{BlockProperties, CocoaLikeProperties},
};
use pumpkin_util::{
    math::{position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::feature::java_set::vanilla_hash_set_order;
use crate::generation::proto_chunk::GenerationCache;

/// Vanilla `CocoaDecorator` (`CocoaDecorator.java:26-51`).
pub struct CocoaTreeDecorator {
    pub probability: f32,
}

impl CocoaTreeDecorator {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        log_positions: &[BlockPos],
    ) {
        // The probability roll happens before the empty check, as in vanilla.
        if random.next_f32() >= self.probability || log_positions.is_empty() {
            return;
        }
        let mut logs = vanilla_hash_set_order(log_positions);
        logs.sort_by_key(|pos| pos.0.y);
        let tree_y = logs[0].0.y;
        for pos in logs.iter().filter(|pos| pos.0.y - tree_y <= 2) {
            for direction in [
                BlockDirection::North,
                BlockDirection::East,
                BlockDirection::South,
                BlockDirection::West,
            ] {
                if random.next_f32() > 0.25 {
                    continue;
                }
                let step = direction.opposite().to_offset();
                let cocoa_pos = pos.offset(Vector3::new(step.x, 0, step.z));
                if !chunk.is_air(&cocoa_pos.0) {
                    continue;
                }
                let mut props = CocoaLikeProperties::default(&Block::COCOA);
                props.age = random.next_bounded_i32(3) as u8;
                props.facing = direction.to_cardinal_direction();
                chunk.set_block_state(
                    &cocoa_pos.0,
                    BlockState::from_id(props.to_state_id(&Block::COCOA)),
                );
            }
        }
    }
}
