use std::sync::Arc;
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
use crate::world::{BlockBreakingProgress, World};

/// Vanilla `BreakDoorGoal.DEFAULT_DOOR_BREAK_TIME`.
const DEFAULT_DOOR_BREAK_TIME: i32 = 240;

/// A `Predicate<Difficulty>` gating when the door may be broken (vanilla's
/// `validDifficulties`).
pub type DifficultyPredicate = Arc<dyn Fn(Difficulty) -> bool + Send + Sync>;

/// Vanilla: `BreakDoorGoal` (and `Vindicator.VindicatorBreakDoorGoal` via `raid_gated`).
///
/// Ports `net.minecraft.world.entity.ai.goal.BreakDoorGoal`: shares `DoorInteractGoal`'s door
/// discovery, but breaks the door open over time (destroying the block) rather than opening it,
/// gated on the `MOB_GRIEFING` gamerule and a caller-supplied valid-difficulty predicate. Note
/// vanilla's `getDoorBreakTime` is `Math.max(240, doorBreakTime)`, so the constructor's `seconds`
/// parameter (Vindicator passes `6`) is irrelevant in practice -- break time is 240 ticks
/// (12 seconds) unless a caller asks for longer.
///
/// The base `BreakDoorGoal` (`BreakDoorGoal.java`) has no raid check and no goal flags.
/// Vindicator's private `VindicatorBreakDoorGoal` (`Vindicator.java:182-205`) adds the `MOVE`
/// flag, gates `canUse` on `hasActiveRaid() && random.nextInt(reducedTickDelay(10)) == 0`, gates
/// `canContinueToUse` on `hasActiveRaid()`, and resets `noActionTime` on `start`. `raid_gated`
/// opts a caller into that variant; other users (e.g. `Zombie`, whose `DOOR_BREAKING_PREDICATE`
/// is plain `d == Difficulty.HARD`) leave it off.
pub struct BreakDoorGoal {
    only_during_raid: bool,
    /// `DoorInteractGoal.doorPos` / `hasDoor`.
    door_pos: Option<BlockPos>,
    valid_difficulties: DifficultyPredicate,
    pub break_time: i32,
    pub last_break_progress: i32,
    pub door_break_time: i32,
}

impl BreakDoorGoal {
    /// `BreakDoorGoal(mob, validDifficulties)` (`BreakDoorGoal.java:16-19`).
    #[must_use]
    pub fn new(valid_difficulties: impl Fn(Difficulty) -> bool + Send + Sync + 'static) -> Self {
        Self {
            only_during_raid: false,
            door_pos: None,
            valid_difficulties: Arc::new(valid_difficulties),
            break_time: 0,
            last_break_progress: -1,
            door_break_time: -1,
        }
    }

    /// `BreakDoorGoal(mob, seconds, validDifficulties)` (`BreakDoorGoal.java:21-24`).
    #[must_use]
    pub fn with_door_break_time(
        door_break_time: i32,
        valid_difficulties: impl Fn(Difficulty) -> bool + Send + Sync + 'static,
    ) -> Self {
        let mut goal = Self::new(valid_difficulties);
        goal.door_break_time = door_break_time;
        goal
    }

    /// Vanilla: `Vindicator.VindicatorBreakDoorGoal` -- gates use on `hasActiveRaid()`. See the
    /// struct doc for why this is a builder rather than baked in.
    #[must_use]
    pub const fn raid_gated(mut self, only_during_raid: bool) -> Self {
        self.only_during_raid = only_during_raid;
        self
    }

    /// `BreakDoorGoal.getDoorBreakTime` (`BreakDoorGoal.java:26-28`).
    #[must_use]
    pub const fn get_door_break_time(&self) -> i32 {
        if self.door_break_time > DEFAULT_DOOR_BREAK_TIME {
            self.door_break_time
        } else {
            DEFAULT_DOOR_BREAK_TIME
        }
    }

    #[must_use]
    pub fn is_valid_difficulty(&self, difficulty: Difficulty) -> bool {
        (self.valid_difficulties)(difficulty)
    }

    /// `DoorInteractGoal.isOpen` (`DoorInteractGoal.java:26-38`): clears `hasDoor` once the
    /// block is no longer a door.
    fn is_open(&mut self, world: &World) -> bool {
        let Some(door_pos) = self.door_pos else {
            return false;
        };
        let block = world.get_block(&door_pos);
        if !block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_DOORS) {
            self.door_pos = None;
            return false;
        }
        InteractWithDoorGoal::is_door_open(world, &door_pos)
    }

    /// `DoorInteractGoal.canUse` (`DoorInteractGoal.java:50-75`).
    fn find_door(&mut self, mob: &dyn Mob, world: &World) -> bool {
        let mob_entity = mob.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;

        if !entity.horizontal_collision.load(Relaxed) {
            return false;
        }

        let navigator = mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(path) = navigator.get_current_path() else {
            return false;
        };
        if path.is_done() {
            return false;
        }

        // Vanilla scans from node 0 (not the next node) up to `nextNodeIndex + 2`, and measures
        // the distance to the door's block corner at the mob's own Y.
        let mob_pos = entity.pos.load();
        let scan_end = std::cmp::min(path.get_next_node_index() + 2, path.get_node_count());
        for i in 0..scan_end {
            let Some(node) = path.get_node(i) else {
                continue;
            };
            let check_pos = BlockPos::new(node.pos.0.x, node.pos.0.y + 1, node.pos.0.z);
            let dx = mob_pos.x - f64::from(check_pos.0.x);
            let dz = mob_pos.z - f64::from(check_pos.0.z);
            if dx * dx + dz * dz <= 2.25
                && InteractWithDoorGoal::is_mob_interactable_door(world, &check_pos)
            {
                self.door_pos = Some(check_pos);
                return true;
            }
        }

        let check_pos = entity.block_pos.load().up();
        if InteractWithDoorGoal::is_mob_interactable_door(world, &check_pos) {
            self.door_pos = Some(check_pos);
            return true;
        }
        self.door_pos = None;
        false
    }
}

impl Default for BreakDoorGoal {
    /// `Zombie.breakDoorGoal` (`Zombie.java:95-98`): hard difficulty only.
    fn default() -> Self {
        Self::new(hard_only)
    }
}

impl Goal for BreakDoorGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if self.only_during_raid {
                let living = &mob.get_mob_entity().living_entity;
                if !living.has_active_raid()
                    || mob.get_random().random_range(0..to_goal_ticks(10)) != 0
                {
                    return false;
                }
            }

            let world = mob.get_entity().world.load_full();
            if !self.find_door(mob, &world) {
                return false;
            }
            let level_info = world.level_info.load();
            if !level_info.game_rules.mob_griefing {
                return false;
            }
            self.is_valid_difficulty(level_info.difficulty) && !self.is_open(&world)
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            if self.only_during_raid && !mob_entity.living_entity.has_active_raid() {
                return false;
            }
            let Some(door_pos) = self.door_pos else {
                return false;
            };
            let entity = &mob_entity.living_entity.entity;
            let world = entity.world.load_full();

            if self.break_time > self.get_door_break_time() || self.is_open(&world) {
                return false;
            }
            // `doorPos.closerToCenterThan(mob.position(), 2.0)`: strict, 3D, block center.
            let dist_sq = door_pos
                .to_centered_f64()
                .squared_distance_to_vec(&entity.pos.load());
            if dist_sq >= 4.0 {
                return false;
            }
            self.is_valid_difficulty(world.level_info.load().difficulty)
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            // `DoorInteractGoal.start` only seeds the `passed` tracking, which
            // `BreakDoorGoal.canContinueToUse` never consults.
            self.break_time = 0;
            if self.only_during_raid {
                mob.get_mob_entity().no_action_time.store(0, Relaxed);
            }
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

            // `BreakDoorGoal.tick` (:62-67): 1-in-20 door-hit sound (level event 1019) plus an
            // arm swing.
            if mob.get_random().random_range(0..20) == 0 {
                world.sync_world_event(WorldEvent::SoundZombieWoodenDoor, door_pos, 0);
                mob.get_mob_entity().living_entity.swing_hand().await;
            }

            self.break_time += 1;
            let progress =
                (self.break_time as f32 / self.get_door_break_time() as f32 * 10.0) as i32;
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

            if self.break_time == self.get_door_break_time()
                && self.is_valid_difficulty(world.level_info.load().difficulty)
            {
                mob.break_door(door_pos).await;
                // `level.removeBlock(doorPos, false)` (:77): a plain `setBlock` to the fluid's
                // legacy block with flags 3, no drops and no destroy effects of its own. Doors
                // cannot be waterlogged, so that legacy block is always air.
                world
                    .set_block_state(&door_pos, BlockStateId::AIR, BlockFlags::NOTIFY_ALL)
                    .await;
                world.sync_world_event(WorldEvent::SoundZombieDoorCrash, door_pos, 0);
                // Vanilla reads the state *after* `removeBlock` for the 2001 payload (:79).
                let state_id = world.get_block_state_id(&door_pos);
                world.sync_world_event(
                    WorldEvent::ParticlesDestroyBlock,
                    door_pos,
                    i32::from(state_id.as_u16()),
                );
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    /// Base `BreakDoorGoal` sets no flags; `VindicatorBreakDoorGoal` sets `MOVE`.
    fn controls(&self) -> Controls {
        if self.only_during_raid {
            Controls::MOVE
        } else {
            Controls::empty()
        }
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
    use super::*;

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

    #[test]
    fn door_break_time_default_and_custom() {
        let default_goal = BreakDoorGoal::default();
        assert_eq!(default_goal.get_door_break_time(), 240);

        let custom_goal = BreakDoorGoal::with_door_break_time(300, hard_only);
        assert_eq!(custom_goal.get_door_break_time(), 300);

        // Vindicator passes 6, which `Math.max(240, doorBreakTime)` discards.
        let low_goal = BreakDoorGoal::with_door_break_time(6, normal_or_hard);
        assert_eq!(low_goal.get_door_break_time(), 240);
    }

    #[test]
    fn valid_difficulty() {
        let hard_only_goal = BreakDoorGoal::default();
        assert!(hard_only_goal.is_valid_difficulty(Difficulty::Hard));
        assert!(!hard_only_goal.is_valid_difficulty(Difficulty::Normal));
        assert!(!hard_only_goal.is_valid_difficulty(Difficulty::Easy));
        assert!(!hard_only_goal.is_valid_difficulty(Difficulty::Peaceful));

        let closure_goal = BreakDoorGoal::new(|d| d != Difficulty::Peaceful);
        assert!(closure_goal.is_valid_difficulty(Difficulty::Easy));
    }

    #[test]
    fn break_progress_calculation() {
        let break_time = 120;
        let total_time = 240;
        let progress = (break_time as f32 / total_time as f32 * 10.0) as i32;
        assert_eq!(progress, 5);

        let end_progress = (240.0f32 / 240.0f32 * 10.0) as i32;
        assert_eq!(end_progress, 10);
    }
}
