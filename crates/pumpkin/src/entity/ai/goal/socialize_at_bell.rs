use std::sync::Arc;

use rand::RngExt;

use super::{Controls, Goal, GoalFuture};
use crate::entity::EntityBase;
use crate::entity::ai::goal::villager_schedule::{VillagerActivity, villager_activity_for_time};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use pumpkin_data::entity::EntityType;

const SPEED_MODIFIER: f64 = 0.3;
const BELL_DISTANCE_SQR: f64 = 16.0;
const VILLAGER_DISTANCE_SQR: f64 = 32.0;

/// Goal-based server port of `SocializeAtBell.create`.
///
/// Vanilla source: `net/minecraft/world/entity/ai/behavior/SocializeAtBell.java:14-41`.
/// Pumpkin's villager implementation stores the meeting point directly on the entity rather
/// than in a Brain `GlobalPos` memory, so this goal uses that existing accessor and the existing
/// world raycast for the `NEAREST_VISIBLE_LIVING_ENTITIES` check.
pub struct SocializeAtBellGoal {
    target: Option<Arc<dyn EntityBase>>,
}

impl SocializeAtBellGoal {
    #[must_use]
    pub const fn new() -> Self {
        Self { target: None }
    }

    async fn find_target(mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let meeting_point = mob.get_meeting_point()?;
        let position = entity.pos.load();

        // `memory.pos().closerToCenterThan(body.position(), 4.0)`
        // (`SocializeAtBell.java:28-31`).
        if meeting_point
            .to_centered_f64()
            .squared_distance_to_vec(&position)
            >= BELL_DISTANCE_SQR
        {
            return None;
        }

        let mut candidates: Vec<_> = world
            .get_nearby_entities(position, VILLAGER_DISTANCE_SQR.sqrt())
            .into_values()
            .filter(|candidate| {
                candidate.get_entity().entity_id != entity.entity_id
                    && candidate.get_entity().entity_type == &EntityType::VILLAGER
                    && candidate.get_entity().is_alive()
                    && position.squared_distance_to_vec(&candidate.get_entity().pos.load())
                        <= VILLAGER_DISTANCE_SQR
            })
            .collect();
        candidates.sort_by(|a, b| {
            position
                .squared_distance_to_vec(&a.get_entity().pos.load())
                .total_cmp(&position.squared_distance_to_vec(&b.get_entity().pos.load()))
        });

        for candidate in candidates {
            // `NearestVisibleLivingEntities` supplies the visibility gate used by
            // `SocializeAtBell.java:20,27-32`. This is the same server raycast used by the
            // villager gossip path (`VillagerEntity` in `villager/mod.rs`).
            let visible = world
                .raycast(
                    entity.get_eye_pos(),
                    candidate.get_entity().get_eye_pos(),
                    async |block_pos, world| world.get_block_state(block_pos).is_solid(),
                )
                .await
                .is_none();
            if visible {
                return Some(candidate);
            }
        }
        None
    }
}

impl Default for SocializeAtBellGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for SocializeAtBellGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let world = mob.get_entity().world.load();
            if villager_activity_for_time(world.get_time_of_day().await) != VillagerActivity::Meet
                || mob.get_random().random_range(0..100) != 0
            {
                return false;
            }

            self.target = Self::find_target(mob).await;
            self.target.is_some()
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(target) = self.target.as_ref() else {
                return false;
            };
            let world = mob.get_entity().world.load();
            let Some(meeting_point) = mob.get_meeting_point() else {
                return false;
            };
            let position = mob.get_entity().pos.load();
            let target_position = target.get_entity().pos.load();
            villager_activity_for_time(world.get_time_of_day().await) == VillagerActivity::Meet
                && target.get_entity().is_alive()
                && meeting_point
                    .to_centered_f64()
                    .squared_distance_to_vec(&position)
                    < BELL_DISTANCE_SQR
                && position.squared_distance_to_vec(&target_position) > 1.0
                && !mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_idle()
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            // `interactionTarget.set`, `lookTarget.set`, and `walkTarget.set` in
            // `SocializeAtBell.java:32-35` become the existing goal target, look control, and
            // navigator state.
            if let Some(target) = self.target.as_ref() {
                let position = mob.get_entity().pos.load();
                let target_position = target.get_entity().pos.load();
                mob.get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_progress(NavigatorGoal::new(
                        position,
                        target_position,
                        SPEED_MODIFIER,
                    ));
            }
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            // `new EntityTracker(mob, true)` (`SocializeAtBell.java:34`) is the look target.
            if let Some(target) = self.target.as_ref() {
                mob.get_mob_entity()
                    .look_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .look_at_entity(mob, target);
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.target = None;
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
