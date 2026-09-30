use std::sync::Weak;

use super::{Controls, Goal, GoalFuture, escape_danger::EscapeDangerGoal};
use crate::entity::mob::Mob;
use crate::entity::passive::camel::CamelEntity;
use rand::RngExt;
use std::sync::atomic::Ordering::Relaxed;

/// Vanilla: `CamelAi.RandomSitting.minimalPoseTicks` (`minimalPoseTimeSec = 20`, built at
/// `CamelAi.java:86`). The camel must have held its current pose for at least this long before
/// it is willing to change pose again.
const MIN_POSE_TICKS: i64 = 20 * 20;
/// Chance per eligible tick of changing pose, once `MIN_POSE_TICKS` has elapsed. Vanilla instead
/// picks `CamelAi.RandomSitting` as one weighted option out of a `RunOne` behavior (which only
/// runs while there is no walk target); that arbitration is not ported, so this constant is a
/// from-scratch approximation, not a vanilla value.
const POSE_CHANGE_CHANCE_PER_TICK: f32 = 1.0 / 600.0;

/// Makes an idle camel occasionally sit down for a while, then stand back up.
///
/// This ports the start condition and action of vanilla's `CamelAi.RandomSitting`
/// (`CamelAi.java:114-138`); the pose itself lives on [`CamelEntity`]. Its one simplification is
/// the flat per-tick chance in place of the `RunOne` weighted choice (see
/// `POSE_CHANGE_CHANCE_PER_TICK`). Like the behavior, the goal ends at once: it is the pose, not
/// the goal, that keeps a sitting camel from moving.
pub struct CamelSitGoal {
    camel: Weak<CamelEntity>,
}

impl CamelSitGoal {
    #[must_use]
    pub const fn new(camel: Weak<CamelEntity>) -> Self {
        Self { camel }
    }
}

impl Goal for CamelSitGoal {
    /// `RandomSitting.checkExtraStartConditions` (`CamelAi.java:122-129`).
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(camel) = self.camel.upgrade() else {
                return false;
            };
            let entity = mob.get_entity();
            if entity.touching_water.load(Relaxed)
                || !entity.on_ground.load(Relaxed)
                || entity.leashed_to.lock().await.is_some()
                || mob.has_controlling_passenger().await
                || camel.get_pose_time().await < MIN_POSE_TICKS
                || !camel.can_camel_change_pose()
            {
                return false;
            }

            mob.get_random().random::<f32>() < POSE_CHANGE_CHANCE_PER_TICK
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }

    /// `RandomSitting.start` (`CamelAi.java:131-137`).
    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(camel) = self.camel.upgrade() else {
                return;
            };
            if camel.is_camel_sitting() {
                camel.stand_up().await;
            } else if !mob.is_panicking() {
                camel.sit_down().await;
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

/// `BehaviorBuilder.triggerIf(Predicate.not(Camel::refuseToMove), ...)` around the camel's
/// stroll behavior (`CamelAi.java:84`): the wrapped goal cannot start while the camel is sitting
/// or changing pose.
pub struct RefuseToMoveGate {
    camel: Weak<CamelEntity>,
    inner: Box<dyn Goal>,
}

impl RefuseToMoveGate {
    #[must_use]
    pub fn new(camel: Weak<CamelEntity>, inner: Box<dyn Goal>) -> Box<Self> {
        Box::new(Self { camel, inner })
    }
}

impl Goal for RefuseToMoveGate {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(camel) = self.camel.upgrade() else {
                return false;
            };
            if camel.refuse_to_move().await {
                return false;
            }
            self.inner.can_start(mob).await
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.should_continue(mob)
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.start(mob)
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.stop(mob)
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.tick(mob)
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    fn can_stop(&self) -> bool {
        self.inner.can_stop()
    }

    fn is_panic_goal(&self) -> bool {
        self.inner.is_panic_goal()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}

/// `CamelAi.CamelPanic` (`CamelAi.java:99-112`).
///
/// `AnimalPanic` that stands the camel up instantly before it runs. Its extra `!isMobControlled()` start condition is always true for a
/// camel (`AbstractHorse.isMobControlled` is `false`; only `CamelHusk` overrides it).
pub struct CamelPanicGoal {
    camel: Weak<CamelEntity>,
    inner: EscapeDangerGoal,
}

impl CamelPanicGoal {
    #[must_use]
    pub fn new(camel: Weak<CamelEntity>, speed: f64) -> Box<Self> {
        Box::new(Self {
            camel,
            inner: *EscapeDangerGoal::new(speed),
        })
    }
}

impl Goal for CamelPanicGoal {
    fn is_panic_goal(&self) -> bool {
        true
    }

    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.can_start(mob)
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.should_continue(mob)
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(camel) = self.camel.upgrade() {
                camel.stand_up_instantly().await;
            }
            self.inner.start(mob).await;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.stop(mob)
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.tick(mob)
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
