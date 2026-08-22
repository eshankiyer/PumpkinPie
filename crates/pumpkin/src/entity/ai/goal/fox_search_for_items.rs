//! `Fox.FoxSearchForItemsGoal` (`Fox.java:1243-1275`), registered at priority 11
//! (`Fox.java:201`).
//!
//! Only the navigation half lives here, exactly as in vanilla: the pickup itself runs from
//! `Mob::mob_try_pick_up_items` through `FoxEntity`'s `wants_to_pick_up_item`/`on_item_pickup`
//! overrides, which is where `Fox.pickUpItem` (`Fox.java:535-551`) is modelled.

use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::fox_behavior::can_fox_move;
use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::passive::fox::FoxEntity;

/// `getBoundingBox().inflate(8.0, 8.0, 8.0)` (`Fox.java:1255`).
const SEARCH_RADIUS: f64 = 8.0;
/// `getRandom().nextInt(reducedTickDelay(10)) != 0` (`Fox.java:1251`).
const TRY_INTERVAL: i32 = 10;
/// `getNavigation().moveTo(items.get(0), 1.2F)` (`Fox.java:1257`).
const SPEED: f64 = 1.2;

pub struct FoxSearchForItemsGoal;

impl FoxSearchForItemsGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self)
    }

    /// `Fox.ALLOWED_ITEMS` (`Fox.java:126`): alive, and past its pickup delay.
    fn nearest_allowed_item(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let search_box =
            entity
                .bounding_box
                .load()
                .expand(SEARCH_RADIUS, SEARCH_RADIUS, SEARCH_RADIUS);
        for candidate in world.get_entities_at_box(&search_box) {
            if let Some(item) = candidate.get_item_entity()
                && item.get_entity().is_alive()
                && !item.has_pickup_delay()
            {
                return Some(item.get_entity().pos.load());
            }
        }
        None
    }

    fn move_to_item(mob: &dyn Mob, target: Vector3<f64>) {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(
                mob.get_entity().pos.load(),
                target,
                SPEED,
            ));
    }
}

impl Goal for FoxSearchForItemsGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() else {
                return false;
            };
            if !fox.can_hold_item() {
                return false;
            }
            if mob.get_mob_entity().target.lock().await.is_some() {
                return false;
            }
            if mob
                .get_mob_entity()
                .living_entity
                .last_attacker_id
                .load(std::sync::atomic::Ordering::Relaxed)
                != 0
            {
                return false;
            }
            // `Fox.canMove` (`Fox.java:661-663`); `can_fox_move` adds the target check the
            // stroll goal needs, which is already `None` here.
            if !can_fox_move(mob, fox).await || fox.is_faceplanted() {
                return false;
            }
            let roll = {
                mob.get_random()
                    .random_range(0..to_goal_ticks(TRY_INTERVAL).max(1))
            };
            if roll != 0 {
                return false;
            }
            Self::nearest_allowed_item(mob).is_some()
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(target) = Self::nearest_allowed_item(mob) {
                Self::move_to_item(mob, target);
            }
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let holding = mob
                .cast_any()
                .downcast_ref::<FoxEntity>()
                .is_some_and(|fox| !fox.can_hold_item());
            if holding {
                return;
            }
            if let Some(target) = Self::nearest_allowed_item(mob) {
                Self::move_to_item(mob, target);
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
