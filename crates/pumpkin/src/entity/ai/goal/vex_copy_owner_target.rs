use std::sync::Weak;

use crate::entity::EntityBase;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::mob::Mob;
use crate::entity::mob::vex::VexEntity;

/// Vanilla: `Vex.VexCopyOwnerTargetGoal`.
///
/// Adopts the owner's current target as this vex's own, approximating vanilla's
/// `TargetingConditions.forNonCombat().ignoreLineOfSight().ignoreInvisibilityTesting()`
/// predicate as "owner has a live target".
pub struct VexCopyOwnerTargetGoal {
    vex: Weak<VexEntity>,
}

impl VexCopyOwnerTargetGoal {
    #[must_use]
    pub const fn new(vex: Weak<VexEntity>) -> Self {
        Self { vex }
    }

    fn owner_target(vex: &VexEntity) -> Option<std::sync::Arc<dyn EntityBase>> {
        let owner_id = vex.owner_id()?;
        let world = vex.mob_entity.living_entity.entity.world.load();
        let owner = world.get_entity_by_id(owner_id)?;
        let owner_mob = owner.get_mob()?;
        let target = owner_mob
            .get_mob_entity()
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        target.get_entity().is_alive().then_some(target)
    }
}

impl Goal for VexCopyOwnerTargetGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let Some(vex) = self.vex.upgrade() else {
            return false;
        };
        Self::owner_target(&vex).is_some()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(vex) = self.vex.upgrade() else {
            return;
        };
        let target = Self::owner_target(&vex);
        mob.set_mob_target(target);
    }

    fn controls(&self) -> Controls {
        Controls::TARGET
    }
}
