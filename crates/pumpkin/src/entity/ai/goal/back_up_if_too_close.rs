use std::sync::Arc;

use pumpkin_data::item::Item;

use super::{Controls, Goal, GoalFuture};
use crate::entity::{EntityBase, mob::Mob};

/// Goal-system sibling of vanilla `BackUpIfTooClose`.
///
/// Vanilla `BackUpIfTooClose.create` (`net/minecraft/world/entity/ai/behavior/BackUpIfTooClose.java:11-29`)
/// is a one-shot Piglin Brain behavior. Piglin Brain is flattened onto the Goal system in
/// `piglin.rs`, so this goal is registered there at a higher priority than crossbow attack and
/// performs the same visible-target, close-range retreat.
pub struct BackUpIfTooCloseGoal {
    too_close_distance: f64,
    strafe_speed: f32,
    target: Option<Arc<dyn EntityBase>>,
}

impl BackUpIfTooCloseGoal {
    /// `BackUpIfTooClose.create(tooCloseDistance, strafeSpeed)` (`BackUpIfTooClose.java:11`).
    #[must_use]
    pub const fn new(too_close_distance: f64, strafe_speed: f32) -> Self {
        Self {
            too_close_distance,
            strafe_speed,
            target: None,
        }
    }

    async fn has_crossbow(mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .living_entity
            .held_item(mob)
            .await
            .item
            .id
            == Item::CROSSBOW.id
    }
}

impl Goal for BackUpIfTooCloseGoal {
    /// `BehaviorBuilder`'s `ATTACK_TARGET`/`NEAREST_VISIBLE_LIVING_ENTITIES` gate and the
    /// close-distance test (`BackUpIfTooClose.java:13-21`). Piglin's outer `hasCrossbow` gate
    /// (`PiglinAi.java:177`) is represented by the held-crossbow check here.
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if !Self::has_crossbow(mob).await {
                return false;
            }
            let Some(target) = mob.get_mob_entity().target.lock().await.clone() else {
                return false;
            };
            let entity = mob.get_entity();
            let target_entity = target.get_entity();
            if !target_entity.is_alive()
                || !entity
                    .pos
                    .load()
                    .squared_distance_to_vec(&target_entity.pos.load())
                    .lt(&(self.too_close_distance * self.too_close_distance))
                || !mob
                    .get_mob_entity()
                    .has_line_of_sight(target.as_ref())
                    .await
            {
                return false;
            }

            self.target = Some(target);
            true
        })
    }

    /// A declarative `OneShot` stops after its trigger tick; this goal applies the one-shot
    /// retreat (`BackUpIfTooClose.java:20-25`) from `start` and therefore does not continue.
    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }

    /// `lookTarget.set(new EntityTracker(target, true))`, `strafe(-strafeSpeed, 0)`, and
    /// `setYRot(rotateIfNecessary(..., 0))` (`BackUpIfTooClose.java:22-24`). The existing
    /// look controller supplies the equivalent entity tracker and the zero-step body rotation
    /// copies the current head yaw.
    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(target) = &self.target else {
                return;
            };
            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_entity_with_range(target, 30.0, 30.0);
            mob.get_mob_entity()
                .move_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .strafe(-self.strafe_speed, 0.0);
            let entity = mob.get_entity();
            entity.yaw.store(entity.head_yaw.load());
        })
    }

    /// Clears the cached one-shot target after the Goal selector releases the behavior.
    fn stop<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.target = None;
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
