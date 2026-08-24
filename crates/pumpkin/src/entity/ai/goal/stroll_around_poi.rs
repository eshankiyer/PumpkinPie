use pumpkin_util::math::vector3::Vector3;

use super::random_pos::land_get_pos;
use super::{Controls, Goal, GoalFuture};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;

/// Which claimed POI drives the stroll.
///
/// Vanilla passes the `MemoryModuleType<GlobalPos>` as a parameter
/// (`StrollAroundPoi.java:18`); Pumpkin villagers keep their claimed POIs as plain
/// accessors on `Mob` (the `GlobalPos` -> bare `BlockPos` reduction documented in
/// `entity/ai/brain/memory.rs`), so the memory-type parameter becomes this enum of
/// the two accessors the vanilla callers actually pass.
#[derive(Clone, Copy)]
pub enum StrollPoi {
    /// `StrollAroundPoi.create(MemoryModuleType.JOB_SITE, 0.4F, 4)`
    /// (`VillagerGoalPackages.getWorkPackage`, `VillagerGoalPackages.java:86`).
    JobSite,
    /// `StrollAroundPoi.create(MemoryModuleType.MEETING_POINT, 0.4F, 40)`
    /// (`VillagerGoalPackages.getMeetPackage` priority 2,
    /// `VillagerGoalPackages.java:152`).
    MeetingPoint,
}

/// Goal-system port of `behavior/StrollAroundPoi.java`.
///
/// Vanilla's version keeps a villager that is already near its claimed POI milling
/// about it instead of free-wandering: every 180 ticks it picks a random walkable
/// spot within 8 blocks horizontally / 6 vertically (`LandRandomPos.getPos(body, 8, 6)`,
/// `StrollAroundPoi.java:14-16,31`) and walks there at the configured speed with a
/// close-enough distance of 1 (`:32`). It never starts when the POI is farther than
/// `maxDistanceFromPoi` or in another dimension (`:23`), which is what pins MEET-hour
/// villagers to within 40 blocks of the bell and WORK-hour villagers to within 4 of
/// the job site.
///
/// Structure follows [`super::golem_random_stroll_in_village`] (the closest sibling:
/// a constrained stroll goal driving the same `Navigator`).
///
/// Deviations, all deliberate:
/// - Vanilla's captured `MutableLong nextOkStartTime` cooldown is measured against the
///   level's game time (`timestamp`, `StrollAroundPoi.java:27-33`). Pumpkin goals have no
///   game-time clock handed to them, so the monotonic world age is used; for a 180-tick
///   cooldown the two are interchangeable.
/// - When `LandRandomPos` finds no spot, vanilla *erases* `WALK_TARGET`
///   (`walkTarget.setOrErase`, `:32`) so a Brain sink drops any movement another behavior
///   requested. Here the goal simply declines to start and leaves the `MOVE` control to
///   whoever holds it - erasing would only fight the goal selector's current owner.
/// - The dimension half of the `:23` check is unrepresentable for the same reason as the
///   `GlobalPos` reduction above: claimed POIs are single-dimension `BlockPos`s already.
pub struct StrollAroundPoiGoal {
    poi: StrollPoi,
    speed: f64,
    max_distance_from_poi_sqr: f64,
    /// `MIN_TIME_BETWEEN_STROLLS = 180` (`StrollAroundPoi.java:14`).
    min_time_between_strolls: i64,
    /// Vanilla's captured `nextOkStartTime` (`StrollAroundPoi.java:19`): one per `create`
    /// call, shared across every trigger evaluation of that instance.
    next_ok_start_time: i64,
    wanted: Option<Vector3<f64>>,
}

impl StrollAroundPoiGoal {
    #[must_use]
    pub fn new(poi: StrollPoi, speed: f64, max_distance_from_poi: i32) -> Self {
        Self {
            poi,
            speed,
            max_distance_from_poi_sqr: f64::from(max_distance_from_poi)
                * f64::from(max_distance_from_poi),
            min_time_between_strolls: 180,
            next_ok_start_time: 0,
            wanted: None,
        }
    }

    /// `pos.pos().closerToCenterThan(body.position(), maxDistanceFromPoi)`
    /// (`StrollAroundPoi.java:23`) - inclusive squared-distance compare against the block
    /// center.
    fn near_poi(&self, mob: &dyn Mob, poi_center: Vector3<f64>) -> bool {
        let pos = mob.get_entity().pos.load();
        pos.squared_distance_to_vec(&poi_center) <= self.max_distance_from_poi_sqr
    }

    fn claimed_poi(&self, mob: &dyn Mob) -> Option<pumpkin_util::math::position::BlockPos> {
        match self.poi {
            StrollPoi::JobSite => mob.get_job_site(),
            StrollPoi::MeetingPoint => mob.get_meeting_point(),
        }
    }
}

impl Goal for StrollAroundPoiGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let world = mob.get_entity().world.load();
            // `if (timestamp <= nextOkStartTime.longValue()) return true`
            // (`StrollAroundPoi.java:27-29`): still satisfied during cooldown, no new
            // stroll - expressed here as "do not start".
            let now = world.get_world_age().await;
            if now <= self.next_ok_start_time {
                return false;
            }

            let Some(poi) = self.claimed_poi(mob) else {
                return false;
            };
            if !self.near_poi(mob, poi.to_f64()) {
                return false;
            }

            // Cooldown arms whether or not a spot was found (`StrollAroundPoi.java:33`).
            self.next_ok_start_time = now + self.min_time_between_strolls;

            // `LandRandomPos.getPos(body, STROLL_MAX_XZ_DIST=8, STROLL_MAX_Y_DIST=6)`
            // (`StrollAroundPoi.java:15-16,31`).
            self.wanted = land_get_pos(mob, 8, 6);
            if self.wanted.is_none() {
                return false;
            }
            true
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let navigator_idle = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle();
            !navigator_idle
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(wanted) = self.wanted {
                let pos = mob.get_entity().pos.load();
                mob.get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_progress(NavigatorGoal::new(pos, wanted, self.speed));
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.wanted = None;
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
