use super::avoid_entity::AvoidEntityGoal;
use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;
use pumpkin_data::entity::{EntityType, MobCategory};

/// `Panda.PandaAvoidGoal` (`Panda.java:824-841`).
///
/// The generic avoid goal, gated on the panda being the WORRIED variant and on
/// `Panda.canPerformAction`. `Panda.registerGoals` registers two of these -- players at 8 blocks
/// and monsters at 4.
///
/// The gate is a whole-goal `can_use` check in vanilla, so it stays a `can_start` gate here rather
/// than becoming an `AvoidEntityGoal::with_predicate` per-candidate filter.
pub struct PandaAvoidGoal {
    inner: AvoidEntityGoal,
}

impl PandaAvoidGoal {
    /// `new Panda.PandaAvoidGoal<>(this, Player.class, 8.0F, 2.0, 2.0)`.
    #[must_use]
    pub fn from_player(flee_distance: f64, slow_speed: f64, fast_speed: f64) -> Box<Self> {
        Box::new(Self {
            inner: AvoidEntityGoal::new(&EntityType::PLAYER, flee_distance, slow_speed, fast_speed),
        })
    }

    /// `new Panda.PandaAvoidGoal<>(this, Monster.class, 4.0F, 2.0, 2.0)`. There is no `Monster`
    /// class here, so `MobCategory::MONSTER` stands in, matching `nearest_hostile_target.rs`'s
    /// existing precedent.
    #[must_use]
    pub fn from_monsters(flee_distance: f64, slow_speed: f64, fast_speed: f64) -> Box<Self> {
        Box::new(Self {
            inner: AvoidEntityGoal::new_for_category(
                &MobCategory::MONSTER,
                flee_distance,
                slow_speed,
                fast_speed,
            ),
        })
    }
}

impl Goal for PandaAvoidGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return false;
            };
            if !panda.is_worried() || !panda.can_perform_action().await {
                return false;
            }
            self.inner.can_start(mob).await
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.should_continue(mob)
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
