use std::collections::HashSet;

use pumpkin_data::BlockDirection;
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::feature::java_set::vanilla_hash_set_order;
use crate::generation::proto_chunk::GenerationCache;
use crate::{generation::block_state_provider::BlockStateProvider, world::WorldPortalExt};

/// Vanilla `AttachedToLeavesDecorator` (`AttachedToLeavesDecorator.java:50-80`).
pub struct AttachedToLeavesTreeDecorator {
    pub probability: f32,
    pub exclusion_radius_xz: i32,
    pub exclusion_radius_y: i32,
    pub block_provider: BlockStateProvider,
    pub required_empty_blocks: i32,
    pub directions: Vec<BlockDirection>,
}

impl AttachedToLeavesTreeDecorator {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        foliage_positions: &[BlockPos],
    ) {
        let mut blacklist: HashSet<BlockPos> = HashSet::new();
        let mut leaves = vanilla_hash_set_order(foliage_positions);
        leaves.sort_by_key(|pos| pos.0.y);
        for i in (1..leaves.len()).rev() {
            let j = random.next_bounded_i32(i as i32 + 1) as usize;
            leaves.swap(i, j);
        }

        for leaf_pos in leaves {
            let direction =
                self.directions[random.next_bounded_i32(self.directions.len() as i32) as usize];
            let placement_pos = leaf_pos.offset(direction.to_offset());
            if blacklist.contains(&placement_pos)
                || random.next_f32() >= self.probability
                || !self.has_required_empty_blocks(chunk, leaf_pos, direction)
            {
                continue;
            }
            for x in -self.exclusion_radius_xz..=self.exclusion_radius_xz {
                for y in -self.exclusion_radius_y..=self.exclusion_radius_y {
                    for z in -self.exclusion_radius_xz..=self.exclusion_radius_xz {
                        blacklist.insert(BlockPos::new(
                            placement_pos.0.x + x,
                            placement_pos.0.y + y,
                            placement_pos.0.z + z,
                        ));
                    }
                }
            }
            let state = self
                .block_provider
                .get(random, placement_pos, chunk, block_registry);
            chunk.set_block_state(&placement_pos.0, state);
        }
    }

    fn has_required_empty_blocks<T: GenerationCache>(
        &self,
        chunk: &T,
        leaf_pos: BlockPos,
        direction: BlockDirection,
    ) -> bool {
        let offset = direction.to_offset();
        (1..=self.required_empty_blocks).all(|i| {
            let pos = BlockPos::new(
                leaf_pos.0.x + offset.x * i,
                leaf_pos.0.y + offset.y * i,
                leaf_pos.0.z + offset.z * i,
            );
            GenerationCache::get_block_state(chunk, &pos.0)
                .to_state()
                .is_air()
        })
    }
}
