use std::sync::{Arc, Weak};

use pumpkin_data::entity::EntityType;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        active_target::ActiveTargetGoal,
        destroy_egg::DestroyEggGoal,
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        revenge::{RevengeGoal, same_type_matcher},
        spear_use::SpearUseGoal,
        wander_around::WanderAroundGoal,
        zombie_attack::ZombieAttackGoal,
    },
    living::LivingEntity,
    mob::{Mob, MobEntity},
    persistent_anger::PersistentAnger,
};

pub struct ZombifiedPiglinEntity {
    pub mob_entity: MobEntity,
    pub persistent_anger: PersistentAnger,
}

impl ZombifiedPiglinEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let piglin = Self {
            mob_entity,
            persistent_anger: PersistentAnger::default(),
        };
        let mob_arc = Arc::new(piglin);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc.mob_entity.goals_selector.lock().unwrap();

            // Vanilla registers no `FloatGoal` anywhere in the Zombie/ZombifiedPiglin
            // hierarchy (`Zombie.java`, `Monster.java`, `PathfinderMob.java`) -- zombies sink
            // and walk rather than float.
            goal_selector.add_goal(1, SpearUseGoal::new(1.0, 1.0, 10.0, 2.0));
            // Vanilla `ZombieAttackGoal(this, 1.0, false)` (`ZombifiedPiglin.java:73`), not
            // `MeleeAttackGoal`: gets the correct `pauseWhenMobIdle`/arm-raise behavior.
            goal_selector.add_goal(2, ZombieAttackGoal::new(1.0, false));
            // Inherited from `Zombie.registerGoals` (`Zombie.java:113`): priority 4, not
            // overridden by `ZombifiedPiglin.addBehaviourGoals`.
            goal_selector.add_goal(4, DestroyEggGoal::new(1.0, 3));
            // Vanilla `WaterAvoidingRandomStrollGoal` at priority 7 (`ZombifiedPiglin.java:74`).
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::new_water_avoiding(1.0)));
            // Inherited from `Zombie.registerGoals` at priority 8 (`Zombie.java:114-115`).
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc.mob_entity.target_selector.lock().unwrap();
            // Vanilla `HurtByTargetGoal(this).setAlertOthers()` (`ZombifiedPiglin.java:75`).
            target_selector.add_goal(
                1,
                Box::new(RevengeGoal::with_alert_others(true, same_type_matcher)),
            );

            // Vanilla priority 2: `NearestAttackableTargetGoal(this, Player.class, 10, true,
            // false, this::isAngryAt)` (`ZombifiedPiglin.java:76`) -- the `NeutralMob` anger
            // predicate only sees the candidate, so it closes over a weak handle back to this
            // piglin's own `PersistentAnger`, following the same pattern as `WolfEntity`.
            let angry_weak = mob_weak.clone();
            target_selector.add_goal(
                2,
                Box::new(ActiveTargetGoal::new(
                    &mob_arc.mob_entity,
                    &EntityType::PLAYER,
                    10,
                    true,
                    false,
                    Some(
                        move |target: Arc<LivingEntity>, _world: Arc<crate::world::World>| {
                            let angry_weak = angry_weak.clone();
                            async move {
                                let Some(mob) = angry_weak.upgrade() else {
                                    return false;
                                };
                                let Some(anger) = mob.persistent_anger() else {
                                    return false;
                                };
                                anger.is_angry_at(target.entity.entity_uuid).await
                            }
                        },
                    ),
                )),
            );

            // Vanilla priority 3: `ResetUniversalAngerTargetGoal(this, true)`
            // (`ZombifiedPiglin.java:77`). Pumpkin has no `UNIVERSAL_ANGER` game rule and
            // `PersistentAnger::tick` already expires the anger timer on its own (see
            // `WolfEntity`'s identical deferral), so only the universal-anger-only
            // re-targeting behavior is missing; deferred until that game rule exists.
        };

        mob_arc
    }
}

impl NBTStorage for ZombifiedPiglinEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async { self.persistent_anger.write_nbt(nbt).await })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async { self.persistent_anger.read_nbt(nbt).await })
    }
}

impl Mob for ZombifiedPiglinEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `NeutralMob::updatePersistentAnger(level, true)`, called from
    /// `ZombifiedPiglin::customServerAiStep` (`ZombifiedPiglin.java:110`): while this piglin
    /// currently has a live target (set by `RevengeGoal`), adopt it as the anger target and
    /// (re)start the timer. Same simplified approach as `WolfEntity::mob_tick`.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.persistent_anger.tick().await;

            let current_target = self.mob_entity.target.lock().await.clone();
            if let Some(target) = current_target {
                let target_uuid = target.get_entity().entity_uuid;
                if !self.persistent_anger.is_angry_at(target_uuid).await {
                    self.persistent_anger.set_angry_at(Some(target_uuid)).await;
                    self.persistent_anger.start_timer();
                }
            }
        })
    }

    fn persistent_anger(&self) -> Option<&PersistentAnger> {
        Some(&self.persistent_anger)
    }
}
