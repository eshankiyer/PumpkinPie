// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::{Controls, Goal, to_goal_ticks};
use crate::entity::EntityBase;
use crate::entity::ai::brain::sensor::is_entity_attackable;
use crate::entity::ai::goal::GoalFuture;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::world::World;
use crate::world::scoreboard::entity_scoreboard_name;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::Taggable;
use pumpkin_util::Difficulty;
use rand::RngExt;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use uuid::Uuid;

const UNSET: i32 = 0;
const CAN_TRACK: i32 = 1;
const CANNOT_TRACK: i32 = 2;

fn team_name(
    world: &World,
    scoreboard: &crate::world::scoreboard::Scoreboard,
    entity: &dyn EntityBase,
    visited: &mut HashSet<Uuid>,
) -> Option<String> {
    if !visited.insert(entity.get_entity().entity_uuid) {
        return None;
    }

    if let Some(team) = scoreboard.get_team_for_scoreboard_name(&entity_scoreboard_name(entity)) {
        return Some(team.name.clone());
    }

    let owner_uuid = entity.get_mob().and_then(Mob::get_owner_uuid)?;
    let owner = world.get_entity_by_uuid(owner_uuid)?;
    team_name(world, scoreboard, owner.as_ref(), visited)
}

/// `AbstractIllager.considersEntityAsAlly` (`AbstractIllager.java:32-38`): the base scoreboard
/// team rule, or another `#illager_friends` entity while neither side is on a team.
pub fn illager_considers_entity_as_ally(
    this: &dyn EntityBase,
    other: &dyn EntityBase,
    scoreboard: &crate::world::scoreboard::Scoreboard,
) -> bool {
    let team_of = |entity: &dyn EntityBase| {
        scoreboard
            .get_team_for_scoreboard_name(&entity_scoreboard_name(entity))
            .map(|team| team.name.clone())
    };
    let this_team = team_of(this);
    let other_team = team_of(other);
    if this_team.is_some() && this_team == other_team {
        return true;
    }
    other
        .get_entity()
        .entity_type
        .has_tag(&pumpkin_data::tag::EntityType::MINECRAFT_ILLAGER_FRIENDS)
        && this_team.is_none()
        && other_team.is_none()
}

fn owner_chain_contains(
    world: &World,
    entity: &dyn EntityBase,
    wanted_uuid: Uuid,
    visited: &mut HashSet<Uuid>,
) -> bool {
    if !visited.insert(entity.get_entity().entity_uuid) {
        return false;
    }

    let Some(owner_uuid) = entity.get_mob().and_then(Mob::get_owner_uuid) else {
        return false;
    };
    if owner_uuid == wanted_uuid {
        return true;
    }

    world
        .get_entity_by_uuid(owner_uuid)
        .is_some_and(|owner| owner_chain_contains(world, owner.as_ref(), wanted_uuid, visited))
}

pub struct TrackTargetGoal {
    goal_control: Controls,
    check_visibility: bool,
    check_can_navigate: bool,
    can_navigate_flag: AtomicI32,
    check_can_navigate_cooldown: AtomicI32,
    time_without_visibility: AtomicI32,
    pub max_time_without_visibility: i32,
    target_predicate: TargetPredicate,
    follow_distance_multiplier: f64,
    /// `Sensor.wasEntityAttackableLastNTicks` window in goal invocations; `None` keeps the
    /// plain `TargetGoal.canContinueToUse` checks.
    attackable_grace: Option<i32>,
    /// `Sensor.rememberPositives`' `positivesLeft` (`sensing/Sensor.java:88-97`). Like the
    /// per-brain counter it starts at 0 and is never reset when the goal restarts.
    attackable_positives_left: AtomicI32,
}

impl TrackTargetGoal {
    #[must_use]
    pub fn new(check_visibility: bool, check_can_navigate: bool) -> Self {
        Self {
            goal_control: Controls::TARGET,
            check_visibility,
            check_can_navigate,
            can_navigate_flag: AtomicI32::new(UNSET),
            check_can_navigate_cooldown: AtomicI32::new(0),
            time_without_visibility: AtomicI32::new(0),
            max_time_without_visibility: 60,
            target_predicate: TargetPredicate::create_attackable().ignore_visibility(),
            follow_distance_multiplier: 1.0,
            attackable_grace: None,
            attackable_positives_left: AtomicI32::new(0),
        }
    }

    pub fn with_default(check_visibility: bool) -> Self {
        Self::new(check_visibility, false)
    }

    /// Vanilla `Entity.isAlliedTo`: scoreboard teams plus the owner alliance supplied by
    /// `TamableAnimal.considersEntityAsAlly` and the per-species `considersEntityAsAlly`
    /// overrides (`Mob::considers_entity_as_ally`).
    pub async fn is_allied(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
        Self::entities_allied(mob, target).await
    }

    /// `Entity.isAlliedTo(other)` for any two entities.
    pub async fn entities_allied(first: &dyn EntityBase, second: &dyn EntityBase) -> bool {
        let world = first.get_entity().world.load();
        let scoreboard = world.scoreboard.lock().await;
        let first_team = team_name(&world, &scoreboard, first, &mut HashSet::new());
        let second_team = team_name(&world, &scoreboard, second, &mut HashSet::new());
        let same_team = first_team.is_some() && first_team == second_team;
        let considered_ally = first
            .get_mob()
            .is_some_and(|mob| mob.considers_entity_as_ally(second, &world, &scoreboard))
            || second
                .get_mob()
                .is_some_and(|mob| mob.considers_entity_as_ally(first, &world, &scoreboard));
        drop(scoreboard);

        same_team
            || considered_ally
            || owner_chain_contains(
                &world,
                first,
                second.get_entity().entity_uuid,
                &mut HashSet::new(),
            )
            || owner_chain_contains(
                &world,
                second,
                first.get_entity().entity_uuid,
                &mut HashSet::new(),
            )
    }

    pub const fn set_unseen_memory_ticks(mut self, ticks: i32) -> Self {
        self.max_time_without_visibility = ticks;
        self
    }

    /// Replaces the continuation checks with the Breeze FIGHT activity's
    /// `StopAttackingIfTargetInvalid.create(wasEntityAttackableLastNTicks(body, n).negate())`
    /// (`BreezeAi.java:69`, `sensing/Sensor.java:78-97`): the target is kept until it has
    /// failed `Sensor.isEntityAttackable` for `server_ticks` ticks in a row.
    ///
    /// DEVIATIONS: the counter lives per target goal rather than per brain; the countdown does
    /// not pause while a `WALK_TARGET` exists (no activity system); and
    /// `CANT_REACH_WALK_TARGET_SINCE` tiredness (200 ticks) is not modelled.
    #[must_use]
    pub const fn set_attackable_grace_ticks(mut self, server_ticks: i32) -> Self {
        // `should_continue` runs on every other server tick.
        self.attackable_grace = Some(to_goal_ticks(server_ticks));
        self
    }

    /// `StopAttackingIfTargetInvalid` (`StopAttackingIfTargetInvalid.java:30-49`) with the
    /// `wasEntityAttackableLastNTicks` stop condition: `canAttack`, alive and same-level
    /// failures drop the target at once; only the attackability test is remembered.
    async fn continues_with_grace(
        &self,
        grace: i32,
        mob: &dyn Mob,
        target_base: &dyn EntityBase,
        target: &LivingEntity,
    ) -> bool {
        let mob_entity = mob.get_mob_entity();
        let world = mob_entity.living_entity.entity.world.load_full();
        // `body.canAttack(target)`: `Mob.canAttack` (`Mob.java:256-258`) plus
        // `LivingEntity.canAttack` (`LivingEntity.java:948-950`) and the species override.
        if target.entity.entity_type.id == EntityType::GHAST.id
            || !mob.can_attack(&target.entity)
            || (target.entity.entity_type.id == EntityType::PLAYER.id
                && world.level_info.load().difficulty == Difficulty::Peaceful)
            || !target.can_be_seen_as_enemy()
            || !Arc::ptr_eq(&world, &target.entity.world.load_full())
        {
            return false;
        }

        // `rememberPositives` (`Sensor.java:88-97`).
        if is_entity_attackable(mob, target_base, true).await {
            self.attackable_positives_left
                .store(grace, Ordering::Relaxed);
            return true;
        }
        // `--positivesLeft >= 0`, i.e. the pre-decrement value was positive.
        self.attackable_positives_left
            .fetch_sub(1, Ordering::Relaxed)
            > 0
    }

    /// Vanilla `PolarBearAttackPlayersGoal.getFollowDistance`: the target goal's follow
    /// distance is half the bear's `FOLLOW_RANGE` attribute (PolarBear.java:276-279).
    pub const fn set_follow_distance_multiplier(mut self, multiplier: f64) -> Self {
        self.follow_distance_multiplier = multiplier;
        self
    }

    async fn can_navigate_to_entity(&self, mob: &dyn Mob, target: &LivingEntity) -> bool {
        let cooldown = to_goal_ticks(10 + mob.get_random().random_range(0..5));
        self.check_can_navigate_cooldown
            .store(cooldown, Ordering::Relaxed);

        let mob_entity = mob.get_mob_entity();
        let mut navigator = {
            let navigator = mob_entity.navigator.lock().unwrap();
            navigator.path_probe()
        };
        // `Mob.onPathfindingStart/Done` wrap evaluator preparation and cleanup
        // (`Mob.java:194-198`, `WalkNodeEvaluator.java:39-49`).
        navigator.can_reach_entity_for_mob(mob, target).await
    }

    fn remembers_visible_target(&self, has_line_of_sight: bool) -> bool {
        if has_line_of_sight {
            self.time_without_visibility.store(0, Ordering::Relaxed);
            true
        } else {
            let unseen_ticks = self.time_without_visibility.fetch_add(1, Ordering::Relaxed) + 1;
            unseen_ticks <= to_goal_ticks(self.max_time_without_visibility)
        }
    }

    /// Equivalent to Vanilla's `canAttack` check inside `TargetGoal`
    pub async fn can_track(&mut self, mob: &dyn Mob, target: Option<&LivingEntity>) -> bool {
        let Some(target) = target else {
            return false;
        };

        let mob_entity = mob.get_mob_entity();
        let world = mob_entity.living_entity.entity.world.load();

        let follow_distance = mob_entity
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE)
            * self.follow_distance_multiplier;
        self.target_predicate.base_max_distance = follow_distance;

        // Vanilla `TargetingConditions.test`'s combat branch (`TargetingConditions.java:78`)
        // consults `targeter.canAttack(target)`.
        if !mob.can_attack(&target.entity) {
            return false;
        }

        if !self
            .target_predicate
            .test(&world, Some(&mob_entity.living_entity), target)
            .await
        {
            return false;
        }

        // Vanilla TargetGoal.isWithinHome(target.blockPosition()). Pumpkin's position target
        // and range are the existing home/restriction representation; a range of -1 is
        // unrestricted, matching vanilla's homeRadius sentinel.
        if !mob_entity.is_in_position_target_range_pos(&target.entity.block_pos.load()) {
            return false;
        }

        if self.check_can_navigate {
            let cooldown = self
                .check_can_navigate_cooldown
                .fetch_sub(1, Ordering::Relaxed)
                - 1;
            if cooldown <= 0 {
                self.can_navigate_flag.store(UNSET, Ordering::Relaxed);
            }

            if self.can_navigate_flag.load(Ordering::Relaxed) == UNSET {
                let can_reach = self.can_navigate_to_entity(mob, target).await;
                self.can_navigate_flag.store(
                    if can_reach { CAN_TRACK } else { CANNOT_TRACK },
                    Ordering::Relaxed,
                );
            }

            if self.can_navigate_flag.load(Ordering::Relaxed) == CANNOT_TRACK {
                return false;
            }
        }

        true
    }
}

impl Goal for TrackTargetGoal {
    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            let mob_entity = mob.get_mob_entity();
            let target_arc = mob_entity.target.lock().await.clone();

            let Some(target_base) = target_arc else {
                return false;
            };

            let Some(target) = target_base.get_living_entity() else {
                return false;
            };

            if !target.entity.is_alive() {
                return false;
            }

            if let Some(grace) = self.attackable_grace {
                if !self
                    .continues_with_grace(grace, mob, target_base.as_ref(), target)
                    .await
                {
                    return false;
                }
                mob.set_mob_target(Some(target_base.clone())).await;
                return true;
            }

            if !self.can_track(mob, Some(target)).await {
                return false;
            }

            if Self::is_allied(mob, target_base.as_ref()).await {
                return false;
            }

            let dist_sq = mob_entity
                .living_entity
                .entity
                .pos
                .load()
                .squared_distance_to_vec(&target.entity.pos.load());

            // Get follow range attribute value and check if target is within range
            let follow_range = mob_entity
                .living_entity
                .get_attribute_value(&Attributes::FOLLOW_RANGE)
                * self.follow_distance_multiplier;

            if dist_sq > follow_range * follow_range {
                return false;
            }

            if self.check_visibility {
                // TargetGoal uses LivingEntity.hasLineOfSight, which clips
                // against block collision shapes. Testing `is_solid()` here
                // incorrectly treats outline-only blocks such as fences as
                // transparent.
                let has_line_of_sight = mob_entity.has_line_of_sight(target_base.as_ref()).await;

                if !self.remembers_visible_target(has_line_of_sight) {
                    return false;
                }
            }

            mob.set_mob_target(Some(target_base.clone())).await;
            true
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.can_navigate_flag.store(UNSET, Ordering::Relaxed);
            self.check_can_navigate_cooldown.store(0, Ordering::Relaxed);
            self.time_without_visibility.store(0, Ordering::Relaxed);
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            mob.set_mob_target(None).await;
        })
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

#[cfg(test)]
mod tests {
    use super::{TrackTargetGoal, to_goal_ticks};
    use std::sync::atomic::Ordering;

    #[test]
    fn forgets_unseen_target_after_vanilla_memory_window() {
        let goal = TrackTargetGoal::with_default(true);
        let memory_ticks = to_goal_ticks(goal.max_time_without_visibility);

        for _ in 0..memory_ticks {
            assert!(goal.remembers_visible_target(false));
        }
        assert!(!goal.remembers_visible_target(false));
    }

    #[test]
    fn seeing_target_resets_unseen_memory() {
        let goal = TrackTargetGoal::with_default(true);
        assert!(goal.remembers_visible_target(false));
        assert!(goal.remembers_visible_target(true));
        assert_eq!(goal.time_without_visibility.load(Ordering::Relaxed), 0);
    }
}
