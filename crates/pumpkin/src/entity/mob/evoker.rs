use std::sync::{Arc, Weak};

use pumpkin_data::entity::EntityType;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        active_target::ActiveTargetGoal,
        avoid_entity::AvoidEntityGoal,
        evoker_spell::{
            EvokerAttackSpellGoal, EvokerCastingSpellGoal, EvokerSummonSpellGoal,
            EvokerWololoSpellGoal,
        },
        look_at_entity::LookAtEntityGoal,
        pathfind_to_raid::PathfindToRaidGoal,
        revenge::RevengeGoal,
        spellcaster::SpellcasterState,
        swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
};

pub struct EvokerEntity {
    pub mob_entity: MobEntity,
    /// Vanilla: `SpellcasterIllager.spellCastingTickCount` / `currentSpell`.
    pub spellcaster: SpellcasterState,
    /// Vanilla: `Evoker.wololoTarget`.
    pub wololo_target: tokio::sync::Mutex<Option<Arc<dyn EntityBase>>>,
}

impl EvokerEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let evoker = Self {
            mob_entity,
            spellcaster: SpellcasterState::new(),
            wololo_target: tokio::sync::Mutex::new(None),
        };
        let mob_arc = Arc::new(evoker);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let evoker_weak = Arc::downgrade(&mob_arc);

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                1,
                Box::new(EvokerCastingSpellGoal::new(evoker_weak.clone())),
            );
            goal_selector.add_goal(
                2,
                Box::new(AvoidEntityGoal::new(&EntityType::PLAYER, 8.0, 0.6, 1.0)),
            );
            goal_selector.add_goal(
                3,
                Box::new(AvoidEntityGoal::new(&EntityType::CREAKING, 8.0, 0.6, 1.0)),
            );
            // Raider.java:65, via `super.registerGoals()`: `PathfindToRaidGoal<>(this)`.
            goal_selector.add_goal(3, PathfindToRaidGoal::new());
            goal_selector.add_goal(4, Box::new(EvokerSummonSpellGoal::new(evoker_weak.clone())));
            goal_selector.add_goal(5, Box::new(EvokerAttackSpellGoal::new(evoker_weak.clone())));
            goal_selector.add_goal(6, Box::new(EvokerWololoSpellGoal::new(evoker_weak)));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.6)));
            goal_selector.add_goal(
                9,
                Box::new(LookAtEntityGoal::new(
                    mob_weak.clone(),
                    &EntityType::PLAYER,
                    3.0,
                    1.0,
                    false,
                )),
            );
            // Evoker.java:65: `LookAtPlayerGoal(this, Mob.class, 8.0F)`.
            goal_selector.add_goal(10, LookAtEntityGoal::with_default_any_mob(mob_weak, 8.0));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Evoker.java:66: `HurtByTargetGoal(this, Raider.class).setAlertOthers()`.
            target_selector.add_goal(
                1,
                Box::new(RevengeGoal::new(true).exclude_raiders().alert_others()),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default_and_memory(
                    &mob_arc.mob_entity,
                    &EntityType::PLAYER,
                    true,
                    300,
                ),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default_types_and_memory(
                    &mob_arc.mob_entity,
                    &[&EntityType::VILLAGER, &EntityType::WANDERING_TRADER],
                    false,
                    300,
                ),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, false),
            );
        };

        mob_arc
    }
}

impl NBTStorage for EvokerEntity {
    /// Vanilla: `SpellcasterIllager.addAdditionalSaveData` (`SpellTicks`).
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            nbt.put_int("SpellTicks", self.spellcaster.casting_ticks_left());
        })
    }

    /// Vanilla: `SpellcasterIllager.readAdditionalSaveData` (`SpellTicks`, default 0).
    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.spellcaster
                .set_casting_time(nbt.get_int("SpellTicks").unwrap_or(0));
        })
    }
}

impl Mob for EvokerEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla: `SpellcasterIllager.customServerAiStep`.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.spellcaster.tick();
        })
    }
}
