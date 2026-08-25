//! Port of vanilla `ShootWhenStuck`.

use std::sync::Weak;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::effect::StatusEffect;

use crate::entity::{
    ai::goal::{Controls, Goal, GoalFuture},
    mob::{Mob, breeze::BreezeEntity},
};

const SHOOT_MEMORY_EXPIRY_TICKS: i32 = 60;

/// Vanilla `ShootWhenStuck` (`ShootWhenStuck.java:11-27`) is a one-shot behavior: it
/// requires an attack target and opens the `BREEZE_SHOOT` memory for the normal shoot behavior.
pub struct BreezeShootWhenStuckGoal {
    breeze: Weak<BreezeEntity>,
}

impl BreezeShootWhenStuckGoal {
    /// Constructs the goal that owns the Breeze's `BREEZE_SHOOT` fallback state
    /// (`ShootWhenStuck.java:11-27`).
    #[must_use]
    pub const fn new(breeze: Weak<BreezeEntity>) -> Self {
        Self { breeze }
    }

    /// Mirrors `ShootWhenStuck.checkExtraStartConditions` (`ShootWhenStuck.java:29-31`):
    /// a stuck Breeze is a passenger, is in water, or has Levitation.
    async fn check_extra_start_conditions(&self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        if entity.has_vehicle().await || entity.touching_water.load(Relaxed) {
            return true;
        }

        if let Some(living) = mob.get_living_entity() {
            living.has_effect(&StatusEffect::LEVITATION).await
        } else {
            false
        }
    }
}

impl Goal for BreezeShootWhenStuckGoal {
    /// Maps the behavior's memory requirements (`ShootWhenStuck.java:11-27`) and extra-start
    /// condition (`ShootWhenStuck.java:29-31`) onto Pumpkin's goal selector.
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(breeze) = self.breeze.upgrade() else {
                return false;
            };
            if breeze.shoot_window_ticks() > 0 {
                return false;
            }

            let Some(target) = breeze.mob_entity.target.lock().await.clone() else {
                return false;
            };
            if !target.get_entity().is_alive() {
                return false;
            }

            self.check_extra_start_conditions(mob).await
        })
    }

    /// Vanilla `canStillUse` always returns false (`ShootWhenStuck.java:33-35`), so this
    /// one-shot goal ends immediately after opening the shoot window.
    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }

    /// Vanilla `start` sets `BREEZE_SHOOT` with a 60-tick expiry
    /// (`ShootWhenStuck.java:37-39`).
    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(breeze) = self.breeze.upgrade() {
                breeze.set_shoot_window(SHOOT_MEMORY_EXPIRY_TICKS);
            }
        })
    }

    /// This behavior owns no movement/look/jump control; vanilla only writes a Brain memory
    /// (`ShootWhenStuck.java:37-39`).
    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
