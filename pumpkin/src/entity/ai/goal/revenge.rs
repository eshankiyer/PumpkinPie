use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use super::{Controls, Goal};
use crate::entity::EntityBase;
use crate::entity::ai::goal::GoalFuture;
use crate::entity::ai::goal::track_target::TrackTargetGoal;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;

/// Decides whether a nearby mob should be alerted.
///
/// Takes the alerting mob's own type and the candidate's type. Mirrors vanilla
/// `HurtByTargetGoal.alertOthers`'s `getEntitiesOfClass(this.mob.getClass(), ...)` search class
/// plus the `toIgnoreAlert` exclusion list (`HurtByTargetGoal.java:72-107`) collapsed into one
/// predicate, since Pumpkin has no Java-style class hierarchy to match against.
pub type AlertMatcher = fn(mob_type: &'static EntityType, other_type: &'static EntityType) -> bool;

/// `mob_type == other_type`: the common case.
///
/// Used by `HurtByTargetGoal(this).setAlertOthers()` with no exceptions (e.g.
/// Blaze/Silverfish/Endermite/ZombifiedPiglin), where nothing in this codebase subclasses the
/// alerting mob's own type.
#[must_use]
pub fn same_type_matcher(mob_type: &'static EntityType, other_type: &'static EntityType) -> bool {
    mob_type == other_type
}

pub struct RevengeGoal {
    track_target_goal: TrackTargetGoal,
    target: Option<Arc<dyn EntityBase>>,
    last_attacked_time: i32,
    target_predicate: TargetPredicate,
    /// `None` reproduces vanilla's default of never calling `setAlertOthers()`: no nearby mob
    /// is alerted. `Some` reproduces `setAlertOthers(...)`.
    alert_matcher: Option<AlertMatcher>,
}

impl RevengeGoal {
    #[must_use]
    pub fn new(check_visibility: bool) -> Self {
        let target_predicate = TargetPredicate::create_attackable()
            .ignore_visibility()
            .ignore_distance_scaling_factor();
        Self {
            track_target_goal: TrackTargetGoal::with_default(check_visibility),
            target: None,
            last_attacked_time: 0,
            target_predicate,
            alert_matcher: None,
        }
    }

    /// Vanilla `HurtByTargetGoal.setAlertOthers(Class<?>... exceptTheseTypes)`
    /// (`HurtByTargetGoal.java:53-57`): opts this goal into alerting nearby mobs matching
    /// `matcher` on `start`.
    #[must_use]
    pub fn with_alert_others(check_visibility: bool, matcher: AlertMatcher) -> Self {
        Self {
            alert_matcher: Some(matcher),
            ..Self::new(check_visibility)
        }
    }
}

impl Goal for RevengeGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            let living = &mob_entity.living_entity;

            let attacked_time = living.last_attacked_time.load(Relaxed);
            if attacked_time == self.last_attacked_time {
                return false;
            }

            let attacker_id = living.last_attacker_id.load(Relaxed);
            if attacker_id == 0 {
                return false;
            }

            let world = living.entity.world.load();
            let Some(attacker) = world.get_entity_by_id(attacker_id) else {
                return false;
            };

            let Some(attacker_living) = attacker.get_living_entity() else {
                return false;
            };

            // Vanilla `TamableAnimal::canAttack` unconditionally excludes the mob's own owner
            // from any attack target, regardless of which targeting goal found them; for
            // non-tameable mobs `get_owner_uuid()` is always `None` so this is a no-op.
            if mob.get_owner_uuid() == Some(attacker.get_entity().entity_uuid) {
                return false;
            }

            if !self
                .target_predicate
                .test(&world, Some(&mob_entity.living_entity), attacker_living)
                .await
            {
                return false;
            }

            self.target = Some(attacker);
            true
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { self.track_target_goal.should_continue(mob).await })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            mob.set_mob_target(self.target.clone()).await;

            let mob_entity = mob.get_mob_entity();
            self.last_attacked_time = mob_entity.living_entity.last_attacked_time.load(Relaxed);
            self.track_target_goal.max_time_without_visibility = 300;
            self.track_target_goal.start(mob).await;

            let Some(target) = self.target.as_ref() else {
                return;
            };
            let Some(matcher) = self.alert_matcher else {
                return;
            };
            let mob_entity = mob.get_mob_entity();
            let entity = &mob_entity.living_entity.entity;
            let world = entity.world.load();
            let position = entity.pos.load();
            // Vanilla searches an `AABB.unitCubeFromLowerCorner(pos).inflate(within, 10.0,
            // within)` box (`HurtByTargetGoal.java:74`) -- `±follow_range` horizontally but
            // only `±10` vertically. `get_nearby_entities` is a sphere of radius `follow_range`
            // instead; for tall follow ranges (e.g. Zombie's 35) this over-reaches vertically
            // relative to vanilla. No box-shaped nearby-entity search exists here yet.
            let follow_range = mob_entity
                .living_entity
                .get_attribute_value(&Attributes::FOLLOW_RANGE);
            let entity_type = entity.entity_type;

            for nearby in world
                .get_nearby_entities(position, follow_range)
                .into_values()
            {
                if nearby.get_entity().entity_id == entity.entity_id
                    || !matcher(entity_type, nearby.get_entity().entity_type)
                {
                    continue;
                }
                let Some(nearby_mob) = nearby.get_mob() else {
                    continue;
                };
                if nearby_mob.get_mob_entity().target.lock().await.is_some() {
                    continue;
                }
                nearby_mob.set_mob_target(Some(target.clone())).await;
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.target = None;
            self.track_target_goal.stop(mob).await;
        })
    }

    fn controls(&self) -> Controls {
        self.track_target_goal.controls()
    }
}

#[cfg(test)]
mod tests {
    use super::same_type_matcher;
    use pumpkin_data::entity::EntityType;

    #[test]
    fn same_type_matcher_matches_only_identical_type() {
        assert!(same_type_matcher(&EntityType::BLAZE, &EntityType::BLAZE));
        assert!(!same_type_matcher(&EntityType::BLAZE, &EntityType::ZOMBIE));
    }
}
