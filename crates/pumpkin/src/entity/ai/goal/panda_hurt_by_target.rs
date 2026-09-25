use super::revenge::RevengeGoal;
use super::{Controls, Goal};
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;

/// `Panda.PandaHurtByTargetGoal` (`Panda.java:849-873`).
///
/// Retaliation with an escape hatch: once the panda has either been fed bamboo mid-fight
/// (`gotBamboo`) or landed a bite of its own (`didBite`), it drops the target and stops
/// chasing.
///
/// Vanilla enables alerts (`Panda.java:281`) but overrides `alertOther` so only pandas
/// expressing the aggressive gene join in (`Panda.java:867-872`).
pub struct PandaHurtByTargetGoal {
    inner: RevengeGoal,
}

impl PandaHurtByTargetGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            inner: RevengeGoal::new(true).with_alert_filter(|other| {
                other
                    .cast_any()
                    .downcast_ref::<PandaEntity>()
                    .is_some_and(PandaEntity::is_aggressive_gene)
            }),
        })
    }
}

impl Goal for PandaHurtByTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.inner.can_start(mob)
    }

    fn should_continue(&self, mob: &dyn Mob) -> bool {
        if let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>()
            && (panda.got_bamboo() || panda.did_bite())
        {
            panda.get_mob_entity().set_target(None);
            return false;
        }
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
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
