use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Weak};

use pumpkin_data::entity::EntityType;

use crate::entity::ai::control::drowned_move_control::DrownedMoveControl;
use crate::entity::ai::goal::drowned_attack::DrownedAttackGoal;
use crate::entity::ai::goal::drowned_go_to_beach::DrownedGoToBeachGoal;
use crate::entity::ai::goal::drowned_go_to_water::DrownedGoToWaterGoal;
use crate::entity::ai::goal::drowned_swim_up::DrownedSwimUpGoal;
use crate::entity::ai::goal::drowned_util::is_bright_outside;
use crate::entity::ai::goal::ranged_trident_attack::DrownedTridentAttackGoal;
use crate::entity::living::LivingEntity;
use crate::entity::mob::zombie::ZombieEntityBase;
use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage,
    ai::goal::{
        active_target::ActiveTargetGoal, destroy_egg::DestroyEggGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal, revenge::RevengeGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
};
use crate::world::World;

pub struct DrownedEntity {
    entity: Arc<ZombieEntityBase>,
    searching_for_land: AtomicBool,
    target_in_water: AtomicBool,
    target_is_above: AtomicBool,
}

/// `Drowned#okTarget` (`Drowned.java:223-225`): a potential player target is only valid while
/// it's not bright outside, or while the target itself is in water.
async fn ok_target(target: Arc<LivingEntity>, world: Arc<World>) -> bool {
    !is_bright_outside(&world) || target.entity.touching_water.load(Relaxed)
}

impl DrownedEntity {
    #[allow(clippy::too_many_lines)]
    pub fn new(entity: Entity) -> Arc<Self> {
        // Deliberately not `ZombieEntityBase::new(entity)`: that constructor registers the
        // plain-Zombie goal set (SpearUseGoal, ZombieAttackGoal, the generic ActiveTargetGoal
        // quartet, etc.) into the same shared goal/target selectors this struct holds, and
        // `Drowned#addBehaviourGoals` (`Drowned.java:91-104`) replaces essentially all of that
        // with water-aware goals below. Building `MobEntity` directly and registering only
        // `Drowned`'s own goal set avoids double-registering (this is the fix the file's old
        // "Fix duplicated" TODO was pointing at).
        let mob_entity = MobEntity::new(entity);
        let base = Arc::new(ZombieEntityBase { mob_entity });
        let drowned = Self {
            entity: base,
            searching_for_land: AtomicBool::new(false),
            target_in_water: AtomicBool::new(false),
            target_is_above: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(drowned);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        // `Drowned(EntityType, Level)` (`Drowned.java:75-79`):
        // `this.setPathfindingMalus(PathType.WATER, 0.0F)`. `MobData::new_zombie` (this
        // codebase's `WalkNodeEvaluator` cost table) otherwise costs water at 8.0, the same as
        // any other zombie, so without this override a Drowned would treat wading into water
        // as expensive terrain instead of free movement.
        mob_arc
            .entity
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_pathfinding_malus(crate::entity::ai::pathfinder::node::PathType::Water, 0.0);
        mob_arc
            .entity
            .mob_entity
            .navigator
            .lock()
            .unwrap()
            .set_amphibious(true);
        mob_arc
            .entity
            .mob_entity
            .navigator
            .lock()
            .unwrap()
            .set_pathfinding_malus(crate::entity::ai::pathfinder::node::PathType::Walkable, 6.0);
        mob_arc
            .entity
            .mob_entity
            .navigator
            .lock()
            .unwrap()
            .set_pathfinding_malus(
                crate::entity::ai::pathfinder::node::PathType::WaterBorder,
                4.0,
            );
        *mob_arc.entity.mob_entity.move_control.lock().unwrap() =
            Box::new(DrownedMoveControl::default());

        {
            let mut goal_selector = mob_arc
                .entity
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut target_selector = mob_arc
                .entity
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Vanilla `Zombie#registerGoals` (not overridden by `Drowned`) still contributes
            // these three goals on top of `Drowned#addBehaviourGoals` below. Note the base
            // `Zombie`/`Husk`/`ZombieVillager` goal set in `ZombieEntityBase` includes a
            // `SwimGoal` at priority 0 that has no vanilla counterpart here -- neither
            // `Zombie#registerGoals` nor `Drowned#addBehaviourGoals` ever add a `FloatGoal`,
            // matching real zombies sinking (and eventually converting) in deep water instead
            // of floating. `Drowned` gets its own buoyancy from `DrownedSwimUpGoal` below.
            goal_selector.add_goal(4, DestroyEggGoal::new(1.0, 3));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            // `Drowned#addBehaviourGoals` (`Drowned.java:90-104`).
            goal_selector.add_goal(1, DrownedGoToWaterGoal::new(1.0));
            goal_selector.add_goal(2, DrownedTridentAttackGoal::new(40, 10.0));
            goal_selector.add_goal(2, DrownedAttackGoal::new(1.0, false));
            goal_selector.add_goal(5, DrownedGoToBeachGoal::new(1.0));
            goal_selector.add_goal(6, DrownedSwimUpGoal::new(1.0));
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::new(1.0)));

            // `HurtByTargetGoal(this, Drowned.class).setAlertOthers(ZombifiedPiglin.class)`:
            // `RevengeGoal` here has no equivalent of vanilla's "ignore damage from own type" /
            // "also alert this other type" parameters, so it only alerts other `Drowned`
            // (matching `ZombieEntityBase`'s existing same-type-only alert simplification).
            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            target_selector.add_goal(
                2,
                Box::new(ActiveTargetGoal::new(
                    &mob_arc.entity.mob_entity,
                    &EntityType::PLAYER,
                    10,
                    true,
                    false,
                    Some(ok_target),
                )),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(
                    &mob_arc.entity.mob_entity,
                    &EntityType::VILLAGER,
                    false,
                ),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(
                    &mob_arc.entity.mob_entity,
                    &EntityType::IRON_GOLEM,
                    true,
                ),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(
                    &mob_arc.entity.mob_entity,
                    &EntityType::AXOLOTL,
                    true,
                ),
            );
            // Vanilla restricts this to `Turtle.BABY_ON_LAND_SELECTOR` (baby turtles on land
            // only); no such predicate exists here, matching `ZombieEntityBase`'s existing
            // simplification of targeting any turtle.
            target_selector.add_goal(
                5,
                ActiveTargetGoal::with_default(
                    &mob_arc.entity.mob_entity,
                    &EntityType::TURTLE,
                    true,
                ),
            );
        };

        mob_arc
    }
}

impl NBTStorage for DrownedEntity {}

impl Mob for DrownedEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn wants_to_swim(&self) -> bool {
        self.searching_for_land.load(Relaxed) || self.target_in_water.load(Relaxed)
    }

    fn is_searching_for_land(&self) -> bool {
        self.searching_for_land.load(Relaxed)
    }

    fn target_is_above(&self) -> bool {
        self.target_is_above.load(Relaxed)
    }

    fn mob_is_pushed_by_fluids(&self) -> bool {
        !self
            .entity
            .mob_entity
            .living_entity
            .entity
            .swimming
            .load(Relaxed)
    }

    fn set_searching_for_land(&self, searching: bool) {
        self.searching_for_land.store(searching, Relaxed);
    }

    fn mob_tick<'a>(
        &'a self,
        _caller: &'a Arc<dyn crate::entity::EntityBase>,
    ) -> crate::entity::EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let target = self.entity.mob_entity.get_target().await;
            let living = &self.entity.mob_entity.living_entity;
            let entity = &living.entity;
            let pos = entity.pos.load();
            self.target_in_water.store(
                target
                    .as_ref()
                    .is_some_and(|target| target.get_entity().touching_water.load(Relaxed)),
                Relaxed,
            );
            self.target_is_above.store(
                target
                    .as_ref()
                    .is_some_and(|target| target.get_entity().pos.load().y > pos.y),
                Relaxed,
            );
        })
    }

    fn update_swimming(&self) -> crate::entity::EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = &self.entity.mob_entity.living_entity.entity;
            let target = self.entity.mob_entity.get_target().await;
            let position = entity.pos.load();
            self.target_in_water.store(
                target
                    .as_ref()
                    .is_some_and(|target| target.get_entity().touching_water.load(Relaxed)),
                Relaxed,
            );
            self.target_is_above.store(
                target
                    .as_ref()
                    .is_some_and(|target| target.get_entity().pos.load().y > position.y),
                Relaxed,
            );

            let underwater =
                entity.touching_water.load(Relaxed) && entity.was_eye_in_water.load(Relaxed);
            entity
                .set_swimming(!entity.no_ai.load(Relaxed) && underwater && self.wants_to_swim())
                .await;
        })
    }

    fn custom_travel<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let living = &self.entity.mob_entity.living_entity;
            let entity = &living.entity;
            if !self.wants_to_swim()
                || !entity.touching_water.load(Relaxed)
                || !entity.was_eye_in_water.load(Relaxed)
            {
                return false;
            }

            // `Drowned.travelInWater` (`Drowned.java:243-251`) uses a fixed 0.01
            // movement speed and 0.9 drag while underwater and swimming. It does not
            // run the generic gravity/0.8-water-drag path.
            entity.update_velocity_from_input(living.movement_input.load(), 0.01);
            let velocity = entity.velocity.load();
            entity.move_entity(caller, velocity).await;
            entity.velocity.store(entity.velocity.load() * 0.9);
            true
        })
    }
}
