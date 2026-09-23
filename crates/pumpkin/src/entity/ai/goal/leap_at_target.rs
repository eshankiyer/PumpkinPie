//! Port of `LeapAtTargetGoal.java`.

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::{Controls, Goal, to_goal_ticks};
use crate::entity::mob::Mob;

pub struct LeapAtTargetGoal {
    // LeapAtTargetGoal.java: `yd`, the vertical velocity given to the leap.
    yd: f32,
}

impl LeapAtTargetGoal {
    #[must_use]
    pub fn new(yd: f32) -> Box<Self> {
        Box::new(Self { yd })
    }
}

impl Goal for LeapAtTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        // LeapAtTargetGoal.java:22-24 also gates on `!hasControllingPassenger()`, but no
        // vanilla registrant of this goal (Wolf, Cat, Ocelot, Fox, Spider) is rideable, so
        // that check can never fire and is omitted.
        let Some(target) = mob
            .get_mob_entity()
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        else {
            return false;
        };
        let entity = mob.get_entity();
        let distance = entity
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load());
        if !(4.0..=16.0).contains(&distance) {
            return false;
        }
        if !entity.on_ground.load(Relaxed) {
            return false;
        }
        mob.get_random().random_range(0..to_goal_ticks(5)) == 0
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.get_entity().on_ground.load(Relaxed)
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(target) = mob
            .get_mob_entity()
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        else {
            return;
        };
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let target_pos = target.get_entity().pos.load();
        let velocity = entity.velocity.load();
        let mut delta = Vector3::new(target_pos.x - pos.x, 0.0, target_pos.z - pos.z);
        if delta.length_squared() > 1.0e-7 {
            delta = delta.normalize() * 0.4 + velocity * 0.2;
        }
        entity.set_velocity(Vector3::new(delta.x, f64::from(self.yd), delta.z));
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::JUMP
    }
}
