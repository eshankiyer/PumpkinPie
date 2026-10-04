// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::BlockStateId;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use super::interact_with_door::InteractWithDoorGoal;
use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::mob::Mob;
use crate::world::BlockBreakingProgress;

/// Vanilla: `BreakDoorGoal` (via `Vindicator.VindicatorBreakDoorGoal`).
///
/// Ports `net.minecraft.world.entity.ai.goal.BreakDoorGoal`: shares `DoorInteractGoal`'s door
/// discovery, but breaks the door open over time (destroying the block) rather than opening it,
/// gated on the `MOB_GRIEFING` gamerule and a caller-supplied valid-difficulty predicate. Note
/// vanilla's `getDoorBreakTime` is `Math.max(240, doorBreakTime)`, so the constructor's `seconds`
/// parameter (Vindicator passes `6`) is actually irrelevant in practice -- break time is always
/// 240 ticks (12 seconds).
///
/// The base `BreakDoorGoal.canUse`/`canContinueToUse` (`BreakDoorGoal.java:30-53`) has no raid
/// check at all -- that's added only by Vindicator's private `VindicatorBreakDoorGoal` inner
/// class (`Vindicator.java:188-197`): `canUse` requires `hasActiveRaid()` and a
/// `nextInt(reducedTickDelay(10)) == 0` roll before `super.canUse()`, and `canContinueToUse`
/// requires `hasActiveRaid()`. `raid_gated` opts a caller into both; other users (e.g. `Zombie`,
/// whose `DOOR_BREAKING_PREDICATE` is plain `d == Difficulty.HARD` with no raid involvement)
/// leave it off.
pub struct BreakDoorGoal {
    only_during_raid: bool,
    door_pos: Option<BlockPos>,
    break_time: i32,
    last_break_progress: i32,
    valid_difficulties: fn(Difficulty) -> bool,
}

impl BreakDoorGoal {
    const DOOR_BREAK_TIME: i32 = 240;

    #[must_use]
    pub const fn new(valid_difficulties: fn(Difficulty) -> bool) -> Self {
        Self {
            only_during_raid: false,
            door_pos: None,
            break_time: 0,
            last_break_progress: -1,
            valid_difficulties,
        }
    }

    /// Vanilla: `Vindicator.VindicatorBreakDoorGoal.canUse`/`canContinueToUse` -- gates on
    /// `hasActiveRaid()`. See the struct doc for why this is a builder rather than baked in.
    #[must_use]
    pub const fn raid_gated(mut self, only_during_raid: bool) -> Self {
        self.only_during_raid = only_during_raid;
        self
    }

    fn is_valid_difficulty(&self, difficulty: Difficulty) -> bool {
        (self.valid_difficulties)(difficulty)
    }
}

impl Goal for BreakDoorGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            let entity = &mob_entity.living_entity.entity;

            // `VindicatorBreakDoorGoal.canUse`: raid check, then `nextInt(reducedTickDelay(10))`,
            // then `super.canUse()`.
            if self.only_during_raid
                && (!mob_entity.living_entity.has_active_raid()
                    || mob.get_random().random_range(0..to_goal_ticks(10)) != 0)
            {
                return false;
            }

            if !entity.horizontal_collision.load(Relaxed) {
                return false;
            }

            let world = entity.world.load_full();
            if !world.level_info.load().game_rules.mob_griefing {
                return false;
            }
            let difficulty = world.level_info.load().difficulty;
            if !self.is_valid_difficulty(difficulty) {
                return false;
            }

            let navigator = mob_entity.navigator.lock().unwrap();
            let Some(path) = navigator.get_current_path() else {
                return false;
            };
            if path.is_done() {
                return false;
            }

            // `DoorInteractGoal.canUse` (`DoorInteractGoal.java:59-74`): the first wooden door
            // among nodes `0..min(nextNodeIndex + 2, nodeCount)` within horizontal distance 1.5
            // of the door block's integer corner (`distanceToSqr(doorPos.getX(), getY(),
            // doorPos.getZ())`), else the block above the mob.
            let next_index = path.get_next_node_index();
            let node_count = path.get_node_count();
            let scan_end = std::cmp::min(next_index + 2, node_count);
            let mob_pos = entity.pos.load();

            let mut door_pos = None;
            for i in 0..scan_end {
                if let Some(node) = path.get_node(i) {
                    let check_pos = BlockPos::new(node.pos.0.x, node.pos.0.y + 1, node.pos.0.z);
                    let dx = f64::from(check_pos.0.x) - mob_pos.x;
                    let dz = f64::from(check_pos.0.z) - mob_pos.z;
                    if dx * dx + dz * dz <= 2.25
                        && InteractWithDoorGoal::is_mob_interactable_door(&world, &check_pos)
                    {
                        door_pos = Some(check_pos);
                        break;
                    }
                }
            }
            let door_pos = door_pos.or_else(|| {
                let check_pos = entity.block_pos.load().up();
                InteractWithDoorGoal::is_mob_interactable_door(&world, &check_pos)
                    .then_some(check_pos)
            });

            // `BreakDoorGoal.canUse` (`BreakDoorGoal.java:37`) rejects the chosen door if it is
            // already open rather than searching on.
            let Some(door_pos) = door_pos else {
                return false;
            };
            if InteractWithDoorGoal::is_door_open(&world, &door_pos) {
                return false;
            }
            self.door_pos = Some(door_pos);
            true
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(door_pos) = self.door_pos else {
                return false;
            };
            let mob_entity = mob.get_mob_entity();
            if self.only_during_raid && !mob_entity.living_entity.has_active_raid() {
                return false;
            }
            let entity = &mob_entity.living_entity.entity;
            let world = entity.world.load_full();

            if self.break_time > Self::DOOR_BREAK_TIME
                || InteractWithDoorGoal::is_door_open(&world, &door_pos)
            {
                return false;
            }
            let dist_sq = door_pos
                .to_centered_f64()
                .squared_distance_to_vec(&entity.pos.load());
            // `closerToCenterThan(position, 2.0)` is strict.
            if dist_sq >= 4.0 {
                return false;
            }
            self.is_valid_difficulty(world.level_info.load().difficulty)
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.break_time = 0;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(door_pos) = self.door_pos {
                let entity = &mob.get_mob_entity().living_entity.entity;
                let world = entity.world.load_full();
                world
                    .set_block_breaking(entity, door_pos, BlockBreakingProgress::Stop)
                    .await;
            }
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(door_pos) = self.door_pos else {
                return;
            };
            let entity = &mob.get_mob_entity().living_entity.entity;
            let world = entity.world.load_full();

            // `BreakDoorGoal.java:64-69`: door-bang sound and arm swing.
            if mob.get_random().random_range(0..20) == 0 {
                world.sync_world_event(WorldEvent::SoundZombieWoodenDoor, door_pos, 0);
                mob.get_mob_entity().living_entity.swing_hand().await;
            }

            self.break_time += 1;
            let progress = ((self.break_time as f32 / Self::DOOR_BREAK_TIME as f32) * 10.0) as i32;
            if progress != self.last_break_progress {
                world
                    .set_block_breaking(
                        entity,
                        door_pos,
                        BlockBreakingProgress::Update {
                            stage: progress,
                            speed: None,
                        },
                    )
                    .await;
                self.last_break_progress = progress;
            }

            if self.break_time == Self::DOOR_BREAK_TIME
                && self.is_valid_difficulty(world.level_info.load().difficulty)
            {
                // `level.removeBlock(doorPos, false)` with flags 3. The upper half drops nothing
                // itself; the neighbour update then destroys the lower half with drops, as in
                // vanilla (`Level.setBlock` clears `UPDATE_SUPPRESS_DROPS` for neighbours).
                world
                    .set_block_state(&door_pos, BlockStateId::AIR, BlockFlags::NOTIFY_ALL)
                    .await;
                world.sync_world_event(WorldEvent::SoundZombieDoorCrash, door_pos, 0);
                world.sync_world_event(
                    WorldEvent::ParticlesDestroyBlock,
                    door_pos,
                    world.get_block_state_id(&door_pos).as_u16().into(),
                );
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla: `Vindicator.DOOR_BREAKING_PREDICATE` (`d -> d == Difficulty.NORMAL || d ==
/// Difficulty.HARD`).
#[must_use]
pub const fn normal_or_hard(difficulty: Difficulty) -> bool {
    matches!(difficulty, Difficulty::Normal | Difficulty::Hard)
}

/// Vanilla: `Zombie.DOOR_BREAKING_PREDICATE` (`d -> d == Difficulty.HARD`).
#[must_use]
pub const fn hard_only(difficulty: Difficulty) -> bool {
    matches!(difficulty, Difficulty::Hard)
}

#[cfg(test)]
mod tests {
    use super::{hard_only, normal_or_hard};
    use pumpkin_util::Difficulty;

    #[test]
    fn door_break_predicate_excludes_easy_and_peaceful() {
        assert!(!normal_or_hard(Difficulty::Peaceful));
        assert!(!normal_or_hard(Difficulty::Easy));
        assert!(normal_or_hard(Difficulty::Normal));
        assert!(normal_or_hard(Difficulty::Hard));
    }

    #[test]
    fn zombie_door_break_predicate_is_hard_only() {
        assert!(!hard_only(Difficulty::Peaceful));
        assert!(!hard_only(Difficulty::Easy));
        assert!(!hard_only(Difficulty::Normal));
        assert!(hard_only(Difficulty::Hard));
    }
}
