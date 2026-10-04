use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering::Relaxed},
};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage,
    ai::{
        control::guardian_move_control::GuardianMoveControl,
        goal::{
            active_target::ActiveTargetGoal, guardian_attack::GuardianAttackGoal,
            look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
            move_towards_restriction::MoveTowardsRestrictionGoal, wander_around::WanderAroundGoal,
        },
    },
    mob::{Mob, MobEntity},
};

/// Vanilla `Guardian.hurtServer` (Guardian.java:311-324): a guardian that is not currently
/// swimming reflects 2.0 thorns damage back at whatever living entity dealt the blow directly,
/// unless the damage already avoids guardian thorns or is itself thorns. `moving` is the
/// `isMoving()` flag published by `GuardianMoveControl`.
///
/// Scope reduction: `randomStrollGoal.trigger()` (Guardian.java:319-321) is skipped; a mob
/// cannot reach an individual goal instance out of `goals_selector` here, so a hurt guardian
/// does not immediately re-roll its stroll destination.
pub(super) fn guardian_thorns<'a>(
    moving: &AtomicBool,
    damage_type: DamageType,
    source: Option<&'a dyn EntityBase>,
) -> EntityBaseFuture<'a, ()> {
    let moving = moving.load(Relaxed);
    Box::pin(async move {
        if moving
            || damage_type.has_tag(&tag::DamageType::MINECRAFT_AVOIDS_GUARDIAN_THORNS)
            || damage_type == DamageType::THORNS
        {
            return;
        }
        let Some(attacker) = source else {
            return;
        };
        if attacker.get_living_entity().is_none() {
            return;
        }
        attacker.damage(attacker, 2.0, DamageType::THORNS).await;
    })
}

/// Installs `Guardian.GuardianMoveControl` (Guardian.java:65) and returns the shared
/// `isMoving` flag it maintains.
pub(super) fn install_move_control(mob_entity: &MobEntity) -> Arc<AtomicBool> {
    let moving = Arc::new(AtomicBool::new(false));
    *mob_entity
        .move_control
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Box::new(GuardianMoveControl::new(moving.clone()));
    moving
}

/// `Guardian.travelInWater` (Guardian.java:331-339): `moveRelative(0.1, input)`, move, a flat
/// 0.9 drag and a slight sink when idle and untargeted, replacing the generic water friction.
/// Outside water the generic travel path applies.
pub(super) async fn travel_in_water(
    guardian: &dyn Mob,
    moving: &AtomicBool,
    caller: &Arc<dyn EntityBase>,
) -> bool {
    let mob_entity = guardian.get_mob_entity();
    let living = &mob_entity.living_entity;
    let entity = &living.entity;
    if !entity.touching_water.load(Relaxed) {
        return false;
    }
    entity.update_velocity_from_input(living.movement_input.load(), 0.1);
    entity.move_entity(caller, entity.velocity.load()).await;
    let mut velocity = entity.velocity.load() * 0.9;
    if !moving.load(Relaxed) && mob_entity.get_target().await.is_none() {
        velocity.y -= 0.005;
    }
    entity.velocity.store(velocity);
    true
}

pub struct GuardianEntity {
    pub mob_entity: MobEntity,
    /// Vanilla `Guardian.isMoving`, written by `GuardianMoveControl`.
    moving: Arc<AtomicBool>,
}

impl GuardianEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let moving = install_move_control(&mob_entity);
        let guardian = Self { mob_entity, moving };
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
            // Guardian.java:78: `LookAtPlayerGoal(this, Guardian.class, 12.0F, 0.01F)` -- an
            // explicit 0.01 probability, half `LookAtPlayerGoal`'s 0.02 default.
            goal_selector.add_goal(
                8,
                Box::new(LookAtEntityGoal::new(
                    mob_weak,
                    &EntityType::GUARDIAN,
                    12.0,
                    0.01,
                    false,
                )),
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

    /// `Guardian.getMaxHeadXRot` (`Guardian.java:326-329`).
    fn get_max_look_pitch_change(&self) -> f32 {
        180.0
    }

    fn on_damage<'a>(
        &'a self,
        damage_type: DamageType,
        source: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, ()> {
        guardian_thorns(&self.moving, damage_type, source)
    }

    fn custom_travel<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, bool> {
        Box::pin(travel_in_water(self, &self.moving, caller))
    }
}
