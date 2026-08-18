use super::{Controls, Goal};
use crate::entity::EntityBase;
use crate::entity::ai::goal::GoalFuture;
use crate::entity::ai::pathfinder::{NavigatorGoal, path::Path};
use crate::entity::mob::Mob;
use crate::entity::predicate::EntityPredicate;
use pumpkin_data::{tag, tag::Taggable};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use uuid::Uuid;

const MAX_ATTACK_TIME: i64 = 20;

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

/// Vanilla: `MeleeAttackGoal::canPerformAttack` requires sensing line of sight.
async fn has_melee_line_of_sight(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
    mob.get_mob_entity().has_line_of_sight(target).await
}

pub struct MeleeAttackGoal {
    goal_control: Controls,
    speed: f64,
    pause_when_mob_idle: bool,
    /// Vanilla `MeleeAttackGoal.path`, retained from `canUse` until `start`.
    path: Option<Path>,
    path_target: Option<Uuid>,
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
            path: None,
            path_target: None,
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
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            let time = {
                let world = mob.get_entity().world.load();
                let level_time = world.level_time.lock().await;
                level_time.world_age
            };

            if time - self.last_update_time < MAX_ATTACK_TIME {
                return false;
            }
            self.last_update_time = time;

            let (destination, target) = {
                let target = mob.get_mob_entity().target.lock().await;
                let Some(target) = target.as_ref() else {
                    return false;
                };
                if !target.get_entity().is_alive() {
                    return false;
                }
                (target.get_entity().pos.load(), target.clone())
            };

            // Vanilla `MeleeAttackGoal.canUse`:
            // this.path = this.mob.getNavigation().createPath(target, 0);
            // return this.path != null ? true : this.mob.isWithinMeleeAttackRange(target);
            let mob_entity = mob.get_mob_entity();
            // Pathfinding is async. Probe a scratch navigator so the live navigator remains
            // installed while the world is being read and another AI tick cannot overwrite it.
            let mut navigator = mob_entity.navigator.lock().unwrap().path_probe();
            let path = navigator
                .compute_path_with_reach(&mob_entity.living_entity, destination, 0)
                .await;

            // `canUse` and `start` are separate vanilla phases. Do not install a path for a
            // target that was replaced while the asynchronous probe was running.
            let target_is_current =
                mob_entity
                    .target
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|current| {
                        current.get_entity().entity_uuid == target.get_entity().entity_uuid
                    });
            if !target_is_current {
                self.path = None;
                self.path_target = None;
                return false;
            }

            let path_found = path.is_some();
            self.path = path;
            self.path_target = path_found.then_some(target.get_entity().entity_uuid);
            let in_attack_range =
                !path_found && mob_entity.is_in_attack_range(target.as_ref()).await;
            should_start_melee_goal(path_found, in_attack_range)
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            let target = mob.get_mob_entity().target.lock().await.clone();

            let Some(target) = target else {
                return false;
            };
            if !target.get_entity().is_alive() {
                return false;
            }

            let in_range = mob
                .get_mob_entity()
                .is_in_position_target_range_pos(&target.get_entity().block_pos.load());

            let is_valid_target = !target
                .get_player()
                .is_some_and(|p| p.is_spectator() || p.is_creative());
            let navigation_idle = mob
                .get_mob_entity()
                .navigator
                .try_lock()
                .is_ok_and(|navigator| navigator.is_idle());
            should_continue_melee_goal(
                self.pause_when_mob_idle,
                navigation_idle,
                in_range,
                is_valid_target,
            )
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            // TODO: add missing fields like mob attacking to true and correct Navigation methods

            let target = mob.get_mob_entity().target.lock().await.clone();
            if let Some(target) = target {
                let pathless_in_attack_range = self.path.is_none()
                    && mob
                        .get_mob_entity()
                        .is_in_attack_range(target.as_ref())
                        .await;
                let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
                let target_pos = target.get_entity().pos.load();
                let goal = NavigatorGoal {
                    current_progress: mob.get_entity().pos.load(),
                    destination: target_pos,
                    speed: self.speed,
                };
                let path = self
                    .path_target
                    .is_some_and(|uuid| uuid == target.get_entity().entity_uuid)
                    .then(|| self.path.take())
                    .flatten();
                if let Some(mut path) = path {
                    trim_cauldron_path(&mut path, &mob.get_entity().world.load());
                    navigator.set_path(goal, path);
                } else if !pathless_in_attack_range {
                    navigator.set_progress(goal);
                }
                self.last_target_position = Some(target_pos);
            }
            mob.get_mob_entity().set_attacking(true);
            self.update_countdown_ticks = 0;
            self.cooldown = 0;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            // Only clear target if they switched to creative/spectator
            let should_clear = {
                let target = mob.get_mob_entity().target.lock().await;
                if let Some(entity) = target.as_deref() {
                    !EntityPredicate::ExceptCreativeOrSpectator
                        .test(entity.get_entity())
                        .await
                } else {
                    false
                }
            };
            if should_clear {
                mob.set_mob_target(None).await;
            }

            // Vanilla: this.mob.getNavigation().stop()
            mob.get_mob_entity().navigator.lock().unwrap().stop();
            mob.get_mob_entity().set_attacking(false);
            self.last_target_position = None;
            self.path_target = None;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            let target = mob.get_mob_entity().target.lock().await.clone();
            let Some(target) = target else {
                return;
            };

            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap()
                .look_at_entity_with_range(&target, 30.0, 30.0);

            self.update_countdown_ticks = (self.update_countdown_ticks - 1).max(0);

            let current_target_pos = target.get_entity().pos.load();
            let should_update_nav = self.update_countdown_ticks <= 0
                && (self.pause_when_mob_idle
                    || has_melee_line_of_sight(mob, target.as_ref()).await)
                && (self.last_target_position.is_none_or(|last_pos| {
                    current_target_pos.squared_distance_to_vec(&last_pos) >= 1.0
                }) || mob.get_random().random_range(0..20) == 0);

            if should_update_nav {
                let mob_pos = mob.get_entity().pos.load();
                let dist_sq = mob_pos.squared_distance_to_vec(&current_target_pos);
                let goal = NavigatorGoal {
                    current_progress: mob_pos,
                    destination: current_target_pos,
                    speed: self.speed,
                };
                let mut path_probe = mob.get_mob_entity().navigator.lock().unwrap().path_probe();
                let path = path_probe
                    .compute_path_with_reach(
                        &mob.get_mob_entity().living_entity,
                        current_target_pos,
                        0,
                    )
                    .await;
                let path_found = path.is_some();
                let target_is_current = mob
                    .get_mob_entity()
                    .target
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|current| {
                        current.get_entity().entity_uuid == target.get_entity().entity_uuid
                    });
                let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
                if target_is_current {
                    if let Some(mut path) = path {
                        trim_cauldron_path(&mut path, &mob.get_entity().world.load());
                        navigator.set_path(goal, path);
                    } else {
                        navigator.set_progress(goal);
                    }
                }
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
                && mob
                    .get_mob_entity()
                    .is_in_attack_range(target.as_ref())
                    .await
                && has_melee_line_of_sight(mob, target.as_ref()).await
            {
                self.cooldown = self.get_max_cooldown();
                mob.get_mob_entity().living_entity.swing_hand().await;
                mob.try_attack(target.as_ref()).await;
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

/// `PathNavigation.trimPath` raises waypoints that land in a cauldron and the following waypoint
/// when it would otherwise descend into the same block. This is part of navigation start, not
/// path generation, so goals that reuse a computed path must apply it before `moveTo`.
fn trim_cauldron_path(path: &mut Path, world: &crate::world::World) {
    for index in 0..path.get_node_count() {
        let Some(node) = path.get_node(index).copied() else {
            continue;
        };
        if !world
            .get_block(&node.pos)
            .has_tag(&tag::Block::MINECRAFT_CAULDRONS)
        {
            continue;
        }

        let raised = BlockPos::new(node.pos.0.x, node.pos.0.y + 1, node.pos.0.z);
        path.replace_node(index, node.clone_and_move(raised));
        if let Some(next) = path.get_node(index + 1).copied()
            && node.pos.0.y >= next.pos.0.y
        {
            path.replace_node(
                index + 1,
                next.clone_and_move(BlockPos::new(next.pos.0.x, node.pos.0.y + 1, next.pos.0.z)),
            );
        }
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
