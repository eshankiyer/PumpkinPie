use super::melee_attack::MeleeAttackGoal;
use super::{Controls, Goal};
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
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
            return false;
        };
        if !panda.can_perform_action() {
            return false;
        }
        self.inner.can_start(mob)
    }

    /// Vanilla `PandaAttackGoal` only overrides `canUse`; an attack already under way is not
    /// cancelled by the panda sitting down mid-swing.
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob)
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob)
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
