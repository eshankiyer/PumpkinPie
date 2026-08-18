use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob};
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use std::sync::atomic::Ordering;

pub struct WanderAroundGoal {
    goal_control: Controls,
    speed: f64,
    target: Option<Vector3<f64>>,
    chance: i32,
    force_trigger: bool,
    /// Vanilla: `WaterAvoidingRandomStrollGoal` overrides `getPosition()` to reject candidate
    /// positions inside a liquid.
    avoid_water: bool,
    probability: f32,
}

impl WanderAroundGoal {
    /// Vanilla: `RandomStrollGoal.DEFAULT_INTERVAL` (`RandomStrollGoal.java`).
    const DEFAULT_INTERVAL: i32 = 120;

    #[must_use]
    pub const fn new(speed: f64) -> Self {
        Self::new_with_interval(speed, Self::DEFAULT_INTERVAL)
    }

    /// Vanilla: `RandomStrollGoal(mob, speedModifier, interval)` / `RandomSwimmingGoal`, whose
    /// callers pass a mob-specific interval instead of the 120-tick default.
    #[must_use]
    pub const fn new_with_interval(speed: f64, interval: i32) -> Self {
        Self {
            goal_control: Controls::MOVE,
            speed,
            target: None,
            chance: to_goal_ticks(interval),
            force_trigger: false,
            avoid_water: false,
            probability: 0.0,
        }
    }

    /// Vanilla: `WaterAvoidingRandomStrollGoal`.
    #[must_use]
    pub const fn new_water_avoiding(speed: f64) -> Self {
        Self {
            goal_control: Controls::MOVE,
            speed,
            target: None,
            chance: to_goal_ticks(Self::DEFAULT_INTERVAL),
            force_trigger: false,
            avoid_water: true,
            probability: 0.001,
        }
    }

    /// Vanilla `WaterAvoidingRandomStrollGoal(mob, speed, probability)`.
    #[must_use]
    pub const fn new_water_avoiding_with_probability(speed: f64, probability: f32) -> Self {
        Self {
            goal_control: Controls::MOVE,
            speed,
            target: None,
            chance: to_goal_ticks(Self::DEFAULT_INTERVAL),
            force_trigger: false,
            avoid_water: true,
            probability,
        }
    }

    /// Vanilla: `RandomStrollGoal#setInterval`, e.g. `ElderGuardian`'s constructor
    /// overriding its inherited `randomStrollGoal` interval from 80 to 400.
    #[must_use]
    pub const fn with_interval(mut self, interval: i32) -> Self {
        self.chance = to_goal_ticks(interval);
        self
    }

    /// Whether this is vanilla's `WaterAvoidingRandomStrollGoal` rather than the plain
    /// `RandomStrollGoal`.
    #[must_use]
    pub const fn avoids_water(&self) -> bool {
        self.avoid_water
    }

    /// Vanilla `RandomStrollGoal.trigger` bypasses the interval for the next attempt.
    pub const fn trigger(&mut self) {
        self.force_trigger = true;
    }

    fn is_within_home(mob: &dyn Mob, pos: &BlockPos) -> bool {
        let mob_entity = mob.get_mob_entity();
        let radius = mob_entity.position_target_range.load(Ordering::Relaxed);
        if radius == -1 {
            return true;
        }
        let home = mob_entity.position_target.load();
        let dx = f64::from(home.0.x - pos.0.x);
        let dy = f64::from(home.0.y - pos.0.y);
        let dz = f64::from(home.0.z - pos.0.z);
        let radius_squared = radius.wrapping_mul(radius);
        dx.mul_add(dx, dy.mul_add(dy, dz * dz)) < f64::from(radius_squared)
    }

    /// Mirrors vanilla `DefaultRandomPos` and `LandRandomPos` for the two random-stroll goals.
    /// The client-visible result is the selected bottom-center block, not an unchecked offset.
    fn find_random_target(
        mob: &dyn Mob,
        horizontal_range: i32,
        vertical_range: i32,
        land_only: bool,
    ) -> Option<Vector3<f64>> {
        let entity = &mob.get_mob_entity().living_entity.entity;
        let origin = entity.pos.load();
        let world = entity.world.load();
        let mob_entity = mob.get_mob_entity();
        let home = mob_entity.position_target.load();
        let home_radius = mob_entity.position_target_range.load(Ordering::Relaxed);
        let has_home = home_radius != -1;
        let restrict = has_home && {
            let dx = f64::from(home.0.x) + 0.5 - origin.x;
            let dy = f64::from(home.0.y) + 0.5 - origin.y;
            let dz = f64::from(home.0.z) + 0.5 - origin.z;
            let radius = f64::from(home_radius) + f64::from(horizontal_range) + 1.0;
            dx.mul_add(dx, dy.mul_add(dy, dz * dz)) < radius * radius
        };
        let mut random = mob.get_random();
        let mut best = None;
        let mut best_weight = f64::NEG_INFINITY;

        for _ in 0..10 {
            let dx = random.random_range(-horizontal_range..=horizontal_range);
            let dy = random.random_range(-vertical_range..=vertical_range);
            let dz = random.random_range(-horizontal_range..=horizontal_range);
            let (dx, dz) = if has_home && horizontal_range > 1 {
                let x_bias = random.random_range(0.0..(f64::from(horizontal_range) / 2.0));
                let z_bias = random.random_range(0.0..(f64::from(horizontal_range) / 2.0));
                (
                    f64::from(dx)
                        + if origin.x > f64::from(home.0.x) {
                            -x_bias
                        } else {
                            x_bias
                        },
                    f64::from(dz)
                        + if origin.z > f64::from(home.0.z) {
                            -z_bias
                        } else {
                            z_bias
                        },
                )
            } else {
                (f64::from(dx), f64::from(dz))
            };
            let candidate = BlockPos::new(
                (origin.x + dx).floor() as i32,
                (origin.y + f64::from(dy)).floor() as i32,
                (origin.z + dz).floor() as i32,
            );

            if !(world.get_bottom_y()..=world.get_top_y()).contains(&candidate.0.y)
                || (restrict && !Self::is_within_home(mob, &candidate))
            {
                continue;
            }

            let navigator = mob_entity.navigator.lock().unwrap();
            if !navigator.is_stable_destination(&world, &candidate) {
                continue;
            }
            let candidate_has_malus = navigator.has_pathfinding_malus(&world, &candidate);
            drop(navigator);

            let mut landing = candidate;
            if land_only {
                while landing.0.y <= world.get_top_y() && world.get_block_state(&landing).is_solid()
                {
                    landing = landing.up();
                }
                if world
                    .get_fluid(&landing)
                    .has_tag(&pumpkin_data::tag::Fluid::MINECRAFT_WATER)
                {
                    continue;
                }
                let navigator = mob_entity.navigator.lock().unwrap();
                if navigator.has_pathfinding_malus(&world, &landing) {
                    continue;
                }
            } else if candidate_has_malus {
                continue;
            }

            let weight = mob.get_walk_target_value(&landing);
            if weight > best_weight {
                best_weight = weight;
                best = Some(Vector3::new(
                    f64::from(landing.0.x) + 0.5,
                    f64::from(landing.0.y),
                    f64::from(landing.0.z) + 0.5,
                ));
            }
        }

        best
    }
}

impl Goal for WanderAroundGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if mob.has_controlling_passenger().await {
                return false;
            }

            if mob.get_mob_entity().is_schooling_follower().await {
                return false;
            }

            if !self.force_trigger {
                if mob.get_mob_entity().no_action_time.load(Ordering::Relaxed) >= 100 {
                    return false;
                }

                if mob.get_random().random_range(0..self.chance) != 0 {
                    return false;
                }
            }

            if self.avoid_water {
                let in_water = mob.get_entity().was_touching_water.load(Ordering::Relaxed);
                self.target = if in_water {
                    Self::find_random_target(mob, 15, 7, true)
                        .or_else(|| Self::find_random_target(mob, 10, 7, false))
                } else if mob.get_random().random::<f32>() >= self.probability {
                    Self::find_random_target(mob, 10, 7, true)
                } else {
                    Self::find_random_target(mob, 10, 7, false)
                };
            } else {
                self.target = Self::find_random_target(mob, 10, 7, false);
            }
            if self.target.is_some() {
                self.force_trigger = false;
                true
            } else {
                false
            }
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let navigating = {
                let navigator = mob.get_mob_entity().navigator.lock().unwrap();
                !navigator.is_idle()
            };
            navigating && !mob.has_controlling_passenger().await
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(target) = self.target {
                let pos = mob.get_mob_entity().living_entity.entity.pos.load();
                let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
                navigator.set_progress(NavigatorGoal::new(pos, target, self.speed));
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.target = None;
            mob.get_mob_entity().navigator.lock().unwrap().stop();
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
    fn with_interval_overrides_default_chance() {
        let default_goal = WanderAroundGoal::new(1.0);
        let overridden = WanderAroundGoal::new(1.0).with_interval(400);
        assert_ne!(default_goal.chance, overridden.chance);
        // Vanilla `ElderGuardian`'s override: `randomStrollGoal.setInterval(400)`.
        assert_eq!(overridden.chance, 200);
    }

    #[test]
    fn default_interval_matches_vanilla() {
        let goal = WanderAroundGoal::new(1.0);
        assert_eq!(goal.chance, to_goal_ticks(120));
    }

    #[test]
    fn custom_interval_is_applied() {
        // Fish family: AbstractFish.FishSwimGoal uses interval 40.
        let goal = WanderAroundGoal::new_with_interval(1.0, 40);
        assert_eq!(goal.chance, to_goal_ticks(40));
        assert_ne!(goal.chance, to_goal_ticks(120));
    }

    #[test]
    fn water_avoiding_keeps_default_interval() {
        let goal = WanderAroundGoal::new_water_avoiding(0.4);
        assert_eq!(goal.chance, to_goal_ticks(120));
        assert!(goal.avoid_water);
        assert_eq!(goal.probability, 0.001);
    }

    #[test]
    fn water_avoiding_probability_is_configurable() {
        let goal = WanderAroundGoal::new_water_avoiding_with_probability(0.4, 0.00001);
        assert_eq!(goal.probability, 0.00001);
    }
}
