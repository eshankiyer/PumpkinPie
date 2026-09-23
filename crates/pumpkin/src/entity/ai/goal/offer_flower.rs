use std::sync::Arc;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::boundingbox::BoundingBox;
use rand::RngExt;

use super::fox_behavior::is_bright_outside;
use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::{EntityBase, mob::Mob};

/// Vanilla: `OfferFlowerGoal.OFFER_TICKS` -- how long the golem holds the offering pose.
pub const OFFER_TICKS: i32 = 400;
/// Vanilla: `OfferFlowerGoal.OFFER_TARGET_CONTEXT` -- `TargetingConditions.forNonCombat().range(6.0)`.
const OFFER_RANGE: f64 = 6.0;
/// Vanilla rolls `golem.getRandom().nextInt(8000) != 0` every time `canUse` is polled.
const START_CHANCE: i32 = 8000;
/// Vanilla: `CopperGolem.EQUIPMENT_SLOT_ANTENNA` (`CopperGolem.java:87`).
const EQUIPMENT_SLOT_ANTENNA: EquipmentSlot = EquipmentSlot::SADDLE;

/// Makes an iron golem periodically walk up to and face a nearby villager (or copper golem) and
/// hold out a poppy for a while; a copper golem that is still in reach when the offer runs out
/// gets the poppy on its antenna.
///
/// Vanilla source: `net/minecraft/world/entity/ai/goal/OfferFlowerGoal.java`.
pub struct OfferFlowerGoal {
    entity: Option<Arc<dyn EntityBase>>,
    pub tick: i32,
}

impl Default for OfferFlowerGoal {
    fn default() -> Self {
        Self {
            entity: None,
            tick: 0,
        }
    }
}

impl OfferFlowerGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self::default())
    }

    /// `OfferFlowerGoal.getGolemBoundingBox` (`OfferFlowerGoal.java:88-90`).
    fn golem_bounding_box(mob: &dyn Mob) -> BoundingBox {
        mob.get_entity().bounding_box.load().expand(6.0, 2.0, 6.0)
    }

    /// `ServerLevel.getNearestEntity(CANDIDATE_FOR_IRON_GOLEM_GIFT, OFFER_TARGET_CONTEXT, golem,
    /// x, y, z, getGolemBoundingBox())`: the nearest tagged living entity inside the inflated box
    /// that passes the non-combat targeting conditions.
    async fn find_candidate(mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let mob_entity = mob.get_mob_entity();
        let self_entity = &mob_entity.living_entity.entity;
        let pos = self_entity.pos.load();
        let world = self_entity.world.load_full();
        let conditions =
            TargetPredicate::create_non_attackable().set_base_max_distance(OFFER_RANGE);

        let mut candidates: Vec<(Arc<dyn EntityBase>, f64)> = world
            .get_entities_at_box(&Self::golem_bounding_box(mob))
            .into_iter()
            .filter(|candidate| {
                let candidate_entity = candidate.get_entity();
                candidate_entity.entity_id != self_entity.entity_id
                    && candidate_entity
                        .entity_type
                        .has_tag(&tag::EntityType::MINECRAFT_CANDIDATE_FOR_IRON_GOLEM_GIFT)
            })
            .map(|candidate| {
                let dist_sq = candidate
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&pos);
                (candidate, dist_sq)
            })
            .collect();
        candidates.sort_by(|(_, a), (_, b)| a.total_cmp(b));

        for (candidate, _) in candidates {
            if let Some(living) = candidate.get_living_entity()
                && conditions
                    .test(&world, Some(&mob_entity.living_entity), living)
                    .await
            {
                return Some(candidate);
            }
        }
        None
    }

    /// `IronGolem.offerFlower` (`IronGolem.java`): sets the golem's own offer timer and
    /// broadcasts entity event 11/34. Falls back to the bare entity event for non-golem users.
    fn offer_flower(mob: &dyn Mob, offer: bool) {
        if let Some(golem) = mob.as_iron_golem() {
            golem.offer_flower(offer);
        } else {
            let entity = mob.get_entity();
            let status = if offer {
                EntityStatus::OfferFlower
            } else {
                EntityStatus::StopOfferFlower
            };
            entity.world.load().send_entity_status(entity, status, None);
        }
    }
}

impl Goal for OfferFlowerGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            // Vanilla: `golem.level().isBrightOutside()` (`Level.java:385-387`).
            if !is_bright_outside(&mob.get_entity().world.load()) {
                return false;
            }

            if mob.get_random().random_range(0..START_CHANCE) != 0 {
                return false;
            }

            self.entity = Self::find_candidate(mob).await;
            self.entity.is_some()
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { self.tick > 0 })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            // `this.adjustedTickDelay(400)`: the goal does not tick every server tick, so the
            // countdown is halved and decremented once per goal tick.
            self.tick = to_goal_ticks(OFFER_TICKS);
            Self::offer_flower(mob, true);
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            Self::offer_flower(mob, false);

            // `OfferFlowerGoal.stop` (:62-72): a gift-accepting mob (the copper golem) still in
            // reach when the full offer ran out gets the poppy on its empty antenna slot.
            if self.tick == 0
                && let Some(entity) = self.entity.as_ref()
                && let Some(target) = entity.get_mob()
                && entity
                    .get_entity()
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_ACCEPTS_IRON_GOLEM_GIFT)
                && Self::golem_bounding_box(mob)
                    .intersects(&entity.get_entity().bounding_box.load())
            {
                // Read the slot in its own statement so the equipment lock is released before
                // `set_item_slot_and_drop_when_killed` re-acquires it.
                let antenna_empty = target
                    .get_mob_entity()
                    .living_entity
                    .entity_equipment
                    .lock()
                    .await
                    .get(&EQUIPMENT_SLOT_ANTENNA)
                    .is_empty();
                if antenna_empty {
                    target
                        .set_item_slot_and_drop_when_killed(
                            EQUIPMENT_SLOT_ANTENNA,
                            ItemStack::new(1, &Item::POPPY),
                        )
                        .await;
                }
            }

            self.entity = None;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(entity) = self.entity.as_ref() {
                mob.get_mob_entity()
                    .look_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .look_at_entity_with_range(entity, 30.0, 30.0);
            }
            self.tick -= 1;
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_flower_goal_lifecycle() {
        let mut goal = OfferFlowerGoal::default();
        assert_eq!(goal.tick, 0);
        assert!(goal.entity.is_none());

        goal.tick = to_goal_ticks(OFFER_TICKS);
        assert_eq!(goal.tick, 200);
        let controls = goal.controls();
        assert!(controls.get(Controls::MOVE));
        assert!(controls.get(Controls::LOOK));
    }
}
