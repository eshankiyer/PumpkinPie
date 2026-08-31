use super::revenge::RevengeGoal;
use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;

/// `Panda.PandaHurtByTargetGoal` (`Panda.java:923-945`).
///
/// Retaliation with an escape hatch: once the panda has either been fed bamboo mid-fight
/// (`gotBamboo`) or landed a bite of its own (`didBite`), it drops the target and stops
/// chasing.
///
/// Vanilla also overrides `alertOther` so only AGGRESSIVE-gened pandas join in
/// (`Panda.java:868-871`).
pub struct PandaHurtByTargetGoal {
    inner: RevengeGoal,
}

impl PandaHurtByTargetGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            // `Panda.java:281` calls `setAlertOthers` on this goal.
            inner: RevengeGoal::new(true)
                .alert_others()
                .alert_only_aggressive(),
        })
    }
}

impl Goal for PandaHurtByTargetGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.can_start(mob)
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>()
                && (panda.got_bamboo() || panda.did_bite())
            {
                panda.get_mob_entity().set_target(None).await;
                return false;
            }
            self.inner.should_continue(mob).await
        })
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
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
