use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use super::{Controls, Goal};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::passive::wandering_trader::WanderingTraderEntity;

const FOLLOW_RANGE: f64 = 16.0;

/// Vanilla `TraderLlama.TraderLlamaDefendWanderingTraderGoal` (`TraderLlama.java:132-165`).
///
/// Modeled directly on `owner_hurt_by_target.rs`'s dog-defends-owner shape: "owner"
/// resolution is `leashed_to` downcast to `WanderingTraderEntity` instead of
/// `get_owner_uuid()` + player lookup, and there is no sitting check (llamas don't sit).
pub struct TraderLlamaDefendWanderingTraderGoal {
    target: Option<Arc<dyn EntityBase>>,
    last_attacked_time: i32,
}

impl TraderLlamaDefendWanderingTraderGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            target: None,
            last_attacked_time: 0,
        })
    }
}

impl Goal for TraderLlamaDefendWanderingTraderGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = &mob.get_mob_entity().living_entity.entity;
        let holder = entity
            .leashed_to
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(holder) = holder else {
            return false;
        };
        let Some(trader) = holder.cast_any().downcast_ref::<WanderingTraderEntity>() else {
            return false;
        };

        let attacked_time = trader
            .mob_entity
            .living_entity
            .last_attacked_time
            .load(Relaxed);
        if attacked_time == self.last_attacked_time {
            return false;
        }

        let attacker_id = trader
            .mob_entity
            .living_entity
            .last_attacker_id
            .load(Relaxed);
        if attacker_id == 0 {
            return false;
        }

        let world = entity.world.load_full();
        let Some(attacker) = world.get_entity_by_id(attacker_id) else {
            return false;
        };

        if !attacker.get_entity().is_alive() {
            return false;
        }

        if !mob.can_attack_with_owner(attacker.as_ref(), holder.as_ref()) {
            return false;
        }

        self.target = Some(attacker);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let target = mob
            .get_mob_entity()
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(t) = target.as_ref() else {
            return false;
        };
        if !t.get_entity().is_alive() {
            return false;
        }
        let my_pos = mob.get_entity().pos.load();
        let target_pos = t.get_entity().pos.load();
        my_pos.squared_distance_to_vec(&target_pos) <= FOLLOW_RANGE * FOLLOW_RANGE
    }

    fn start(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone_from(&self.target);

        let entity = &mob_entity.living_entity.entity;
        if let Some(holder) = entity
            .leashed_to
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            && let Some(trader) = holder.cast_any().downcast_ref::<WanderingTraderEntity>()
        {
            self.last_attacked_time = trader
                .mob_entity
                .living_entity
                .last_attacked_time
                .load(Relaxed);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        *mob.get_mob_entity()
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn controls(&self) -> Controls {
        Controls::TARGET
    }
}
