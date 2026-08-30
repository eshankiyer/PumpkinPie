use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::entity::EntityType;

use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::{EntityBase, ai::pathfinder::NavigatorGoal, mob::Mob};

const SEARCH_RADIUS: f64 = 8.0;
const MIN_DISTANCE_SQ: f64 = 9.0;
const MAX_DISTANCE_SQ: f64 = 256.0;

const SEARCH_Y_RANGE: f64 = 4.0;

// `AbstractHorse.followMommy` accepts the horse-family parent types (`AbstractHorse.java:561-568`).
const fn is_horse_family(entity_type: &EntityType) -> bool {
    let id = entity_type.id;
    id == EntityType::HORSE.id
        || id == EntityType::DONKEY.id
        || id == EntityType::MULE.id
        || id == EntityType::SKELETON_HORSE.id
        || id == EntityType::ZOMBIE_HORSE.id
}

pub struct FollowParentGoal {
    speed: f64,
    parent: Option<Arc<dyn EntityBase>>,
    delay: i32,
    horse_family: bool,
}

impl FollowParentGoal {
    #[must_use]
    pub fn new(speed: f64) -> Self {
        Self {
            speed,
            parent: None,
            delay: 0,
            horse_family: false,
        }
    }

    /// `AbstractHorse.followMommy` searches all bred adult horse-family entities, not only the
    /// baby's exact entity type (`AbstractHorse.java:561-568`).
    #[must_use]
    pub fn new_horse(speed: f64) -> Self {
        Self {
            speed,
            parent: None,
            delay: 0,
            horse_family: true,
        }
    }

    fn find_parent(mob: &dyn Mob, horse_family: bool) -> Option<Arc<dyn EntityBase>> {
        let mob_entity = mob.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        let pos = entity.pos.load();
        let my_type = entity.entity_type;
        let world = entity.world.load();

        let nearby =
            world.get_nearby_entities(pos, if horse_family { 16.0 } else { SEARCH_RADIUS });
        let mut closest: Option<(f64, Arc<dyn EntityBase>)> = None;

        for candidate in nearby.values() {
            let c_entity = candidate.get_entity();
            let same_horse_family = is_horse_family(c_entity.entity_type);
            if horse_family {
                if !same_horse_family
                    || c_entity.age.load(Relaxed) < 0
                    || !candidate.get_mob().is_some_and(|mob| mob.is_bred())
                {
                    continue;
                }
            } else if c_entity.entity_type != my_type {
                continue;
            } else if c_entity.age.load(Relaxed) < 0 {
                continue;
            }
            let c_pos = c_entity.pos.load();
            if (pos.y - c_pos.y).abs() > SEARCH_Y_RANGE {
                continue;
            }
            let dist_sq = pos.squared_distance_to_vec(&c_pos);
            if closest.as_ref().is_none_or(|(d, _)| dist_sq <= *d) {
                closest = Some((dist_sq, candidate.clone()));
            }
        }

        let (dist_sq, closest_entity) = closest?;
        if dist_sq < MIN_DISTANCE_SQ {
            return None;
        }
        Some(closest_entity)
    }
}

#[cfg(test)]
mod tests {
    use super::is_horse_family;
    use pumpkin_data::entity::EntityType;

    /// `AbstractHorse.followMommy` targets `AbstractHorse.class`, not the baby's exact class
    /// (`AbstractHorse.java:561-568`).
    #[test]
    fn horse_parent_search_accepts_every_horse_family_type() {
        assert!(is_horse_family(&EntityType::HORSE));
        assert!(is_horse_family(&EntityType::DONKEY));
        assert!(is_horse_family(&EntityType::MULE));
        assert!(is_horse_family(&EntityType::SKELETON_HORSE));
        assert!(is_horse_family(&EntityType::ZOMBIE_HORSE));
        assert!(!is_horse_family(&EntityType::PIG));
    }
}

impl Goal for FollowParentGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let age = mob.get_mob_entity().living_entity.entity.age.load(Relaxed);
            if age >= 0 {
                return false;
            }
            if self.horse_family && !mob.is_bred() {
                return false;
            }
            self.parent = Self::find_parent(mob, self.horse_family);
            self.parent.is_some()
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let age = mob.get_mob_entity().living_entity.entity.age.load(Relaxed);
            if age >= 0 {
                return false;
            }
            let Some(parent) = &self.parent else {
                return false;
            };
            let parent_entity = parent.get_entity();
            if !parent_entity.is_alive() {
                return false;
            }
            let mob_pos = mob.get_mob_entity().living_entity.entity.pos.load();
            let parent_pos = parent_entity.pos.load();
            let dist_sq = mob_pos.squared_distance_to_vec(&parent_pos);
            (MIN_DISTANCE_SQ..=MAX_DISTANCE_SQ).contains(&dist_sq)
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.delay = 0;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.delay -= 1;
            if self.delay > 0 {
                return;
            }
            self.delay = to_goal_ticks(10);
            if let Some(parent) = &self.parent {
                let mob_pos = mob.get_mob_entity().living_entity.entity.pos.load();
                let parent_pos = parent.get_entity().pos.load();
                let mut navigator = mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                navigator.set_progress(NavigatorGoal::new(mob_pos, parent_pos, self.speed));
            }
        })
    }

    fn stop<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.parent = None;
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
