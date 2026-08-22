use pumpkin_data::entity::EntityType;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::move_back_to_village::{
    VillageSectionScan, at_bottom_center_of, section_center, section_cube, section_of,
};
use super::random_pos::{land_get_pos, land_get_pos_towards};
use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::passive::villager::VillagerEntity;
use crate::world::village_poi::{Occupancy, in_sphere};

/// Vanilla `GolemRandomStrollInVillageGoal` (`GolemRandomStrollInVillageGoal.java`).
///
/// A `RandomStrollGoal` subclass registered on `IronGolem` at priority 4
/// (`IronGolem.java:71`, `new GolemRandomStrollInVillageGoal(this, 0.6)`), replacing the plain
/// stroll goal entirely: instead of wandering uniformly, the golem biases its destination
/// towards villagers that want a golem and towards occupied POIs inside the village, which is
/// why a village golem patrols the village rather than drifting out of it.
///
/// `getPosition` (`GolemRandomStrollInVillageGoal.java:28-49`) is a three-way roll:
/// 30% pure random, otherwise villager-first with a POI fallback (70%) or POI-first with a
/// villager fallback (30%), and pure random if both come back empty.
pub struct GolemRandomStrollInVillageGoal {
    goal_control: Controls,
    speed: f64,
    chance: i32,
    force_trigger: bool,
    wanted: Option<Vector3<f64>>,
}

impl GolemRandomStrollInVillageGoal {
    /// `POI_SECTION_SCAN_RADIUS` / `VILLAGER_SCAN_RADIUS` / `RANDOM_POS_XY_DISTANCE` /
    /// `RANDOM_POS_Y_DISTANCE` (`GolemRandomStrollInVillageGoal.java:19-22`).
    const POI_SECTION_SCAN_RADIUS: i32 = 2;
    const VILLAGER_SCAN_RADIUS: f64 = 32.0;
    const RANDOM_POS_XY_DISTANCE: i32 = 10;
    const RANDOM_POS_Y_DISTANCE: i32 = 7;
    /// `super(mob, speedModifier, 240, false)` (`GolemRandomStrollInVillageGoal.java:25`) -
    /// note `checkNoActionTime = false`, so unlike a plain `RandomStrollGoal` this keeps
    /// running for a golem that has been idle a long time.
    const INTERVAL: i32 = 240;
    /// `PoiManager.getInRange(..., sectionPos.center(), 8, IS_OCCUPIED)`
    /// (`GolemRandomStrollInVillageGoal.java:88`).
    const POI_SEARCH_RADIUS: i32 = 8;

    #[must_use]
    pub fn new(speed: f64) -> Box<Self> {
        Box::new(Self {
            goal_control: Controls::MOVE,
            speed,
            chance: to_goal_ticks(Self::INTERVAL),
            force_trigger: false,
            wanted: None,
        })
    }

    /// `getPositionTowardsAnywhere` (`GolemRandomStrollInVillageGoal.java:51-53`).
    fn position_towards_anywhere(mob: &dyn Mob) -> Option<Vector3<f64>> {
        land_get_pos(
            mob,
            Self::RANDOM_POS_XY_DISTANCE,
            Self::RANDOM_POS_Y_DISTANCE,
        )
    }

    /// `getPositionTowardsVillagerWhoWantsGolem` (`GolemRandomStrollInVillageGoal.java:55-65`).
    async fn position_towards_villager_who_wants_golem(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let entity = &mob.get_mob_entity().living_entity.entity;
        let world = entity.world.load();
        let world_age = world.get_world_age().await;
        let box_ = entity.bounding_box.load().expand(
            Self::VILLAGER_SCAN_RADIUS,
            Self::VILLAGER_SCAN_RADIUS,
            Self::VILLAGER_SCAN_RADIUS,
        );

        let mut wanting = Vec::new();
        for candidate in world.get_entities_at_box(&box_) {
            if candidate.get_entity().entity_type != &EntityType::VILLAGER {
                continue;
            }
            let wants = candidate
                .cast_any()
                .downcast_ref::<VillagerEntity>()
                .is_some_and(|villager| villager.wants_to_spawn_golem(world_age));
            if wants {
                wanting.push(candidate);
            }
        }
        if wanting.is_empty() {
            return None;
        }

        let index = mob.get_random().random_range(0..wanting.len());
        let target_pos = wanting[index].get_entity().pos.load();
        land_get_pos_towards(
            mob,
            Self::RANDOM_POS_XY_DISTANCE,
            Self::RANDOM_POS_Y_DISTANCE,
            target_pos,
        )
    }

    /// `getRandomVillageSection` (`GolemRandomStrollInVillageGoal.java:77-83`) plus
    /// `getRandomPoiWithinSection` (lines 85-92) plus `getPositionTowardsPoi` (lines 67-75).
    async fn position_towards_poi(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let entity = &mob.get_mob_entity().living_entity.entity;
        let world = entity.world.load();
        let pos = entity.block_pos.load();
        let here = section_of(pos);

        let scan = VillageSectionScan::around(&world, pos, Self::POI_SECTION_SCAN_RADIUS).await;
        let village_sections: Vec<Vector3<i32>> = section_cube(here, Self::POI_SECTION_SCAN_RADIUS)
            .into_iter()
            .filter(|section| scan.sections_to_village(*section) == 0)
            .collect();
        if village_sections.is_empty() {
            return None;
        }
        let section = village_sections[mob.get_random().random_range(0..village_sections.len())];

        // Vanilla's predicate here is `poiType -> true`, i.e. *any* POI type, not just the
        // village tag - hence the `None` type filter. `IS_OCCUPIED` still excludes
        // non-claimable types such as nether portals, whose `maxTickets` is 0, so
        // `freeTickets != maxTickets` can never hold for them.
        let center = section_center(section);
        let candidates: Vec<BlockPos> = {
            let mut storage = world.portal_poi.lock().await;
            storage
                .get_in_square_with_tickets(center, Self::POI_SEARCH_RADIUS, None)
                .into_iter()
                .filter(|(candidate, free, max)| {
                    in_sphere(center, *candidate, Self::POI_SEARCH_RADIUS)
                        && Occupancy::IsOccupied.matches(*free, *max)
                })
                .map(|(candidate, _, _)| candidate)
                .collect()
        };
        if candidates.is_empty() {
            return None;
        }

        let target = candidates[mob.get_random().random_range(0..candidates.len())];
        land_get_pos_towards(
            mob,
            Self::RANDOM_POS_XY_DISTANCE,
            Self::RANDOM_POS_Y_DISTANCE,
            at_bottom_center_of(target),
        )
    }

    /// `GolemRandomStrollInVillageGoal.getPosition` (`GolemRandomStrollInVillageGoal.java:28-49`).
    async fn get_position(mob: &dyn Mob) -> Option<Vector3<f64>> {
        if mob.get_random().random::<f32>() < 0.3 {
            return Self::position_towards_anywhere(mob);
        }

        let target = if mob.get_random().random::<f32>() < 0.7 {
            match Self::position_towards_villager_who_wants_golem(mob).await {
                Some(target) => Some(target),
                None => Self::position_towards_poi(mob).await,
            }
        } else {
            match Self::position_towards_poi(mob).await {
                Some(target) => Some(target),
                None => Self::position_towards_villager_who_wants_golem(mob).await,
            }
        };

        target.map_or_else(|| Self::position_towards_anywhere(mob), Some)
    }
}

impl Goal for GolemRandomStrollInVillageGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            // `RandomStrollGoal.canUse` (`RandomStrollGoal.java:36-62`), with
            // `checkNoActionTime = false` so the `no_action_time` branch never applies.
            if mob.has_controlling_passenger().await {
                return false;
            }
            if !self.force_trigger && mob.get_random().random_range(0..self.chance) != 0 {
                return false;
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
    fn interval_matches_vanilla() {
        let goal = GolemRandomStrollInVillageGoal::new(0.6);
        assert_eq!(goal.chance, to_goal_ticks(240));
        assert_eq!(goal.chance, 120);
    }

    #[test]
    fn a_non_claimable_poi_never_counts_as_occupied() {
        // Nether portals register with `max_tickets = 0`, so vanilla's `IS_OCCUPIED`
        // (`freeTickets != maxTickets`) can never hold for one - which is what makes the
        // "any POI type" scan here safe without a type filter.
        assert!(!Occupancy::IsOccupied.matches(0, 0));
    }
}
