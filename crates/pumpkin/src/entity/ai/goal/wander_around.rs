use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use rand::RngExt;
use std::sync::atomic::Ordering::Relaxed;

pub struct WanderAroundGoal {
    goal_control: Controls,
    speed: f64,
    target: Option<Vector3<f64>>,
    chance: i32,
    /// Vanilla: `WaterAvoidingRandomStrollGoal` overrides `getPosition()` to reject candidate
    /// positions inside a liquid.
    avoid_water: bool,
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
            avoid_water: false,
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
            avoid_water: true,
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

    fn find_default_wander_target(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let entity = &mob.get_mob_entity().living_entity.entity;
        let pos = entity.pos.load();
        let world = entity.world.load();
        let mut rng = mob.get_random();
        let origin = BlockPos::new(
            pos.x.floor() as i32,
            pos.y.floor() as i32,
            pos.z.floor() as i32,
        );
        let horizontal_dist = 10;
        let min_y = world.dimension.min_y;
        let max_y = min_y + world.dimension.height - 1;
        let mut best: Option<(f64, BlockPos)> = None;

        for _ in 0..10 {
            let candidate = BlockPos::new(
                origin.0.x + rng.random_range(-horizontal_dist..=horizontal_dist),
                origin.0.y + rng.random_range(-7..=7),
                origin.0.z + rng.random_range(-horizontal_dist..=horizontal_dist),
            );
            if candidate.0.y < min_y
                || candidate.0.y > max_y
                || !Self::is_within_restriction(mob, &candidate, horizontal_dist)
            {
                continue;
            }

            let navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !navigator.is_stable_destination(&world, &candidate)
                || navigator.has_pathfinding_malus(&world, &candidate)
            {
                continue;
            }

            let weight = mob.get_walk_target_value(&candidate);
            if best.is_none_or(|(best_weight, _)| weight > best_weight) {
                best = Some((weight, candidate));
            }
        }

        best.map(|(_, candidate)| {
            Vector3::new(
                candidate.0.x as f64 + 0.5,
                candidate.0.y as f64,
                candidate.0.z as f64 + 0.5,
            )
        })
    }

    fn is_within_restriction(mob: &dyn Mob, candidate: &BlockPos, horizontal_dist: i32) -> bool {
        let Some(home) = mob.get_home() else {
            return true;
        };
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let range = mob.get_mob_entity().position_target_range.load(Relaxed);
        if range < 0 {
            return true;
        }
        let dy = pos.y - (f64::from(home.0.y) + 0.5);
        let dx = pos.x - (f64::from(home.0.x) + 0.5);
        let dz = pos.z - (f64::from(home.0.z) + 0.5);
        let max_distance = f64::from(range + horizontal_dist + 1);
        if dx * dx + dy * dy + dz * dz >= max_distance * max_distance {
            return true;
        }
        let candidate_dy = f64::from(candidate.0.y - home.0.y);
        let candidate_dx = f64::from(candidate.0.x - home.0.x);
        let candidate_dz = f64::from(candidate.0.z - home.0.z);
        candidate_dx * candidate_dx + candidate_dy * candidate_dy + candidate_dz * candidate_dz
            < f64::from(range) * f64::from(range)
    }

    /// Vanilla `LandRandomPos.getPos(mob, horizontalDist, 7)`.
    fn find_land_wander_target(mob: &dyn Mob, horizontal_dist: i32) -> Option<Vector3<f64>> {
        let entity = &mob.get_mob_entity().living_entity.entity;
        let pos = entity.pos.load();
        let world = entity.world.load();
        let origin = BlockPos::new(
            pos.x.floor() as i32,
            pos.y.floor() as i32,
            pos.z.floor() as i32,
        );
        let min_y = world.dimension.min_y;
        let max_y = min_y + world.dimension.height - 1;
        let mut rng = mob.get_random();
        let mut best: Option<(f64, BlockPos)> = None;

        for _ in 0..10 {
            let mut candidate = BlockPos::new(
                origin.0.x + rng.random_range(-horizontal_dist..=horizontal_dist),
                origin.0.y + rng.random_range(-7..=7),
                origin.0.z + rng.random_range(-horizontal_dist..=horizontal_dist),
            );

            if candidate.0.y < min_y
                || candidate.0.y > max_y
                || !Self::is_within_restriction(mob, &candidate, horizontal_dist)
                || !mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_stable_destination(&world, &candidate)
            {
                continue;
            }

            while candidate.0.y <= max_y && world.get_block_state(&candidate).is_solid() {
                candidate = candidate.up();
            }

            if candidate.0.y > max_y
                || world
                    .get_fluid(&candidate)
                    .has_tag(&tag::Fluid::MINECRAFT_WATER)
            {
                continue;
            }

            let navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if navigator.has_pathfinding_malus(&world, &candidate) {
                continue;
            }

            let weight = mob.get_walk_target_value(&candidate);
            if best.is_none_or(|(best_weight, _)| weight > best_weight) {
                best = Some((weight, candidate));
            }
        }

        best.map(|(_, pos)| {
            Vector3::new(pos.0.x as f64 + 0.5, pos.0.y as f64, pos.0.z as f64 + 0.5)
        })
    }
}

impl Goal for WanderAroundGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if mob.get_mob_entity().no_action_time.load(Relaxed) >= 100
                || mob.has_controlling_passenger().await
            {
                return false;
            }

            if mob.get_random().random_range(0..self.chance) != 0 {
                return false;
            }

            if self.avoid_water {
                let in_water = mob.get_entity().touching_water.load(Relaxed);
                self.target = if in_water {
                    Self::find_land_wander_target(mob, 15)
                        .or_else(|| Self::find_default_wander_target(mob))
                } else if mob.get_random().random::<f32>() >= 0.001 {
                    Self::find_land_wander_target(mob, 10)
                } else {
                    Self::find_default_wander_target(mob)
                };
                return self.target.is_some();
            }

            self.target = Self::find_default_wander_target(mob);
            self.target.is_some()
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
            if let Some(target) = self.target {
                let pos = mob.get_mob_entity().living_entity.entity.pos.load();
                let mut navigator = mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                navigator.set_progress(NavigatorGoal::new(pos, target, self.speed));
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.target = None;
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
    }
}
