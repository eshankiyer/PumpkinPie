use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering::Relaxed},
};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::tracked_data;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::vector3::Vector3;

use super::drowned_move_control::DrownedMoveControl;
use super::move_control::Operation;
use super::{Control, MoveControlTrait};
use crate::entity::mob::Mob;

/// `Guardian.GuardianMoveControl` (`Guardian.java:437-483`).
///
/// Guardians do not steer through `xxa`/`zza`: while a path is being followed the controller
/// pushes the velocity directly with a sinusoidal wobble, eases the look target and publishes
/// `Guardian.isMoving` (`DATA_ID_MOVING`) for the client tail/spike animation and the thorns check.
pub struct GuardianMoveControl {
    wanted_x: f64,
    wanted_y: f64,
    wanted_z: f64,
    speed_modifier: f64,
    operation: Operation,
    /// Vanilla `Guardian.isMoving`, shared with the owning entity.
    moving: Arc<AtomicBool>,
    /// Stands in for vanilla `Entity.tickCount`, which only feeds the cosmetic wobble phase.
    tick_count: i32,
}

impl GuardianMoveControl {
    #[must_use]
    pub const fn new(moving: Arc<AtomicBool>) -> Self {
        Self {
            wanted_x: 0.0,
            wanted_y: 0.0,
            wanted_z: 0.0,
            speed_modifier: 0.0,
            operation: Operation::Wait,
            moving,
            tick_count: 0,
        }
    }

    /// `Guardian.setMoving`: only a changed value is synchronised, like `SynchedEntityData.set`.
    fn set_moving(&self, mob: &dyn Mob, value: bool) {
        if self.moving.swap(value, Relaxed) != value {
            mob.get_entity().send_meta_data(
                &[Metadata::new(tracked_data::guardian::ID_MOVING, value)],
                None,
            );
        }
    }
}

impl Control for GuardianMoveControl {}

impl MoveControlTrait for GuardianMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        self.tick_count = self.tick_count.wrapping_add(1);
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        let entity = &living.entity;

        let navigation_done = mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_idle();
        if self.operation != Operation::MoveTo || navigation_done {
            living.set_speed(0.0);
            self.set_moving(mob, false);
            return;
        }

        let pos = entity.pos.load();
        let dx = self.wanted_x - pos.x;
        let dy = self.wanted_y - pos.y;
        let dz = self.wanted_z - pos.z;
        let length = (dx * dx + dy * dy + dz * dz).sqrt();
        let xd = dx / length;
        let yd = dy / length;
        let zd = dz / length;

        let y_rot_d = (dz.atan2(dx).to_degrees() as f32) - 90.0;
        let yaw = DrownedMoveControl::rotlerp(entity.yaw.load(), y_rot_d, 90.0);
        entity.yaw.store(yaw);
        entity.body_yaw.store(yaw);

        let target_speed =
            (self.speed_modifier * living.get_attribute_value(&Attributes::MOVEMENT_SPEED)) as f32;
        let current_speed = living.speed.load() as f32;
        let new_speed = current_speed + (target_speed - current_speed) * 0.125;
        living.set_speed(f64::from(new_speed));

        let phase = f64::from(self.tick_count.wrapping_add(entity.entity_id));
        let push = (phase * 0.5).sin() * 0.05;
        let cos = f64::from(yaw.to_radians()).cos();
        let sin = f64::from(yaw.to_radians()).sin();
        let y_push = (phase * 0.75).sin() * 0.05;
        entity.velocity.store(
            entity.velocity.load()
                + Vector3::new(
                    push * cos,
                    (y_push * (sin + cos)).mul_add(0.25, f64::from(new_speed) * yd * 0.1),
                    push * sin,
                ),
        );

        let new_look_x = xd.mul_add(2.0, pos.x);
        let new_look_y = entity.get_eye_y() + yd / length;
        let new_look_z = zd.mul_add(2.0, pos.z);
        let mut look_control = mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (old_x, old_y, old_z) = if look_control.is_looking_at_target() {
            let wanted = look_control.get_wanted_position();
            (wanted.x, wanted.y, wanted.z)
        } else {
            (new_look_x, new_look_y, new_look_z)
        };
        look_control.look_at_with_range(
            (new_look_x - old_x).mul_add(0.125, old_x),
            (new_look_y - old_y).mul_add(0.125, old_y),
            (new_look_z - old_z).mul_add(0.125, old_z),
            10.0,
            40.0,
        );
        drop(look_control);
        self.set_moving(mob, true);
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

    fn get_speed_modifier(&self) -> f64 {
        self.speed_modifier
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
