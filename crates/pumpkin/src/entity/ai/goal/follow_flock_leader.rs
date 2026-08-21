// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::{EntityBase, mob::Mob};

/// Vanilla: `FollowFlockLeaderGoal.INTERVAL_TICKS` -- base cooldown between school-forming
/// attempts (`200 + random.nextInt(200) % 20`).
const INTERVAL_TICKS: i32 = 200;
/// Search box inflation used by `FollowFlockLeaderGoal.canUse`.
const SCHOOL_RANGE: f64 = 8.0;
const IN_RANGE_OF_LEADER_SQ: f64 = 121.0;

/// Makes schooling fish (cod, salmon, tropical fish) cluster with, and follow, one leader fish of
/// the same species picked from nearby individuals.
///
/// Vanilla source: `net/minecraft/world/entity/ai/goal/FollowFlockLeaderGoal.java`, backed by
/// per-fish state in `net/minecraft/world/entity/animal/fish/AbstractSchoolingFish.java`
/// (`leader`/`schoolSize`/`isFollower`/`canBeFollowed`).
pub struct FollowFlockLeaderGoal {
    goal_control: Controls,
    next_start_tick: AtomicI32,
    time_to_recalc_path: AtomicI32,
}

/// A leader and the fish that would join it, as `AbstractSchoolingFish` pairs them.
type School = (Arc<dyn EntityBase>, Vec<Arc<dyn EntityBase>>);

impl FollowFlockLeaderGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            goal_control: Controls::MOVE,
            // `new()` is kept for existing registrations. The first can_start call seeds the
            // same randomized delay that vanilla creates in the goal constructor.
            next_start_tick: AtomicI32::new(-1),
            time_to_recalc_path: AtomicI32::new(0),
        })
    }

    fn roll_next_start_tick(mob: &dyn Mob) -> i32 {
        to_goal_ticks(INTERVAL_TICKS + mob.get_random().random_range(0..200) % 20)
    }

    fn find_school(mob: &dyn Mob) -> Option<School> {
        let self_entity = mob.get_entity();
        let self_uuid = self_entity.entity_uuid;
        let self_type = self_entity.entity_type;
        let world = self_entity.world.load();
        let search_box =
            self_entity
                .bounding_box
                .load()
                .expand(SCHOOL_RANGE, SCHOOL_RANGE, SCHOOL_RANGE);

        // `get_entities_at_box` preserves the world's entity-list order, matching the order
        // consumed by vanilla's `getEntitiesOfClass` stream. The existing sphere/HashMap helper
        // would both select the wrong shape and lose that ordering.
        let mut candidates = Vec::new();
        for candidate in world.get_entities_at_box(&search_box) {
            let candidate_entity = candidate.get_entity();
            if candidate_entity.entity_type != self_type || !candidate_entity.is_alive() {
                continue;
            }
            let Some(candidate_mob) = candidate.get_mob() else {
                continue;
            };
            if candidate_mob
                .get_mob_entity()
                .can_be_followed_by_schooling_fish()
                || !candidate_mob.get_mob_entity().is_schooling_follower()
            {
                candidates.push(candidate);
            }
        }

        let leader = candidates
            .iter()
            .find(|candidate| {
                candidate.get_mob().is_some_and(|candidate_mob| {
                    candidate_mob
                        .get_mob_entity()
                        .can_be_followed_by_schooling_fish()
                })
            })
            .cloned()
            .or_else(|| world.get_entity_by_uuid(self_uuid))?;

        Some((leader, candidates))
    }
}

impl Default for FollowFlockLeaderGoal {
    fn default() -> Self {
        *Self::new()
    }
}

impl Goal for FollowFlockLeaderGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if mob.get_mob_entity().has_schooling_followers() {
                return false;
            }
            if mob.get_mob_entity().is_schooling_follower() {
                return true;
            }

            let remaining = self.next_start_tick.load(Ordering::Relaxed);
            if remaining < 0 {
                self.next_start_tick
                    .store(Self::roll_next_start_tick(mob) - 1, Ordering::Relaxed);
                return false;
            }
            if remaining > 0 {
                self.next_start_tick.fetch_sub(1, Ordering::Relaxed);
                return false;
            }
            self.next_start_tick
                .store(Self::roll_next_start_tick(mob), Ordering::Relaxed);

            let Some((leader, candidates)) = Self::find_school(mob) else {
                return false;
            };

            let Some(leader_mob) = leader.get_mob() else {
                return false;
            };
            let remaining = leader_mob.get_mob_entity().schooling_followers_remaining();

            // Vanilla applies the capacity limit before filtering out the leader itself. Keep
            // that order, and preserve the query order instead of imposing an entity-id sort.
            for candidate in candidates.into_iter().take(remaining) {
                if candidate.get_entity().entity_id == leader.get_entity().entity_id {
                    continue;
                }
                let Some(candidate_mob) = candidate.get_mob() else {
                    continue;
                };
                if candidate_mob.get_mob_entity().is_schooling_follower() {
                    continue;
                }
                let _ = candidate_mob
                    .get_mob_entity()
                    .start_schooling_following(&leader);
            }

            mob.get_mob_entity().is_schooling_follower()
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if !mob.get_mob_entity().is_schooling_follower() {
                return false;
            }
            let Some(leader) = mob.get_mob_entity().schooling_leader() else {
                return false;
            };

            let dist_sq = mob
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&leader.get_entity().pos.load());
            dist_sq <= IN_RANGE_OF_LEADER_SQ
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.time_to_recalc_path.store(0, Ordering::Relaxed);
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(leader) = mob.get_mob_entity().schooling_leader() else {
                return;
            };
            mob.get_mob_entity().stop_schooling_following_if(&leader);
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let remaining = self.time_to_recalc_path.fetch_sub(1, Ordering::Relaxed) - 1;
            if remaining > 0 {
                return;
            }
            self.time_to_recalc_path
                .store(to_goal_ticks(10), Ordering::Relaxed);

            let Some(leader) = mob.get_mob_entity().schooling_leader() else {
                return;
            };

            let pos = mob.get_entity().pos.load();
            let target: Vector3<f64> = leader.get_entity().pos.load();
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap()
                .set_progress(NavigatorGoal::new(pos, target, 1.0));
        })
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}
