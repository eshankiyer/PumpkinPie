//! Vanilla `MoveThroughVillageGoal` (`MoveThroughVillageGoal.java`).
//!
//! `Zombie.addBehaviourGoals` registers it at priority 6 as `new MoveThroughVillageGoal(this,
//! 1.0, true, 4, this::canBreakDoors)` (`Zombie.java:122`).
//!
//! At night a zombie standing near a village walks to an occupied village point of interest it has
//! not visited yet, heading for the door on the way when its path crosses one.
//!
//! Deviations, all from the missing infrastructure rather than from the vanilla logic:
//! * `LandRandomPos.getPos(mob, 15, 7, weight)` scores each of its ten candidates with an
//!   asynchronous village lookup, which `random_pos::land_get_pos` cannot take, so the candidates
//!   come from [`land_get_candidates`] and are scored here (the same split as
//!   `fox_stroll_through_village.rs`). Candidates with a `-inf` weight are never picked, as in
//!   `RandomPos.generateRandomPos`.
//! * `PoiManager.find(... radius 10, IS_OCCUPIED)` and `ServerLevel.isVillage` are answered from
//!   one snapshot of the occupied village POIs around the mob instead of one store query per
//!   candidate; the POI set cannot change within a single `canUse`, so the answers are the same.
//!   `find` is `findFirst` over an unordered stream in vanilla, so "first" is whichever POI the
//!   store yields first.
//! * `ServerLevel.isCloseToVillage` is a constant-time lookup in vanilla, but here it scans the
//!   POI store, and the goal selector polls `canUse` every tick for every idle zombie. A zombie
//!   that is not close to a village therefore waits [`VILLAGE_RECHECK_TICKS`] before asking again,
//!   which a walking zombie cannot outrun by more than a couple of blocks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::drowned_util::is_bright_outside;
use super::interact_with_door::InteractWithDoorGoal;
use super::move_back_to_village::{VillageSectionScan, at_bottom_center_of, section_of};
use super::random_pos::{default_get_pos_towards, land_get_candidates};
use super::{Controls, Goal, GoalFuture};
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::ai::pathfinder::{NavigationKind, NavigatorGoal};
use crate::entity::mob::Mob;
use crate::world::World;
use crate::world::village_poi::{distance_sq, in_sphere};

/// `isCloseToVillage(pos, 6)` (`MoveThroughVillageGoal.java:61`).
const VILLAGE_SECTION_DISTANCE: i32 = 6;
/// `ServerLevel.isVillage(pos)` is `isCloseToVillage(pos, 1)` (`ServerLevel.java:1542-1543`).
const IS_VILLAGE_SECTION_DISTANCE: i32 = 1;
/// `LandRandomPos.getPos(this.mob, 15, 7, ...)` (`MoveThroughVillageGoal.java:65-68`).
const LAND_HORIZONTAL_DIST: i32 = 15;
const LAND_VERTICAL_DIST: i32 = 7;
/// The `find(..., 10, IS_OCCUPIED)` radius (`MoveThroughVillageGoal.java:75, 84`).
const POI_SEARCH_RADIUS: i32 = 10;
/// The farthest a candidate's POI search sphere can reach from the mob: a candidate lies at most
/// `sqrt(15^2 + 7^2 + 15^2)` blocks away, which is under 23, and the search sphere adds 10.
const POI_SNAPSHOT_RADIUS: i32 = 33;
/// `DefaultRandomPos.getPosTowards(this.mob, 10, 7, ...)` (`MoveThroughVillageGoal.java:95`).
const TOWARDS_HORIZONTAL_DIST: i32 = 10;
const TOWARDS_VERTICAL_DIST: i32 = 7;
/// `visited.size() > 15` (`MoveThroughVillageGoal.java:148`).
const MAX_VISITED: usize = 15;
/// How long a "not close to a village" answer is reused; see the module documentation.
const VILLAGE_RECHECK_TICKS: i32 = 20;

pub struct MoveThroughVillageGoal {
    speed: f64,
    path: Option<Path>,
    poi_pos: Option<BlockPos>,
    only_at_night: bool,
    visited: Vec<BlockPos>,
    distance_to_poi: i32,
    /// The `BooleanSupplier canDealWithDoors`; `Zombie.canBreakDoors`.
    can_deal_with_doors: Arc<AtomicBool>,
    /// The mob tick before which the village gate is not asked again.
    next_village_check_tick: i32,
}

impl MoveThroughVillageGoal {
    #[must_use]
    pub fn new(
        speed: f64,
        only_at_night: bool,
        distance_to_poi: i32,
        can_deal_with_doors: Arc<AtomicBool>,
    ) -> Box<Self> {
        Box::new(Self {
            speed,
            path: None,
            poi_pos: None,
            only_at_night,
            visited: Vec::new(),
            distance_to_poi,
            can_deal_with_doors,
            next_village_check_tick: 0,
        })
    }

    /// `hasNotVisited` (`MoveThroughVillageGoal.java:137-145`).
    fn has_not_visited(&self, poi: BlockPos) -> bool {
        !self.visited.contains(&poi)
    }

    /// `updateVisited` (`MoveThroughVillageGoal.java:147-151`).
    fn update_visited(&mut self) {
        if self.visited.len() > MAX_VISITED {
            self.visited.remove(0);
        }
    }

    /// `level.getPoiManager().find(e -> e.is(PoiTypeTags.VILLAGE), this::hasNotVisited, center, 10,
    /// IS_OCCUPIED)`, answered from a snapshot of the occupied village POIs.
    fn find_unvisited_poi(&self, snapshot: &[BlockPos], center: BlockPos) -> Option<BlockPos> {
        snapshot
            .iter()
            .copied()
            .find(|poi| in_sphere(center, *poi, POI_SEARCH_RADIUS) && self.has_not_visited(*poi))
    }

    /// The weight function of `LandRandomPos.getPos` (`MoveThroughVillageGoal.java:69-77`), where
    /// `None` stands for `Double.NEGATIVE_INFINITY`.
    fn village_weight(
        &self,
        scan: &VillageSectionScan,
        snapshot: &[BlockPos],
        candidate: BlockPos,
        mob_pos: BlockPos,
    ) -> Option<f64> {
        if scan.sections_to_village(section_of(candidate)) > IS_VILLAGE_SECTION_DISTANCE {
            return None;
        }
        let poi = self.find_unvisited_poi(snapshot, candidate)?;
        Some(-(distance_sq(poi, mob_pos) as f64))
    }
}

impl MoveThroughVillageGoal {
    /// The path half of `canUse` (`MoveThroughVillageGoal.java:89-117`): a path to the POI, else
    /// to a partial step towards it, redirected to the first wooden door the path crosses.
    async fn plan_path(
        mob: &dyn Mob,
        world: &Arc<World>,
        target: BlockPos,
        can_deal_with_doors: bool,
    ) -> Option<Path> {
        let mob_entity = mob.get_mob_entity();
        // The path is searched on a copy of the navigator, so the live one keeps steering the mob
        // meanwhile. Vanilla brackets each `createPath` with `setCanOpenDoors(canDealWithDoors)`
        // ... `setCanOpenDoors(true)` on the live navigation, which leaves it `true` for good; the
        // copy is brought to the same state at each step and the live navigator gets the final
        // value.
        let mut probe = mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .path_probe();
        probe.set_can_open_doors(can_deal_with_doors);
        let target_pos = Vector3::new(
            f64::from(target.0.x),
            f64::from(target.0.y),
            f64::from(target.0.z),
        );
        let mut path = probe
            .compute_path_with_reach_for_mob(mob, target_pos, 0)
            .await;
        probe.set_can_open_doors(true);
        mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_can_open_doors(true);

        if path.is_none() {
            let partial_step = default_get_pos_towards(
                mob,
                TOWARDS_HORIZONTAL_DIST,
                TOWARDS_VERTICAL_DIST,
                at_bottom_center_of(target),
                std::f64::consts::FRAC_PI_2,
            )?;
            probe.set_can_open_doors(can_deal_with_doors);
            path = probe
                .compute_path_with_reach_for_mob(mob, partial_step, 0)
                .await;
            probe.set_can_open_doors(true);
            path.as_ref()?;
        }

        // Head for the first door the path crosses: `DoorBlock.isWoodenDoor(level, (x, y + 1,
        // z))` on each node (`MoveThroughVillageGoal.java:108-115`).
        let door_node = path.as_ref().and_then(|path| {
            (0..path.get_node_count())
                .filter_map(|index| path.get_node(index))
                .find(|node| {
                    let door_pos = BlockPos::new(node.pos.0.x, node.pos.0.y + 1, node.pos.0.z);
                    InteractWithDoorGoal::is_mob_interactable_door(world, &door_pos)
                })
                .map(|node| node.pos)
        });
        if let Some(node_pos) = door_node {
            path = probe
                .compute_path_with_reach_for_mob(
                    mob,
                    Vector3::new(
                        f64::from(node_pos.0.x),
                        f64::from(node_pos.0.y),
                        f64::from(node_pos.0.z),
                    ),
                    0,
                )
                .await;
        }
        path
    }
}

/// `BlockPos.closerToCenterThan(position, distance)` (`Vec3i.java:197-214`).
fn closer_to_center_than(pos: BlockPos, position: Vector3<f64>, distance: f64) -> bool {
    pos.to_centered_f64().squared_distance_to_vec(&position) < distance * distance
}

/// `GoalUtils.hasGroundPathNavigation` (`GoalUtils.java`).
fn has_ground_navigation(mob: &dyn Mob) -> bool {
    mob.get_mob_entity()
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .navigation_kind()
        == NavigationKind::Ground
}

impl Goal for MoveThroughVillageGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if !has_ground_navigation(mob) {
                return false;
            }

            self.update_visited();
            let mob_entity = mob.get_mob_entity();
            let entity = &mob_entity.living_entity.entity;
            let world = entity.world.load_full();
            if self.only_at_night && is_bright_outside(&world) {
                return false;
            }

            let now = mob_entity.tick_count.load(Ordering::Relaxed);
            if now < self.next_village_check_tick {
                return false;
            }
            let pos = entity.block_pos.load();
            // One snapshot serves `isCloseToVillage(pos, 6)`, every candidate's `isVillage` and
            // every POI `find`: the candidates lie within one section of the mob.
            let scan = VillageSectionScan::around(&world, pos, 1).await;
            if scan.sections_to_village(section_of(pos)) > VILLAGE_SECTION_DISTANCE {
                self.next_village_check_tick = now + VILLAGE_RECHECK_TICKS;
                return false;
            }
            let snapshot = world
                .village_poi_positions_in_range(pos, POI_SNAPSHOT_RADIUS)
                .await;

            // `RandomPos.generateRandomPos`: the highest weight wins, `-inf` never does.
            let mut best: Option<(f64, BlockPos)> = None;
            for candidate in land_get_candidates(mob, LAND_HORIZONTAL_DIST, LAND_VERTICAL_DIST) {
                let Some(weight) = self.village_weight(&scan, &snapshot, candidate, pos) else {
                    continue;
                };
                if best.is_none_or(|(best_weight, _)| weight > best_weight) {
                    best = Some((weight, candidate));
                }
            }
            let Some((_, land_pos)) = best else {
                return false;
            };

            let Some(target) = self.find_unvisited_poi(&snapshot, land_pos) else {
                return false;
            };
            self.poi_pos = Some(target);

            let can_deal_with_doors = self.can_deal_with_doors.load(Ordering::Relaxed);
            self.path = Self::plan_path(mob, &world, target, can_deal_with_doors).await;
            self.path.is_some()
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            let navigation_done = mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle();
            if navigation_done {
                return false;
            }
            let Some(poi) = self.poi_pos else {
                return false;
            };
            let entity = &mob_entity.living_entity.entity;
            !closer_to_center_than(
                poi,
                entity.pos.load(),
                f64::from(entity.width()) + f64::from(self.distance_to_poi),
            )
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(path) = self.path.take() else {
                return;
            };
            let target = path.get_target();
            let destination = Vector3::new(
                f64::from(target.x),
                f64::from(target.y),
                f64::from(target.z),
            );
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_path(
                    NavigatorGoal::new(mob.get_entity().pos.load(), destination, self.speed),
                    path,
                );
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(poi) = self.poi_pos else {
                return;
            };
            let navigation_done = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle();
            if navigation_done
                || closer_to_center_than(
                    poi,
                    mob.get_entity().pos.load(),
                    f64::from(self.distance_to_poi),
                )
            {
                self.visited.push(poi);
            }
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal() -> MoveThroughVillageGoal {
        *MoveThroughVillageGoal::new(1.0, true, 4, Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn visited_pois_are_not_offered_again() {
        let mut goal = goal();
        let poi = BlockPos::new(10, 64, 10);
        let other = BlockPos::new(12, 64, 10);
        goal.visited.push(poi);
        assert!(!goal.has_not_visited(poi));
        assert!(goal.has_not_visited(other));
        // `find` skips the visited POI and takes the next one in range.
        assert_eq!(
            goal.find_unvisited_poi(&[poi, other], BlockPos::new(11, 64, 10)),
            Some(other)
        );
    }

    #[test]
    fn visited_list_drops_its_oldest_entry_past_fifteen() {
        let mut goal = goal();
        for x in 0..=16 {
            goal.visited.push(BlockPos::new(x, 64, 0));
        }
        goal.update_visited();
        assert_eq!(goal.visited.len(), 16);
        assert_eq!(goal.visited[0], BlockPos::new(1, 64, 0));
        // Fifteen entries or fewer are left alone.
        goal.visited.truncate(15);
        goal.update_visited();
        assert_eq!(goal.visited.len(), 15);
    }

    #[test]
    fn poi_search_is_a_ten_block_sphere() {
        let goal = goal();
        let center = BlockPos::new(0, 64, 0);
        let inside = BlockPos::new(10, 64, 0);
        let outside = BlockPos::new(8, 64, 8);
        assert_eq!(
            goal.find_unvisited_poi(&[outside, inside], center),
            Some(inside)
        );
        assert_eq!(goal.find_unvisited_poi(&[outside], center), None);
    }

    #[test]
    fn closer_to_center_than_measures_from_the_block_centre() {
        let poi = BlockPos::new(0, 64, 0);
        // The centre is (0.5, 64.5, 0.5): 4.5 away on X is inside 5.0 but not inside 4.0.
        let position = Vector3::new(5.0, 64.5, 0.5);
        assert!(closer_to_center_than(poi, position, 5.0));
        assert!(!closer_to_center_than(poi, position, 4.0));
        // Strictly closer: exactly the distance does not count.
        assert!(!closer_to_center_than(poi, position, 4.5));
    }
}
