use std::collections::HashSet;
use std::sync::{Arc, PoisonError, Weak};

use pumpkin_data::attributes::Attributes;
use pumpkin_util::math::position::BlockPos;

use super::interact_with_door::InteractWithDoorGoal;
use super::{Controls, Goal, GoalFuture};
use crate::block::blocks::doors::set_door_open;
use crate::entity::mob::Mob;
use crate::world::World;

/// `InteractWithDoor.COOLDOWN_BEFORE_RERUNNING_IN_SAME_NODE`.
const COOLDOWN_BEFORE_RERUNNING_IN_SAME_NODE: i32 = 20;
/// `InteractWithDoor.SKIP_CLOSING_DOOR_IF_FURTHER_AWAY_THAN`.
const SKIP_CLOSING_DOOR_IF_FURTHER_AWAY_THAN: f64 = 3.0;
/// `InteractWithDoor.MAX_DISTANCE_TO_HOLD_DOOR_OPEN_FOR_OTHER_MOBS`.
const MAX_DISTANCE_TO_HOLD_DOOR_OPEN_FOR_OTHER_MOBS: f64 = 2.0;

/// Vanilla brain behaviour `InteractWithDoor` (`InteractWithDoor.java:32-83`), run every tick in
/// the core activity of villagers, copper golems, piglins and piglin brutes.
///
/// Opens the mob-interactable door the mob is walking out of or into along its path, remembers
/// it (the `DOORS_TO_CLOSE` memory), and closes remembered doors once the mob has moved a node
/// past them, unless another mob of the same type is walking through. Unlike
/// [`InteractWithDoorGoal`] (the `DoorInteractGoal` port), it needs no collision to trigger.
pub struct BrainInteractWithDoorGoal {
    last_checked_node: Option<BlockPos>,
    remaining_cooldown: i32,
    /// `MemoryModuleType.DOORS_TO_CLOSE`. Vanilla stores `GlobalPos`; the set is cleared when
    /// the mob changes world, which is what `isDoorTooFarAway`'s dimension test amounts to.
    doors_to_close: Option<HashSet<BlockPos>>,
    doors_world: Weak<World>,
}

impl BrainInteractWithDoorGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            last_checked_node: None,
            remaining_cooldown: 0,
            doors_to_close: None,
            doors_world: Weak::new(),
        })
    }

    /// `InteractWithDoor.closeDoorsThatIHaveOpenedOrPassedThrough` (`InteractWithDoor.java:85-119`).
    async fn close_doors_passed_through(
        world: &Arc<World>,
        mob: &dyn Mob,
        moving_from: Option<BlockPos>,
        moving_to: Option<BlockPos>,
        doors: &mut HashSet<BlockPos>,
    ) {
        let candidates: Vec<BlockPos> = doors
            .iter()
            .copied()
            .filter(|door| moving_from != Some(*door) && moving_to != Some(*door))
            .collect();
        let mob_pos = mob.get_entity().pos.load();
        for door in candidates {
            // Every remembered door that is not on the current step is forgotten; it is only
            // closed when it is near, still an open interactable door, and nobody is following.
            doors.remove(&door);
            let too_far = door.to_centered_f64().squared_distance_to_vec(&mob_pos)
                >= SKIP_CLOSING_DOOR_IF_FURTHER_AWAY_THAN * SKIP_CLOSING_DOOR_IF_FURTHER_AWAY_THAN;
            if too_far
                || !InteractWithDoorGoal::is_mob_interactable_door(world, &door)
                || !InteractWithDoorGoal::is_door_open(world, &door)
                || Self::are_other_mobs_coming_through_door(world, mob, &door)
            {
                continue;
            }
            set_door_open(world, &door, false).await;
        }
    }

    /// `InteractWithDoor.areOtherMobsComingThroughDoor` (`InteractWithDoor.java:121-129`) over
    /// the `NEAREST_LIVING_ENTITIES` set (`NearestLivingEntitySensor`: bounding box inflated by
    /// `FOLLOW_RANGE`, alive, not self).
    fn are_other_mobs_coming_through_door(world: &World, mob: &dyn Mob, door: &BlockPos) -> bool {
        let living = &mob.get_mob_entity().living_entity;
        let entity = &living.entity;
        let follow_range = living.get_attribute_value(&Attributes::FOLLOW_RANGE);
        let search_box = entity.bounding_box.load().expand_all(follow_range);
        let door_center = door.to_centered_f64();
        let max_dist_sq = MAX_DISTANCE_TO_HOLD_DOOR_OPEN_FOR_OTHER_MOBS
            * MAX_DISTANCE_TO_HOLD_DOOR_OPEN_FOR_OTHER_MOBS;
        world.get_entities_at_box(&search_box).iter().any(|other| {
            let other_entity = other.get_entity();
            other_entity.entity_id != entity.entity_id
                && other_entity.is_alive()
                && other_entity.entity_type == entity.entity_type
                && door_center.squared_distance_to_vec(&other_entity.pos.load()) < max_dist_sq
                && other
                    .get_mob()
                    .is_some_and(|other_mob| Self::is_mob_coming_through_door(other_mob, door))
        })
    }

    /// `InteractWithDoor.isMobComingThroughDoor` (`InteractWithDoor.java:131-148`).
    fn is_mob_coming_through_door(mob: &dyn Mob, door: &BlockPos) -> bool {
        let navigator = mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(path) = navigator.get_current_path() else {
            return false;
        };
        if path.is_done() {
            return false;
        }
        let Some(moving_from) = path.get_previous_node() else {
            return false;
        };
        moving_from.pos == *door || path.get_next_node().is_some_and(|node| node.pos == *door)
    }
}

impl Goal for BrainInteractWithDoorGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let world = mob.get_entity().world.load_full();
            let current_world = Arc::downgrade(&world);
            if !Weak::ptr_eq(&self.doors_world, &current_world) {
                if let Some(doors) = self.doors_to_close.as_mut() {
                    doors.clear();
                }
                self.doors_world = current_world;
            }

            let (moving_from, moving_to) = {
                let navigator = mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let Some(path) = navigator.get_current_path() else {
                    return;
                };
                if path.not_started() || path.is_done() {
                    return;
                }
                let (Some(from), Some(to)) = (path.get_previous_node(), path.get_next_node())
                else {
                    return;
                };
                (from.pos, to.pos)
            };

            // `InteractWithDoor.java:43-49`: the cooldown is re-armed while the next node is
            // unchanged and only counts down once it changes.
            if self.last_checked_node == Some(moving_to) {
                self.remaining_cooldown = COOLDOWN_BEFORE_RERUNNING_IN_SAME_NODE;
            } else {
                self.remaining_cooldown -= 1;
                if self.remaining_cooldown > 0 {
                    return;
                }
            }
            self.last_checked_node = Some(moving_to);

            // The door being left is always remembered, even if it was already open.
            if InteractWithDoorGoal::is_mob_interactable_door(&world, &moving_from) {
                if !InteractWithDoorGoal::is_door_open(&world, &moving_from) {
                    set_door_open(&world, &moving_from, true).await;
                }
                self.doors_to_close
                    .get_or_insert_with(HashSet::new)
                    .insert(moving_from);
            }

            // The door being entered is only remembered when it is opened here.
            if InteractWithDoorGoal::is_mob_interactable_door(&world, &moving_to)
                && !InteractWithDoorGoal::is_door_open(&world, &moving_to)
            {
                set_door_open(&world, &moving_to, true).await;
                self.doors_to_close
                    .get_or_insert_with(HashSet::new)
                    .insert(moving_to);
            }

            if let Some(doors) = self.doors_to_close.as_mut() {
                Self::close_doors_passed_through(
                    &world,
                    mob,
                    Some(moving_from),
                    Some(moving_to),
                    doors,
                )
                .await;
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
