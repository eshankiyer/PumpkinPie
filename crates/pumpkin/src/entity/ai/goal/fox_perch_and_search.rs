//! `Fox.PerchAndSearchGoal` (`Fox.java:1303-1365`), a `Fox.FoxBehaviorGoal` subclass
//! registered at priority 13 (`Fox.java:203`): the fox sits down and looks around a few times.

use std::sync::atomic::Ordering::Relaxed;

use rand::RngExt;

use super::fox_behavior::alertable;
use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;
use crate::entity::passive::fox::FoxEntity;

pub struct FoxPerchAndSearchGoal {
    rel_x: f64,
    rel_z: f64,
    look_time: i32,
    looks_remaining: i32,
}

impl FoxPerchAndSearchGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            rel_x: 0.0,
            rel_z: 0.0,
            look_time: 0,
            looks_remaining: 0,
        })
    }

    /// `resetLook` (`Fox.java:1358-1363`).
    fn reset_look(&mut self, mob: &dyn Mob) {
        let mut rng = mob.get_random();
        let angle = std::f64::consts::TAU * rng.random::<f64>();
        self.rel_x = angle.cos();
        self.rel_z = angle.sin();
        self.look_time = 80 + rng.random_range(0..20);
    }
}

impl Goal for FoxPerchAndSearchGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() else {
                return false;
            };
            // `FoxBehaviorGoal.canUse` (`Fox.java:1100-1103`).
            if fox.is_sitting() || fox.is_sleeping() || fox.is_crouching() || fox.is_faceplanted() {
                return false;
            }
            if mob
                .get_mob_entity()
                .living_entity
                .last_attacker_id
                .load(Relaxed)
                != 0
            {
                return false;
            }
            if { mob.get_random().random::<f32>() } >= 0.02 {
                return false;
            }
            if mob.get_mob_entity().target.lock().await.is_some() {
                return false;
            }
            let idle = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle();
            if !idle || fox.is_pouncing() {
                return false;
            }
            !alertable(mob)
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { self.looks_remaining > 0 })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.reset_look(mob);
            self.looks_remaining = 2 + { mob.get_random().random_range(0..3) };
            if let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() {
                fox.set_sitting(true);
            }
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() {
                fox.set_sitting(false);
            }
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.look_time -= 1;
            if self.look_time <= 0 {
                self.looks_remaining -= 1;
                self.reset_look(mob);
            }
            let entity = mob.get_entity();
            let pos = entity.pos.load();
            let eye_y = pos.y + entity.get_eye_height();
            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at(mob, pos.x + self.rel_x, eye_y, pos.z + self.rel_z);
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
