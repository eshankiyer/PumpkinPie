use crate::generation::feature::java_set::vanilla_hash_set_order;
use crate::{generation::proto_chunk::GenerationCache, world::WorldPortalExt};
use alter_ground::AlterGroundTreeDecorator;
use attached_to_leaves::AttachedToLeavesTreeDecorator;
use attached_to_logs::AttachedToLogsTreeDecorator;
use beehive::BeehiveTreeDecorator;
use cocoa::CocoaTreeDecorator;
use creaking_heart::CreakingHeartTreeDecorator;
use leave_vine::LeavesVineTreeDecorator;
use pale_moss::PaleMossTreeDecorator;
use place_on_ground::PlaceOnGroundTreeDecorator;
use pumpkin_util::{math::position::BlockPos, random::RandomGenerator};

use trunk_vine::TrunkVineTreeDecorator;

pub mod alter_ground;
pub mod attached_to_leaves;
pub mod attached_to_logs;
pub mod beehive;
pub mod cocoa;
pub mod creaking_heart;
pub mod leave_vine;
pub mod pale_moss;
pub mod place_on_ground;
pub mod trunk_vine;

pub enum TreeDecorator {
    TrunkVine(TrunkVineTreeDecorator),
    LeaveVine(LeavesVineTreeDecorator),
    PaleMoss(PaleMossTreeDecorator),
    CreakingHeart(CreakingHeartTreeDecorator),
    Cocoa(CocoaTreeDecorator),
    Beehive(BeehiveTreeDecorator),
    AlterGround(AlterGroundTreeDecorator),
    AttachedToLeaves(AttachedToLeavesTreeDecorator),
    PlaceOnGround(PlaceOnGroundTreeDecorator),
    AttachedToLogs(AttachedToLogsTreeDecorator),
}

impl TreeDecorator {
    #[expect(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        min_y: i8,
        height: u16,
        feature_name: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        root_positions: &[BlockPos],
        log_positions: &[BlockPos],
        foliage_positions: &[BlockPos],
        decorations: &mut Vec<BlockPos>,
    ) {
        match self {
            Self::TrunkVine(_decorator) => {
                TrunkVineTreeDecorator::generate(chunk, random, log_positions, decorations);
            }
            Self::LeaveVine(decorator) => {
                decorator.generate(chunk, random, foliage_positions, decorations);
            }
            Self::PaleMoss(decorator) => decorator.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                log_positions,
                foliage_positions,
                decorations,
            ),
            Self::CreakingHeart(decorator) => {
                decorator.generate(chunk, random, log_positions, decorations);
            }
            Self::Cocoa(decorator) => decorator.generate(chunk, random, log_positions, decorations),
            Self::Beehive(decorator) => {
                decorator.generate(
                    chunk,
                    random,
                    log_positions,
                    foliage_positions,
                    decorations,
                );
            }
            Self::AlterGround(decorator) => decorator.generate(
                chunk,
                block_registry,
                random,
                root_positions,
                log_positions,
                decorations,
            ),
            Self::PlaceOnGround(decorator) => decorator.generate(
                chunk,
                block_registry,
                random,
                root_positions,
                log_positions,
                decorations,
            ),
            Self::AttachedToLeaves(decorator) => {
                decorator.generate(chunk, block_registry, random, foliage_positions, decorations);
            }
            Self::AttachedToLogs(decorator) => {
                decorator.generate(chunk, block_registry, random, log_positions, decorations);
            }
        }
    }

    /// Vanilla `TreeFeature.getLowestTrunkOrRootOfTree` (`TreeFeature.java:234-248`) over the
    /// lists `TreeDecorator.Context` builds (`TreeDecorator.java:45-50`): each `HashSet` copied in
    /// Java iteration order, then stable-sorted by Y.
    pub(super) fn get_leaf_litter_positions(
        root_positions: &[BlockPos],
        log_positions: &[BlockPos],
    ) -> Vec<BlockPos> {
        let mut logs = vanilla_hash_set_order(log_positions);
        logs.sort_by_key(|pos| pos.0.y);
        // The root set ignores re-adds, but the mangrove root placer can report a position twice.
        let mut unique_roots: Vec<BlockPos> = Vec::with_capacity(root_positions.len());
        for pos in root_positions {
            if !unique_roots.contains(pos) {
                unique_roots.push(*pos);
            }
        }
        let mut roots = vanilla_hash_set_order(&unique_roots);
        roots.sort_by_key(|pos| pos.0.y);

        let Some(first_root) = roots.first() else {
            return logs;
        };
        if logs.first().is_some_and(|log| log.0.y == first_root.0.y) {
            logs.extend(roots);
            return logs;
        }
        roots
    }
}
