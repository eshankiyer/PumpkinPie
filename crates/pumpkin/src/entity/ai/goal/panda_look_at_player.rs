use std::sync::{Arc, Weak};

use super::look_at_entity::LookAtEntityGoal;
use super::{Controls, Goal};
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;
use crate::entity::{EntityBase, player::Player};
use pumpkin_data::entity::EntityType;

/// `Panda.PandaLookAtPlayerGoal` (`Panda.java:910-970`).
///
/// The generic look-at-player goal with two changes. It is gated on `Panda.canPerformAction`, and
/// `PandaBreedGoal` can force a specific look target onto it (the nearest player, when the panda
/// can't find bamboo to breed over). Forced targets still require the probability roll and
/// use the inherited range check and finite look duration.
pub struct PandaLookAtPlayerGoal {
    inner: Box<LookAtEntityGoal>,
}

impl PandaLookAtPlayerGoal {
    #[must_use]
    pub fn new(mob_weak: Weak<dyn Mob>, range: f32) -> Box<Self> {
        Box::new(Self {
            inner: LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, range),
        })
    }

    /// Consumes any pending `setTarget` request from `PandaBreedGoal`.
    fn take_forced_target(panda: &PandaEntity) -> Option<Arc<dyn EntityBase>> {
        let uuid = panda.take_forced_look_target()?;
        let world = panda.get_mob_entity().living_entity.entity.world.load();
        world
            .players
            .load()
            .iter()
            .find(|p: &&Arc<Player>| p.gameprofile.id == uuid)
            .map(|p| p.clone() as Arc<dyn EntityBase>)
    }
}

impl Goal for PandaLookAtPlayerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
            return false;
        };
        if let Some(target) = Self::take_forced_target(panda) {
            self.inner.set_look_target(target);
        }

        // Vanilla rolls before checking canPerformAction and retains lookAt when
        // either gate rejects a start, so a breeding request survives a failed roll.
        self.inner.can_start_preserving_target(mob) && panda.can_perform_action()
    }

    fn should_continue(&self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob)
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>()
            && let Some(target) = Self::take_forced_target(panda)
        {
            self.inner.set_look_target(target);
        }
        self.inner.tick(mob);
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
