use pumpkin_util::math::clamp_angle;

use crate::entity::Entity;

const HEAD_STABLE_ANGLE: f32 = 15.0;
const DELAY_UNTIL_STARTING_TO_FACE_FORWARD: i32 = 10;
const HOW_LONG_IT_TAKES_TO_FACE_FORWARD: f32 = 10.0;

/// `BodyRotationControl` (`BodyRotationControl.java:6-66`).
#[derive(Default)]
pub struct BodyRotationControl {
    head_stable_time: i32,
    last_stable_y_head_rot: f32,
}

impl BodyRotationControl {
    /// `BodyRotationControl.clientTick` (`BodyRotationControl.java:18-38`).
    pub fn client_tick(
        &mut self,
        entity: &Entity,
        max_head_y_rot: f32,
        carrying_mob_passenger: bool,
    ) {
        let pos = entity.pos.load();
        let last_pos = entity.last_pos.load();
        let (body_yaw, head_yaw) = self.step(
            is_moving(pos.x - last_pos.x, pos.z - last_pos.z),
            carrying_mob_passenger,
            entity.yaw.load(),
            entity.head_yaw.load(),
            entity.body_yaw.load(),
            max_head_y_rot,
        );
        entity.body_yaw.store(body_yaw);
        entity.head_yaw.store(head_yaw);
    }

    fn step(
        &mut self,
        moving: bool,
        carrying_mob_passenger: bool,
        y_rot: f32,
        mut y_head_rot: f32,
        mut y_body_rot: f32,
        max_head_y_rot: f32,
    ) -> (f32, f32) {
        if moving {
            y_body_rot = y_rot;
            y_head_rot = clamp_angle(y_head_rot, y_body_rot, max_head_y_rot);
            self.last_stable_y_head_rot = y_head_rot;
            self.head_stable_time = 0;
        } else if !carrying_mob_passenger {
            if (y_head_rot - self.last_stable_y_head_rot).abs() > HEAD_STABLE_ANGLE {
                self.head_stable_time = 0;
                self.last_stable_y_head_rot = y_head_rot;
                y_body_rot = clamp_angle(y_body_rot, y_head_rot, max_head_y_rot);
            } else {
                self.head_stable_time += 1;
                if self.head_stable_time > DELAY_UNTIL_STARTING_TO_FACE_FORWARD {
                    let time_since_starting_to_face_forward =
                        self.head_stable_time - DELAY_UNTIL_STARTING_TO_FACE_FORWARD;
                    let face_forward_fraction = (time_since_starting_to_face_forward as f32
                        / HOW_LONG_IT_TAKES_TO_FACE_FORWARD)
                        .clamp(0.0, 1.0);
                    let angle_remaining = max_head_y_rot * (1.0 - face_forward_fraction);
                    y_body_rot = clamp_angle(y_body_rot, y_head_rot, angle_remaining);
                }
            }
        }
        (y_body_rot, y_head_rot)
    }
}

/// `BodyRotationControl.isMoving` (`BodyRotationControl.java:61-65`).
fn is_moving(dx: f64, dz: f64) -> bool {
    dx.mul_add(dx, dz * dz) > f64::from(2.500_000_3e-7_f32)
}

#[cfg(test)]
mod tests {
    use super::{BodyRotationControl, is_moving};

    #[test]
    fn moving_snaps_body_to_yaw_and_clamps_head() {
        let mut control = BodyRotationControl::default();
        let (body, head) = control.step(true, false, 90.0, -30.0, 0.0, 75.0);
        assert_eq!(body, 90.0);
        assert_eq!(head, 15.0);
        assert_eq!(control.head_stable_time, 0);
        assert_eq!(control.last_stable_y_head_rot, 15.0);
    }

    #[test]
    fn large_head_turn_drags_body_within_limit() {
        let mut control = BodyRotationControl::default();
        let (body, head) = control.step(false, false, 0.0, 100.0, 0.0, 75.0);
        assert_eq!(head, 100.0);
        assert_eq!(body, 25.0);
        assert_eq!(control.head_stable_time, 0);
    }

    #[test]
    fn stable_head_turns_body_forward_after_delay() {
        let mut control = BodyRotationControl::default();
        let mut body = 0.0;
        for _ in 0..18 {
            (body, _) = control.step(false, false, 0.0, 10.0, body, 75.0);
        }
        assert_eq!(body, 0.0);
        (body, _) = control.step(false, false, 0.0, 10.0, body, 75.0);
        assert!((body - 2.5).abs() < 1.0e-4);
        (body, _) = control.step(false, false, 0.0, 10.0, body, 75.0);
        assert_eq!(body, 10.0);
    }

    #[test]
    fn carrying_mob_passenger_freezes_idle_body() {
        let mut control = BodyRotationControl::default();
        let (body, head) = control.step(false, true, 0.0, 100.0, 5.0, 75.0);
        assert_eq!((body, head), (5.0, 100.0));
        assert_eq!(control.head_stable_time, 0);
    }

    #[test]
    fn movement_threshold_matches_float_constant() {
        assert!(!is_moving(0.0005, 0.0));
        assert!(is_moving(0.0006, 0.0));
    }
}
