use std::sync::{Arc, Mutex};

use super::{Controls, Goal};
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob, player::Player};
use pumpkin_data::attributes::Attributes;
use pumpkin_data::item::Item;
use pumpkin_util::math::vector3::Vector3;

/// `TemptGoal.java`: `DEFAULT_STOP_DISTANCE`.
const DEFAULT_STOP_DISTANCE: f64 = 2.5;
const SCARE_RANGE_SQUARED: f64 = 36.0;
const SCARE_MOVE_THRESHOLD_SQUARED: f64 = 0.01;
const SCARE_ROT_THRESHOLD: f32 = 5.0;

struct ScareState {
    prev_pos: Vector3<f64>,
    prev_yaw: f32,
    prev_pitch: f32,
}

pub struct TemptGoal {
    goal_control: Controls,
    speed: f64,
    tempt_items: &'static [&'static Item],
    can_scare: bool,
    target_player: Mutex<Option<Arc<Player>>>,
    cooldown: i32,
    scare_state: Mutex<ScareState>,
    stop_distance: f64,
    /// Vanilla `TemptGoal#isRunning`: tracks whether the goal is actively chasing
    /// a tempting player. Used externally by Ocelot, Cat, and Strider to gate
    /// interaction/animation logic.
    is_running: bool,
}

/// Vanilla `TemptGoal#canContinueToUse`'s scare check, factored out as a pure
/// function of the tracked previous player state and its current state.
/// Returns `false` (goal should abort) if the mob is within the scare range
/// and the player has moved or rotated more than the jitter threshold since
/// the last recorded checkpoint.
fn passes_scare_check(
    mob_to_player_dist_sq: f64,
    player_pos: Vector3<f64>,
    prev_pos: Vector3<f64>,
    player_yaw: f32,
    player_pitch: f32,
    prev_yaw: f32,
    prev_pitch: f32,
) -> bool {
    if mob_to_player_dist_sq >= SCARE_RANGE_SQUARED {
        return true;
    }
    if player_pos.squared_distance_to_vec(&prev_pos) > SCARE_MOVE_THRESHOLD_SQUARED {
        return false;
    }
    if (player_pitch - prev_pitch).abs() > SCARE_ROT_THRESHOLD
        || (player_yaw - prev_yaw).abs() > SCARE_ROT_THRESHOLD
    {
        return false;
    }
    true
}

impl TemptGoal {
    #[must_use]
    pub fn new(speed: f64, tempt_items: &'static [&'static Item], can_scare: bool) -> Self {
        Self::with_stop_distance(speed, tempt_items, can_scare, DEFAULT_STOP_DISTANCE)
    }

    /// `TemptGoal.java`'s 5-arg constructor, for species that pass a non-default
    /// `stopDistance` (e.g. Happy Ghast: 7.0, Sulfur Cube: 1.0).
    #[must_use]
    pub fn with_stop_distance(
        speed: f64,
        tempt_items: &'static [&'static Item],
        can_scare: bool,
        stop_distance: f64,
    ) -> Self {
        Self {
            goal_control: Controls::MOVE | Controls::LOOK,
            speed,
            tempt_items,
            can_scare,
            target_player: Mutex::new(None),
            cooldown: 0,
            scare_state: Mutex::new(ScareState {
                prev_pos: Vector3::new(0.0, 0.0, 0.0),
                prev_yaw: 0.0,
                prev_pitch: 0.0,
            }),
            stop_distance,
            is_running: false,
        }
    }

    fn is_tempt_item(&self, stack: &pumpkin_data::item_stack::ItemStack) -> bool {
        stack.item_count > 0 && self.tempt_items.iter().any(|i| i.id == stack.item.id)
    }

    /// Vanilla `TemptGoal#isRunning`: whether the goal is actively chasing a
    /// tempting player. Called externally by Ocelot, Cat, and Strider.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.is_running
    }

    fn is_holding_tempt_item(&self, player: &Player) -> bool {
        let main = player.inventory().held_item();
        if self.is_tempt_item(&main) {
            return true;
        }
        let off = player.inventory().off_hand_item();
        self.is_tempt_item(&off)
    }

    fn tempt_range(mob: &dyn Mob) -> f64 {
        mob.get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::TEMPT_RANGE)
    }

    fn find_tempting_player(&self, mob: &dyn Mob) -> Option<Arc<Player>> {
        let mob_entity = mob.get_mob_entity();
        let pos = mob_entity.living_entity.entity.pos.load();
        let world = mob_entity.living_entity.entity.world.load();
        let range = Self::tempt_range(mob);

        // Vanilla TemptGoal#canUse: `getNearestPlayer` selects the closest qualifying player,
        // not an arbitrary one in range.
        let mut nearest: Option<(Arc<Player>, f64)> = None;
        for player in world.get_nearby_players(pos, range) {
            if !self.is_holding_tempt_item(&player) {
                continue;
            }
            let dist = pos.squared_distance_to_vec(&player.get_entity().pos.load());
            if nearest.as_ref().is_none_or(|(_, best)| dist < *best) {
                nearest = Some((player, dist));
            }
        }
        nearest.map(|(player, _)| player)
    }
}

impl Goal for TemptGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return false;
        }
        let target_player = self.find_tempting_player(mob);
        *self
            .target_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target_player;
        let target_player = self
            .target_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(player) = target_player.as_ref() {
            self.scare_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .prev_pos = player.get_entity().pos.load();
            self.is_running = true;
            true
        } else {
            false
        }
    }

    fn should_continue(&self, mob: &dyn Mob) -> bool {
        let Some(player) = self
            .target_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        else {
            return false;
        };

        if self.can_scare {
            let mob_entity = mob.get_mob_entity();
            let mob_pos = mob_entity.living_entity.entity.pos.load();
            let player_pos = player.get_entity().pos.load();
            let player_entity = player.get_entity();
            let player_yaw = player_entity.yaw.load();
            let player_pitch = player_entity.pitch.load();
            let dist_sq = mob_pos.squared_distance_to_vec(&player_pos);
            let mut scare_state = self
                .scare_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let ok = passes_scare_check(
                dist_sq,
                player_pos,
                scare_state.prev_pos,
                player_yaw,
                player_pitch,
                scare_state.prev_yaw,
                scare_state.prev_pitch,
            );

            if dist_sq >= SCARE_RANGE_SQUARED {
                scare_state.prev_pos = player_pos;
            }
            scare_state.prev_yaw = player_yaw;
            scare_state.prev_pitch = player_pitch;

            if !ok {
                return false;
            }
        }

        let target_player = self.find_tempting_player(mob);
        let is_tempting = target_player.is_some();
        *self
            .target_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target_player;
        is_tempting
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let target_player = self
            .target_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(player) = target_player {
            let mob_entity = mob.get_mob_entity();
            let player_pos = player.get_entity().pos.load();

            mob_entity
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at(
                    mob,
                    player_pos.x,
                    player.get_entity().get_eye_y(),
                    player_pos.z,
                );

            let mob_pos = mob_entity.living_entity.entity.pos.load();
            if mob_pos.squared_distance_to_vec(&player_pos)
                > self.stop_distance * self.stop_distance
            {
                let mut navigator = mob_entity
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                navigator.set_progress(NavigatorGoal::new(mob_pos, player_pos, self.speed));
            } else {
                mob_entity
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
            }
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        *self
            .target_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.is_running = false;
        self.cooldown = 100;
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(x: f64, y: f64, z: f64) -> Vector3<f64> {
        Vector3::new(x, y, z)
    }

    #[test]
    fn far_away_always_passes() {
        // Outside the 6-block scare range, jitter never matters.
        assert!(passes_scare_check(
            100.0,
            pos(50.0, 0.0, 0.0),
            pos(0.0, 0.0, 0.0),
            180.0,
            180.0,
            0.0,
            0.0,
        ));
    }

    #[test]
    fn near_no_movement_passes() {
        assert!(passes_scare_check(
            4.0,
            pos(1.0, 0.0, 0.0),
            pos(1.0, 0.0, 0.0),
            10.0,
            10.0,
            10.0,
            10.0,
        ));
    }

    #[test]
    fn near_small_jitter_passes() {
        // 0.05 blocks moved (sq = 0.0025), under the 0.01 sq threshold.
        assert!(passes_scare_check(
            4.0,
            pos(1.05, 0.0, 0.0),
            pos(1.0, 0.0, 0.0),
            10.0,
            10.0,
            10.0,
            10.0,
        ));
    }

    #[test]
    fn near_movement_over_threshold_fails() {
        // 0.2 blocks moved (sq = 0.04), over the 0.01 sq threshold.
        assert!(!passes_scare_check(
            4.0,
            pos(1.2, 0.0, 0.0),
            pos(1.0, 0.0, 0.0),
            10.0,
            10.0,
            10.0,
            10.0,
        ));
    }

    #[test]
    fn near_at_exact_range_boundary_is_far() {
        // dist_sq == 36.0 is treated as "far" (>=), matching vanilla's `< 36.0` gate.
        assert!(passes_scare_check(
            36.0,
            pos(500.0, 0.0, 0.0),
            pos(0.0, 0.0, 0.0),
            180.0,
            180.0,
            0.0,
            0.0,
        ));
    }

    #[test]
    fn near_yaw_rotation_over_threshold_fails() {
        assert!(!passes_scare_check(
            4.0,
            pos(1.0, 0.0, 0.0),
            pos(1.0, 0.0, 0.0),
            10.0,
            20.0,
            10.0,
            10.0,
        ));
    }

    #[test]
    fn near_pitch_rotation_over_threshold_fails() {
        assert!(!passes_scare_check(
            4.0,
            pos(1.0, 0.0, 0.0),
            pos(1.0, 0.0, 0.0),
            20.0,
            10.0,
            10.0,
            10.0,
        ));
    }

    #[test]
    fn near_rotation_at_exact_threshold_passes() {
        // abs diff == 5.0 is not > 5.0, so it should pass.
        assert!(passes_scare_check(
            4.0,
            pos(1.0, 0.0, 0.0),
            pos(1.0, 0.0, 0.0),
            15.0,
            15.0,
            10.0,
            10.0,
        ));
    }
}
