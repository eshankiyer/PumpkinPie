//! Port of `CatSitOnBlockGoal.java`.

use std::pin::Pin;
use std::sync::{Arc, Weak};

use pumpkin_data::Block;
use pumpkin_data::block_properties::{
    BedPart, BlockProperties, FurnaceLikeProperties, WhiteBedLikeProperties,
};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;

use super::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use super::{Controls, Goal, GoalFuture, ParentHandle};
use crate::entity::mob::Mob;
use crate::entity::passive::cat::CatEntity;
use crate::entity::passive::tamable::TamableAnimal;
use crate::world::World;

/// `CatSitOnBlockGoal.java:18`: `super(cat, speedModifier, 8)`.
const SEARCH_RANGE: i32 = 8;

/// Vanilla `CatSitOnBlockGoal` (`CatSitOnBlockGoal.java:14-60`): a tamed cat that has not been
/// ordered to sit walks to a nearby chest, lit furnace or bed foot and sits on top of it.
///
/// Divergence: vanilla's chest branch (`CatSitOnBlockGoal.java:51-52`) additionally requires
/// `ChestBlockEntity.getOpenCount(level, pos) < 1`, so a cat will not claim a chest somebody is
/// looking into. `ChestBlockEntity::viewers` is private to `block::entities::chest` here and no
/// world-level accessor exposes it, so a chest is accepted regardless of its open count. The
/// only observable difference is a cat sitting on an open chest.
pub struct CatSitOnBlockGoal {
    cat: Weak<CatEntity>,
    move_to_target_pos_goal: MoveToTargetPosGoal<Self>,
}

impl CatSitOnBlockGoal {
    #[must_use]
    pub fn new(cat: Weak<CatEntity>, speed: f64) -> Box<Self> {
        let mut this = Box::new(Self {
            cat,
            move_to_target_pos_goal: MoveToTargetPosGoal::with_default(
                ParentHandle::none(),
                speed,
                SEARCH_RANGE,
            ),
        });

        // SAFETY: `this` heap allocation address is pinned in Box and outlives `ParentHandle` references.
        this.move_to_target_pos_goal.move_to_target_pos = unsafe { ParentHandle::new(&this) };

        this
    }

    /// `CatSitOnBlockGoal.canUse` (line 24): `this.cat.isTame() && !this.cat.isOrderedToSit()`.
    fn cat_may_sit(&self) -> bool {
        self.cat
            .upgrade()
            .is_some_and(|cat| cat.is_tame() && !cat.mob_entity.is_ordered_to_sit())
    }

    fn is_valid(world: &Arc<World>, block_pos: BlockPos) -> bool {
        // `isValidTarget` line 47: the block above must be empty.
        if !world.get_block_state(&block_pos.up()).is_air() {
            return false;
        }
        let (block, state_id) = world.get_block_and_state_id(&block_pos);
        if block.id == Block::CHEST.id {
            // See the type-level note: vanilla also checks the chest's open count.
            return true;
        }
        if block.id == Block::FURNACE.id {
            return FurnaceLikeProperties::from_state_id(state_id, block).lit;
        }
        // `isValidTarget` line 56: any bed whose `PART` is not `HEAD`.
        block.has_tag(&tag::Block::MINECRAFT_BEDS)
            && WhiteBedLikeProperties::from_state_id(state_id, block).part != BedPart::Head
    }
}

impl MoveToTargetPos for CatSitOnBlockGoal {
    fn is_target_pos<'a>(
        &'a self,
        world: Arc<World>,
        block_pos: BlockPos,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { Self::is_valid(&world, block_pos) })
    }
}

impl Goal for CatSitOnBlockGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            // Vanilla short-circuits before `super.canUse()`, so the goal's start cooldown does
            // not tick down while the cat is untamed or ordered to sit.
            self.cat_may_sit() && self.move_to_target_pos_goal.can_start(mob).await
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { self.move_to_target_pos_goal.should_continue(mob).await })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.move_to_target_pos_goal.start(mob).await;
            // `start` line 30: `this.cat.setInSittingPose(false)`.
            if let Some(cat) = self.cat.upgrade() {
                cat.set_sitting(false);
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.move_to_target_pos_goal.stop(mob).await;
            // `stop` line 36: `this.cat.setInSittingPose(false)`.
            if let Some(cat) = self.cat.upgrade() {
                cat.set_sitting(false);
            }
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.move_to_target_pos_goal.tick(mob).await;
            // `tick` line 42: `this.cat.setInSittingPose(this.isReachedTarget())`. Vanilla's
            // `SynchedEntityData` swallows no-op writes; `set_sitting` here always sends a
            // metadata packet, so the write is guarded on an actual change.
            if let Some(cat) = self.cat.upgrade() {
                let reached = self.move_to_target_pos_goal.reached;
                if cat.is_sitting() != reached {
                    cat.set_sitting(reached);
                }
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
