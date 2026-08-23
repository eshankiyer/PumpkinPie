//! Port of `CatLieOnBedGoal.java`.

use std::pin::Pin;
use std::sync::{Arc, Weak};

use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;

use super::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use super::{Controls, Goal, GoalFuture, ParentHandle};
use crate::entity::mob::Mob;
use crate::entity::passive::cat::CatEntity;
use crate::entity::passive::tamable::TamableAnimal;
use crate::world::World;

/// `Cat.java:112`: `new CatLieOnBedGoal(this, 1.1, 8)`.
const SEARCH_RANGE: i32 = 8;
/// `CatLieOnBedGoal.java:14`: the `verticalSearchRange` argument of `MoveToBlockGoal`.
const VERTICAL_SEARCH_RANGE: i32 = 6;
/// `CatLieOnBedGoal.java:16`: `this.verticalSearchStart = -2`.
const VERTICAL_SEARCH_START: i32 = -2;
/// `CatLieOnBedGoal.nextStartTick` (line 32) returns a flat 40 instead of
/// `MoveToBlockGoal`'s randomised 200..400.
const NEXT_START_TICK: i32 = 40;

/// Vanilla `CatLieOnBedGoal` (`CatLieOnBedGoal.java:10-56`): a tamed cat that is neither
/// ordered to sit nor already lying walks to a nearby bed and lies down on it.
pub struct CatLieOnBedGoal {
    cat: Weak<CatEntity>,
    move_to_target_pos_goal: MoveToTargetPosGoal<Self>,
}

impl CatLieOnBedGoal {
    #[must_use]
    pub fn new(cat: Weak<CatEntity>, speed: f64) -> Box<Self> {
        let mut goal = MoveToTargetPosGoal::new(
            ParentHandle::none(),
            speed,
            SEARCH_RANGE,
            VERTICAL_SEARCH_RANGE,
        );
        goal.lowest_y = VERTICAL_SEARCH_START;

        let mut this = Box::new(Self {
            cat,
            move_to_target_pos_goal: goal,
        });

        // SAFETY: `this` heap allocation address is pinned in Box and outlives `ParentHandle` references.
        this.move_to_target_pos_goal.move_to_target_pos = unsafe { ParentHandle::new(&this) };

        this
    }

    /// `CatLieOnBedGoal.canUse` (line 22), minus the `super.canUse()` half.
    fn cat_may_lie(&self) -> bool {
        self.cat.upgrade().is_some_and(|cat| {
            cat.is_tame() && !cat.mob_entity.is_ordered_to_sit() && !cat.is_lying()
        })
    }

    fn is_valid(world: &Arc<World>, block_pos: BlockPos) -> bool {
        // `isValidTarget` line 55.
        world.get_block_state(&block_pos.up()).is_air()
            && world
                .get_block(&block_pos)
                .has_tag(&tag::Block::MINECRAFT_BEDS)
    }
}

impl MoveToTargetPos for CatLieOnBedGoal {
    fn is_target_pos<'a>(
        &'a self,
        world: Arc<World>,
        block_pos: BlockPos,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { Self::is_valid(&world, block_pos) })
    }
}

impl Goal for CatLieOnBedGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            if !self.cat_may_lie() {
                return false;
            }
            // `MoveToBlockGoal.canUse` (lines 37-44) with this goal's `nextStartTick` override
            // (line 32). `MoveToTargetPosGoal::can_start` would use the base class's randomised
            // 200..400 interval, so the cooldown is driven here instead.
            let goal = &mut self.move_to_target_pos_goal;
            if goal.cooldown > 0 {
                goal.cooldown -= 1;
                return false;
            }
            goal.cooldown = NEXT_START_TICK;
            goal.find_target_pos(mob).await
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { self.move_to_target_pos_goal.should_continue(mob).await })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.move_to_target_pos_goal.start(mob).await;
            // `start` line 28: `this.cat.setInSittingPose(false)`.
            if let Some(cat) = self.cat.upgrade()
                && cat.is_sitting()
            {
                cat.set_sitting(false);
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.move_to_target_pos_goal.stop(mob).await;
            // `stop` line 39: `this.cat.setLying(false)`.
            if let Some(cat) = self.cat.upgrade()
                && cat.is_lying()
            {
                cat.set_lying(false);
            }
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.move_to_target_pos_goal.tick(mob).await;
            let Some(cat) = self.cat.upgrade() else {
                return;
            };
            // `tick` lines 44-50. The `set_*` calls send metadata packets unconditionally here
            // (vanilla's `SynchedEntityData` drops no-op writes), so each is change-guarded.
            if cat.is_sitting() {
                cat.set_sitting(false);
            }
            let lying = self.move_to_target_pos_goal.reached;
            if cat.is_lying() != lying {
                cat.set_lying(lying);
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.move_to_target_pos_goal.controls()
    }
}
