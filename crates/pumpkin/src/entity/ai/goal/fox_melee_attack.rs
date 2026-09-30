use super::melee_attack::MeleeAttackGoal;
use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;
use crate::entity::passive::fox::FoxEntity;
use pumpkin_data::sound::Sound;

/// `Fox.FoxMeleeAttackGoal`: the generic `MeleeAttackGoal` with an extra gate -- a fox that's
/// sitting, sleeping, crouching (stalking prey), or faceplanted never bites.
///
/// Vanilla overrides `checkAndPerformAttack` (`Fox.java:1086-1092`): no swing animation, and
/// `FOX_BITE` plays after the hit. This drives the inner goal's movement and runs that step.
pub struct FoxMeleeAttackGoal {
    inner: MeleeAttackGoal,
}

impl FoxMeleeAttackGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            inner: MeleeAttackGoal::new(1.2, true),
        })
    }

    fn gated(mob: &dyn Mob) -> bool {
        mob.cast_any().downcast_ref::<FoxEntity>().is_some_and(|f| {
            !f.is_sitting() && !f.is_sleeping() && !f.is_crouching() && !f.is_faceplanted()
        })
    }
}

impl Goal for FoxMeleeAttackGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { Self::gated(mob) && self.inner.can_start(mob).await })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        // Vanilla `FoxMeleeAttackGoal` only overrides `canUse`, not `canContinueToUse` -- once
        // started, a bite in progress isn't cancelled by e.g. going sleepy mid-attack.
        self.inner.should_continue(mob)
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() {
                fox.set_is_interested(false);
            }
            self.inner.start(mob).await;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.stop(mob)
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(target) = self.inner.tick_movement(mob).await else {
                return;
            };
            if self.inner.can_perform_attack(mob, target.as_ref()).await {
                self.inner.reset_attack_cooldown();
                mob.try_attack(target.as_ref()).await;
                mob.get_entity().play_sound(Sound::EntityFoxBite);
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
