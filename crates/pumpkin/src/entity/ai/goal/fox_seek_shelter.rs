//! `Fox.SeekShelterGoal` (`Fox.java:1367-1399`), a `FleeSunGoal` subclass registered at
//! priority 6 (`Fox.java:193`).
//!
//! `flee_sun.rs` hard-codes `Monster.getWalkTargetValue` and gates on `isOnFire`, neither of
//! which applies to a fox, so the parent's `getHidePos`/`setWantedPos`
//! (`FleeSunGoal.java:41-73`) is re-derived here against `Animal.getWalkTargetValue`.

use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::fox_behavior::{animal_walk_target_value, is_bright_outside};
use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::passive::fox::FoxEntity;

/// `Fox.SeekShelterGoal.interval` (`Fox.java:1368`), `reducedTickDelay(100)`.
const INTERVAL: i32 = 100;
const SEARCH_ATTEMPTS: usize = 10;

pub struct FoxSeekShelterGoal {
    speed: f64,
    interval: i32,
    wanted: Option<Vector3<f64>>,
}

impl FoxSeekShelterGoal {
    #[must_use]
    pub fn new(speed: f64) -> Box<Self> {
        Box::new(Self {
            speed,
            interval: to_goal_ticks(INTERVAL),
            wanted: None,
        })
    }

    /// `FleeSunGoal.getHidePos` (`FleeSunGoal.java:63-73`).
    fn hide_pos(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let current = entity.block_pos.load();
        let mut rng = mob.get_random();

        for _ in 0..SEARCH_ATTEMPTS {
            let candidate = current.add(
                rng.random_range(-10..10),
                rng.random_range(-3..3),
                rng.random_range(-10..10),
            );
            if !world.can_see_sky(&candidate) && animal_walk_target_value(&world, &candidate) < 0.0
            {
                return Some(Vector3::new(
                    f64::from(candidate.0.x) + 0.5,
                    f64::from(candidate.0.y),
                    f64::from(candidate.0.z) + 0.5,
                ));
            }
        }
        None
    }
}

impl Goal for FoxSeekShelterGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() else {
                return false;
            };
            if fox.is_sleeping() || mob.get_mob_entity().target.lock().await.is_some() {
                return false;
            }

            let entity = mob.get_entity();
            let world = entity.world.load();
            let pos = entity.block_pos.load();

            // Thunder is the fast path: no interval countdown, and it ignores the village and
            // daylight tests (`Fox.java:1377-1378`).
            if world.is_thundering().await && world.can_see_sky(&pos) {
                self.wanted = Self::hide_pos(mob);
                return self.wanted.is_some();
            }

            if self.interval > 0 {
                self.interval -= 1;
                return false;
            }
            self.interval = INTERVAL;

            if !is_bright_outside(&world) || !world.can_see_sky(&pos) {
                return false;
            }
            // `ServerLevel.isVillage(pos)` is `isCloseToVillage(pos, 1)`
            // (`ServerLevel.java:1542-1544`).
            if world.is_close_to_village(pos, 1).await {
                return false;
            }

            self.wanted = Self::hide_pos(mob);
            self.wanted.is_some()
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            !mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle()
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() {
                fox.clear_states();
            }
            let Some(wanted) = self.wanted else {
                return;
            };
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_progress(NavigatorGoal::new(
                    mob.get_entity().pos.load(),
                    wanted,
                    self.speed,
                ));
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
