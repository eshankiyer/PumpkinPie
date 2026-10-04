use super::{Controls, Goal, GoalFuture, random_pos, to_goal_ticks};
use crate::block::pathfindable::{PathComputationType, is_pathfindable};
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob};
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use std::sync::atomic::Ordering;

/// Target search and run condition of one vanilla brain `RandomStroll` behaviour
/// (`RandomStroll.java:23-41`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrainStrollKind {
    /// `RandomStroll.stroll`: `LandRandomPos.getPos(body, horizontal, vertical)`, skipped while
    /// in water unless `may_stroll_from_water`.
    Land {
        horizontal: i32,
        vertical: i32,
        may_stroll_from_water: bool,
    },
    /// `RandomStroll.swim`: `getTargetSwimPos`, only while in water.
    Swim,
}

/// One `RandomStroll` entry of a brain `GateBehavior`/`RunOne`, with its speed modifier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrainStroll {
    pub kind: BrainStrollKind,
    pub speed: f64,
}

impl BrainStroll {
    /// `RandomStroll.stroll(speed)` (`RandomStroll.java:23-25`).
    #[must_use]
    pub const fn land(speed: f64) -> Self {
        Self::land_range(speed, 10, 7)
    }

    /// `RandomStroll.stroll(speed, false)` (`RandomStroll.java:27-29`).
    #[must_use]
    pub const fn land_not_from_water(speed: f64) -> Self {
        Self {
            kind: BrainStrollKind::Land {
                horizontal: 10,
                vertical: 7,
                may_stroll_from_water: false,
            },
            speed,
        }
    }

    /// `RandomStroll.stroll(speed, maxHorizontal, maxVertical)` (`RandomStroll.java:31-33`).
    #[must_use]
    pub const fn land_range(speed: f64, horizontal: i32, vertical: i32) -> Self {
        Self {
            kind: BrainStrollKind::Land {
                horizontal,
                vertical,
                may_stroll_from_water: true,
            },
            speed,
        }
    }

    /// `RandomStroll.swim(speed)` (`RandomStroll.java:39-41`).
    #[must_use]
    pub const fn swim(speed: f64) -> Self {
        Self {
            kind: BrainStrollKind::Swim,
            speed,
        }
    }
}

pub struct WanderAroundGoal {
    goal_control: Controls,
    speed: f64,
    target: Option<Vector3<f64>>,
    chance: i32,
    force_trigger: bool,
    /// Vanilla: `WaterAvoidingRandomStrollGoal` overrides `getPosition()` to reject candidate
    /// positions inside a liquid.
    avoid_water: bool,
    /// Vanilla: `RandomSwimmingGoal.getPosition()` keeps only positions that are pathfindable
    /// for WATER through `BehaviorUtils.getRandomSwimmablePos`.
    swim_only: bool,
    probability: f32,
    /// Vanilla brain `RandomStroll` behaviours tried in order (`RandomStroll.strollFlyOrSwim`,
    /// `RandomStroll.java:43-55`); empty for the `RandomStrollGoal` family.
    brain_strolls: &'static [BrainStroll],
    /// Target search of the flying `RandomStrollGoal` subclasses.
    flight: FlightSearch,
}

/// Which flying `getPosition()` override, if any, replaces the ground target search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FlightSearch {
    None,
    /// `WaterAvoidingRandomFlyingGoal.getPosition` (`WaterAvoidingRandomFlyingGoal.java:14-22`).
    WaterAvoidingFlying,
    /// `Parrot.ParrotWanderGoal.getPosition` (`Parrot.java:478-488`).
    ParrotWander,
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
            swim_only: false,
            probability: 0.0,
            brain_strolls: &[],
            flight: FlightSearch::None,
        }
    }

    /// Vanilla brain `RandomStroll.stroll`/`swim` behaviours (`RandomStroll.java:23-55`), tried
    /// in order like the siblings of an `ORDERED`/`TRY_ALL` `GateBehavior`: an entry whose run
    /// condition fails, or that finds no target, leaves `WALK_TARGET` absent so the next one
    /// runs. Unlike `RandomStrollGoal` there is no `noActionTime` gate.
    ///
    /// `interval` stands in for the brain's cadence: 1 where the stroll sits in a gate with no
    /// `DoNothing` sibling (it is retried every brain tick while `WALK_TARGET` is absent).
    // DEVIATION: inside a `RunOne` the cadence comes from the weighted pick against
    // `DoNothing`/look-target siblings, which a goal interval cannot reproduce; those callers
    // keep `RandomStrollGoal`'s 120-tick default.
    #[must_use]
    pub const fn new_brain_stroll(strolls: &'static [BrainStroll], interval: i32) -> Self {
        let speed = if strolls.is_empty() {
            1.0
        } else {
            strolls[0].speed
        };
        Self {
            goal_control: Controls::MOVE,
            speed,
            target: None,
            chance: to_goal_ticks(interval),
            force_trigger: false,
            avoid_water: false,
            swim_only: false,
            probability: 0.0,
            brain_strolls: strolls,
            flight: FlightSearch::None,
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
            swim_only: false,
            probability: 0.001,
            brain_strolls: &[],
            flight: FlightSearch::None,
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
            swim_only: false,
            probability,
            brain_strolls: &[],
            flight: FlightSearch::None,
        }
    }

    /// `RandomSwimmingGoal(PathfinderMob, speedModifier, interval)`
    /// (`RandomSwimmingGoal.java:8-11`) uses the ordinary random-stroll lifecycle but replaces
    /// its target search with `BehaviorUtils.getRandomSwimmablePos` (`BehaviorUtils.java:159-168`).
    #[must_use]
    pub const fn new_swimming_with_interval(speed: f64, interval: i32) -> Self {
        Self {
            goal_control: Controls::MOVE,
            speed,
            target: None,
            chance: to_goal_ticks(interval),
            force_trigger: false,
            avoid_water: false,
            swim_only: true,
            probability: 0.0,
            brain_strolls: &[],
            flight: FlightSearch::None,
        }
    }

    /// Vanilla: `WaterAvoidingRandomFlyingGoal(mob, speedModifier)`, which keeps
    /// `WaterAvoidingRandomStrollGoal`'s default interval and `0.001` probability.
    #[must_use]
    pub const fn new_water_avoiding_flying(speed: f64) -> Self {
        let mut goal = Self::new_water_avoiding(speed);
        goal.flight = FlightSearch::WaterAvoidingFlying;
        goal
    }

    /// Vanilla: `Parrot.ParrotWanderGoal(mob, speedModifier)` (`Parrot.java:472-475`).
    #[must_use]
    pub const fn new_parrot_wander(speed: f64) -> Self {
        let mut goal = Self::new_water_avoiding(speed);
        goal.flight = FlightSearch::ParrotWander;
        goal
    }

    /// `WaterAvoidingRandomFlyingGoal.getPosition` (`WaterAvoidingRandomFlyingGoal.java:14-22`).
    /// `getViewVector(0.0F)` takes the head yaw (`LivingEntity.getViewYRot`), so the current head
    /// look angle stands in for the previous-tick one.
    fn find_flying_target(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let view = mob.get_head_look_angle();
        let cone = f64::from(std::f32::consts::FRAC_PI_2);
        random_pos::hover_get_pos(mob, 8, 7, view.x, view.z, cone, 3, 1)
            .or_else(|| random_pos::air_and_water_get_pos(mob, 8, 4, -2, view.x, view.z, cone))
    }

    /// `Parrot.ParrotWanderGoal.getTreePos` (`Parrot.java:490-512`): the first leaf or log top
    /// with two air blocks above, scanned in `BlockPos.betweenClosed` order (x fastest, then y,
    /// then z) - not the nearest one.
    fn find_tree_pos(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let world = entity.world.load();
        let mob_pos = BlockPos::floored_v(pos);
        for z in (pos.z - 3.0).floor() as i32..=(pos.z + 3.0).floor() as i32 {
            for y in (pos.y - 6.0).floor() as i32..=(pos.y + 6.0).floor() as i32 {
                for x in (pos.x - 3.0).floor() as i32..=(pos.x + 3.0).floor() as i32 {
                    let candidate = BlockPos::new(x, y, z);
                    if candidate == mob_pos {
                        continue;
                    }
                    // The leaves tag holds exactly vanilla's `LeavesBlock` instances.
                    let below = world.get_block(&candidate.down());
                    let can_sit_on = below.has_tag(&pumpkin_data::tag::Block::MINECRAFT_LEAVES)
                        || below.has_tag(&pumpkin_data::tag::Block::MINECRAFT_LOGS);
                    if can_sit_on
                        && world.get_block_state(&candidate).is_air()
                        && world.get_block_state(&candidate.up()).is_air()
                    {
                        return Some(Vector3::new(
                            f64::from(x) + 0.5,
                            f64::from(y),
                            f64::from(z) + 0.5,
                        ));
                    }
                }
            }
        }
        None
    }

    /// `Parrot.ParrotWanderGoal.getPosition` (`Parrot.java:478-488`). As in vanilla, a
    /// successful tree roll overwrites the in-water land result even when no tree is found.
    fn find_parrot_target(&self, mob: &dyn Mob) -> Option<Vector3<f64>> {
        let in_water_pos = if mob.get_entity().was_touching_water.load(Ordering::Relaxed) {
            random_pos::land_get_pos(mob, 15, 15)
        } else {
            None
        };
        let pos = if mob.get_random().random::<f32>() >= self.probability {
            Self::find_tree_pos(mob)
        } else {
            in_water_pos
        };
        pos.or_else(|| Self::find_flying_target(mob))
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

    /// The `RandomStroll` entries of [`Self::new_brain_stroll`], in order; the first to produce a
    /// target becomes the walk target at its own speed.
    fn pick_brain_stroll(&mut self, mob: &dyn Mob) -> bool {
        let in_water = mob.get_entity().was_touching_water.load(Ordering::Relaxed);
        for stroll in self.brain_strolls {
            let target = match stroll.kind {
                BrainStrollKind::Land {
                    horizontal,
                    vertical,
                    may_stroll_from_water,
                } => {
                    if !may_stroll_from_water && in_water {
                        continue;
                    }
                    random_pos::land_get_pos(mob, horizontal, vertical)
                }
                BrainStrollKind::Swim => {
                    if !in_water {
                        continue;
                    }
                    random_pos::get_target_swim_pos(mob)
                }
            };
            if let Some(target) = target {
                self.target = Some(target);
                self.speed = stroll.speed;
                self.force_trigger = false;
                return true;
            }
        }
        false
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

    fn is_swimmable(world: &crate::world::World, pos: &BlockPos) -> bool {
        is_pathfindable(world.get_block_state(pos), PathComputationType::Water)
    }

    /// Mirrors vanilla `DefaultRandomPos` and `LandRandomPos` for the two random-stroll goals.
    /// The client-visible result is the selected bottom-center block, not an unchecked offset.
    fn find_random_target(
        mob: &dyn Mob,
        horizontal_range: i32,
        vertical_range: i32,
        land_only: bool,
        swim_only: bool,
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

            if swim_only && !Self::is_swimmable(&world, &candidate) {
                continue;
            }

            let navigator = mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                let navigator = mob_entity
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
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

            if mob.get_mob_entity().is_schooling_follower() {
                return false;
            }

            if !self.brain_strolls.is_empty() {
                if !self.force_trigger && mob.get_random().random_range(0..self.chance) != 0 {
                    return false;
                }
                return self.pick_brain_stroll(mob);
            }

            if !self.force_trigger {
                if mob.get_mob_entity().no_action_time.load(Ordering::Relaxed) >= 100 {
                    return false;
                }

                if mob.get_random().random_range(0..self.chance) != 0 {
                    return false;
                }
            }

            if self.flight == FlightSearch::WaterAvoidingFlying {
                self.target = Self::find_flying_target(mob);
            } else if self.flight == FlightSearch::ParrotWander {
                self.target = self.find_parrot_target(mob);
            } else if self.avoid_water {
                let in_water = mob.get_entity().was_touching_water.load(Ordering::Relaxed);
                self.target = if in_water {
                    Self::find_random_target(mob, 15, 7, true, false)
                        .or_else(|| Self::find_random_target(mob, 10, 7, false, false))
                } else if mob.get_random().random::<f32>() >= self.probability {
                    Self::find_random_target(mob, 10, 7, true, false)
                } else {
                    Self::find_random_target(mob, 10, 7, false, false)
                };
            } else {
                self.target = Self::find_random_target(mob, 10, 7, false, self.swim_only);
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
        assert_eq!(goal.probability, 0.001);
    }

    #[test]
    fn water_avoiding_probability_is_configurable() {
        let goal = WanderAroundGoal::new_water_avoiding_with_probability(0.4, 0.00001);
        assert_eq!(goal.probability, 0.00001);
    }

    #[test]
    fn brain_stroll_uses_first_entry_speed_and_interval() {
        static STROLLS: [BrainStroll; 2] =
            [BrainStroll::swim(0.5), BrainStroll::land_not_from_water(0.15)];
        let goal = WanderAroundGoal::new_brain_stroll(&STROLLS, 1);
        assert_eq!(goal.chance, 1);
        assert_eq!(goal.speed, 0.5);
        assert_eq!(
            STROLLS[1].kind,
            BrainStrollKind::Land {
                horizontal: 10,
                vertical: 7,
                may_stroll_from_water: false
            }
        );
    }

    #[test]
    fn flying_strolls_keep_water_avoiding_timing() {
        // `WaterAvoidingRandomFlyingGoal` and `ParrotWanderGoal` inherit
        // `WaterAvoidingRandomStrollGoal(mob, speed)`: interval 120, probability 0.001.
        for (goal, flight) in [
            (
                WanderAroundGoal::new_water_avoiding_flying(1.0),
                FlightSearch::WaterAvoidingFlying,
            ),
            (
                WanderAroundGoal::new_parrot_wander(1.0),
                FlightSearch::ParrotWander,
            ),
        ] {
            assert_eq!(goal.chance, to_goal_ticks(120));
            assert_eq!(goal.probability, 0.001);
            assert_eq!(goal.flight, flight);
        }
        assert_eq!(WanderAroundGoal::new(1.0).flight, FlightSearch::None);
    }

    #[test]
    fn plain_goals_have_no_brain_strolls() {
        assert!(WanderAroundGoal::new(1.0).brain_strolls.is_empty());
        assert!(WanderAroundGoal::new_water_avoiding(1.0).brain_strolls.is_empty());
    }
}
