use std::sync::Weak;

use rand::RngExt;

use super::{Controls, Goal};
use crate::entity::{
    ai::goal::GoalFuture,
    mob::{ABSTRACT_HORSE_AMBIENT_SOUND_INTERVAL, Mob},
    passive::equine::AbstractHorse,
};

/// Vanilla `RandomStandGoal.canUse` (`RandomStandGoal.java:30-40`), factored out as a pure
/// function of the post-increment counter and the two random rolls it consumes.
/// Returns `(should_reset, should_stand)`: whether the interval counter should reset, and
/// whether the goal should actually trigger standing this call.
#[must_use]
const fn evaluate_stand_trigger(
    next_stand: i32,
    roll_1000: i32,
    roll_10: i32,
    is_immobile: bool,
) -> (bool, bool) {
    if next_stand > 0 && roll_1000 < next_stand {
        (true, !is_immobile && roll_10 == 0)
    } else {
        (false, false)
    }
}

/// `AbstractHorse.java:141-143`'s conditional `RandomStandGoal` registration.
///
/// Generic over the concrete horse-family species since each keeps its own `Arc<Self>`.
pub struct AmbientStandGoal<T> {
    horse: Weak<T>,
    next_stand: i32,
}

impl<T: AbstractHorse + Mob> AmbientStandGoal<T> {
    #[must_use]
    pub fn new(horse: Weak<T>) -> Box<Self> {
        Box::new(Self {
            horse,
            // `RandomStandGoal`'s constructor calls `resetStandInterval` immediately; every
            // `AbstractHorse` reports the same `getAmbientStandInterval`.
            next_stand: -ABSTRACT_HORSE_AMBIENT_SOUND_INTERVAL,
        })
    }
}

impl<T: AbstractHorse + Mob + Send + Sync + 'static> Goal for AmbientStandGoal<T> {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(horse) = self.horse.upgrade() else {
                return false;
            };

            self.next_stand += 1;
            let roll_1000 = mob.get_random().random_range(0..1000);
            let roll_10 = mob.get_random().random_range(0..10);
            let (should_reset, should_stand) =
                evaluate_stand_trigger(self.next_stand, roll_1000, roll_10, horse.is_immobile());

            if should_reset {
                // `resetStandInterval`: `getAmbientStandInterval` is `getAmbientSoundInterval`
                // (`AbstractHorse.java:1033-1035`).
                self.next_stand = -mob.get_ambient_sound_interval();
            }

            should_stand
        })
    }

    /// `RandomStandGoal.canContinueToUse` always returns `false`: this goal is instantaneous.
    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(horse) = self.horse.upgrade() {
                horse.stand_if_possible();
            }
            // `RandomStandGoal.playStandSound` (`RandomStandGoal.java:17-25`): the ambient sound
            // (`AbstractHorse.getAmbientStandSound`, `AbstractHorse.java:692-694`) through the
            // plain `Entity.playSound(sound)`, so volume and pitch are `1.0`
            // (`Entity.java:1492-1496`).
            let entity = mob.get_entity();
            if !entity.is_silent()
                && let Some(sound) = mob.get_ambient_sound()
            {
                entity.world.load().play_sound_fine(
                    sound,
                    mob.get_sound_source(),
                    &entity.pos.load(),
                    1.0,
                    1.0,
                );
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_threshold_does_not_trigger() {
        // roll_1000 (500) is not < next_stand (10): no reset, no stand.
        assert_eq!(evaluate_stand_trigger(10, 500, 0, false), (false, false));
    }

    #[test]
    fn negative_next_stand_never_triggers() {
        assert_eq!(evaluate_stand_trigger(-79, 999, 0, false), (false, false));
    }

    #[test]
    fn threshold_met_but_immobile_resets_without_standing() {
        assert_eq!(evaluate_stand_trigger(50, 10, 0, true), (true, false));
    }

    #[test]
    fn threshold_met_and_lucky_roll_stands() {
        assert_eq!(evaluate_stand_trigger(50, 10, 0, false), (true, true));
    }

    #[test]
    fn threshold_met_but_unlucky_roll_resets_without_standing() {
        assert_eq!(evaluate_stand_trigger(50, 10, 5, false), (true, false));
    }
}
