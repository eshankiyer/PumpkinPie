use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::attributes::Attributes;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;

use super::move_control::Operation;
use super::{Control, MoveControlTrait};
use crate::entity::mob::Mob;

/// Vanilla `SmoothSwimmingMoveControl` (`net/minecraft/world/entity/ai/control/SmoothSwimmingMoveControl.java:7-75`).
///
/// The slow-turning swim controller shared by `Dolphin` (`Dolphin.java:91`), `Frog`
/// (`Frog.java:92`), `Tadpole` (`Tadpole.java:61`), `AbstractNautilus`
/// (`AbstractNautilus.java:91`) and, through its `AxolotlMoveControl` subclass
/// (`Axolotl.java:611-618`), the axolotl.
///
/// Unlike the base [`super::move_control::MoveControl`] it never snaps the yaw by up to
/// 90 degrees per tick and never requests jumps; while swimming it steers both yaw
/// (bounded by `maxTurnY`) and pitch (bounded by `maxTurnX`), and drives forward motion
/// from the pitch so that vertical travel happens along the body axis.
pub struct SmoothSwimmingMoveControl {
    /// Vanilla `maxTurnX` (`SmoothSwimmingMoveControl.java:10`): pitch clamp per retarget.
    max_turn_x: f32,
    /// Vanilla `maxTurnY` (`SmoothSwimmingMoveControl.java:11`): yaw turn budget per tick.
    max_turn_y: f32,
    /// Vanilla `inWaterSpeedModifier` (`SmoothSwimmingMoveControl.java:12`).
    in_water_speed_modifier: f32,
    /// Vanilla `outsideWaterSpeedModifier` (`SmoothSwimmingMoveControl.java:13`).
    outside_water_speed_modifier: f32,
    /// Vanilla `applyGravity` (`SmoothSwimmingMoveControl.java:14`).
    apply_gravity: bool,
    wanted_x: f64,
    wanted_y: f64,
    wanted_z: f64,
    speed_modifier: f64,
    operation: Operation,
}

impl SmoothSwimmingMoveControl {
    #[must_use]
    pub const fn new(
        max_turn_x: i32,
        max_turn_y: i32,
        in_water_speed_modifier: f32,
        outside_water_speed_modifier: f32,
        apply_gravity: bool,
    ) -> Self {
        Self {
            max_turn_x: max_turn_x as f32,
            max_turn_y: max_turn_y as f32,
            in_water_speed_modifier,
            outside_water_speed_modifier,
            apply_gravity,
            wanted_x: 0.0,
            wanted_y: 0.0,
            wanted_z: 0.0,
            speed_modifier: 0.0,
            operation: Operation::Wait,
        }
    }

    /// Vanilla `MoveControl.rotlerp` (`MoveControl.java:132-146`).
    fn rotlerp(start: f32, end: f32, max_change: f32) -> f32 {
        let mut diff = wrap_degrees(end - start);
        if diff > max_change {
            diff = max_change;
        }
        if diff < -max_change {
            diff = -max_change;
        }
        let result = start + diff;
        if result < 0.0 {
            result + 360.0
        } else if result > 360.0 {
            result - 360.0
        } else {
            result
        }
    }

    /// Vanilla `getTurningSpeedFactor`
    /// (`SmoothSwimmingMoveControl.java:73-75`): outside water the mob slows down the more
    /// yaw is still left to turn.
    fn turning_speed_factor(left_to_turn: f32) -> f32 {
        1.0 - ((left_to_turn - 10.0) / 50.0).clamp(0.0, 1.0)
    }
}

impl Control for SmoothSwimmingMoveControl {}

impl MoveControlTrait for SmoothSwimmingMoveControl {
    /// Vanilla `SmoothSwimmingMoveControl.tick`
    /// (`SmoothSwimmingMoveControl.java:27-71`). The `MOVE_TO && !navigation.isDone()` gate
    /// maps to `operation == MoveTo` plus the workspace navigator's `is_idle`.
    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        let entity = &living.entity;

        // Lines 29-31: small buoyancy impulse while swimming, for the controllers that
        // request it (dolphin/frog/tadpole/nautilus do; axolotl passes `false`).
        if self.apply_gravity && entity.touching_water.load(Relaxed) {
            entity
                .velocity
                .store(entity.velocity.load() + Vector3::new(0.0, 0.005, 0.0));
        }

        let navigation_idle = mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_idle();
        if self.operation != Operation::MoveTo || navigation_idle {
            // Lines 65-70: full stop when there is no active destination.
            living.set_speed(0.0);
            living.movement_input.store(Vector3::default());
            return;
        }

        // Lines 34-39.
        let pos = entity.pos.load();
        let xd = self.wanted_x - pos.x;
        let yd = self.wanted_y - pos.y;
        let zd = self.wanted_z - pos.z;
        let dd = xd * xd + yd * yd + zd * zd;
        if dd < 2.500_000_3e-7 {
            living.movement_input.store(Vector3::default());
            return;
        }

        // Lines 41-44: steer yaw toward the destination and align body/head with it.
        let y_rot_d = (zd.atan2(xd).to_degrees() as f32) - 90.0;
        entity
            .yaw
            .store(Self::rotlerp(entity.yaw.load(), y_rot_d, self.max_turn_y));
        entity.body_yaw.store(entity.yaw.load());
        entity.head_yaw.store(entity.yaw.load());

        // Line 45.
        let speed =
            (self.speed_modifier * living.get_attribute_value(&Attributes::MOVEMENT_SPEED)) as f32;

        if entity.touching_water.load(Relaxed) {
            // Line 46-47.
            living.set_speed(f64::from(speed * self.in_water_speed_modifier));

            // Lines 48-53: pitch toward the destination, clamped to `maxTurnX`, approached
            // at a fixed 5 degrees per tick (`rotateTowards`, Control.java:6-9 == the trait's
            // `change_angle`).
            let horizontal = xd.hypot(zd);
            if yd.abs() > 1.0e-5 || horizontal.abs() > 1.0e-5 {
                let x_rot_d = -(yd.atan2(horizontal).to_degrees() as f32);
                let x_rot_d = wrap_degrees(x_rot_d).clamp(-self.max_turn_x, self.max_turn_x);
                entity
                    .pitch
                    .store(self.change_angle(entity.pitch.load(), x_rot_d, 5.0));
            }

            // Lines 55-58: forward/up thrust follows the current pitch (vanilla writes
            // `zza`/`yya`; movement_input is this codebase's xxa/yya/zza triple).
            let pitch_rad = f64::from(entity.pitch.load()).to_radians();
            let (sin, cos) = pitch_rad.sin_cos();
            living.movement_input.store(Vector3::new(
                0.0,
                -sin * f64::from(speed),
                cos * f64::from(speed),
            ));
        } else {
            // Lines 59-63: on land only the speed changes; the navigator keeps driving the
            // ordinary ground inputs, scaled down while the mob still has turning left to do.
            let left_to_turn = wrap_degrees(entity.yaw.load() - y_rot_d).abs();
            let factor = Self::turning_speed_factor(left_to_turn);
            living.set_speed(f64::from(
                speed * self.outside_water_speed_modifier * factor,
            ));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_speed_factor_matches_vanilla_thresholds() {
        // Fully turned (<= 10 degrees): full speed.
        assert_eq!(SmoothSwimmingMoveControl::turning_speed_factor(10.0), 1.0);
        // Half-way through the 10..60 ramp: half speed.
        assert_eq!(SmoothSwimmingMoveControl::turning_speed_factor(35.0), 0.5);
        // >= 60 degrees left to turn: stopped until the yaw catches up.
        assert_eq!(SmoothSwimmingMoveControl::turning_speed_factor(60.0), 0.0);
        assert_eq!(SmoothSwimmingMoveControl::turning_speed_factor(180.0), 0.0);
    }

    #[test]
    fn rotlerp_clamps_and_wraps_into_positive_range() {
        assert_eq!(SmoothSwimmingMoveControl::rotlerp(0.0, 30.0, 90.0), 30.0);
        // Clamped to the max change budget.
        assert_eq!(SmoothSwimmingMoveControl::rotlerp(0.0, 170.0, 90.0), 90.0);
        // Wraps back into [0, 360] like vanilla's rotlerp: wrap_degrees(20 - 350) = 30, so
        // 350 + 30 = 380, which wraps down to 20.
        assert_eq!(SmoothSwimmingMoveControl::rotlerp(350.0, 20.0, 45.0), 20.0);
    }
}
