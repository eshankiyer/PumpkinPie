use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use super::TreeDecorator;
use crate::generation::proto_chunk::GenerationCache;
use crate::{generation::block_state_provider::BlockStateProvider, world::WorldPortalExt};

/// Vanilla `AlterGroundDecorator` (`AlterGroundDecorator.java:23-66`).
pub struct AlterGroundTreeDecorator {
    pub provider: BlockStateProvider,
}

impl AlterGroundTreeDecorator {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        root_positions: &[BlockPos],
        log_positions: &[BlockPos],
    ) {
        let positions = TreeDecorator::get_leaf_litter_positions(root_positions, log_positions);
        let Some(first) = positions.first() else {
            return;
        };
        let min_y = first.0.y;
        let lowest: Vec<BlockPos> = positions
            .iter()
            .copied()
            .filter(|pos| pos.0.y == min_y)
            .collect();
        for pos in lowest {
            self.place_circle(chunk, block_registry, random, pos.west().north());
            self.place_circle(chunk, block_registry, random, pos.east().east().north());
            self.place_circle(chunk, block_registry, random, pos.west().south().south());
            self.place_circle(
                chunk,
                block_registry,
                random,
                pos.east().east().south().south(),
            );
            for _ in 0..5 {
                let placement = random.next_bounded_i32(64);
                let xx = placement % 8;
                let zz = placement / 8;
                if xx == 0 || xx == 7 || zz == 0 || zz == 7 {
                    let target = BlockPos::new(pos.0.x - 3 + xx, pos.0.y, pos.0.z - 3 + zz);
                    self.place_circle(chunk, block_registry, random, target);
                }
            }
        }
    }

    fn place_circle<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) {
        for xx in -2i32..=2 {
            for zz in -2i32..=2 {
                if xx.abs() != 2 || zz.abs() != 2 {
                    let target = BlockPos::new(pos.0.x + xx, pos.0.y, pos.0.z + zz);
                    self.place_block_at(chunk, block_registry, random, target);
                }
            }
        }
    }

    fn place_block_at<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) {
        for dy in (-3..=2).rev() {
            let cursor = BlockPos::new(pos.0.x, pos.0.y + dy, pos.0.z);
            if let Some(state) = self
                .provider
                .get_optional(block_registry, chunk, random, cursor)
            {
                chunk.set_block_state(&cursor.0, state);
                break;
            }
            if !chunk.is_air(&cursor.0) && dy < 0 {
                break;
            }
        }
    }
}
