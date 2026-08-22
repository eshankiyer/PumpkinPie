use std::sync::atomic::Ordering::Relaxed;

use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;
use rand::RngExt;

/// `Panda.PandaLieOnBackGoal` (`Panda.java:947-991`): a LAZY panda occasionally flops onto its
/// back and stays there until a random roll ends it, then sits out a 200-tick cooldown.
pub struct PandaLieOnBackGoal {
    /// `PandaLieOnBackGoal.cooldown`, compared against the panda's tick count.
    cooldown: i32,
}

impl PandaLieOnBackGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self { cooldown: 0 })
    }

    /// The `canContinueToUse` body shared verbatim with `PandaSitGoal` (`Panda.java:963-968` and
    /// `Panda.java:1078-1083`): a non-lazy panda gets an extra 1-in-600 chance to stop early each
    /// tick, and every panda a 1-in-2000 chance; being in water ends it outright.
    pub(crate) fn continue_roll(
        is_lazy: bool,
        in_water: bool,
        tick_600: i32,
        tick_2000: i32,
    ) -> bool {
        if in_water {
            return false;
        }
        let mut rng = rand::rng();
        if !is_lazy && rng.random_range(0..tick_600) == 1 {
            return false;
        }
        rng.random_range(0..tick_2000) != 1
    }
}

impl Goal for PandaLieOnBackGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return false;
            };
            if self.cooldown >= panda.get_mob_entity().tick_count.load(Relaxed) {
                return false;
            }
            if !panda.is_lazy() || !panda.can_perform_action().await {
                return false;
            }
            rand::rng().random_range(0..self.get_tick_count(400)) == 1
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return false;
            };
            Self::continue_roll(
                panda.is_lazy(),
                panda.get_mob_entity().living_entity.is_in_water(),
                self.get_tick_count(600),
                self.get_tick_count(2000),
            )
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() {
                panda.set_on_back(true);
            }
            self.cooldown = 0;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() {
                panda.set_on_back(false);
                self.cooldown = panda.get_mob_entity().tick_count.load(Relaxed) + 200;
            }
        })
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::PandaLieOnBackGoal;

    #[test]
    fn water_always_ends_the_flop() {
        // Both roll bounds set to 1 so `random_range(0..1)` is deterministically 0, i.e. neither
        // random branch can end the goal -- water must still end it.
        assert!(!PandaLieOnBackGoal::continue_roll(true, true, 1, 1));
        assert!(!PandaLieOnBackGoal::continue_roll(false, true, 1, 1));
    }

    #[test]
    fn a_dry_panda_keeps_lying_when_neither_roll_hits_one() {
        assert!(PandaLieOnBackGoal::continue_roll(true, false, 1, 1));
        assert!(PandaLieOnBackGoal::continue_roll(false, false, 1, 1));
    }
}
