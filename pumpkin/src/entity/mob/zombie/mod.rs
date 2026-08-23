use super::{Mob, MobEntity};
use crate::entity::ai::goal::destroy_egg::DestroyEggGoal;
use crate::entity::ai::goal::look_around::RandomLookAroundGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::spear_use::SpearUseGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::{
    Entity, NBTStorage, NbtFuture,
    ai::goal::{active_target::ActiveTargetGoal, look_at_entity::LookAtEntityGoal},
};
use pumpkin_data::entity::EntityType;
use pumpkin_nbt::compound::NbtCompound;
use std::sync::{Arc, Weak};

pub mod drowned;
pub mod husk;
#[allow(clippy::module_inception)]
pub mod zombie;
pub mod zombie_villager;

/// Vanilla `Zombie.java:124`: `HurtByTargetGoal(this).setAlertOthers(ZombifiedPiglin.class)`.
/// `getEntitiesOfClass(this.mob.getClass(), ...)` matches `this` plus every subclass, so a
/// plain `Zombie` alerts `Zombie`/`Husk`/`Drowned`/`ZombieVillager` (but not `ZombifiedPiglin`, the
/// explicit exception). Pumpkin's `ZombieEntityBase` is shared verbatim by all four types, so
/// this whitelist approximates that broadcast for all of them alike; a strict port would only
/// have a plain `Zombie` alert the whole family while `Husk`/`Drowned`/`ZombieVillager` (which have no
/// subclasses of their own) alert just their own exact type.
fn zombie_family_alert(_mob_type: &'static EntityType, other_type: &'static EntityType) -> bool {
    other_type == &EntityType::ZOMBIE
        || other_type == &EntityType::HUSK
        || other_type == &EntityType::DROWNED
        || other_type == &EntityType::ZOMBIE_VILLAGER
}

pub struct ZombieEntityBase {
    pub mob_entity: MobEntity,
}

impl ZombieEntityBase {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let zombie = Self { mob_entity };
        let mob_arc = Arc::new(zombie);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc.mob_entity.goals_selector.lock().unwrap();
            let mut target_selector = mob_arc.mob_entity.target_selector.lock().unwrap();

            // Vanilla `Zombie`/`ZombieVillager`/`ZombifiedPiglin` register no `FloatGoal` at
            // all (confirmed absent from `Zombie.java`, `Monster.java`, `PathfinderMob.java`
            // in the decompiled source) -- zombies sink and walk along the bottom rather than
            // float, unlike most other mobs.
            goal_selector.add_goal(2, SpearUseGoal::new(1.0, 1.0, 10.0, 2.0));
            goal_selector.add_goal(3, ZombieAttackGoal::new(1.0, false));
            goal_selector.add_goal(4, DestroyEggGoal::new(1.0, 3));
            // Vanilla `WaterAvoidingRandomStrollGoal` (`Zombie.java:123`).
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::new_water_avoiding(1.0)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(
                1,
                Box::new(RevengeGoal::with_alert_others(true, zombie_family_alert)),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
            // Vanilla `NearestAttackableTargetGoal(this, AbstractVillager.class, false)`
            // (`Zombie.java:126`): mustSee is `false`, unlike the Player/IronGolem/Turtle goals.
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::VILLAGER, false),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, true),
            );
            target_selector.add_goal(
                5,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::TURTLE, true),
            );
        };

        mob_arc
    }
}

impl NBTStorage for ZombieEntityBase {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        self.mob_entity.living_entity.write_nbt(nbt)
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        self.mob_entity.living_entity.read_nbt_non_mut(nbt)
    }
}

impl Mob for ZombieEntityBase {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
}

#[cfg(test)]
mod tests {
    use super::zombie_family_alert;
    use pumpkin_data::entity::EntityType;

    #[test]
    fn alerts_the_zombie_family() {
        for other in [
            &EntityType::ZOMBIE,
            &EntityType::HUSK,
            &EntityType::DROWNED,
            &EntityType::ZOMBIE_VILLAGER,
        ] {
            assert!(zombie_family_alert(&EntityType::ZOMBIE, other));
        }
    }

    #[test]
    fn does_not_alert_zombified_piglin() {
        // Vanilla `Zombie.java:124`: `setAlertOthers(ZombifiedPiglin.class)` explicitly
        // excludes it from the family broadcast.
        assert!(!zombie_family_alert(
            &EntityType::ZOMBIE,
            &EntityType::ZOMBIFIED_PIGLIN
        ));
    }

    #[test]
    fn does_not_alert_unrelated_mobs() {
        assert!(!zombie_family_alert(
            &EntityType::ZOMBIE,
            &EntityType::SKELETON
        ));
    }
}
