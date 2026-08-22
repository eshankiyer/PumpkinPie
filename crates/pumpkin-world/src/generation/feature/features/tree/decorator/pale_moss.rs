use pumpkin_data::{
    Block, BlockState,
    block_properties::{BlockProperties, PaleHangingMossLikeProperties},
};
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::feature::configured_features::CONFIGURED_FEATURES;
use crate::generation::feature::java_set::vanilla_hash_set_order;
use crate::generation::proto_chunk::GenerationCache;
use crate::world::WorldPortalExt;

/// Vanilla `PaleMossDecorator` (`PaleMossDecorator.java:44-86`).
#[expect(clippy::struct_field_names)]
pub struct PaleMossTreeDecorator {
    pub leaves_probability: f32,
    pub trunk_probability: f32,
    pub ground_probability: f32,
}

impl PaleMossTreeDecorator {
    #[expect(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        min_y: i8,
        height: u16,
        feature_name: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        log_positions: &[BlockPos],
        foliage_positions: &[BlockPos],
    ) {
        if log_positions.is_empty() {
            return;
        }
        let mut shuffled = vanilla_hash_set_order(log_positions);
        shuffled.sort_by_key(|pos| pos.0.y);
        for i in (1..shuffled.len()).rev() {
            let j = random.next_bounded_i32(i as i32 + 1) as usize;
            shuffled.swap(i, j);
        }
        // Collections.min by Y: the first minimum in iteration order of the shuffled copy.
        let origin = shuffled
            .iter()
            .copied()
            .reduce(|a, b| if b.0.y < a.0.y { b } else { a })
            .unwrap_or(shuffled[0]);

        if random.next_f32() < self.ground_probability
            && let Some(patch) = CONFIGURED_FEATURES
                .get(&pumpkin_data::configured_feature::ConfiguredFeature::PaleMossPatch)
        {
            patch.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                origin.up(),
            );
        }

        let logs = vanilla_hash_set_order(log_positions);
        let mut logs = logs;
        logs.sort_by_key(|pos| pos.0.y);
        for pos in logs {
            if random.next_f32() < self.trunk_probability {
                let down = pos.down();
                if chunk.is_air(&down.0) {
                    Self::add_moss_hanger(chunk, random, down);
                }
            }
        }
        let mut leaves = vanilla_hash_set_order(foliage_positions);
        leaves.sort_by_key(|pos| pos.0.y);
        for pos in leaves {
            if random.next_f32() < self.leaves_probability {
                let down = pos.down();
                if chunk.is_air(&down.0) {
                    Self::add_moss_hanger(chunk, random, down);
                }
            }
        }
    }

    fn add_moss_hanger<T: GenerationCache>(
        chunk: &mut T,
        random: &mut RandomGenerator,
        start: BlockPos,
    ) {
        let mut pos = start;
        while chunk.is_air(&pos.down().0) && random.next_f32() >= 0.5 {
            let mut props = PaleHangingMossLikeProperties::default(&Block::PALE_HANGING_MOSS);
            props.tip = false;
            chunk.set_block_state(
                &pos.0,
                BlockState::from_id(props.to_state_id(&Block::PALE_HANGING_MOSS)),
            );
            pos = pos.down();
        }
        let mut props = PaleHangingMossLikeProperties::default(&Block::PALE_HANGING_MOSS);
        props.tip = true;
        chunk.set_block_state(
            &pos.0,
            BlockState::from_id(props.to_state_id(&Block::PALE_HANGING_MOSS)),
        );
    }
}
