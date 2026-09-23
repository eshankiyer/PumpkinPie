use std::sync::atomic::Ordering::Relaxed;

use super::{Controls, Goal};
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::mob::Mob;

/// `PolarBear.PolarBearHurtByTargetGoal` (PolarBear.java:282-302): a baby bear that gets hurt
/// alerts nearby adult bears (via `RevengeGoal::alert_only_adults`) but never fights back
/// itself.
///
/// An adult that gets hurt never alerts (the base `HurtByTargetGoal` only alerts when
/// `setAlertOthers()` was called, which this goal never does).
pub struct PolarBearHurtByTargetGoal {
    inner: RevengeGoal,
}

impl PolarBearHurtByTargetGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            // `PolarBearHurtByTargetGoal.start` calls `alertOthers` for a baby bear
            // (`PolarBear.java:284-291`), even though its constructor does not call
            // `setAlertOthers`.
            inner: RevengeGoal::new(true)
                .alert_others()
                .alert_only_adults()
                .alert_only_when_self_is_baby(),
        })
    }
}

impl Goal for PolarBearHurtByTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.inner.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
        if mob.get_entity().age.load(Relaxed) < 0 {
            self.inner.stop(mob);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob)
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
