//! Port of `TryFindLandNearWater` + `TryLaySpawnOnFluidNearLand` (`FrogAi.initLaySpawnActivity`,
//! `FrogAi.java:143-168`), merged into one `Goal`.
//!
//! Vanilla runs these as two brain behaviors in the `LAY_SPAWN` activity, both gated on the
//! `IS_PREGNANT` memory: one sets a walk target on land near water, the other places the
//! frogspawn once the frog is standing there and erases the memory. Pumpkin has no memory
//! system, so `IS_PREGNANT` is a plain flag on `FrogEntity` and the walk half is expressed with
//! `MoveToTargetPosGoal`, exactly the way `turtle_lay_egg.rs` expresses `Turtle.TurtleLayEggGoal`.
//!
//! Deviations, all consequences of that merge:
//! - `TryFindLandNearWater.create(8, 1.0F)` samples random offsets around the frog and keeps the
//!   first land-next-to-water hit; `MoveToTargetPosGoal` instead sweeps a box around the frog.
//!   The acceptance test is the same one (`TryFindLandNearWater.java`: a non-water block with
//!   air above and water horizontally adjacent), so where the frog ends up is equivalent -- only
//!   the search order differs.
//! - Vanilla's `LAY_SPAWN` activity also runs `StartAttacking` and a `RunOne` idle bundle
//!   alongside laying. Those are the frog's ordinary goals here and keep running by priority.

use std::sync::Weak;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::Block;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use super::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use super::{Controls, Goal, ParentHandle};
use crate::entity::mob::Mob;
use crate::entity::passive::frog::FrogEntity;
use crate::world::World;

/// `TryFindLandNearWater.create(8, 1.0F)` (`FrogAi.java:150`).
const SEARCH_RANGE: i32 = 8;

/// The four horizontal directions `Direction.Plane.HORIZONTAL` iterates
/// (`TryLaySpawnOnFluidNearLand.java:27`), as (dx, dz).
const HORIZONTAL: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];

/// Walks a pregnant frog to land beside water and lays a frogspawn block on the water.
pub struct FrogLaySpawnGoal {
    frog: Weak<FrogEntity>,
    move_to_target_pos_goal: MoveToTargetPosGoal<Self>,
}

impl FrogLaySpawnGoal {
    #[must_use]
    pub fn new(frog: Weak<FrogEntity>, speed: f64) -> Box<Self> {
        let mut this = Box::new(Self {
            frog,
            move_to_target_pos_goal: MoveToTargetPosGoal::with_default(
                ParentHandle::none(),
                speed,
                SEARCH_RANGE,
            ),
        });

        // SAFETY: the `Box` allocation is pinned and outlives the `ParentHandle` reference,
        // the same contract `TurtleLayEggGoal::new` relies on.
        this.move_to_target_pos_goal.move_to_target_pos = unsafe { ParentHandle::new(&this) };

        this
    }

    fn is_pregnant(&self) -> bool {
        self.frog.upgrade().is_some_and(|frog| frog.is_pregnant())
    }

    /// `TryLaySpawnOnFluidNearLand`'s fluid test (`TryLaySpawnOnFluidNearLand.java:29-33`):
    /// vanilla accepts `FluidTags.SUPPORTS_FROGSPAWN` (water only, per the generated tag) or
    /// `BlockTags.SUPPORTS_FROGSPAWN` (currently empty), and requires the block's up-face
    /// collision shape to be empty.
    fn supports_frogspawn(world: &World, pos: &BlockPos) -> bool {
        let state = world.get_block_state(pos);
        !state.is_solid() && world.get_block(pos).id == Block::WATER.id
    }

    /// `TryLaySpawnOnFluidNearLand.create` (`TryLaySpawnOnFluidNearLand.java:22-46`), run once
    /// the frog is standing on land. Returns true when a frogspawn was placed.
    fn try_lay_spawn(mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        if entity.touching_water.load(Relaxed) || !entity.on_ground.load(Relaxed) {
            return false;
        }

        let world = entity.world.load();
        let below = entity.block_pos.load().down();
        for (dx, dz) in HORIZONTAL {
            let relative = BlockPos::new(below.0.x + dx, below.0.y, below.0.z + dz);
            if !Self::supports_frogspawn(&world, &relative) {
                continue;
            }

            let spawn_pos = relative.up();
            if !world.get_block_state(&spawn_pos).is_air() {
                continue;
            }

            world.set_block_state(
                &spawn_pos,
                Block::FROGSPAWN.default_state.id,
                BlockFlags::NOTIFY_ALL,
            );
            world.play_sound(
                Sound::EntityFrogLaySpawn,
                SoundCategory::Blocks,
                &entity.pos.load(),
            );
            return true;
        }

        false
    }
}

impl Goal for FrogLaySpawnGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !self.is_pregnant() {
            return false;
        }
        self.move_to_target_pos_goal.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.is_pregnant() && self.move_to_target_pos_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.move_to_target_pos_goal.start(mob)
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.move_to_target_pos_goal.stop(mob)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.move_to_target_pos_goal.tick(mob);

        if !self.move_to_target_pos_goal.reached {
            return;
        }
        let Some(frog) = self.frog.upgrade() else {
            return;
        };
        if !frog.is_pregnant() {
            return;
        }

        if Self::try_lay_spawn(mob) {
            // `pregnant.erase()` (`TryLaySpawnOnFluidNearLand.java:39`).
            frog.set_pregnant(false);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.move_to_target_pos_goal.controls()
    }
}

impl MoveToTargetPos for FrogLaySpawnGoal {
    /// `TryFindLandNearWater`'s acceptance test: dry land with headroom and water beside it.
    fn is_target_pos(&self, world: std::sync::Arc<World>, block_pos: BlockPos) -> bool {
        if !world.get_block_state(&block_pos.up()).is_air() {
            return false;
        }
        if !world.get_block_state(&block_pos).is_solid() {
            return false;
        }
        HORIZONTAL.into_iter().any(|(dx, dz)| {
            let neighbour = BlockPos::new(block_pos.0.x + dx, block_pos.0.y, block_pos.0.z + dz);
            Self::supports_frogspawn(&world, &neighbour)
        })
    }
}
