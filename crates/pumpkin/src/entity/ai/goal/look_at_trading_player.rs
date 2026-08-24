use std::sync::{Arc, Weak};

use super::look_at_entity::LookAtEntityGoal;
use super::{Controls, Goal, GoalFuture};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use pumpkin_data::entity::EntityType;

/// Vanilla `LookAtTradingPlayerGoal`
/// (`net/minecraft/world/entity/ai/goal/LookAtTradingPlayerGoal.java:6-23`), registered by
/// `WanderingTrader.registerGoals` at priority 1 (`WanderingTrader.java:89`).
///
/// It is a `LookAtPlayerGoal(this, Player.class, 8.0F)` subclass (`:9-12`) whose only override
/// is `canUse` (`:15-22`): while a trade session is open (`AbstractVillager.isTrading`) it pins
/// the look target to `getTradingPlayer()` instead of running the superclass's probability roll
/// and nearest-player scan. `canContinueToUse`, `start`, `stop` and `tick` are inherited from
/// `LookAtPlayerGoal` unchanged, so they are delegated to the wrapped [`LookAtEntityGoal`]
/// (this codebase's `LookAtPlayerGoal` port).
pub struct LookAtTradingPlayerGoal {
    inner: Box<LookAtEntityGoal>,
}

impl LookAtTradingPlayerGoal {
    #[must_use]
    pub fn new(mob_weak: Weak<dyn Mob>) -> Self {
        Self {
            // `super(villager, Player.class, 8.0F)` (`LookAtTradingPlayerGoal.java:10`).
            inner: LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
        }
    }
}

impl Goal for LookAtTradingPlayerGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            // `canUse` (`LookAtTradingPlayerGoal.java:15-22`):
            // `if (this.villager.isTrading()) { this.lookAt = this.villager.getTradingPlayer(); return true; }`.
            // `Mob::get_trading_player` resolves to `Some` exactly while the merchant session
            // tracked by `trading_player` is active (see `WanderingTraderEntity` /
            // `VillagerEntity` overrides), so a single lookup covers both vanilla calls.
            if let Some(player) = mob.get_trading_player() {
                self.inner.set_look_target(player as Arc<dyn EntityBase>);
                return true;
            }
            false
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

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
