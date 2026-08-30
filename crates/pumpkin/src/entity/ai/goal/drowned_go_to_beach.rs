use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::BlockDirection;
use pumpkin_util::math::position::BlockPos;

use super::drowned_util::is_bright_outside;
use super::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use super::{Controls, Goal, GoalFuture, ParentHandle};
use crate::entity::mob::Mob;
use crate::world::World;

/// `Drowned.DrownedGoToBeachGoal` (`Drowned.java:342-379`): a swimming drowned climbs onto
/// nearby dry land.
///
/// Gated the same as `okTarget`/`DrownedAttackGoal` on it not being bright outside (chasing a
/// target onto land only happens once it's safe from sunlight). `MoveToBlockGoal(this,
/// speedModifier, 8, 2)` supplies range 8, `maxYDifference` 2.
pub struct DrownedGoToBeachGoal {
    move_to_target_pos_goal: MoveToTargetPosGoal<Self>,
}

impl DrownedGoToBeachGoal {
    #[must_use]
    pub fn new(speed: f64) -> Box<Self> {
        let mut this = Box::new(Self {
            move_to_target_pos_goal: MoveToTargetPosGoal::new(ParentHandle::none(), speed, 8, 2),
        });
        // SAFETY: `this` heap allocation address is pinned in Box and outlives `ParentHandle` references.
        this.move_to_target_pos_goal.move_to_target_pos = unsafe { ParentHandle::new(&this) };
        this
    }
}

impl Goal for DrownedGoToBeachGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if !self.move_to_target_pos_goal.can_start(mob).await {
                return false;
            }
            let entity = mob.get_entity();
            let world = entity.world.load();
            if is_bright_outside(&world) {
                return false;
            }
            if !entity.touching_water.load(Relaxed) {
                return false;
            }
            entity.pos.load().y >= f64::from(world.sea_level - 3)
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { self.move_to_target_pos_goal.should_continue(mob).await })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move { self.move_to_target_pos_goal.start(mob).await })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move { self.move_to_target_pos_goal.stop(mob).await })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move { self.move_to_target_pos_goal.tick(mob).await })
    }

    fn should_run_every_tick(&self) -> bool {
        self.move_to_target_pos_goal.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.move_to_target_pos_goal.controls()
    }
}

impl MoveToTargetPos for DrownedGoToBeachGoal {
    /// `isValidTarget` checks the two empty blocks above and then
    /// `BlockState.entityCanStandOn` (`Drowned.java:364-367`). The generated upward
    /// side-support flag is the existing collision-face equivalent of
    /// `BlockBehaviour.entityCanStandOn` (`BlockBehaviour.java:705-710`).
    fn is_target_pos<'a>(
        &'a self,
        world: Arc<World>,
        block_pos: BlockPos,
    ) -> std::pin::Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            let above = block_pos.up();
            let above2 = above.up();
            if !world.get_block_state(&above).is_air() || !world.get_block_state(&above2).is_air() {
                return false;
            }
            can_stand_on(world.get_block_state(&block_pos))
        })
    }
}

// `BlockState.entityCanStandOn` checks a full upward collision face
// (`BlockBehaviour.java:705-710`); `is_side_solid(Up)` is the generated equivalent used here.
const fn can_stand_on(state: &pumpkin_data::BlockState) -> bool {
    state.is_side_solid(BlockDirection::Up)
}

#[cfg(test)]
mod tests {
    use super::can_stand_on;
    use pumpkin_data::Block;

    // `Drowned.isValidTarget` delegates standability to the candidate state
    // (`Drowned.java:364-367`).
    #[test]
    fn standability_uses_the_upward_collision_face() {
        assert!(can_stand_on(Block::GLASS.default_state));
        assert!(!can_stand_on(Block::AIR.default_state));
    }
}
