use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{Controls, Goal};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::mob::creeper::CreeperEntity;

pub struct CreeperIgniteGoal {
    goal_control: Controls,
    creeper: Arc<CreeperEntity>,
    target: Option<Arc<dyn EntityBase>>,
}

impl CreeperIgniteGoal {
    #[must_use]
    pub const fn new(creeper: Arc<CreeperEntity>) -> Self {
        Self {
            goal_control: Controls::MOVE,
            creeper,
            target: None,
        }
    }

    const fn should_swell(distance_squared: f64, has_line_of_sight: bool) -> bool {
        distance_squared <= 49.0 && has_line_of_sight
    }

    fn has_line_of_sight(mob: &dyn Mob, target: &dyn crate::entity::EntityBase) -> bool {
        mob.get_mob_entity().has_line_of_sight(target)
    }

    /// Vanilla `SwellGoal.canUse`: an already lit fuse keeps the goal alive regardless of
    /// distance, otherwise swelling only begins for a live target inside 3 blocks.
    fn can_swell(&self, mob: &dyn Mob) -> bool {
        if self.creeper.fuse_speed.load(Ordering::Relaxed) > 0 {
            return true;
        }

        let target_lock = mob
            .get_mob_entity()
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(target) = target_lock.as_ref() {
            if !target.get_entity().is_alive() {
                return false;
            }
            let dist_sq = mob
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&target.get_entity().pos.load());
            return dist_sq < 9.0;
        }

        false
    }
}

impl Goal for CreeperIgniteGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.can_swell(mob)
    }

    /// Vanilla `SwellGoal` inherits `canContinueToUse` from `Goal`, which returns `canUse()`.
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_swell(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        let target = mob
            .get_mob_entity()
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        self.target.clone_from(&target);
    }

    // Vanilla `SwellGoal.stop` only clears the cached target, it never resets the fuse.
    fn stop(&mut self, _mob: &dyn Mob) {
        self.target = None;
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = self.target.as_ref() else {
            self.creeper.set_fuse_speed(-1);
            return;
        };

        if !target.get_entity().is_alive() {
            self.creeper.set_fuse_speed(-1);
            return;
        }

        let dist_sq = mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load());

        let has_line_of_sight = Self::has_line_of_sight(mob, target.as_ref());
        if Self::should_swell(dist_sq, has_line_of_sight) {
            self.creeper.set_fuse_speed(1);
        } else {
            self.creeper.set_fuse_speed(-1);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

#[cfg(test)]
mod tests {
    use super::CreeperIgniteGoal;

    #[test]
    fn swelling_requires_vanilla_distance_and_line_of_sight() {
        assert!(CreeperIgniteGoal::should_swell(49.0, true));
        assert!(!CreeperIgniteGoal::should_swell(49.0, false));
        assert!(!CreeperIgniteGoal::should_swell(49.000_001, true));
    }
}
