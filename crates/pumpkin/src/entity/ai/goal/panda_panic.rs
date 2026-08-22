use super::escape_danger::EscapeDangerGoal;
use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;

/// `Panda.PandaPanicGoal` (`Panda.java:955-973`): the generic panic goal, except a panda that has
/// sat down stops navigating and drops out of the panic instead of continuing to flee.
pub struct PandaPanicGoal {
    inner: Box<EscapeDangerGoal>,
}

impl PandaPanicGoal {
    #[must_use]
    pub fn new(speed: f64) -> Box<Self> {
        Box::new(Self {
            inner: EscapeDangerGoal::new(speed),
        })
    }
}

impl Goal for PandaPanicGoal {
    fn is_panic_goal(&self) -> bool {
        true
    }

    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.can_start(mob)
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let sitting = mob
                .cast_any()
                .downcast_ref::<PandaEntity>()
                .is_some_and(PandaEntity::is_sitting_panda);
            if sitting {
                mob.get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
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
