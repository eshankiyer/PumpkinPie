use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::ai::goal::track_target::TrackTargetGoal;
use crate::entity::ai::goal::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;
use crate::entity::mob::witch::WitchEntity;

/// Vanilla: `NearestHealableRaiderTargetGoal<Raider>` as wired by `Witch.registerGoals`.
///
/// (`target = Raider.class`, `mustSee = true`, subselector `hasActiveRaid() &&
/// !target.is(EntityTypes.WITCH)`). Finds the nearest raid-mate (excluding other witches) while
/// an active raid is running; `Witch::mob_tick` drives the 200-tick cooldown externally. The
/// `500` vanilla passes to the super constructor is the unused `randomInterval`, not a radius.
pub struct NearestHealableRaiderTargetGoal {
    track_target_goal: TrackTargetGoal,
    target: Option<Arc<dyn EntityBase>>,
}

impl NearestHealableRaiderTargetGoal {
    #[must_use]
    pub fn new() -> Self {
        Self {
            track_target_goal: TrackTargetGoal::with_default(true),
            target: None,
        }
    }

    /// `NearestAttackableTargetGoal.findTarget` (`NearestAttackableTargetGoal.java:62-72`):
    /// raiders whose bounding box intersects the mob's box inflated by `FOLLOW_RANGE`, tested
    /// against `forCombat().range(followRange)`, nearest to the eye position wins. Vanilla has no
    /// health filter; `Witch.performRangedAttack` picks regeneration for healthy raiders.
    async fn find_target(mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let mob_entity = mob.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        let world = entity.world.load();
        let follow_range = mob_entity
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE);
        let mut predicate = TargetPredicate::create_attackable();
        predicate.base_max_distance = follow_range;

        let mut search_pos = entity.pos.load();
        search_pos.y += f64::from(entity.entity_dimension.load().eye_height);
        let self_id = entity.entity_id;
        let search_box = entity.bounding_box.load().expand_all(follow_range);

        let mut candidates: Vec<(Arc<dyn EntityBase>, f64)> = world
            .get_entities_at_box(&search_box)
            .into_iter()
            .filter_map(|candidate| {
                let candidate_entity = candidate.get_entity();
                if candidate_entity.entity_id == self_id
                    || candidate_entity.entity_type == &EntityType::WITCH
                    || !candidate_entity
                        .entity_type
                        .has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
                {
                    return None;
                }
                let dist = candidate_entity
                    .pos
                    .load()
                    .squared_distance_to_vec(&search_pos);
                Some((candidate, dist))
            })
            .collect();
        candidates.sort_by(|a, b| a.1.total_cmp(&b.1));

        for (candidate, _) in candidates {
            if let Some(living) = candidate.get_living_entity()
                && !TrackTargetGoal::is_allied(mob, candidate.as_ref()).await
                && mob.can_attack(candidate.get_entity())
                && predicate
                    .test(&world, Some(&mob_entity.living_entity), living)
                    .await
            {
                return Some(candidate);
            }
        }
        None
    }
}

impl Default for NearestHealableRaiderTargetGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for NearestHealableRaiderTargetGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(witch) = mob.cast_any().downcast_ref::<WitchEntity>() else {
                return false;
            };
            if witch.heal_cooldown.load(Relaxed) > 0 {
                return false;
            }
            if !mob.get_random().random_bool(0.5) {
                return false;
            }
            if !mob.get_mob_entity().living_entity.has_active_raid() {
                return false;
            }
            self.target = Self::find_target(mob).await;
            self.target.is_some()
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { self.track_target_goal.should_continue(mob).await })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(witch) = mob.cast_any().downcast_ref::<WitchEntity>() {
                witch.heal_cooldown.store(to_goal_ticks(200), Relaxed);
            }
            mob.set_mob_target(self.target.clone()).await;
            self.track_target_goal.start(mob).await;
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
