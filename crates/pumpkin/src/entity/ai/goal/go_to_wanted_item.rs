//! Goal-system port of `GoToWantedItem` (`GoToWantedItem.java:13-51`) plus the
//! `NearestItemSensor` scan that feeds it (`NearestItemSensor.java:24-34`).
//!
//! Vanilla splits this in two: `NearestItemSensor` writes the nearest wanted item into
//! `NEAREST_VISIBLE_WANTED_ITEM`, and `GoToWantedItem` turns that memory into a walk target.
//! `ai/brain/` here already carries both halves, but only for a mob that owns a `Brain`
//! (today just the allay). The piglin is driven entirely by the goal system, the shape
//! `warden.rs` established, so the two halves are collapsed into one goal: scan, then walk.
//!
//! What the goal keeps from vanilla:
//! * `maxDistToWalk` (`PiglinAi.java:218` passes 9), used as both the scan radius and the
//!   give-up distance. Vanilla's sensor sees 32 blocks but the behavior refuses to walk to
//!   anything past `maxDistToWalk`, so a single radius of 9 is the honest merge.
//! * The `canPickUpLoot` gate (`GoToWantedItem.java:39`) and the item filter, both routed
//!   through `Mob::can_pick_up_loot`/`Mob::wants_to_pick_up_item`, so this goal is generic
//!   over whatever mob registers it.
//! * `Sensor`'s 20-tick scan interval, rather than re-boxing the world every tick.
//!
//! What it does not keep:
//! * The line-of-sight filter (`NearestItemSensor.java:31`). `ai/brain/sensor/nearest_item.rs`
//!   raycasts for it; a raycast per candidate per scan is affordable there because the allay
//!   is rare, and skipping it here only means a piglin also walks toward gold it cannot
//!   currently see, which the pathfinder then either reaches or gives up on.
//! * `ITEM_PICKUP_COOLDOWN_TICKS` (`GoToWantedItem.java:37`), which no piglin behavior sets.
//! * The world-border bounds check (`GoToWantedItem.java:38`).
//!
//! The pickup itself is not here: `Mob::mob_try_pick_up_items` already runs vanilla's
//! `Mob.aiStep` looting loop (`Mob.java:461-477`) every tick, so this goal only has to close
//! the distance until the item falls inside that loop's reach box.

use std::sync::{Arc, PoisonError};

use pumpkin_util::math::vector3::Vector3;

use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::item::ItemEntity;
use crate::entity::mob::Mob;

/// `Sensor.getScanRate` default, which `NearestItemSensor` inherits.
const SCAN_INTERVAL_TICKS: i32 = 20;

/// `FollowMobGoal`-style path recalculation cadence; a dropped item barely moves, so there is
/// no need to re-path more often than this.
const RECALC_PATH_INTERVAL: i32 = 10;

pub struct GoToWantedItemGoal {
    speed_modifier: f64,
    /// `GoToWantedItem.create(..., maxDistToWalk)`.
    max_dist_to_walk: f64,
    max_dist_to_walk_sq: f64,
    scan_countdown: i32,
    time_to_recalc_path: i32,
    wanted: Option<Arc<ItemEntity>>,
}

impl GoToWantedItemGoal {
    #[must_use]
    pub fn new(speed_modifier: f64, max_dist_to_walk: f64) -> Box<Self> {
        Box::new(Self {
            speed_modifier,
            max_dist_to_walk,
            max_dist_to_walk_sq: max_dist_to_walk * max_dist_to_walk,
            scan_countdown: 0,
            time_to_recalc_path: 0,
            wanted: None,
        })
    }

    /// The scan half of `NearestItemSensor.doTick`, narrowed to `maxDistToWalk`.
    async fn find_wanted_item(&self, mob: &dyn Mob) -> Option<Arc<ItemEntity>> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let mob_pos = entity.pos.load();
        let search_box = entity.bounding_box.load().expand(
            self.max_dist_to_walk,
            self.max_dist_to_walk,
            self.max_dist_to_walk,
        );

        let mut candidates: Vec<(f64, Arc<ItemEntity>)> = Vec::new();
        for candidate in world.get_entities_at_box(&search_box) {
            let Some(item_entity) = candidate.clone().get_item_entity() else {
                continue;
            };
            if !item_entity.get_entity().is_alive() {
                continue;
            }
            let distance = candidate
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&mob_pos);
            if distance > self.max_dist_to_walk_sq {
                continue;
            }
            candidates.push((distance, item_entity));
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));

        for (_, item_entity) in candidates {
            // The stack guard is a tokio mutex; clone the stack out and drop it before
            // anything else, the way `ai/brain/sensor/nearest_item.rs` does.
            let stack = item_entity.get_item_stack().lock().await.clone();
            if stack.is_empty() {
                continue;
            }
            if mob.wants_to_pick_up_item(&world, &stack) {
                return Some(item_entity);
            }
        }
        None
    }

    /// Vanilla puts this behavior in the IDLE activity (`PiglinAi.java:218`), and
    /// `updateActivity` only reaches IDLE when neither FIGHT nor AVOID is runnable
    /// (`PiglinAi.java:307-309`); FIGHT is exactly the case where `ATTACK_TARGET` is set. The
    /// goal system has no activities, so the activity gate becomes this refusal to run while
    /// the mob has a target -- without it a piglin at priority 3 would break off a fight to
    /// fetch gold, which vanilla never does.
    async fn has_attack_target(mob: &dyn Mob) -> bool {
        mob.get_mob_entity().target.lock().await.is_some()
    }

    /// Whether the remembered item is still there and still wanted -- vanilla re-derives this
    /// every sensor tick by rewriting the memory, and `Brain` erases the memory when the item
    /// entity dies.
    async fn wanted_still_valid(&self, mob: &dyn Mob) -> bool {
        let Some(item_entity) = self.wanted.as_ref() else {
            return false;
        };
        if !item_entity.get_entity().is_alive() {
            return false;
        }
        let entity = mob.get_entity();
        let distance = item_entity
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&entity.pos.load());
        if distance > self.max_dist_to_walk_sq {
            return false;
        }
        let world = entity.world.load();
        let stack = item_entity.get_item_stack().lock().await.clone();
        !stack.is_empty() && mob.wants_to_pick_up_item(&world, &stack)
    }
}

impl Goal for GoToWantedItemGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if !mob.can_pick_up_loot() {
                return false;
            }
            if self.scan_countdown > 0 {
                self.scan_countdown -= 1;
                return false;
            }
            self.scan_countdown = SCAN_INTERVAL_TICKS;
            if Self::has_attack_target(mob).await {
                return false;
            }
            self.wanted = self.find_wanted_item(mob).await;
            self.wanted.is_some()
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            mob.can_pick_up_loot()
                && !Self::has_attack_target(mob).await
                && self.wanted_still_valid(mob).await
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.time_to_recalc_path = 0;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.wanted = None;
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .stop();
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            let Some(item_entity) = self.wanted.clone() else {
                return;
            };
            let mob_entity = mob.get_mob_entity();
            let item_pos = item_entity.get_entity().pos.load();

            // `lookTarget.set(new EntityTracker(item, true))` (`GoToWantedItem.java:42`).
            mob_entity
                .look_control
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .look_at(mob, item_pos.x, item_pos.y, item_pos.z);

            self.time_to_recalc_path -= 1;
            if self.time_to_recalc_path > 0 {
                return;
            }
            self.time_to_recalc_path = to_goal_ticks(RECALC_PATH_INTERVAL);

            // `walkTarget.set(new WalkTarget(new EntityTracker(item, false), speed, 0))`
            // (`GoToWantedItem.java:41`): the acceptable radius is 0, so the mob walks all the
            // way onto the item and `Mob.aiStep`'s looting box does the rest.
            let self_pos = mob.get_entity().pos.load();
            let dest = Vector3::new(item_pos.x, item_pos.y, item_pos.z);
            mob_entity
                .navigator
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .set_progress(NavigatorGoal::new(self_pos, dest, self.speed_modifier));
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
