use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::attributes::Attributes;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;

use super::move_control::{MoveControl, Operation};
use super::{Control, MoveControlTrait};
use crate::entity::mob::Mob;

/// `Drowned.DrownedMoveControl` (`Drowned.java:442-480`).
///
/// Drowned use the ordinary controller on land, but while swimming they accelerate toward the
/// navigation target and apply the small vanilla buoyancy impulse every tick.
pub struct DrownedMoveControl {
    generic: MoveControl,
    wanted_x: f64,
    wanted_y: f64,
    wanted_z: f64,
    speed_modifier: f64,
    operation: Operation,
}

impl Default for DrownedMoveControl {
    fn default() -> Self {
        Self {
            generic: MoveControl::default(),
            wanted_x: 0.0,
            wanted_y: 0.0,
            wanted_z: 0.0,
            speed_modifier: 0.0,
            operation: Operation::Wait,
        }
    }
}

impl Control for DrownedMoveControl {}

impl MoveControlTrait for DrownedMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        let entity = &living.entity;

        if !mob.wants_to_swim() || !entity.touching_water.load(Relaxed) {
            if !entity.on_ground.load(Relaxed) {
                entity
                    .velocity
                    .store(entity.velocity.load() + Vector3::new(0.0, -0.008, 0.0));
            }
            self.generic.wanted_x = self.wanted_x;
            self.generic.wanted_y = self.wanted_y;
            self.generic.wanted_z = self.wanted_z;
            self.generic.speed_modifier = self.speed_modifier;
            self.generic.operation = self.operation;
            self.generic.tick(mob);
            self.operation = self.generic.operation;
            return;
        }

        if mob.target_is_above() || mob.is_searching_for_land() {
            entity
                .velocity
                .store(entity.velocity.load() + Vector3::new(0.0, 0.002, 0.0));
        }

        if self.operation != Operation::MoveTo || mob_entity.navigator.lock().unwrap().is_idle() {
            living.set_speed(0.0);
            return;
        }

        let pos = entity.pos.load();
        let xd = self.wanted_x - pos.x;
        let yd = self.wanted_y - pos.y;
        let zd = self.wanted_z - pos.z;
        let distance = (xd * xd + yd * yd + zd * zd).sqrt();
        let y_rot = (zd.atan2(xd).to_degrees() as f32) - 90.0;
        entity
            .yaw
            .store(Self::rotlerp(entity.yaw.load(), y_rot, 90.0));
        entity.body_yaw.store(entity.yaw.load());

        let target_speed =
            (self.speed_modifier * living.get_attribute_value(&Attributes::MOVEMENT_SPEED)) as f32;
        let current_speed = living.speed.load() as f32;
        let new_speed = current_speed + (target_speed - current_speed) * 0.125;
        living.set_speed(f64::from(new_speed));
        entity.velocity.store(
            entity.velocity.load()
                + Vector3::new(
                    f64::from(new_speed) * xd * 0.005,
                    f64::from(new_speed) * (yd / distance) * 0.1,
                    f64::from(new_speed) * zd * 0.005,
                ),
        );
    }

    fn has_wanted(&self) -> bool {
        self.operation == Operation::MoveTo
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.wanted_x = x;
        self.wanted_y = y;
        self.wanted_z = z;
        self.speed_modifier = speed_modifier;
        if self.operation != Operation::Jumping {
            self.operation = Operation::MoveTo;
        }
    }

    fn get_wanted_x(&self) -> f64 {
        self.wanted_x
    }

    fn get_wanted_y(&self) -> f64 {
        self.wanted_y
    }

    fn get_wanted_z(&self) -> f64 {
        self.wanted_z
    }
}

impl DrownedMoveControl {
    fn rotlerp(start: f32, end: f32, max_change: f32) -> f32 {
        let diff = wrap_degrees(end - start).clamp(-max_change, max_change);
        let result = start + diff;
        if result < 0.0 {
            result + 360.0
        } else if result > 360.0 {
            result - 360.0
        } else {
            result
        }
    }
}
