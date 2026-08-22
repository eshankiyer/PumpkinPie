use super::melee_attack::MeleeAttackGoal;
use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;

/// `Panda.PandaAttackGoal` (`Panda.java:809-822`): the generic melee goal behind
/// `Panda.canPerformAction`, so a panda that is on its back, scared, eating, rolling or sitting
/// never starts an attack.
pub struct PandaAttackGoal {
    inner: MeleeAttackGoal,
}

impl PandaAttackGoal {
    #[must_use]
    pub fn new(speed: f64, pause_when_mob_idle: bool) -> Box<Self> {
        Box::new(Self {
            inner: MeleeAttackGoal::new(speed, pause_when_mob_idle),
        })
    }
}

impl Goal for PandaAttackGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return false;
            };
            if !panda.can_perform_action().await {
                return false;
            }
            self.inner.can_start(mob).await
        })
    }

    /// Vanilla `PandaAttackGoal` only overrides `canUse`; an attack already under way is not
    /// cancelled by the panda sitting down mid-swing.
    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.should_continue(mob)
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.start(mob)
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.stop(mob)
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.tick(mob)
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
