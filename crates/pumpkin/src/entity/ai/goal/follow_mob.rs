//! Port of `FollowMobGoal.java`.

use std::sync::Arc;

use pumpkin_util::math::vector3::Vector3;

use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::mob::Mob;

/// `FollowMobGoal.tick` line 77: `this.adjustedTickDelay(10)`.
const RECALC_PATH_INTERVAL: i32 = 10;

/// Vanilla `FollowMobGoal` (`FollowMobGoal.java:14-97`): the mob tags along behind any *other*
/// kind of mob it can see, stopping once it is within `stop_distance`.
///
/// Divergences from vanilla, both in `tick` (lines 84-92):
///
/// * The `LookControl` comparison vanilla uses to decide whether a mob that is already close
///   enough should still back away is dropped -- `LookControl`'s wanted-position getters are not
///   exposed here. The distance half of that condition (`distSqr <= stopDistance`) is kept, so
///   the back-away step still happens when the followed mob is very close, just not when it
///   happens to be staring at this mob from slightly further off.
/// * Vanilla's `followPredicate` is `mob.getClass() != input.getClass()`; the analogue used here
///   is a different `EntityType`, which agrees for every registrant (`Parrot`, `Allay`) since
///   each entity class maps to exactly one type.
pub struct FollowMobGoal {
    speed_modifier: f64,
    /// Vanilla compares a *squared* distance against this unsquared field in `tick` line 87;
    /// that quirk is preserved, so it is kept alongside `stop_distance_sq`.
    stop_distance: f64,
    stop_distance_sq: f64,
    area_size: f64,
    following: Option<Arc<dyn EntityBase>>,
    time_to_recalc_path: i32,
    old_water_cost: f32,
}

impl FollowMobGoal {
    #[must_use]
    pub fn new(speed_modifier: f64, stop_distance: f32, area_size: f32) -> Box<Self> {
        Box::new(Self {
            speed_modifier,
            stop_distance: f64::from(stop_distance),
            stop_distance_sq: f64::from(stop_distance) * f64::from(stop_distance),
            area_size: f64::from(area_size),
            following: None,
            time_to_recalc_path: 0,
            old_water_cost: 0.0,
        })
    }

    /// `FollowMobGoal.canUse` (lines 39-51).
    fn find_mob_to_follow(&self, mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let self_entity = mob.get_entity();
        let self_type = self_entity.entity_type;
        let world = self_entity.world.load();
        let search_box =
            self_entity
                .bounding_box
                .load()
                .expand(self.area_size, self.area_size, self.area_size);
        world
            .get_entities_at_box(&search_box)
            .into_iter()
            .find(|c| {
                let entity = c.get_entity();
                entity.entity_type != self_type
                    && c.get_mob().is_some()
                    && !entity.invisible.load(std::sync::atomic::Ordering::Relaxed)
            })
    }
}

impl Goal for FollowMobGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            self.following = self.find_mob_to_follow(mob);
            self.following.is_some()
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            // `canContinueToUse` line 55.
            let Some(following) = self.following.as_ref() else {
                return false;
            };
            let navigator_idle = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle();
            if navigator_idle {
                return false;
            }
            let self_pos = mob.get_entity().pos.load();
            let target_pos = following.get_entity().pos.load();
            self_pos.squared_distance_to_vec(&target_pos) > self.stop_distance_sq
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            // `start` lines 59-63.
            self.time_to_recalc_path = 0;
            let mut navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.old_water_cost = navigator.get_pathfinding_malus(PathType::Water);
            navigator.set_pathfinding_malus(PathType::Water, 0.0);
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            // `stop` lines 66-70.
            self.following = None;
            let mut navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            navigator.stop();
            navigator.set_pathfinding_malus(PathType::Water, self.old_water_cost);
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            // `tick` lines 73-95.
            let Some(following) = self.following.clone() else {
                return;
            };
            let mob_entity = mob.get_mob_entity();
            let self_entity = mob.get_entity();
            if self_entity.is_leashed().await {
                return;
            }
            let target_entity = following.get_entity();
            let target_pos = target_entity.pos.load();
            mob_entity
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at(mob, target_pos.x, target_entity.get_eye_y(), target_pos.z);

            self.time_to_recalc_path -= 1;
            if self.time_to_recalc_path > 0 {
                return;
            }
            self.time_to_recalc_path = to_goal_ticks(RECALC_PATH_INTERVAL);

            let self_pos = self_entity.pos.load();
            let dist_sq = self_pos.squared_distance_to_vec(&target_pos);
            let mut navigator = mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if dist_sq > self.stop_distance_sq {
                navigator.set_progress(NavigatorGoal::new(
                    self_pos,
                    target_pos,
                    self.speed_modifier,
                ));
            } else {
                navigator.stop();
                // See the type-level note: only the distance half of vanilla's back-away
                // condition is ported.
                if dist_sq <= self.stop_distance {
                    let delta =
                        Vector3::new(target_pos.x - self_pos.x, 0.0, target_pos.z - self_pos.z);
                    navigator.set_progress(NavigatorGoal::new(
                        self_pos,
                        Vector3::new(self_pos.x - delta.x, self_pos.y, self_pos.z - delta.z),
                        self.speed_modifier,
                    ));
                }
            }
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
