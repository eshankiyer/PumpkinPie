//! Port of `ShowTradesToPlayer.java`.
//!
//! Vanilla drives this from the Brain: `SetLookAndInteract.create(EntityTypes.PLAYER, 4)`
//! (`VillagerGoalPackages.java:95,156`) writes `INTERACTION_TARGET`, and
//! `ShowTradesToPlayer` (`ShowTradesToPlayer.java:18-135`) runs against that memory in the
//! WORK/MEET/IDLE packages (`VillagerGoalPackages.java:94,155,195`). Pumpkin's villager is
//! Goal-driven (see `breed.rs` for the same mapping applied to `VillagerMakeLove`), so this
//! goal fuses "find a nearby player" into its own start conditions instead of reading the
//! memory.

use std::sync::Arc;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_protocol::java::client::play::MerchantOffer;

use super::{Controls, Goal, GoalFuture};
use crate::entity::ageable::AgeableMob;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::passive::villager::VillagerEntity;
use crate::entity::{EntityBase, mob::Mob};

/// `ShowTradesToPlayer.MAX_LOOK_TIME` (`ShowTradesToPlayer.java:19`).
const MAX_LOOK_TIME: i32 = 900;
/// `ShowTradesToPlayer.STARTING_LOOK_TIME` (`ShowTradesToPlayer.java:20`).
const STARTING_LOOK_TIME: i32 = 40;
/// `checkExtraStartConditions` distance gate `body.distanceToSqr(target) <= 17.0`
/// (`ShowTradesToPlayer.java:38`).
const MAX_DISTANCE_SQR: f64 = 17.0;
/// Vanilla villager default main-hand drop chance restored on stop
/// (`clearHeldItem`, `ShowTradesToPlayer.java:109`).
const DEFAULT_MAIN_HAND_DROP_CHANCE: f32 = 0.085;

pub struct ShowTradesToPlayerGoal {
    /// The interaction target picked in `can_start`; held fixed for the run, matching
    /// vanilla's stable `INTERACTION_TARGET` memory for the behavior's lifetime.
    target: Option<Arc<dyn EntityBase>>,
    /// `playerItemStack` (`ShowTradesToPlayer.java:21`), tracked by item id like vanilla's
    /// `ItemStack.isSameItem` check (`ShowTradesToPlayer.java:76`).
    player_item_id: Option<u16>,
    /// `displayItems` (`ShowTradesToPlayer.java:22`).
    display_items: Vec<ItemStack>,
    cycle_counter: i32,
    display_index: usize,
    look_time: i32,
}

impl Default for ShowTradesToPlayerGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl ShowTradesToPlayerGoal {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            target: None,
            player_item_id: None,
            display_items: Vec::new(),
            cycle_counter: 0,
            display_index: 0,
            look_time: 0,
        }
    }

    /// Fused entry condition: `INTERACTION_TARGET` is only ever a player within the
    /// interact radius (`SetLookAndInteract.create(EntityTypes.PLAYER, 4)`), and
    /// `checkExtraStartConditions` (`ShowTradesToPlayer.java:31-39`) additionally requires
    /// both parties alive and squared distance at most 17.
    async fn find_interaction_target(villager: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let entity = villager.get_entity();
        let self_pos = entity.pos.load();
        let world = entity.world.load();
        // Rejects spectators via `is_part_of_game`, same as every Look-style goal here.
        let predicate = TargetPredicate::create_non_attackable().ignore_visibility();

        let mut best: Option<(f64, Arc<dyn EntityBase>)> = None;
        for candidate in world
            .get_nearby_entities(self_pos, MAX_DISTANCE_SQR.sqrt())
            .values()
        {
            if candidate.get_entity().entity_type != &EntityType::PLAYER
                || !candidate.get_entity().is_alive()
            {
                continue;
            }
            let Some(living) = candidate.get_living_entity() else {
                continue;
            };
            if !predicate.test(&world, None, living).await {
                continue;
            }
            let dist = self_pos.squared_distance_to_vec(&candidate.get_entity().pos.load());
            if dist > MAX_DISTANCE_SQR {
                continue;
            }
            match &best {
                Some((best_dist, _)) if dist >= *best_dist => {}
                _ => best = Some((dist, Arc::clone(candidate))),
            }
        }
        best.map(|(_, candidate)| candidate)
    }

    /// `updateDisplayItems` (`ShowTradesToPlayer.java:95-101`): every in-stock offer whose
    /// cost A or cost B is the item the player is holding contributes its result stack
    /// (`offer.assemble()`).
    async fn update_display_items(
        &self,
        held_id: u16,
        villager: &VillagerEntity,
    ) -> Vec<ItemStack> {
        let offers = villager.offers.lock().await;
        offers
            .iter()
            .filter(|offer| Self::offer_matches(offer, held_id))
            .map(|offer| (*offer.output.0).clone())
            .collect()
    }

    /// `playerItemStackMatchesCostOfOffer` (`ShowTradesToPlayer.java:103-105`) plus the
    /// `!offer.isOutOfStock()` filter (`ShowTradesToPlayer.java:97`).
    fn offer_matches(offer: &MerchantOffer, held_id: u16) -> bool {
        if offer.is_out_of_stock() {
            return false;
        }
        offer.base_cost_a.0.item.id == held_id
            || offer
                .cost_b
                .as_ref()
                .is_some_and(|cost_b| cost_b.0.item.id == held_id)
    }

    /// `lookAtTarget` (`ShowTradesToPlayer.java:117-122`): pin the head on the player.
    fn look_at_target(mob: &dyn Mob, target: &Arc<dyn EntityBase>) {
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at(mob, target_pos.x, target_entity.get_eye_y(), target_pos.z);
    }

    /// `displayAsHeldItem` (`ShowTradesToPlayer.java:112-115`): put the stack in the main
    /// hand and zero its drop chance so the displayed offer can never be dropped.
    async fn display_as_held_item(villager: &VillagerEntity, stack: ItemStack) {
        let living = &villager.mob_entity.living_entity;
        living
            .entity_equipment
            .lock()
            .await
            .equipment
            .insert(EquipmentSlot::MAIN_HAND, stack.clone());
        living
            .equipment_drop_chances
            .lock()
            .await
            .insert(EquipmentSlot::MAIN_HAND.clone(), 0.0);
        living.send_equipment_changes(&[(EquipmentSlot::MAIN_HAND, stack)]);
    }

    /// `clearHeldItem` (`ShowTradesToPlayer.java:107-110`): empty main hand and restore the
    /// default villager drop chance.
    async fn clear_held_item(villager: &VillagerEntity) {
        Self::display_as_held_item(villager, ItemStack::EMPTY.clone()).await;
        villager
            .mob_entity
            .living_entity
            .equipment_drop_chances
            .lock()
            .await
            .insert(
                EquipmentSlot::MAIN_HAND.clone(),
                DEFAULT_MAIN_HAND_DROP_CHANCE,
            );
    }

    /// `tick` body (`ShowTradesToPlayer.java:53-64`), minus the look-time bookkeeping the
    /// goal lifecycle owns.
    async fn tick_inner(&mut self, villager: &VillagerEntity, target: &Arc<dyn EntityBase>) {
        // `findItemsToDisplay` (`ShowTradesToPlayer.java:73-89`): recompute the display list
        // whenever the player's held item changes.
        let held = villager
            .mob_entity
            .living_entity
            .held_item(target.as_ref())
            .await;
        let held_id = (!held.is_empty()).then_some(held.item.id);
        let changed = self.player_item_id != held_id;
        if changed {
            self.player_item_id = held_id;
            self.display_items.clear();
        }

        if changed && let Some(held_id) = held_id {
            self.display_items = self.update_display_items(held_id, villager).await;
            if !self.display_items.is_empty() {
                // `findItemsToDisplay` lines 85-86: `this.lookTime = 900; displayFirstItem`.
                self.look_time = MAX_LOOK_TIME;
                let first = self.display_items[0].clone();
                Self::display_as_held_item(villager, first).await;
            }
        }

        if self.display_items.is_empty() {
            // `tick` lines 58-61: nothing to show; cap the remaining look time.
            Self::clear_held_item(villager).await;
            self.look_time = self.look_time.min(STARTING_LOOK_TIME);
        } else {
            // `displayCyclingItems` (`ShowTradesToPlayer.java:124-134`): rotate through two
            // or more offers once every 40 ticks.
            if self.display_items.len() >= 2 {
                self.cycle_counter += 1;
                if self.cycle_counter >= 40 {
                    self.cycle_counter = 0;
                    self.display_index += 1;
                    if self.display_index > self.display_items.len() - 1 {
                        self.display_index = 0;
                    }
                    let next = self.display_items[self.display_index].clone();
                    Self::display_as_held_item(villager, next).await;
                }
            }
        }

        self.look_time -= 1;
    }
}

impl Goal for ShowTradesToPlayerGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() else {
                return false;
            };
            // `!body.isBaby()` (`ShowTradesToPlayer.java:38`); babies have no trades anyway.
            if villager.is_baby() {
                return false;
            }
            let Some(target) = Self::find_interaction_target(mob).await else {
                return false;
            };
            self.target = Some(target);
            true
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            // `canStillUse` (`ShowTradesToPlayer.java:41-43`): conditions must still hold and
            // look time must remain. The stored target standing in for `INTERACTION_TARGET`
            // must still be alive and in range.
            if self.look_time <= 0 {
                return false;
            }
            let Some(target) = self.target.clone() else {
                return false;
            };
            if !target.get_entity().is_alive() {
                return false;
            }
            let dist_sq = mob
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&target.get_entity().pos.load());
            dist_sq <= MAX_DISTANCE_SQR
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            // `start` (`ShowTradesToPlayer.java:45-51`).
            self.cycle_counter = 0;
            self.display_index = 0;
            self.look_time = STARTING_LOOK_TIME;
            if let Some(target) = self.target.clone() {
                Self::look_at_target(mob, &target);
            }
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let (Some(villager), Some(target)) = (
                mob.cast_any().downcast_ref::<VillagerEntity>(),
                self.target.clone(),
            ) else {
                return;
            };
            Self::look_at_target(mob, &target);
            self.tick_inner(villager, &target).await;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            // `stop` (`ShowTradesToPlayer.java:66-71`). Erasing INTERACTION_TARGET is
            // implicit: the goal drops its target reference.
            self.target = None;
            self.player_item_id = None;
            if let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() {
                Self::clear_held_item(villager).await;
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        // Vanilla registers this alongside movement behaviors with no MOVE claim of its
        // own; it only pins the look target (`lookAtTarget`, line 120).
        Controls::LOOK
    }
}
