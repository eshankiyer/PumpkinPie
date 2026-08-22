//! Port of vanilla's villager breeding behavior.
//!
//! Vanilla drives this from the Brain: `VillagerGoalPackages.getIdlePackage`
//! (`VillagerGoalPackages.java:180-183`) sets `MemoryModuleType.BREED_TARGET` through
//! `InteractWith.of(EntityTypes.VILLAGER, 8, AgeableMob::canBreed, AgeableMob::canBreed,
//! BREED_TARGET, speedModifier, 2)`, and `VillagerMakeLove` (`VillagerMakeLove.java:19-119`)
//! runs against that memory. Pumpkin has no Brain/Memory/Activity graph, so - following the
//! `warden.rs` precedent - the memory is replaced by a field on the goal and the activity gate
//! is read from the existing `villager_schedule::villager_activity_for_time` clock.
//!
//! Carried across: the 8-block `canBreed`-filtered partner search, the 275 + rand(50) tick
//! gestation (`VillagerMakeLove.java:45`), the 5.0-squared-distance proximity requirement and
//! walk-up (`VillagerMakeLove.java:51-52`), `eatAndDigestFood` on both parents, the vacant
//! `PoiTypes.HOME` claim that gates whether a baby is produced at all
//! (`VillagerMakeLove.java:64-78, 92-94`), age 6000 on both parents and -24000 on the child
//! (`VillagerMakeLove.java:107-109`), the bed handover to the child
//! (`VillagerMakeLove.java:116-119`), and the entity events 12/13/18.
//!
//! NOT carried across: `BehaviorUtils.targetIsValid`'s visibility test (no
//! `NEAREST_VISIBLE_LIVING_ENTITIES` sensor here) and `canReach`'s path check on the candidate
//! bed (`VillagerMakeLove.java:96-99`), because `Navigator` exposes no "can this path be
//! completed" query. Both only ever *reject* a breed, so their absence makes breeding slightly
//! more permissive, never less.

use std::sync::Arc;

use rand::RngExt;

use super::VillagerEntity;
use crate::entity::{
    EntityBase,
    ai::{
        goal::{Controls, Goal, GoalFuture, villager_schedule},
        pathfinder::NavigatorGoal,
    },
    mob::Mob,
};

/// `VillagerGoalPackages.INTERACT_DIST_SQR` (`VillagerGoalPackages.java:28`).
const INTERACT_DIST_SQR: f64 = 5.0;
/// `VillagerGoalPackages.INTERACT_WALKUP_DIST` (`VillagerGoalPackages.java:29`), squared.
const WALKUP_DIST_SQR: f64 = 4.0;
/// `InteractWith.of(EntityTypes.VILLAGER, 8, ...)` (`VillagerGoalPackages.java:182`).
const PARTNER_SEARCH_RANGE: f64 = 8.0;

pub struct VillagerBreedGoal {
    speed: f64,
    partner: Option<Arc<dyn EntityBase>>,
    /// Ticks left until `VillagerMakeLove.birthTimestamp` is reached.
    birth_countdown: i32,
}

impl VillagerBreedGoal {
    #[must_use]
    pub const fn new(speed: f64) -> Self {
        Self {
            speed,
            partner: None,
            birth_countdown: 0,
        }
    }

    fn as_villager(entity: &Arc<dyn EntityBase>) -> Option<&VillagerEntity> {
        entity.cast_any().downcast_ref::<VillagerEntity>()
    }

    async fn find_partner(villager: &VillagerEntity) -> Option<Arc<dyn EntityBase>> {
        let entity = villager.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        let my_uuid = entity.entity_uuid;

        let mut best: Option<(f64, Arc<dyn EntityBase>)> = None;
        for candidate in world
            .get_nearby_entities(pos, PARTNER_SEARCH_RANGE)
            .values()
        {
            let candidate_entity = candidate.get_entity();
            if candidate_entity.entity_uuid == my_uuid
                || !candidate_entity.is_alive()
                || candidate_entity.entity_type != &pumpkin_data::entity::EntityType::VILLAGER
            {
                continue;
            }
            let Some(other) = Self::as_villager(candidate) else {
                continue;
            };
            if !other.can_breed_villager().await {
                continue;
            }
            let dist = pos.squared_distance_to_vec(&candidate_entity.pos.load());
            match &best {
                Some((best_dist, _)) if dist >= *best_dist => {}
                _ => best = Some((dist, candidate.clone())),
            }
        }
        best.map(|(_, candidate)| candidate)
    }

    /// `BehaviorUtils.lockGazeAndWalkToEachOther(body, target, 0.5F, 2)`
    /// (`VillagerMakeLove.java:42, 52`), reduced to this side's half of the walk: the partner
    /// runs its own copy of this goal and moves itself.
    fn walk_towards(&self, villager: &VillagerEntity, partner: &Arc<dyn EntityBase>) {
        let entity = villager.get_entity();
        let pos = entity.pos.load();
        let partner_pos = partner.get_entity().pos.load();
        entity.look_at(partner.get_entity().get_eye_pos());
        if pos.squared_distance_to_vec(&partner_pos) <= WALKUP_DIST_SQR {
            return;
        }
        villager
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(pos, partner_pos, self.speed));
    }
}

impl Goal for VillagerBreedGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() else {
                return false;
            };
            // `BREED_TARGET` is only ever set from `getIdlePackage`; no other villager
            // activity package contains the `InteractWith` that writes it
            // (`VillagerGoalPackages.java:175-218`).
            let world = villager.get_entity().world.load();
            if villager_schedule::villager_activity_for_time(world.get_time_of_day().await)
                != villager_schedule::VillagerActivity::Idle
            {
                return false;
            }
            if !villager.can_breed_villager().await {
                return false;
            }
            let Some(partner) = Self::find_partner(villager).await else {
                return false;
            };
            self.partner = Some(partner);
            // `VillagerMakeLove.start`: `275 + random.nextInt(50)`.
            self.birth_countdown = 275 + rand::rng().random_range(0..50);
            true
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let (Some(villager), Some(partner)) = (
                mob.cast_any().downcast_ref::<VillagerEntity>(),
                self.partner.clone(),
            ) else {
                return;
            };
            self.walk_towards(villager, &partner);
            // `level.broadcastEntityEvent(body, (byte)18)` - the in-love hearts.
            villager.send_breeding_event(pumpkin_data::entity::EntityStatus::InLoveHearts);
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            // `VillagerMakeLove.canStillUse`: the gestation timer must not have elapsed and
            // breeding must still be possible for both sides. Note this deliberately does not
            // re-check the activity clock - vanilla's behavior keeps running once started.
            if self.birth_countdown < 0 {
                return false;
            }
            let (Some(villager), Some(partner)) = (
                mob.cast_any().downcast_ref::<VillagerEntity>(),
                self.partner.as_ref(),
            ) else {
                return false;
            };
            if !partner.get_entity().is_alive() {
                return false;
            }
            let Some(other) = Self::as_villager(partner) else {
                return false;
            };
            villager.can_breed_villager().await && other.can_breed_villager().await
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let (Some(villager), Some(partner)) = (
                mob.cast_any().downcast_ref::<VillagerEntity>(),
                self.partner.clone(),
            ) else {
                return;
            };
            // Vanilla's `birthTimestamp` is absolute game time, so the behavior always ends
            // ~325 ticks after it started even if the pair never converges
            // (`VillagerMakeLove.canStillUse`, `VillagerMakeLove.java:37`). Decrement before
            // the proximity gate so a pair separated by a fence cannot hold MOVE forever.
            self.birth_countdown -= 1;

            let pos = villager.get_entity().pos.load();
            if pos.squared_distance_to_vec(&partner.get_entity().pos.load()) > INTERACT_DIST_SQR {
                return;
            }
            self.walk_towards(villager, &partner);

            if self.birth_countdown >= 0 {
                // `body.getRandom().nextInt(35) == 0` -> event 12 on both sides.
                if rand::rng().random_range(0..35) == 0 {
                    villager.send_breeding_event(pumpkin_data::entity::EntityStatus::LoveHearts);
                }
                return;
            }

            let Some(other) = Self::as_villager(&partner) else {
                return;
            };
            villager.eat_and_digest_food().await;
            other.eat_and_digest_food().await;
            villager.try_to_give_birth(other).await;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.partner = None;
            self.birth_countdown = 0;
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
