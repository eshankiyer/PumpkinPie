// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::{Controls, Goal};
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::predicate::EntityPredicate;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

const MAX_ATTACK_TIME: i64 = 20;

#[allow(clippy::fn_params_excessive_bools)]
const fn should_continue_melee_goal(
    following_target_even_if_not_seen: bool,
    navigation_idle: bool,
    target_in_range: bool,
    target_valid: bool,
) -> bool {
    if following_target_even_if_not_seen {
        target_valid && target_in_range
    } else {
        !navigation_idle
    }
}

/// Vanilla `MeleeAttackGoal.canUse`: a path was found, or (failing that) the target is
/// already within melee range without needing to move.
const fn should_start_melee_goal(path_found: bool, in_attack_range: bool) -> bool {
    path_found || in_attack_range
}

/// Vanilla: `MeleeAttackGoal::canPerformAttack` requires sensing line of sight. The sensing
/// implementation remains asynchronous in the fork while the goal API is synchronous upstream.
fn has_melee_line_of_sight(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
    futures::executor::block_on(mob.get_mob_entity().has_line_of_sight(target))
}

pub struct MeleeAttackGoal {
    goal_control: Controls,
    speed: f64,
    pause_when_mob_idle: bool,
    #[expect(dead_code)]
    target_location: Vector3<f64>,
    update_countdown_ticks: i32,
    pub cooldown: i32,
    #[expect(dead_code)]
    attack_interval_ticks: i32,
    last_update_time: i64,
    last_target_position: Option<Vector3<f64>>,
}

impl MeleeAttackGoal {
    #[must_use]
    pub fn new(speed: f64, pause_when_mob_idle: bool) -> Self {
        Self {
            goal_control: Controls::MOVE | Controls::LOOK,
            speed: speed.max(0.23), // Ensure minimum visible speed
            pause_when_mob_idle,
            target_location: Vector3::new(0.0, 0.0, 0.0),
            update_countdown_ticks: 0,
            cooldown: 0,
            attack_interval_ticks: 20,
            last_update_time: 0,
            last_target_position: None,
        }
    }

    #[must_use]
    pub fn get_max_cooldown(&self) -> i32 {
        self.get_tick_count(20)
    }
}

impl Goal for MeleeAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let time = mob.get_entity().world.load().get_world_age();

        if time - self.last_update_time < MAX_ATTACK_TIME {
            return false;
        }
        self.last_update_time = time;

        let target = mob.get_mob_entity().get_target();

        let Some(target) = target.as_ref() else {
            return false;
        };
        if !target.get_entity().is_alive() {
            return false;
        }
        // Vanilla `MeleeAttackGoal.canUse`: retain the path-reachability gate and fall back to
        // the melee range when navigation cannot reach the target.
        let mob_entity = mob.get_mob_entity();
        let path_found = mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .can_reach_within(&mob_entity.living_entity, target.get_entity().pos.load(), 0.0);
        should_start_melee_goal(
            path_found,
            !path_found && mob_entity.is_in_attack_range(target.as_ref()),
        )
    }

    fn should_continue(&self, mob: &dyn Mob) -> bool {
        let target = mob.get_mob_entity().get_target().clone();

        let Some(target) = target else {
            return false;
        };
        if !target.get_entity().is_alive() {
            return false;
        }

        let navigation_idle = mob
                .get_mob_entity()
                .navigator
                .try_lock()
                .is_ok_and(|navigator| navigator.is_idle());
        let is_valid_target = !target
            .get_player()
            .is_some_and(|p| p.is_spectator() || p.is_creative());

        let target_in_range = mob
            .get_mob_entity()
            .is_in_position_target_range_pos(&target.get_entity().block_pos.load());

        should_continue_melee_goal(
            self.pause_when_mob_idle,
            navigation_idle,
            target_in_range,
            is_valid_target,
        )
    }

    fn start(&mut self, mob: &dyn Mob) {
        // TODO: add missing fields like mob attacking to true and correct Navigation methods

        let target = mob.get_mob_entity().get_target().clone();
        if let Some(target) = target {
            let mut navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let target_pos = target.get_entity().pos.load();
            navigator.set_progress(NavigatorGoal {
                current_progress: mob.get_entity().pos.load(),
                destination: target_pos,
                speed: self.speed,
            });
            self.last_target_position = Some(target_pos);
        }
        mob.get_mob_entity().set_attacking(true);
        self.update_countdown_ticks = 0;
        self.cooldown = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        // Only clear target if they switched to creative/spectator
        let should_clear = mob
            .get_mob_entity()
            .get_target()
            .as_deref()
            .is_some_and(|entity| {
                !EntityPredicate::ExceptCreativeOrSpectator.test(entity.get_entity())
            });
        if should_clear {
            mob.set_mob_target(None);
        }

        // Vanilla: this.mob.getNavigation().stop()
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        mob.get_mob_entity().set_attacking(false);
        self.last_target_position = None;
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let target = mob.get_mob_entity().get_target().clone();
        let Some(target) = target else {
            return;
        };

        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 30.0, 30.0);

        self.update_countdown_ticks = (self.update_countdown_ticks - 1).max(0);

        let current_target_pos = target.get_entity().pos.load();
        let should_update_nav = self.update_countdown_ticks <= 0
            && (self.pause_when_mob_idle || has_melee_line_of_sight(mob, target.as_ref()))
            && (self.last_target_position.is_none_or(|last_pos| {
                current_target_pos.squared_distance_to_vec(&last_pos) >= 1.0
            }) || mob.get_random().random_range(0..20) == 0);

        if should_update_nav {
            let mob_pos = mob.get_entity().pos.load();
            let dist_sq = mob_pos.squared_distance_to_vec(&current_target_pos);
            let mut navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let path_found = navigator.can_reach_within(
                &mob.get_mob_entity().living_entity,
                current_target_pos,
                0.0,
            );
            navigator.set_progress(NavigatorGoal {
                current_progress: mob_pos,
                destination: current_target_pos,
                speed: self.speed,
            });
            self.last_target_position = Some(current_target_pos);
            self.update_countdown_ticks = 4 + mob.get_random().random_range(0..7);
            if dist_sq > 1024.0 {
                self.update_countdown_ticks += 10;
            } else if dist_sq > 256.0 {
                self.update_countdown_ticks += 5;
            }
            if !path_found {
                // Vanilla adds `FAILED_PATH_FINDING_PENALTY` (15 ticks) when moveTo fails.
                self.update_countdown_ticks += 15;
            }
        }

        self.cooldown = (self.cooldown - 1).max(0);

        if self.cooldown <= 0
            && mob.get_mob_entity().is_in_attack_range(target.as_ref())
            && has_melee_line_of_sight(mob, target.as_ref())
        {
            self.cooldown = self.get_max_cooldown();
            mob.get_mob_entity().living_entity.swing_hand();
            // Keep the fork's effect-aware Mob::try_attack dispatch, including
            // `Mob::on_successful_attack`, across the synchronous goal API.
            futures::executor::block_on(mob.try_attack(target.as_ref()));
        }
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
    use super::{should_continue_melee_goal, should_start_melee_goal};

    #[test]
    fn in_range_targets_continue_when_navigation_is_idle() {
        assert!(!should_continue_melee_goal(false, true, true, false));
        assert!(!should_continue_melee_goal(false, true, false, false));
        assert!(!should_continue_melee_goal(true, true, false, true));
        assert!(!should_continue_melee_goal(true, false, true, false));
        assert!(should_continue_melee_goal(false, false, false, false));
        assert!(should_continue_melee_goal(true, false, true, true));
    }

    #[test]
    fn starts_when_path_found_regardless_of_range() {
        assert!(should_start_melee_goal(true, true));
        assert!(should_start_melee_goal(true, false));
    }

    #[test]
    fn falls_back_to_attack_range_when_no_path() {
        assert!(should_start_melee_goal(false, true));
        assert!(!should_start_melee_goal(false, false));
    }
}
