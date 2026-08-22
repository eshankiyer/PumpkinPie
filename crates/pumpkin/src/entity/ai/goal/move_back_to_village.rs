use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use std::sync::atomic::Ordering;

use super::random_pos::default_get_pos_towards;
use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::world::World;
use crate::world::village_poi::{
    MAX_VILLAGE_DISTANCE, Occupancy, VILLAGE_TAG_POI_TYPES, section_chebyshev_distance,
};

/// Section coordinates of `pos` - vanilla `SectionPos.of(BlockPos)`.
#[must_use]
pub(crate) const fn section_of(pos: BlockPos) -> Vector3<i32> {
    Vector3::new(pos.0.x >> 4, pos.0.y >> 4, pos.0.z >> 4)
}

/// Block position at the center of a chunk section - vanilla `SectionPos.center()`.
#[must_use]
pub(crate) const fn section_center(section: Vector3<i32>) -> BlockPos {
    BlockPos(Vector3::new(
        (section.x << 4) + 8,
        (section.y << 4) + 8,
        (section.z << 4) + 8,
    ))
}

/// Vanilla `Vec3.atBottomCenterOf(BlockPos)`.
#[must_use]
pub(crate) fn at_bottom_center_of(pos: BlockPos) -> Vector3<f64> {
    Vector3::new(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y),
        f64::from(pos.0.z) + 0.5,
    )
}

/// One snapshot of every *occupied* village-tag POI near a mob, so a goal can evaluate
/// `ServerLevel.sectionsToVillage` for many candidate sections without re-querying the POI
/// store per section.
///
/// `BehaviorUtils.findSectionClosestToVillage` (`BehaviorUtils.java:107-113`) and
/// `GolemRandomStrollInVillageGoal.getRandomVillageSection`
/// (`GolemRandomStrollInVillageGoal.java:77-83`) both evaluate `sectionsToVillage` over a
/// 5x5x5 section cube; calling `World::sections_to_village` 125 times would take the POI
/// storage lock 375 times per `getPosition()`. The POI set does not change within a single
/// evaluation, so one scan is equivalent and vastly cheaper.
///
/// The metric itself is unchanged from `World::sections_to_village`: Chebyshev distance in
/// sections to the nearest POI matching `Occupancy::IsOccupied` (vanilla's
/// `PoiManager.isVillageCenter` requires an *occupied* POI, not merely an existing one),
/// saturating at `MAX_VILLAGE_DISTANCE + 1`.
pub(crate) struct VillageSectionScan {
    occupied: Vec<BlockPos>,
}

impl VillageSectionScan {
    /// `section_radius` is how far from `center` the caller intends to evaluate sections; the
    /// scan covers that plus `MAX_VILLAGE_DISTANCE`, so every evaluated section sees every POI
    /// that could give it a non-saturated distance.
    pub(crate) async fn around(world: &World, center: BlockPos, section_radius: i32) -> Self {
        let block_radius = (section_radius + MAX_VILLAGE_DISTANCE + 1) * 16;
        let mut storage = world.portal_poi.lock().await;
        let mut occupied = Vec::new();
        for poi_type in VILLAGE_TAG_POI_TYPES {
            occupied.extend(
                storage
                    .get_in_square_with_tickets(center, block_radius, Some(poi_type))
                    .into_iter()
                    .filter(|(_, free, max)| Occupancy::IsOccupied.matches(*free, *max))
                    .map(|(pos, _, _)| pos),
            );
        }
        Self { occupied }
    }

    /// Vanilla `ServerLevel.sectionsToVillage(SectionPos)` (`ServerLevel.java:1554-1555`).
    pub(crate) fn sections_to_village(&self, section: Vector3<i32>) -> i32 {
        let probe = section_center(section);
        let mut best = MAX_VILLAGE_DISTANCE + 1;
        for candidate in &self.occupied {
            let distance = section_chebyshev_distance(probe, *candidate);
            if distance < best {
                best = distance;
            }
        }
        best
    }
}

/// Every section in the `(2r+1)^3` cube centered on `section` - vanilla `SectionPos.cube`
/// (`SectionPos.java:236-241`) via `betweenClosedStream` (line 249) and `Cursor3D.advance`
/// (`Cursor3D.java:29-40`).
///
/// Iteration order matters and is reproduced exactly: `Cursor3D` decodes its linear index as
/// `x = i % width`, `y = (i / width) % height`, `z = i / (width * height)`, so X varies
/// fastest and Z slowest. `BehaviorUtils.findSectionClosestToVillage` takes `Stream.min`,
/// which keeps the *first* of equally-close sections, so this order is what decides ties.
pub(crate) fn section_cube(section: Vector3<i32>, radius: i32) -> Vec<Vector3<i32>> {
    let mut sections = Vec::new();
    for z in (section.z - radius)..=(section.z + radius) {
        for y in (section.y - radius)..=(section.y + radius) {
            for x in (section.x - radius)..=(section.x + radius) {
                sections.push(Vector3::new(x, y, z));
            }
        }
    }
    sections
}

/// Vanilla `MoveBackToVillageGoal` (`MoveBackToVillageGoal.java`).
///
/// A `RandomStrollGoal` subclass that only runs when the mob is *outside* a village, and then
/// strolls towards whichever nearby section is closer to one.
///
/// Registered on `IronGolem` at priority 2 (`IronGolem.java:70`,
/// `new MoveBackToVillageGoal(this, 0.6, false)`), which is what keeps a village golem from
/// wandering off into the wilderness permanently.
pub struct MoveBackToVillageGoal {
    goal_control: Controls,
    speed: f64,
    /// `RandomStrollGoal.interval`, already run through `to_goal_ticks`
    /// (vanilla `reducedTickDelay`).
    chance: i32,
    check_no_action_time: bool,
    force_trigger: bool,
    wanted: Option<Vector3<f64>>,
}

impl MoveBackToVillageGoal {
    /// `MAX_XZ_DIST` / `MAX_Y_DIST` (`MoveBackToVillageGoal.java:13-14`), also the
    /// `RandomStrollGoal` interval passed by its constructor (line 17).
    const MAX_XZ_DIST: i32 = 10;
    const MAX_Y_DIST: i32 = 7;
    const INTERVAL: i32 = 10;
    /// `BehaviorUtils.findSectionClosestToVillage(level, sectionPos, 2)`
    /// (`MoveBackToVillageGoal.java:32`).
    const SECTION_SCAN_RADIUS: i32 = 2;

    #[must_use]
    pub fn new(speed: f64, check_no_action_time: bool) -> Box<Self> {
        Box::new(Self {
            goal_control: Controls::MOVE,
            speed,
            chance: to_goal_ticks(Self::INTERVAL),
            check_no_action_time,
            force_trigger: false,
            wanted: None,
        })
    }

    /// `MoveBackToVillageGoal.getPosition` (`MoveBackToVillageGoal.java:27-36`).
    async fn get_position(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let entity = &mob.get_mob_entity().living_entity.entity;
        let world = entity.world.load();
        let pos = entity.block_pos.load();
        let here = section_of(pos);

        let scan = VillageSectionScan::around(&world, pos, Self::SECTION_SCAN_RADIUS).await;
        // `BehaviorUtils.findSectionClosestToVillage` (`BehaviorUtils.java:107-113`): among
        // the cube, keep only sections strictly closer to a village than the mob's own
        // section, then take the minimum. `Stream.min` keeps the first of equal minima, so
        // the tie-break depends on `SectionPos.cube`'s iteration order, which
        // `section_cube` reproduces.
        let distance_here = scan.sections_to_village(here);
        let mut best = here;
        let mut best_distance = distance_here;
        for section in section_cube(here, Self::SECTION_SCAN_RADIUS) {
            let distance = scan.sections_to_village(section);
            if distance < distance_here && distance < best_distance {
                best_distance = distance;
                best = section;
            }
        }

        if best == here {
            return None;
        }

        default_get_pos_towards(
            mob,
            Self::MAX_XZ_DIST,
            Self::MAX_Y_DIST,
            at_bottom_center_of(section_center(best)),
            std::f64::consts::FRAC_PI_2,
        )
    }
}

impl Goal for MoveBackToVillageGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            // `MoveBackToVillageGoal.canUse` (`MoveBackToVillageGoal.java:20-25`): already in
            // a village, nothing to do. `ServerLevel.isVillage(pos)` is
            // `isCloseToVillage(pos, 1)` (`ServerLevel.java:1542-1543`).
            let entity = &mob.get_mob_entity().living_entity.entity;
            let world = entity.world.load();
            if world.is_close_to_village(entity.block_pos.load(), 1).await {
                return false;
            }

            // `RandomStrollGoal.canUse` (`RandomStrollGoal.java:36-62`).
            if mob.has_controlling_passenger().await {
                return false;
            }
            if !self.force_trigger {
                if self.check_no_action_time
                    && mob.get_mob_entity().no_action_time.load(Ordering::Relaxed) >= 100
                {
                    return false;
                }
                if mob.get_random().random_range(0..self.chance) != 0 {
                    return false;
                }
            }

            self.wanted = Self::get_position(mob).await;
            if self.wanted.is_none() {
                return false;
            }
            self.force_trigger = false;
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
            !navigator_idle && !mob.has_controlling_passenger().await
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(wanted) = self.wanted {
                let pos = mob.get_mob_entity().living_entity.entity.pos.load();
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
        self.goal_control
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_helpers_round_trip() {
        let pos = BlockPos(Vector3::new(35, 70, -17));
        assert_eq!(section_of(pos), Vector3::new(2, 4, -2));
        assert_eq!(
            section_center(Vector3::new(2, 4, -2)),
            BlockPos(Vector3::new(40, 72, -24))
        );
    }

    #[test]
    fn section_cube_has_expected_size_and_contains_center() {
        let center = Vector3::new(0, 4, 0);
        let cube = section_cube(center, 2);
        assert_eq!(cube.len(), 125);
        assert!(cube.contains(&center));
        // `Cursor3D` varies X fastest and Z slowest; the tie-break in
        // `findSectionClosestToVillage` depends on it.
        assert_eq!(cube[0], Vector3::new(-2, 2, -2));
        assert_eq!(cube[1], Vector3::new(-1, 2, -2));
        assert_eq!(cube[5], Vector3::new(-2, 3, -2));
        assert_eq!(cube[25], Vector3::new(-2, 2, -1));
    }

    #[test]
    fn empty_scan_saturates_at_max_village_distance_plus_one() {
        let scan = VillageSectionScan {
            occupied: Vec::new(),
        };
        assert_eq!(
            scan.sections_to_village(Vector3::new(0, 4, 0)),
            MAX_VILLAGE_DISTANCE + 1
        );
    }

    #[test]
    fn scan_reports_chebyshev_section_distance() {
        let scan = VillageSectionScan {
            occupied: vec![BlockPos(Vector3::new(0, 64, 0))],
        };
        // Same section as the POI.
        assert_eq!(scan.sections_to_village(Vector3::new(0, 4, 0)), 0);
        // Three sections away on X, one on Z -> Chebyshev 3.
        assert_eq!(scan.sections_to_village(Vector3::new(3, 4, 1)), 3);
    }
}
