use std::sync::atomic::Ordering::Relaxed;

use super::{Controls, Goal, GoalFuture};
use crate::entity::ageable::AgeableMob;
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

/// `Panda.PandaRollGoal` (`Panda.java:1035-1074`).
///
/// Babies and PLAYFUL pandas somersault. A ledge directly ahead (the block one down and one
/// forward being air) triggers it outright; otherwise a playful panda rolls on a 1-in-60 tick
/// roll and anything else on 1-in-500.
///
/// `isInterruptable` is `false` in vanilla, so `can_stop` is `false` here -- once a somersault
/// starts, nothing preempts it; `Panda.handleRoll` runs the 32-step arc and clears the flag.
pub struct PandaRollGoal;

impl PandaRollGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self)
    }

    /// `Mth.sign` applied to `PandaRollGoal.canUse`'s facing-direction step: a component whose
    /// magnitude exceeds 0.5 contributes its sign, anything smaller contributes nothing.
    const fn step(component: f64) -> i32 {
        if component > 0.5 {
            1
        } else if component < -0.5 {
            -1
        } else {
            0
        }
    }
}

impl Goal for PandaRollGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return false;
            };
            let entity = &panda.get_mob_entity().living_entity.entity;
            if (!panda.is_baby() && !panda.is_playful()) || !entity.on_ground.load(Relaxed) {
                return false;
            }
            if !panda.can_perform_action().await {
                return false;
            }

            let angle = f64::from(entity.yaw.load()).to_radians();
            let x_step = Self::step(-angle.sin());
            let z_step = Self::step(angle.cos());
            let ahead = entity
                .block_pos
                .load()
                .offset(Vector3::new(x_step, -1, z_step));
            if entity.world.load().get_block_state(&ahead).is_air() {
                return true;
            }

            let mut rng = rand::rng();
            if panda.is_playful() && rng.random_range(0..self.get_tick_count(60)) == 1 {
                return true;
            }
            rng.random_range(0..self.get_tick_count(500)) == 1
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() {
                panda.roll(true);
            }
        })
    }

    /// `Goal.isInterruptable() == false`.
    fn can_stop(&self) -> bool {
        false
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}

#[cfg(test)]
mod tests {
    use super::PandaRollGoal;

    #[test]
    fn only_a_dominant_facing_component_contributes_a_step() {
        assert_eq!(PandaRollGoal::step(1.0), 1);
        assert_eq!(PandaRollGoal::step(-1.0), -1);
        assert_eq!(PandaRollGoal::step(0.5), 0);
        assert_eq!(PandaRollGoal::step(-0.5), 0);
        assert_eq!(PandaRollGoal::step(0.0), 0);
    }
}
