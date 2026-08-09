use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::drowned_util::is_bright_outside;
use super::{Controls, Goal, GoalFuture};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::pathfinder::pathfinding_context::PathfindingContext;
use crate::entity::mob::Mob;

/// `Drowned.DrownedSwimUpGoal` (`Drowned.java:482-529`): a submerged drowned swims up toward
/// the surface once it's deep enough.
///
/// Gated on it not being bright outside (same "safe from sunlight" condition as
/// `DrownedGoToBeachGoal`).
///
/// Vanilla drives this with `DefaultRandomPos.getPosTowards` (a random pathfinder-node pick
/// biased toward the surface) plus a `searchingForLand`/`stuck` state pair consumed by
/// `Drowned#wantsToSwim` and the custom `DrownedMoveControl`. This codebase has no swimming
/// pathfinder mode (`NodeEvaluator::can_swim` is unused, see `walk_node_evaluator.rs`) and no
/// per-mob `MoveControl` override point, so this goal approximates the behavior by steering
/// the navigator straight toward the surface above the drowned's current position instead.
pub struct DrownedSwimUpGoal {
    speed: f64,
    stuck: bool,
}

impl DrownedSwimUpGoal {
    #[must_use]
    pub fn new(speed: f64) -> Box<Self> {
        Box::new(Self {
            speed,
            stuck: false,
        })
    }

    fn wants_to_surface(mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        !is_bright_outside(&world)
            && entity.touching_water.load(Relaxed)
            && entity.pos.load().y < f64::from(world.sea_level - 2)
    }

    fn find_surface_pos(
        mob: &dyn Mob,
        world: &Arc<crate::world::World>,
        pos: Vector3<f64>,
    ) -> Option<Vector3<f64>> {
        let candidates = {
            let mut rng = mob.get_random();
            let center = -std::f64::consts::FRAC_PI_2;
            let mut candidates = Vec::with_capacity(10);
            for _ in 0..10 {
                let angle =
                    center + (2.0 * rng.random::<f64>() - 1.0) * std::f64::consts::FRAC_PI_2;
                let distance = rng.random::<f64>().sqrt() * 4.0 * std::f64::consts::SQRT_2;
                let x_offset = -distance * angle.sin();
                let z_offset = distance * angle.cos();
                if x_offset.abs() > 4.0 || z_offset.abs() > 4.0 {
                    continue;
                }
                let x = pos.x + x_offset;
                let z = pos.z + z_offset;
                let y = pos.y + f64::from(rng.random_range(-8..=8));
                candidates.push(BlockPos::floored(x, y, z));
            }
            candidates
        };

        let home = mob.get_home();
        let mut best = None;
        let mut best_weight = f64::NEG_INFINITY;
        for candidate in candidates {
            if !world.is_in_height_limit(candidate.0.y)
                || home.is_some_and(|home| {
                    let dx = f64::from(candidate.0.x - home.0.x);
                    let dz = f64::from(candidate.0.z - home.0.z);
                    dx.mul_add(dx, dz * dz) > 1.0
                })
            {
                continue;
            }

            let below = BlockPos::new(candidate.0.x, candidate.0.y - 1, candidate.0.z);
            if world.get_block_state(&below).is_air() {
                continue;
            }

            let mut context = PathfindingContext::new(candidate.0, Arc::clone(world));
            let path_type = context.get_land_node_type(candidate.0);
            let malus = if path_type == crate::entity::ai::pathfinder::node::PathType::Water {
                0.0
            } else {
                path_type.get_malus()
            };
            if malus != 0.0 {
                continue;
            }

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
        best
    }
}

impl Goal for DrownedSwimUpGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { Self::wants_to_surface(mob) })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { Self::wants_to_surface(mob) && !self.stuck })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let entity = mob.get_entity();
            let world = entity.world.load();
            let pos = entity.pos.load();
            if pos.y >= f64::from(world.sea_level - 1) {
                return;
            }
            let should_choose = {
                let navigator = mob.get_mob_entity().navigator.lock().unwrap();
                navigator.is_idle() || navigator.close_to_next_pos(pos)
            };
            if !should_choose {
                return;
            }

            let Some(destination) = Self::find_surface_pos(mob, &world, pos) else {
                self.stuck = true;
                return;
            };
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap()
                .set_progress(NavigatorGoal::new(pos, destination, self.speed));
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.stuck = false;
            mob.set_searching_for_land(true);
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            mob.set_searching_for_land(false);
            self.stuck = false;
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
