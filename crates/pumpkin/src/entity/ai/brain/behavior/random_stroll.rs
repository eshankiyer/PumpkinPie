//! Port of `behavior/RandomStroll.java`'s brain behavior variants.

use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::behavior::{Behavior, OneShot, OneShotTrigger};
use crate::entity::ai::brain::memory::{
    MemoryKeyId, MemoryStatus, PositionTracker, WalkTarget, WalkTargetMemory,
};
use crate::entity::mob::Mob;

use crate::entity::ai::goal::random_pos::air_and_water_get_pos;

/// `RandomStroll.MAX_XZ_DIST` (`:19`).
const MAX_XZ_DIST: f64 = 10.0;
/// `RandomStroll.MAX_Y_DIST` (`:20`).
const MAX_Y_DIST: f64 = 7.0;

pub struct RandomStrollFly {
    speed_modifier: f32,
}

impl RandomStrollFly {
    /// `RandomStroll.fly(speedModifier)` (`RandomStroll.java:35-37`). Entry condition is
    /// `WALK_TARGET` absent (`:46`), so a stroll never overwrites an in-flight walk target.
    // Returns a boxed trait object, not Self by name -- constructor pattern for this behavior/sensor family.
    #[allow(clippy::new_ret_no_self)]
    #[must_use]
    pub fn new(speed_modifier: f32) -> Box<dyn Behavior> {
        Box::new(OneShot::new(
            Self { speed_modifier },
            vec![(MemoryKeyId::WalkTarget, MemoryStatus::ValueAbsent)],
        ))
    }
}

impl OneShotTrigger for RandomStrollFly {
    fn debug_name(&self) -> &'static str {
        "RandomStroll.fly"
    }

    /// `strollFlyOrSwim`'s trigger (`RandomStroll.java:46-54`), with
    /// `AirAndWaterRandomPos.getPos` supplying the target (`:79-82`).
    fn trigger(&mut self, mob: &dyn Mob, brain: &Brain, _game_time: i64) -> bool {
        let view = mob.get_looking_vector();
        if let Some(target) = air_and_water_get_pos(
            mob,
            MAX_XZ_DIST as i32,
            MAX_Y_DIST as i32,
            -2,
            view.x,
            view.z,
            std::f64::consts::FRAC_PI_2,
        ) {
            brain.set::<WalkTargetMemory>(WalkTarget::new(
                PositionTracker::of_position(target),
                self.speed_modifier,
                0,
            ));
        } else {
            brain.erase::<WalkTargetMemory>();
        }
        true
    }
}
