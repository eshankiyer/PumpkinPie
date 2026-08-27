use std::sync::atomic::Ordering;

use pumpkin_util::GameMode;

use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;
use crate::entity::passive::parrot::ParrotEntity;

/// `LandOnOwnersShoulderGoal` (`LandOnOwnersShoulderGoal.java:1-40`): a tamed, un-sat parrot
/// that isn't leashed walks onto its owner's shoulder once their bounding boxes overlap.
///
/// Vanilla generalizes this over `ShoulderRidingEntity`, whose only concrete subclass is
/// `Parrot`; ported directly against `ParrotEntity` rather than introducing that
/// intermediate abstraction for a single user.
pub struct LandOnOwnersShoulderGoal {
    is_sitting_on_shoulder: bool,
}

impl LandOnOwnersShoulderGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            is_sitting_on_shoulder: false,
        })
    }
}

impl Goal for LandOnOwnersShoulderGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            let Some(owner_uuid) = mob_entity.owner.load() else {
                return false;
            };
            let world = mob_entity.living_entity.entity.world.load();
            let Some(owner) = world.get_player_by_uuid(owner_uuid) else {
                return false;
            };

            // `ownerThatCanBeSatOn`: not spectator, not flying, not in water, not in
            // powder snow (`LandOnOwnersShoulderGoal.java:17-19`).
            let owner_can_be_sat_on = owner.gamemode.load() != GameMode::Spectator
                && !owner.abilities.lock().await.flying
                && !owner
                    .living_entity
                    .entity
                    .touching_water
                    .load(Ordering::Relaxed)
                && !owner.living_entity.entity.is_in_powder_snow();

            let Some(parrot) = mob.cast_any().downcast_ref::<ParrotEntity>() else {
                return false;
            };

            !mob_entity.is_ordered_to_sit() && owner_can_be_sat_on && parrot.can_sit_on_shoulder()
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.is_sitting_on_shoulder = false;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if self.is_sitting_on_shoulder {
                return;
            }
            let mob_entity = mob.get_mob_entity();
            if mob_entity.is_ordered_to_sit() || mob_entity.living_entity.entity.is_leashed().await
            {
                return;
            }
            let Some(owner_uuid) = mob_entity.owner.load() else {
                return;
            };
            let world = mob_entity.living_entity.entity.world.load();
            let Some(owner) = world.get_player_by_uuid(owner_uuid) else {
                return;
            };

            let mob_bb = mob_entity.living_entity.entity.bounding_box.load();
            let owner_bb = owner.living_entity.entity.bounding_box.load();
            if !mob_bb.intersects(&owner_bb) {
                return;
            }

            let Some(parrot) = mob.cast_any().downcast_ref::<ParrotEntity>() else {
                return;
            };
            self.is_sitting_on_shoulder = parrot.set_entity_on_shoulder(&owner).await;
        })
    }

    /// `Goal.isInterruptable` isn't ported as its own trait method in this codebase, but
    /// vanilla stops interrupting this goal once perched (`LandOnOwnersShoulderGoal.java:24-27`);
    /// re-declining every `can_start` after the parrot discards itself achieves the same effect,
    /// since a discarded entity's goal selector no longer ticks.
    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { !self.is_sitting_on_shoulder })
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
