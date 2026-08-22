use std::sync::Arc;

use pumpkin_util::math::vector3::Vector3;

use super::random_pos::default_get_pos_towards;
use super::{Controls, Goal, GoalFuture};
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;

/// Vanilla `MoveTowardsTargetGoal` (`MoveTowardsTargetGoal.java`).
///
/// Closes distance on the mob's *existing* attack target without attacking it, by pathing to a
/// random point picked in the target's direction rather than at the target itself
/// (`DefaultRandomPos.getPosTowards(mob, 16, 7, target.position(), PI/2)`,
/// `MoveTowardsTargetGoal.java:37`). Registered on `IronGolem` at priority 2
/// (`IronGolem.java:69`, `within = 32.0`), so a golem that has a target but cannot melee it
/// yet still walks it down.
///
/// Note the asymmetric range test: `canUse` rejects at `> within^2`
/// (`MoveTowardsTargetGoal.java:33`) while `canContinueToUse` requires `< within^2`
/// (line 50) - exactly at the boundary the goal can start but not continue. Kept as-is.
pub struct MoveTowardsTargetGoal {
    goal_control: Controls,
    speed: f64,
    within: f32,
    target: Option<Arc<dyn EntityBase>>,
    wanted: Option<Vector3<f64>>,
}

impl MoveTowardsTargetGoal {
    #[must_use]
    pub fn new(speed: f64, within: f32) -> Box<Self> {
        Box::new(Self {
            goal_control: Controls::MOVE,
            speed,
            within,
            target: None,
            wanted: None,
        })
    }

    /// `LivingEntity.distanceToSqr(entity)` between the two entity positions.
    fn distance_to_sqr(mob: &dyn Mob, target: &Arc<dyn EntityBase>) -> f64 {
        let a = mob.get_mob_entity().living_entity.entity.pos.load();
        let b = target.get_entity().pos.load();
        let dx = a.x - b.x;
        let dy = a.y - b.y;
        let dz = a.z - b.z;
        dx.mul_add(dx, dy.mul_add(dy, dz * dz))
    }

    fn within_sq(&self) -> f64 {
        f64::from(self.within) * f64::from(self.within)
    }
}

impl Goal for MoveTowardsTargetGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            self.target = mob.get_mob_entity().get_target().await;
            let Some(target) = self.target.clone() else {
                return false;
            };

            if Self::distance_to_sqr(mob, &target) > self.within_sq() {
                self.target = None;
                return false;
            }

            let target_pos = target.get_entity().pos.load();
            self.wanted =
                default_get_pos_towards(mob, 16, 7, target_pos, std::f64::consts::FRAC_PI_2);
            if self.wanted.is_none() {
                self.target = None;
                return false;
            }
            true
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(target) = self.target.clone() else {
                return false;
            };
            let navigator_idle = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle();
            !navigator_idle
                && target.get_entity().is_alive()
                && Self::distance_to_sqr(mob, &target) < self.within_sq()
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

    fn stop<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            // Vanilla `stop()` only clears the target; it deliberately does not stop the
            // navigation (`MoveTowardsTargetGoal.java:53-56`).
            self.target = None;
            self.wanted = None;
        })
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}
