use decorator::TreeDecorator;
use foliage::{FoliagePlacer, FoliageSetter};
use pumpkin_data::block_properties::{BlockProperties, OakLeavesLikeProperties};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState, BlockStateId, tag};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use pumpkin_util::random::RandomGenerator;
use root::RootPlacer;

use trunk::TrunkPlacer;

use crate::generation::feature::java_set::{JavaHashSet, vanilla_hash_set_order};
use crate::generation::proto_chunk::GenerationCache;
use crate::generation::{block_state_provider::BlockStateProvider, feature::size::FeatureSize};
use crate::world::WorldPortalExt;

pub mod decorator;
pub mod foliage;
pub mod root;
pub mod trunk;

pub struct TreeFeature {
    pub trunk_provider: BlockStateProvider,
    pub trunk_placer: TrunkPlacer,
    pub foliage_provider: BlockStateProvider,
    pub foliage_placer: FoliagePlacer,
    pub minimum_size: FeatureSize,
    pub ignore_vines: bool,
    pub decorators: Vec<TreeDecorator>,
    pub below_trunk_provider: BlockStateProvider,
    pub root_placer: Option<RootPlacer>,
}

pub struct TreeNode {
    center: BlockPos,
    foliage_radius: i32,
    giant_trunk: bool,
}

impl TreeFeature {
    #[expect(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        block_registry: &dyn WorldPortalExt,
        chunk: &mut T,
        min_y: i8,
        height: u16,
        feature_name: pumpkin_data::placed_feature::PlacedFeature, // This placed feature
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let (log_positions, root_positions, foliage_positions) = self.generate_main(
            block_registry,
            chunk,
            min_y,
            height,
            feature_name,
            random,
            pos,
        );
        // `TreeFeature.place` (`TreeFeature.java:153-167`): a failed or empty tree runs no
        // decorator and reports failure, which is what makes a blocked sapling stay in place.
        if log_positions.is_empty() && foliage_positions.is_empty() {
            return false;
        }

        let mut decorations = Vec::new();
        for decorator in &self.decorators {
            decorator.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                &root_positions,
                &log_positions,
                &foliage_positions,
                &mut decorations,
            );
        }
        Self::update_leaves(
            chunk,
            &root_positions,
            &log_positions,
            &foliage_positions,
            &decorations,
        );
        true
    }

    /// Vanilla `TreeFeature.updateLeaves` (`TreeFeature.java:170-232`): walks outwards from the
    /// trunk through `getOptionalDistanceAt` blocks inside the tree's bounds and writes each
    /// reached leaf's `distance`, so generated canopies do not decay.
    fn update_leaves<T: GenerationCache>(
        chunk: &mut T,
        root_positions: &[BlockPos],
        log_positions: &[BlockPos],
        foliage_positions: &[BlockPos],
        decorations: &[BlockPos],
    ) {
        const MAX_DISTANCE: usize = 7;

        // `BoundingBox.encapsulatingPositions` over roots, trunks, foliage and decorations.
        let mut all = root_positions
            .iter()
            .chain(log_positions)
            .chain(foliage_positions)
            .chain(decorations);
        let Some(first) = all.next() else {
            return;
        };
        let (mut min, mut max) = (first.0, first.0);
        for pos in all {
            min = Vector3::new(min.x.min(pos.0.x), min.y.min(pos.0.y), min.z.min(pos.0.z));
            max = Vector3::new(max.x.max(pos.0.x), max.y.max(pos.0.y), max.z.max(pos.0.z));
        }
        let span = Vector3::new(max.x - min.x + 1, max.y - min.y + 1, max.z - min.z + 1);
        let index = |pos: Vector3<i32>| -> Option<usize> {
            let local = pos.sub(&min);
            if local.x < 0
                || local.y < 0
                || local.z < 0
                || local.x >= span.x
                || local.y >= span.y
                || local.z >= span.z
            {
                return None;
            }
            Some(((local.x * span.y + local.y) * span.z + local.z) as usize)
        };
        let mut shape = vec![false; (span.x * span.y * span.z) as usize];

        for pos in decorations.iter().chain(root_positions) {
            if let Some(i) = index(pos.0) {
                shape[i] = true;
            }
        }

        let mut to_check: [JavaHashSet; MAX_DISTANCE] = std::array::from_fn(|_| JavaHashSet::new());
        // `toCheck[0].addAll(logs)` iterates the trunk `HashSet`.
        for pos in vanilla_hash_set_order(log_positions) {
            to_check[0].add(pos);
        }

        let mut smallest = 0;
        while smallest < MAX_DISTANCE {
            let Some(pos) = to_check[smallest].pop_first() else {
                smallest += 1;
                continue;
            };
            let Some(i) = index(pos.0) else {
                continue;
            };
            if smallest != 0 {
                let state_id = GenerationCache::get_block_state(chunk, &pos.0);
                let block = Block::from_state_id(state_id);
                if OakLeavesLikeProperties::handles_block_id(block.id) {
                    let mut props = OakLeavesLikeProperties::from_state_id(state_id, block);
                    props.distance = smallest as u8;
                    chunk.set_block_state(&pos.0, BlockState::from_id(props.to_state_id(block)));
                }
            }
            shape[i] = true;

            for direction in BlockDirection::all() {
                let neighbor = pos.0.add(&direction.to_offset());
                let Some(n) = index(neighbor) else {
                    continue;
                };
                if shape[n] {
                    continue;
                }
                let state_id = GenerationCache::get_block_state(chunk, &neighbor);
                if let Some(distance) = Self::optional_distance_at(state_id) {
                    let new_distance = distance.min(smallest + 1);
                    if new_distance < MAX_DISTANCE {
                        to_check[new_distance].add(BlockPos(neighbor));
                        smallest = smallest.min(new_distance);
                    }
                }
            }
        }
    }

    /// Vanilla `LeavesBlock.getOptionalDistanceAt` (`LeavesBlock.java:129-135`).
    fn optional_distance_at(state_id: BlockStateId) -> Option<usize> {
        let block = Block::from_state_id(state_id);
        if block.has_tag(&tag::Block::MINECRAFT_PREVENTS_NEARBY_LEAF_DECAY) {
            Some(0)
        } else if OakLeavesLikeProperties::handles_block_id(block.id) {
            Some(OakLeavesLikeProperties::from_state_id(state_id, block).distance as usize)
        } else {
            None
        }
    }

    pub fn can_replace_or_log(state: &BlockState, id: BlockId) -> bool {
        Self::can_replace(state, id) || id.has_tag(tag::Block::MINECRAFT_LOGS)
    }

    pub fn is_air_or_leaves(state: &BlockState, id: BlockId) -> bool {
        state.is_air() || id.has_tag(tag::Block::MINECRAFT_LEAVES)
    }

    pub fn can_replace(state: &BlockState, id: BlockId) -> bool {
        state.is_air() || id.has_tag(tag::Block::MINECRAFT_REPLACEABLE_BY_TREES)
    }

    #[expect(clippy::too_many_arguments)]
    fn generate_main<T: GenerationCache>(
        &self,
        block_registry: &dyn WorldPortalExt,
        chunk: &mut T,
        min_y: i8,
        world_height: u16,
        _feature_name: pumpkin_data::placed_feature::PlacedFeature, // This placed feature
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> (Vec<BlockPos>, Vec<BlockPos>, Vec<BlockPos>) {
        let height = self.trunk_placer.get_height(random);
        // Vanilla `TreeFeature.doPlace` (`TreeFeature.java:65-68`) samples the foliage height
        // and radius right after the tree height, before the trunk origin and free-space checks.
        let foliage_height = self
            .foliage_placer
            .r#type
            .get_random_height(random, height as i32);
        let base_height = height as i32 - foliage_height;
        let foliage_radius = self.foliage_placer.get_random_radius(random, base_height);

        let trunk_start = self
            .root_placer
            .as_ref()
            .map_or(pos, |placer| placer.trunk_offset(pos, random));

        // Build-height check (`TreeFeature.java:70-72`): `getMaxY` is the topmost buildable Y.
        let bottom = i32::from(min_y);
        let top_y = bottom + i32::from(world_height) - 1;
        if pos.0.y.min(trunk_start.0.y) < bottom + 1
            || pos.0.y.max(trunk_start.0.y) + height as i32 + 1 > top_y + 1
        {
            return (vec![], vec![], vec![]);
        }

        let clipped_height = self.minimum_size.min_clipped_height;
        let top = self.get_top(height, chunk, trunk_start);
        if top < height && top < clipped_height.map_or(u32::MAX, |h| h as u32) {
            return (vec![], vec![], vec![]);
        }

        let root_positions = if let Some(placer) = &self.root_placer {
            match placer.generate(chunk, block_registry, random, pos, trunk_start) {
                Some(positions) => positions,
                None => return (vec![], vec![], vec![]),
            }
        } else {
            Vec::new()
        };

        let trunk_state = self.trunk_provider.get(random, pos, chunk, block_registry);

        let (nodes, logs) = self.trunk_placer.generate(
            block_registry,
            top,
            trunk_start,
            chunk,
            random,
            &self.below_trunk_provider,
            trunk_state,
        );

        // One setter for the whole tree (`TreeFeature.java:127,137-148`), so every attachment
        // shares `isSet` and the decorators see each foliage position once.
        let mut setter = FoliageSetter::new(&self.foliage_provider, block_registry);
        for node in nodes {
            self.foliage_placer.generate(
                chunk,
                random,
                &node,
                foliage_height,
                foliage_radius,
                &mut setter,
            );
        }
        (logs, root_positions, setter.into_positions())
    }

    fn get_top<T: GenerationCache>(&self, height: u32, chunk: &T, init_pos: BlockPos) -> u32 {
        for y in 0..=height + 1 {
            let j = self.minimum_size.r#type.get_radius(height, y as i32);
            for x in -j..=j {
                for z in -j..=j {
                    let pos = BlockPos(init_pos.0.add_raw(x, y as i32, z));
                    let rstate = GenerationCache::get_block_state(chunk, &pos.0);
                    let block = rstate.to_block_id();
                    if self.trunk_placer.is_free(rstate.to_state(), block)
                        && (self.ignore_vines || block != BlockId::VINE)
                    {
                        continue;
                    }
                    return y.saturating_sub(2);
                }
            }
        }
        height
    }
}
