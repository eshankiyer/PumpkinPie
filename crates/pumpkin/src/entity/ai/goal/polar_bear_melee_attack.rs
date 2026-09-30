use super::melee_attack::MeleeAttackGoal;
use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;
use crate::entity::passive::polar_bear::PolarBearEntity;

/// `PolarBear.PolarBearMeleeAttackGoal` (PolarBear.java:304-336): on top of the generic melee
/// attack, a bear that's close to its target but not yet swinging rears up (`setStanding(true)`).
///
/// Overrides `checkAndPerformAttack` with vanilla's own branch tree: the bear hits without a
/// swing animation, and otherwise pins the cooldown at its maximum while far from the target so
/// it must count down (standing, growling from 10 ticks out) before its first hit. This drives
/// the inner goal's movement via `tick_movement` and runs that step itself.
pub struct PolarBearMeleeAttackGoal {
    inner: MeleeAttackGoal,
}

impl PolarBearMeleeAttackGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            inner: MeleeAttackGoal::new(1.25, true),
        })
    }
}

impl Goal for PolarBearMeleeAttackGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.can_start(mob)
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.should_continue(mob)
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.start(mob)
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.inner.stop(mob).await;
            if let Some(bear) = mob.cast_any().downcast_ref::<PolarBearEntity>() {
                bear.set_standing(false);
            }
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(target) = self.inner.tick_movement(mob).await else {
                return;
            };
            let Some(bear) = mob.cast_any().downcast_ref::<PolarBearEntity>() else {
                return;
            };

            // Vanilla `PolarBearMeleeAttackGoal.checkAndPerformAttack` (PolarBear.java:310-329).
            if self.inner.can_perform_attack(mob, target.as_ref()).await {
                self.inner.reset_attack_cooldown();
                mob.try_attack(target.as_ref()).await;
                bear.set_standing(false);
                return;
            }

            let target_entity = target.get_entity();
            let dist_sq = mob
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&target_entity.pos.load());
            let near_reach = f64::from(target_entity.entity_dimension.load().width) + 3.0;

            if dist_sq < near_reach * near_reach {
                if self.inner.is_time_to_attack() {
                    bear.set_standing(false);
                    self.inner.reset_attack_cooldown();
                }
                if self.inner.cooldown <= 10 {
                    bear.set_standing(true);
                    bear.play_warning_sound();
                }
            } else {
                self.inner.reset_attack_cooldown();
                bear.set_standing(false);
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
