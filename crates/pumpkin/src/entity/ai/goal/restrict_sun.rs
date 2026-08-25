//! Port of vanilla `RestrictSunGoal`
//! (`net/minecraft/world/entity/ai/goal/RestrictSunGoal.java:8-33`).
//!
//! While the mob stands in daylight without head armor, this goal flags its ground navigator's
//! avoid-sun mode (`GroundPathNavigation.setAvoidSun(true)`), which makes every navigation tick
//! truncate the active path before its first sky-exposed node so shade-followers (skeletons)
//! never path into daylight. The navigation half lives in
//! [`crate::entity::ai::pathfinder::Navigator::trim_avoiding_sun`].
//!
//! Registered at priority 2 for all skeleton variants (`AbstractSkeleton.java:76`); pumpkin has
//! no other `RestrictSunGoal` registrant.

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::data_component_impl::EquipmentSlot;

use super::{Controls, Goal, GoalFuture};
use crate::entity::ai::pathfinder::{NavigationKind, Navigator};
use crate::entity::mob::Mob;

/// `Level.isBrightOutside` (`Level.java:384-386`): `skyDarken < 4`.
const BRIGHT_OUTSIDE_SKY_DARKEN: u8 = 4;

pub struct RestrictSunGoal;

impl RestrictSunGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self)
    }

    /// `GoalUtils.hasGroundPathNavigation` (`GoalUtils.java`): the mob navigates on the ground.
    const fn has_ground_navigation(navigator: &Navigator) -> bool {
        matches!(navigator.navigation_kind(), NavigationKind::Ground)
    }
}

impl Default for RestrictSunGoal {
    fn default() -> Self {
        Self
    }
}

impl Goal for RestrictSunGoal {
    /// Vanilla `canUse` (`RestrictSunGoal.java:16-18`):
    /// `level().isBrightOutside() && getItemBySlot(HEAD).isEmpty() && hasGroundPathNavigation(mob)`.
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            let living = &mob_entity.living_entity;
            let entity = &living.entity;

            let world = entity.world.load();
            // Fixed-time dimensions never have daylight cycles; treat them as not bright so
            // sun-restriction never engages there (matches FleeSunGoal's reading).
            if world.dimension.fixed_time.is_some()
                || world.sky_darken.load(Relaxed) >= BRIGHT_OUTSIDE_SKY_DARKEN
            {
                return false;
            }

            if let Ok(equipment) = living.entity_equipment.try_lock()
                && !equipment.get(&EquipmentSlot::HEAD).is_empty()
            {
                return false;
            }

            let navigator = mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Self::has_ground_navigation(&navigator)
        })
    }

    /// Vanilla `start` (`RestrictSunGoal.java:21-25`): `pathNavigation.setAvoidSun(true)`.
    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let mut navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if Self::has_ground_navigation(&navigator) {
                navigator.set_avoid_sun(true);
            }
        })
    }

    /// Vanilla `stop` (`RestrictSunGoal.java:28-32`): clears the flag on the same guard.
    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let mut navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if Self::has_ground_navigation(&navigator) {
                navigator.set_avoid_sun(false);
            }
        })
    }

    /// Vanilla `RestrictSunGoal` registers no `Goal.Flag` set.
    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
