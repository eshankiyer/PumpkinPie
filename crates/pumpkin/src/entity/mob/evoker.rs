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
        raider_celebration::RaiderCelebrationGoal,
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
    /// Vanilla `Raider.IS_CELEBRATING` (synced data).
    is_celebrating: std::sync::atomic::AtomicBool,
}

impl EvokerEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let evoker = Self {
            mob_entity,
            spellcaster: SpellcasterState::new(),
            wololo_target: tokio::sync::Mutex::new(None),
            is_celebrating: std::sync::atomic::AtomicBool::new(false),
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
            // Raider.java:67, via `super.registerGoals()`: `RaiderCelebration`.
            goal_selector.add_goal(5, RaiderCelebrationGoal::new());
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

    /// Vanilla: `Raider.setCelebrating` (`Raider.java:177-179`).
    fn set_celebrating(&self, celebrating: bool) {
        if self
            .is_celebrating
            .swap(celebrating, std::sync::atomic::Ordering::Relaxed)
            != celebrating
        {
            self.mob_entity.living_entity.entity.send_meta_data(
                &[pumpkin_protocol::java::client::play::Metadata::new(
                    pumpkin_data::tracked_data::evoker::IS_CELEBRATING,
                    celebrating,
                )],
                None,
            );
        }
    }

    /// Vanilla: `Evoker.java:76-79`.
    fn get_celebrate_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        Some(pumpkin_data::sound::Sound::EntityEvokerCelebrate)
    }

    /// `Evoker.considersEntityAsAlly` (`Evoker.java:82-100`): itself, the illager rule, or a
    /// vex whose owner is this evoker or an ally under the illager rule.
    fn considers_entity_as_ally(
        &self,
        other: &dyn EntityBase,
        world: &crate::world::World,
        scoreboard: &crate::world::scoreboard::Scoreboard,
    ) -> bool {
        use crate::entity::ai::goal::track_target::illager_considers_entity_as_ally;

        let other_entity = other.get_entity();
        if other_entity.entity_id == self.mob_entity.living_entity.entity.entity_id {
            return true;
        }
        if illager_considers_entity_as_ally(self, other, scoreboard) {
            return true;
        }
        // A vex's root owner is its summoner, which is not itself ownable.
        if let Some(vex) = other.cast_any().downcast_ref::<super::vex::VexEntity>()
            && let Some(root_owner) = vex.owner_id().and_then(|id| world.get_entity_by_id(id))
        {
            return root_owner.get_entity().entity_id
                == self.mob_entity.living_entity.entity.entity_id
                || illager_considers_entity_as_ally(self, root_owner.as_ref(), scoreboard);
        }
        false
    }

    /// Vanilla: `SpellcasterIllager.customServerAiStep`.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.spellcaster.tick();
        })
    }
}
