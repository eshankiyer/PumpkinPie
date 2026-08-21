use std::sync::{Arc, Weak};

use pumpkin_data::entity::EntityType;

use crate::entity::{
    Entity, NBTStorage,
    ai::goal::{
        active_target::ActiveTargetGoal, guardian_attack::GuardianAttackGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        move_towards_restriction::MoveTowardsRestrictionGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
};

pub struct GuardianEntity {
    pub mob_entity: MobEntity,
}

impl GuardianEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let guardian = Self { mob_entity };
        let mob_arc = Arc::new(guardian);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let target_weak = mob_weak.clone();

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Priorities follow Guardian#registerGoals; the attack goal must outrank the
            // wander/look goals it shares MOVE and LOOK controls with. No float/swim goal:
            // vanilla `Guardian.registerGoals` doesn't register one.
            goal_selector.add_goal(4, Box::new(GuardianAttackGoal::new()));
            goal_selector.add_goal(5, MoveTowardsRestrictionGoal::new(1.0));
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::new_with_interval(1.0, 80)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::GUARDIAN, 12.0),
            );
            goal_selector.add_goal(9, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(
                1,
                Box::new(ActiveTargetGoal::new_types(
                    &mob_arc.mob_entity,
                    &[
                        &EntityType::PLAYER,
                        &EntityType::SQUID,
                        &EntityType::GLOW_SQUID,
                        &EntityType::AXOLOTL,
                    ],
                    10,
                    true,
                    false,
                    Some(
                        move |target: crate::entity::ai::target_predicate::TargetData,
                              _world: Arc<crate::world::World>| {
                            let target_weak = target_weak.clone();
                            async move {
                                let Some(guardian) = target_weak.upgrade() else {
                                    return false;
                                };
                                // `Guardian.GuardianAttackSelector.test` (Guardian.java:433).
                                guardian
                                    .get_entity()
                                    .pos
                                    .load()
                                    .squared_distance_to_vec(&target.target_pos)
                                    > 9.0
                            }
                        },
                    ),
                )),
            );
        };

        mob_arc
    }
}

impl NBTStorage for GuardianEntity {}

impl Mob for GuardianEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
}
