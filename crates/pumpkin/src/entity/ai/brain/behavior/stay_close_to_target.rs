//! Port of `behavior/StayCloseToTarget.java` (declarative, so a `OneShot` here).

use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::behavior::{Behavior, OneShot, OneShotTrigger};
use crate::entity::ai::brain::memory::{
    LookTargetMemory, MemoryKeyId, MemoryStatus, PositionTracker, WalkTarget, WalkTargetMemory,
};
use crate::entity::mob::Mob;

/// Vanilla's `Function<LivingEntity, Optional<PositionTracker>> targetPositionGetter`
/// (`StayCloseToTarget.java:13`). A plain `fn` pointer rather than a boxed closure: every
/// caller in vanilla passes a static method reference.
pub type TargetPositionGetter = fn(&dyn Mob, &Brain) -> Option<PositionTracker>;

/// Vanilla's `Predicate<LivingEntity> shouldRunPredicate` (`StayCloseToTarget.java:14`).
pub type ShouldRunPredicate = fn(&dyn Mob, &Brain) -> bool;

pub struct StayCloseToTarget {
    target_position_getter: TargetPositionGetter,
    should_run: ShouldRunPredicate,
    close_enough: i32,
    too_far: f64,
    speed_modifier: f32,
}

impl StayCloseToTarget {
    /// `StayCloseToTarget.create(targetPositionGetter, shouldRunPredicate, closeEnough,
    /// tooFar, speedModifier)` (`StayCloseToTarget.java:12-38`).
    // Returns a boxed trait object, not Self by name -- constructor pattern for this behavior/sensor family.
    #[allow(clippy::new_ret_no_self)]
    #[must_use]
    pub fn new(
        target_position_getter: TargetPositionGetter,
        should_run: ShouldRunPredicate,
        close_enough: i32,
        too_far: i32,
        speed_modifier: f32,
    ) -> Box<dyn Behavior> {
        Box::new(OneShot::new(
            Self {
                target_position_getter,
                should_run,
                close_enough,
                too_far: f64::from(too_far),
                speed_modifier,
            },
            vec![
                (MemoryKeyId::LookTarget, MemoryStatus::Registered),
                (MemoryKeyId::WalkTarget, MemoryStatus::Registered),
            ],
        ))
    }
}

impl OneShotTrigger for StayCloseToTarget {
    fn debug_name(&self) -> &'static str {
        "StayCloseToTarget"
    }

    /// `StayCloseToTarget.java:21-35`: bail when there is no target or the predicate says no,
    /// bail again when already within `tooFar` (this behavior only fires to close a gap), and
    /// otherwise write both intent memories.
    fn trigger(&mut self, mob: &dyn Mob, brain: &Brain, _game_time: i64) -> bool {
        let Some(target) = (self.target_position_getter)(mob, brain) else {
            return false;
        };
        if !(self.should_run)(mob, brain) {
            return false;
        }
        let Some(target_pos) = target.current_position() else {
            return false;
        };

        let mob_pos = mob.get_mob_entity().living_entity.entity.pos.load();
        // `Vec3.closerThan(Vec3, double)` is an inclusive squared-distance compare.
        if mob_pos.squared_distance_to_vec(&target_pos) < self.too_far * self.too_far {
            return false;
        }

        brain.set::<LookTargetMemory>(target.clone());
        brain.set::<WalkTargetMemory>(WalkTarget::new(
            target,
            self.speed_modifier,
            self.close_enough,
        ));
        true
    }
}
