use super::{Goal, GoalFuture};
use crate::entity::ageable::AgeableMob;
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;
use rand::RngExt;

/// `Panda.PandaSneezeGoal` (`Panda.java:1109-1133`): only baby pandas sneeze, and a WEAK cub
/// sneezes twelve times as often as any other (1-in-500 per goal tick against 1-in-6000).
///
/// The sneeze itself is a one-shot: `canContinueToUse` is `false`, so `start` raises the flag and
/// `Panda.tick`'s sneeze counter runs it out and fires `afterSneeze`.
pub struct PandaSneezeGoal;

impl PandaSneezeGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self)
    }
}

impl Goal for PandaSneezeGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return false;
            };
            if !panda.is_baby() || !panda.can_perform_action().await {
                return false;
            }
            let mut rng = rand::rng();
            if panda.is_weak() && rng.random_range(0..self.get_tick_count(500)) == 1 {
                return true;
            }
            rng.random_range(0..self.get_tick_count(6000)) == 1
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() {
                panda.sneeze(true);
            }
        })
    }
}
