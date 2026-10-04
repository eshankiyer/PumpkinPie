use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::{Controls, Goal, GoalFuture};
use crate::entity::EntityBase;
use crate::entity::passive::cat::CatEntity;
use crate::entity::passive::ocelot::OcelotEntity;
use crate::entity::passive::tamable::TamableAnimal;
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob, player::Player};
use pumpkin_data::attributes::Attributes;
use pumpkin_data::item::Item;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use uuid::Uuid;

/// `TemptGoal.java`: `DEFAULT_STOP_DISTANCE`.
const DEFAULT_STOP_DISTANCE: f64 = 2.5;
const SCARE_RANGE_SQUARED: f64 = 36.0;
const SCARE_MOVE_THRESHOLD_SQUARED: f64 = 0.01;
const SCARE_ROT_THRESHOLD: f32 = 5.0;

/// The vanilla `TemptGoal` subclasses that override `canScare`/`canUse`/`tick`.
enum TemptVariant {
    Plain,
    /// `Ocelot.OcelotTemptGoal` (`Ocelot.java:302-314`): never scared while trusting.
    Ocelot,
    /// `Cat.CatTemptGoal` (`Cat.java:648-676`): only untamed, and a randomly selected player
    /// never scares the cat.
    Cat { selected_player: Option<Uuid> },
    /// `NautilusAi`'s `FollowTemptation` (`NautilusAi.java:86`): stop distance is 2.5 for a baby
    /// and 3.5 for an adult, evaluated each tick.
    Nautilus,
}

pub struct TemptGoal {
    goal_control: Controls,
    speed: f64,
    tempt_items: &'static [&'static Item],
    can_scare: bool,
    target_player: Option<Arc<Player>>,
    cooldown: i32,
    prev_pos: Vector3<f64>,
    prev_yaw: f32,
    prev_pitch: f32,
    stop_distance: f64,
    /// Vanilla `TemptGoal#isRunning`: tracks whether the goal is actively chasing
    /// a tempting player. Used externally by Ocelot, Cat, and Strider to gate
    /// interaction/animation logic.
    is_running: bool,
    variant: TemptVariant,
    /// Mirror of `is_running` for owners that cannot reach the boxed goal (Ocelot's
    /// `temptGoal.isRunning()` check in `mobInteract`).
    running_flag: Option<Arc<AtomicBool>>,
    /// `TemptGoal.ForNonPathfinders` (`TemptGoal.java:132-146`): steers the mob's move control
    /// instead of its path navigator.
    non_pathfinder: bool,
    /// Stands in for the brain behaviour `FollowTemptation`, whose look goes through
    /// `LookAtTargetSink` -> `LookControl.setLookAt(Vec3)` (`FollowTemptation.java:87`,
    /// `LookAtTargetSink.java:23`) and so uses the mob's default head speeds instead of
    /// `TemptGoal.tick`'s `getMaxHeadYRot() + 20`.
    brain_follow_temptation: bool,
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
            target_player: None,
            cooldown: 0,
            prev_pos: Vector3::new(0.0, 0.0, 0.0),
            prev_yaw: 0.0,
            prev_pitch: 0.0,
            stop_distance,
            is_running: false,
            variant: TemptVariant::Plain,
            running_flag: None,
            non_pathfinder: false,
            brain_follow_temptation: false,
        }
    }

    /// Marks this goal as a stand-in for the brain behaviour `FollowTemptation`.
    #[must_use]
    pub const fn as_brain_follow_temptation(mut self) -> Self {
        self.brain_follow_temptation = true;
        self
    }

    /// Publishes `isRunning` (set in `start`, cleared in `stop`) to `flag`.
    #[must_use]
    pub fn with_running_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.running_flag = Some(flag);
        self
    }

    /// `Ocelot.OcelotTemptGoal` (`Ocelot.java:302-314`).
    #[must_use]
    pub fn for_ocelot(speed: f64, tempt_items: &'static [&'static Item], can_scare: bool) -> Self {
        Self {
            variant: TemptVariant::Ocelot,
            ..Self::new(speed, tempt_items, can_scare)
        }
    }

    /// Nautilus `FollowTemptation(mob -> 1.3F, mob -> mob.isBaby() ? 2.5 : 3.5)`
    /// (`NautilusAi.java:86`); it has no scare check.
    #[must_use]
    pub fn for_nautilus(speed: f64, tempt_items: &'static [&'static Item]) -> Self {
        Self {
            variant: TemptVariant::Nautilus,
            brain_follow_temptation: true,
            ..Self::new(speed, tempt_items, false)
        }
    }

    /// `Cat.CatTemptGoal` (`Cat.java:648-676`).
    #[must_use]
    pub fn for_cat(speed: f64, tempt_items: &'static [&'static Item], can_scare: bool) -> Self {
        Self {
            variant: TemptVariant::Cat {
                selected_player: None,
            },
            ..Self::new(speed, tempt_items, can_scare)
        }
    }

    /// `TemptGoal.ForNonPathfinders` (`TemptGoal.java:132-146`), used by the Happy Ghast.
    #[must_use]
    pub fn for_non_pathfinders(
        speed: f64,
        tempt_items: &'static [&'static Item],
        can_scare: bool,
        stop_distance: f64,
    ) -> Self {
        Self {
            non_pathfinder: true,
            ..Self::with_stop_distance(speed, tempt_items, can_scare, stop_distance)
        }
    }

    /// Vanilla `TemptGoal.canScare`, including the `Ocelot`/`Cat` overrides.
    fn can_scare(&self, mob: &dyn Mob, player: &Player) -> bool {
        match &self.variant {
            TemptVariant::Plain => self.can_scare,
            TemptVariant::Ocelot => {
                self.can_scare
                    && !mob
                        .cast_any()
                        .downcast_ref::<OcelotEntity>()
                        .is_some_and(OcelotEntity::is_trusting)
            }
            TemptVariant::Nautilus => false,
            TemptVariant::Cat { selected_player } => {
                if *selected_player == Some(player.get_entity().entity_uuid) {
                    false
                } else {
                    self.can_scare
                }
            }
        }
    }

    /// The extra `canUse` condition of `Cat.CatTemptGoal` (`!cat.isTame()`).
    fn allowed_to_tempt(&self, mob: &dyn Mob) -> bool {
        !matches!(self.variant, TemptVariant::Cat { .. })
            || !mob
                .cast_any()
                .downcast_ref::<CatEntity>()
                .is_some_and(CatEntity::is_tame)
    }

    fn stop_distance(&self, mob: &dyn Mob) -> f64 {
        if matches!(self.variant, TemptVariant::Nautilus) {
            return if mob.get_entity().age.load(Ordering::Relaxed) < 0 {
                2.5
            } else {
                3.5
            };
        }
        self.stop_distance
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

    /// Vanilla `TemptGoal.stopNavigation`; `ForNonPathfinders` waits its move control instead.
    fn stop_navigation(&self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        if self.non_pathfinder {
            mob_entity
                .move_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_wait();
        } else {
            mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
        }
    }

    async fn is_holding_tempt_item(&self, player: &Player) -> bool {
        let main = player.inventory().held_item().await;
        if self.is_tempt_item(&main) {
            return true;
        }
        let off = player.inventory().off_hand_item().await;
        self.is_tempt_item(&off)
    }

    fn tempt_range(mob: &dyn Mob) -> f64 {
        mob.get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::TEMPT_RANGE)
    }

    async fn find_tempting_player(&self, mob: &dyn Mob) -> Option<Arc<Player>> {
        let mob_entity = mob.get_mob_entity();
        let pos = mob_entity.living_entity.entity.pos.load();
        let world = mob_entity.living_entity.entity.world.load();
        let range = Self::tempt_range(mob);

        // Vanilla TemptGoal#canUse: `getNearestPlayer` selects the closest qualifying player,
        // not an arbitrary one in range.
        let mut nearest: Option<(Arc<Player>, f64)> = None;
        for player in world.get_nearby_players(pos, range) {
            if !self.is_holding_tempt_item(&player).await {
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
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if self.cooldown > 0 {
                self.cooldown -= 1;
                return false;
            }
            self.target_player = self.find_tempting_player(mob).await;
            if !self.allowed_to_tempt(mob) {
                return false;
            }
            if let Some(player) = &self.target_player {
                self.prev_pos = player.get_entity().pos.load();
                self.is_running = true;
                true
            } else {
                false
            }
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(player) = self.target_player.clone() else {
                return false;
            };

            if self.can_scare(mob, &player) {
                let mob_entity = mob.get_mob_entity();
                let mob_pos = mob_entity.living_entity.entity.pos.load();
                let player_pos = player.get_entity().pos.load();
                let player_entity = player.get_entity();
                let player_yaw = player_entity.yaw.load();
                let player_pitch = player_entity.pitch.load();

                let dist_sq = mob_pos.squared_distance_to_vec(&player_pos);
                let ok = passes_scare_check(
                    dist_sq,
                    player_pos,
                    self.prev_pos,
                    player_yaw,
                    player_pitch,
                    self.prev_yaw,
                    self.prev_pitch,
                );

                if dist_sq >= SCARE_RANGE_SQUARED {
                    self.prev_pos = player_pos;
                }
                self.prev_yaw = player_yaw;
                self.prev_pitch = player_pitch;

                if !ok {
                    return false;
                }
            }

            self.target_player = self.find_tempting_player(mob).await;
            self.target_player.is_some() && self.allowed_to_tempt(mob)
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(player) = self.target_player.clone() else {
                return;
            };
            let mob_entity = mob.get_mob_entity();
            let player_pos = player.get_entity().pos.load();

            let player_eye_y = player.get_entity().get_eye_y();
            let mut look_control = mob_entity
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.brain_follow_temptation {
                look_control.look_at(mob, player_pos.x, player_eye_y, player_pos.z);
            } else {
                // `TemptGoal.tick` (`TemptGoal.java:112`).
                look_control.look_at_with_range(
                    player_pos.x,
                    player_eye_y,
                    player_pos.z,
                    mob.get_max_head_rotation() + 20.0,
                    mob.get_max_look_pitch_change(),
                );
            }
            drop(look_control);

            let mob_pos = mob_entity.living_entity.entity.pos.load();
            let stop_distance = self.stop_distance(mob);
            if mob_pos.squared_distance_to_vec(&player_pos) < stop_distance * stop_distance {
                self.stop_navigation(mob);
            } else if self.non_pathfinder {
                // `ForNonPathfinders.navigateTowards`: a random point on the line from the mob
                // to the player's eyes.
                let eye = Vector3::new(player_pos.x, player.get_entity().get_eye_y(), player_pos.z);
                let t: f64 = mob.get_random().random();
                let target = (eye - mob_pos) * t + mob_pos;
                mob_entity
                    .move_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_wanted_position(target.x, target.y, target.z, self.speed);
            } else {
                mob_entity
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_progress(NavigatorGoal::new(mob_pos, player_pos, self.speed));
            }

            if matches!(self.variant, TemptVariant::Cat { .. }) {
                let select = self.get_tick_count(600);
                let deselect = self.get_tick_count(500);
                let no_selection = matches!(
                    self.variant,
                    TemptVariant::Cat {
                        selected_player: None
                    }
                );
                // Vanilla short-circuits: the deselect roll only happens when
                // the select branch was not taken.
                if no_selection && mob.get_random().random_range(0..select) == 0 {
                    if let TemptVariant::Cat { selected_player } = &mut self.variant {
                        *selected_player = Some(player.get_entity().entity_uuid);
                    }
                } else if mob.get_random().random_range(0..deselect) == 0
                    && let TemptVariant::Cat { selected_player } = &mut self.variant
                {
                    *selected_player = None;
                }
            }
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(flag) = &self.running_flag {
                flag.store(true, Ordering::Relaxed);
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(flag) = &self.running_flag {
                flag.store(false, Ordering::Relaxed);
            }
            self.target_player = None;
            self.stop_navigation(mob);
            self.cooldown = 100;
            self.is_running = false;
        })
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

    #[test]
    fn constructors_select_variant() {
        assert!(matches!(
            TemptGoal::for_cat(0.6, &[], true).variant,
            TemptVariant::Cat {
                selected_player: None
            }
        ));
        assert!(matches!(
            TemptGoal::for_ocelot(0.6, &[], true).variant,
            TemptVariant::Ocelot
        ));
        let ghast = TemptGoal::for_non_pathfinders(1.0, &[], false, 7.0);
        assert!(ghast.non_pathfinder);
        assert!((ghast.stop_distance - 7.0).abs() < f64::EPSILON);
        assert!(!TemptGoal::new(1.0, &[], false).non_pathfinder);
    }

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
