// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Weak;
use std::sync::atomic::Ordering::SeqCst;

use pumpkin_util::math::{position::BlockPos, vector2::Vector2, vector3::Vector3};
use rand::RngExt;

use super::{Controls, Goal, GoalFuture};
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob, passive::turtle::TurtleEntity};

/// Vanilla: `Turtle.TurtleTravelGoal` (`Turtle.java:577-649`).
pub struct TurtleTravelGoal {
    turtle: Weak<TurtleEntity>,
    speed: f64,
    travel_pos: Option<BlockPos>,
    stuck: bool,
}

impl TurtleTravelGoal {
    #[must_use]
    pub fn new(turtle: Weak<TurtleEntity>, speed: f64) -> Box<Self> {
        Box::new(Self {
            turtle,
            speed,
            travel_pos: None,
            stuck: false,
        })
    }

    /// `DefaultRandomPos.getPosTowards`, including its ten weighted candidates and navigation
    /// stability/malus checks. Turtle travel uses this to advance toward its distant travel pos.
    fn random_pos_towards(
        mob: &dyn Mob,
        target: Vector3<f64>,
        horizontal_distance: i32,
        vertical_distance: i32,
        max_angle: f64,
    ) -> Option<Vector3<f64>> {
        let entity = mob.get_entity();
        let origin = entity.pos.load();
        let direction = target - origin;
        let angle_center = direction.z.atan2(direction.x) - std::f64::consts::FRAC_PI_2;
        let mut random = mob.get_random();
        let mut best = None;
        let mut best_weight = f64::NEG_INFINITY;

        for _ in 0..10 {
            let angle = angle_center + (2.0 * random.random::<f64>() - 1.0) * max_angle;
            let distance = random.random::<f64>().sqrt()
                * f64::from(horizontal_distance)
                * std::f64::consts::SQRT_2;
            let x = -distance * angle.sin();
            let z = distance * angle.cos();
            if x.abs() > f64::from(horizontal_distance) || z.abs() > f64::from(horizontal_distance)
            {
                continue;
            }

            let candidate = BlockPos::new(
                (origin.x + x).floor() as i32,
                (origin.y + f64::from(random.random_range(-vertical_distance..=vertical_distance)))
                    .floor() as i32,
                (origin.z + z).floor() as i32,
            );
            let world = entity.world.load();
            if !(world.get_bottom_y()..=world.get_top_y()).contains(&candidate.0.y) {
                continue;
            }

            let navigator = mob.get_mob_entity().navigator.lock().unwrap();
            if navigator.is_stable_destination(&world, &candidate)
                && !navigator.has_pathfinding_malus(&world, &candidate)
            {
                let weight = mob.get_walk_target_value(&candidate);
                if weight > best_weight {
                    best_weight = weight;
                    best = Some(Vector3::new(
                        f64::from(candidate.0.x) + 0.5,
                        f64::from(candidate.0.y),
                        f64::from(candidate.0.z) + 0.5,
                    ));
                }
            }
        }

        best
    }

    /// `ServerLevel.hasChunksAt` for the 69x69 block box vanilla checks around a waypoint.
    fn has_chunks_at(world: &crate::world::World, x: i32, z: i32) -> bool {
        let min = BlockPos::new(x - 34, 0, z - 34).chunk_position();
        let max = BlockPos::new(x + 34, 0, z + 34).chunk_position();
        (min.x..=max.x).all(|chunk_x| {
            (min.y..=max.y)
                .all(|chunk_z| world.level.is_chunk_loaded(&Vector2::new(chunk_x, chunk_z)))
        })
    }
}

impl Goal for TurtleTravelGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(turtle) = self.turtle.upgrade() else {
                return false;
            };
            if turtle.is_going_home() || turtle.has_egg() {
                return false;
            }
            mob.get_entity().touching_water.load(SeqCst)
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(turtle) = self.turtle.upgrade() else {
                return false;
            };
            if turtle.is_going_home() || turtle.has_egg() {
                return false;
            }
            if turtle.get_mob_entity().is_in_love() {
                return false;
            }
            !mob.get_mob_entity().navigator.lock().unwrap().is_idle() && !self.stuck
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let entity = mob.get_entity();
            let my_pos = entity.pos.load();
            let world = entity.world.load();
            let mut rng = mob.get_random();

            let xt = f64::from(rng.random_range(-512i32..=512));
            let mut yt = f64::from(rng.random_range(-4i32..=4));
            let zt = f64::from(rng.random_range(-512i32..=512));

            if yt + my_pos.y > f64::from(world.sea_level - 1) {
                yt = 0.0;
            }

            self.travel_pos = Some(BlockPos::new(
                (my_pos.x + xt).floor() as i32,
                (my_pos.y + yt).floor() as i32,
                (my_pos.z + zt).floor() as i32,
            ));
            self.stuck = false;
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap()
                .set_turtle_travel(true);
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.travel_pos = None;
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap()
                .set_turtle_travel(false);
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(target) = self.travel_pos else {
                self.stuck = true;
                return;
            };

            let navigator_idle = mob.get_mob_entity().navigator.lock().unwrap().is_idle();
            if navigator_idle {
                let target = Vector3::new(
                    f64::from(target.0.x) + 0.5,
                    f64::from(target.0.y),
                    f64::from(target.0.z) + 0.5,
                );
                let mut next =
                    Self::random_pos_towards(mob, target, 16, 3, std::f64::consts::PI / 10.0);
                if next.is_none() {
                    next = Self::random_pos_towards(mob, target, 8, 7, std::f64::consts::PI / 2.0);
                }
                let Some(next) = next else {
                    self.stuck = true;
                    return;
                };

                let world = mob.get_entity().world.load();
                if !Self::has_chunks_at(&world, next.x.floor() as i32, next.z.floor() as i32) {
                    self.stuck = true;
                    return;
                }

                let my_pos = mob.get_entity().pos.load();
                let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
                navigator.set_progress(NavigatorGoal::new(my_pos, next, self.speed));
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
