use std::sync::{Arc, Weak};

use super::look_at_entity::LookAtEntityGoal;
use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;
use crate::entity::{EntityBase, player::Player};
use pumpkin_data::entity::EntityType;

/// `Panda.PandaLookAtPlayerGoal` (`Panda.java:993-1033`).
///
/// The generic look-at-player goal with two changes. It is gated on `Panda.canPerformAction`, and
/// `PandaBreedGoal` can force a specific look target onto it (the nearest player, when the panda
/// can't find bamboo to breed over) -- a forced target bypasses the usual probability roll.
pub struct PandaLookAtPlayerGoal {
    inner: Box<LookAtEntityGoal>,
    /// The player `PandaBreedGoal` pushed at this goal, resolved and held for the run.
    forced_target: Option<Arc<dyn EntityBase>>,
}

impl PandaLookAtPlayerGoal {
    #[must_use]
    pub fn new(mob_weak: Weak<dyn Mob>, range: f32) -> Box<Self> {
        Box::new(Self {
            inner: LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, range),
            forced_target: None,
        })
    }

    /// Consumes any pending `setTarget` request from `PandaBreedGoal`.
    fn take_forced_target(panda: &PandaEntity) -> Option<Arc<dyn EntityBase>> {
        let uuid = panda.take_forced_look_target()?;
        let world = panda.get_mob_entity().living_entity.entity.world.load();
        world
            .players
            .load()
            .iter()
            .find(|p: &&Arc<Player>| p.gameprofile.id == uuid)
            .map(|p| p.clone() as Arc<dyn EntityBase>)
    }
}

impl Goal for PandaLookAtPlayerGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return false;
            };
            if !panda.can_perform_action().await {
                return false;
            }

            // Divergence, stated rather than glossed: vanilla's `canUse`
            // (`Panda.java:1013-1032`) runs the probability roll FIRST and only then falls back
            // to a nearest-player search when `lookAt` is null, so a forced target there still
            // waits for a passing roll. Here the forced target is honoured immediately, so the
            // "can't breed" look happens on the tick the breed goal asks for it instead of an
            // average of fifty ticks later.
            if let Some(target) = Self::take_forced_target(panda) {
                self.forced_target = Some(target);
                return true;
            }

            self.inner.can_start(mob).await
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if let Some(target) = &self.forced_target {
                return target.get_entity().is_alive();
            }
            self.inner.should_continue(mob).await
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.start(mob)
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.forced_target = None;
            self.inner.stop(mob).await;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(target) = self.forced_target.clone() {
                let target_entity = target.get_entity();
                let pos = target_entity.pos.load();
                let eye_y = target_entity.get_eye_y();
                mob.get_mob_entity()
                    .look_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .look_at(mob, pos.x, eye_y, pos.z);
                return;
            }
            self.inner.tick(mob).await;
        })
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
