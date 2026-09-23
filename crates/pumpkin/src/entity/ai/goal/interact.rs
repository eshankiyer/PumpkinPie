use pumpkin_data::entity::EntityType;

use super::{Controls, Goal};
use crate::entity::ai::goal::look_at_entity::LookAtEntityGoal;
use crate::entity::mob::Mob;
use std::sync::Weak;

/// Vanilla `InteractGoal` is a `LookAtPlayerGoal` with both LOOK and MOVE flags
/// (`InteractGoal.java:7-16`).
///
/// The inherited behavior only looks at the selected living entity; claiming MOVE prevents a
/// lower-priority movement goal from running concurrently.
pub struct InteractGoal {
    inner: LookAtEntityGoal,
}

impl InteractGoal {
    /// `InteractGoal(Mob, Class<? extends LivingEntity>, float, float)` delegates to the
    /// corresponding `LookAtPlayerGoal` constructor (`InteractGoal.java:13-16`,
    /// `LookAtPlayerGoal.java:29-35`).
    #[must_use]
    pub fn new(
        mob: Weak<dyn Mob>,
        target_type: &'static EntityType,
        look_distance: f32,
        probability: f32,
        only_horizontal: bool,
    ) -> Box<Self> {
        Box::new(Self {
            inner: LookAtEntityGoal::new(
                mob,
                target_type,
                look_distance,
                probability,
                only_horizontal,
            ),
        })
    }
}

impl Goal for InteractGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.inner.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob)
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob)
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    /// Vanilla `InteractGoal` sets `Goal.Flag.LOOK` and `Goal.Flag.MOVE`
    /// (`InteractGoal.java:10,15`).
    fn controls(&self) -> Controls {
        Controls::LOOK | Controls::MOVE
    }
}
