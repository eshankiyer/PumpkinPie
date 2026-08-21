//! Port of `PiglinAi.avoidRepellent` (`PiglinAi.java:290-292`) together with the
//! `NEAREST_REPELLENT` half of `PiglinSpecificSensor.doTick`
//! (`PiglinSpecificSensor.java:112-120`).
//!
//! Vanilla splits this in two: the sensor writes `NEAREST_REPELLENT` from
//! `BlockPos.findClosestMatch(blockPosition(), 8, 4, ...)`, and the behavior is
//! `SetWalkTargetAwayFrom.pos(NEAREST_REPELLENT, 1.0F, 8, false)`, which runs in both the IDLE
//! and CELEBRATE activities. With no memory system to route a block position through, both
//! halves live in this one `Goal`.
//!
//! Deviations:
//! - `SetWalkTargetAwayFrom` re-picks a walk target every tick while the memory is present; here
//!   the scan is throttled to `SCAN_INTERVAL_TICKS`, the same throttle `hoglin.rs` already
//!   applies to its own identical repellent scan (17x17x9 block reads is not a per-tick cost).
//! - The flee position comes from `AvoidEntityGoal::find_flee_position`
//!   (`NoPenaltyTargeting.findFrom`), while vanilla's `SetWalkTargetAwayFrom` uses
//!   `DefaultRandomPos.getPosAway` with a radius of 8. Both pick a random reachable point in a
//!   cone facing away; the search radius differs.

use pumpkin_data::Block;
use pumpkin_data::block_properties::{BlockProperties, CampfireLikeProperties};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::avoid_entity::AvoidEntityGoal;
use super::{Controls, Goal, GoalFuture};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::world::World;

/// `BlockPos.findClosestMatch(blockPosition(), 8, 4, ...)`
/// (`PiglinSpecificSensor.java:113`).
const RANGE_HORIZONTAL: i32 = 8;
const RANGE_VERTICAL: i32 = 4;
/// `SetWalkTargetAwayFrom.pos(..., 1.0F, 8, false)` (`PiglinAi.java:291`).
const FLEE_SPEED: f64 = 1.0;
/// See the module doc; matches `hoglin.rs`'s `REPELLENT_SCAN_INTERVAL_TICKS`.
const SCAN_INTERVAL_TICKS: i32 = 20;

/// Walks a piglin away from the nearest piglin-repellent block.
pub struct PiglinAvoidRepellentGoal {
    scan_countdown: i32,
    flee_pos: Option<Vector3<f64>>,
}

impl PiglinAvoidRepellentGoal {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            scan_countdown: 0,
            flee_pos: None,
        }
    }

    /// `PiglinSpecificSensor.isValidRepellent` (`PiglinSpecificSensor.java:117-121`): a block in
    /// the `piglin_repellents` tag, except that a soul campfire only counts while it is lit.
    fn is_valid_repellent(world: &World, pos: &BlockPos) -> bool {
        let (block, state) = world.get_block_and_state(pos);
        if !block.has_tag(&tag::Block::MINECRAFT_PIGLIN_REPELLENTS) {
            return false;
        }
        if block.id == Block::SOUL_CAMPFIRE.id {
            return CampfireLikeProperties::from_state_id(state.id, block).lit;
        }
        true
    }

    /// `BlockPos.findClosestMatch`: the nearest match, so the box is scanned in full and the
    /// closest hit wins rather than the first.
    fn find_nearest_repellent(mob: &dyn Mob) -> Option<BlockPos> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let center = entity.block_pos.load();

        let mut best: Option<(i32, BlockPos)> = None;
        for dy in -RANGE_VERTICAL..=RANGE_VERTICAL {
            for dx in -RANGE_HORIZONTAL..=RANGE_HORIZONTAL {
                for dz in -RANGE_HORIZONTAL..=RANGE_HORIZONTAL {
                    let pos = BlockPos(center.0 + Vector3::new(dx, dy, dz));
                    if !Self::is_valid_repellent(&world, &pos) {
                        continue;
                    }
                    let dist_sq = dx * dx + dy * dy + dz * dz;
                    if best.is_none_or(|(best_dist, _)| dist_sq < best_dist) {
                        best = Some((dist_sq, pos));
                    }
                }
            }
        }
        best.map(|(_, pos)| pos)
    }
}

impl Default for PiglinAvoidRepellentGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for PiglinAvoidRepellentGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if self.scan_countdown > 0 {
                self.scan_countdown -= 1;
                return false;
            }
            self.scan_countdown = SCAN_INTERVAL_TICKS;

            let Some(repellent) = Self::find_nearest_repellent(mob) else {
                return false;
            };
            let Some(flee_pos) =
                AvoidEntityGoal::find_flee_position(mob, &repellent.to_centered_f64())
            else {
                return false;
            };
            self.flee_pos = Some(flee_pos);
            true
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            !navigator.is_idle()
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(flee_pos) = self.flee_pos {
                let mob_pos = mob.get_entity().pos.load();
                let mut navigator = mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                navigator.set_progress(NavigatorGoal::new(mob_pos, flee_pos, FLEE_SPEED));
            }
        })
    }

    fn stop<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.flee_pos = None;
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
