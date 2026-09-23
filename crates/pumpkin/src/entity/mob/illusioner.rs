// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{Arc, Weak};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;

use pumpkin_nbt::compound::NbtCompound;

use crate::entity::{
    Entity, EntityBase, NBTStorage,
    ai::goal::{
        active_target::ActiveTargetGoal,
        avoid_entity::AvoidEntityGoal,
        illusioner_spell::{
            IllusionerBlindnessSpellGoal, IllusionerCastingSpellGoal, IllusionerMirrorSpellGoal,
        },
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        pathfind_to_raid::PathfindToRaidGoal,
        ranged_bow_attack::RangedBowAttackGoal,
        revenge::RevengeGoal,
        spellcaster::SpellcasterState,
        swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            ObtainRaidLeaderBannerGoal, Raider, RaiderCelebrationGoal, RaiderData,
            RaiderMoveThroughVillageGoal,
        },
    },
};

pub struct IllusionerEntity {
    pub mob_entity: MobEntity,
    /// Vanilla: `SpellcasterIllager.spellCastingTickCount` / `currentSpell`.
    pub spellcaster: SpellcasterState,
    /// Vanilla `Raider`/`PatrollingMonster` fields.
    pub raider_data: RaiderData,
}

impl IllusionerEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let illusioner = Self {
            mob_entity,
            spellcaster: SpellcasterState::new(),
            raider_data: RaiderData::default(),
        };
        let mob_arc = Arc::new(illusioner);
        mob_arc
            .mob_entity
            .living_entity
            .entity_equipment
            .try_lock()
            .expect("new illusioner equipment is uncontended")
            .equipment
            .insert(EquipmentSlot::MAIN_HAND, ItemStack::new(1, &Item::BOW));
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let illusioner_weak = Arc::downgrade(&mob_arc);

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Inherited via `super.registerGoals()`, registered before Illusioner's own goals:
            // PatrollingMonster.java:40 then Raider.java:64-67.
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal));
            goal_selector.add_goal(3, PathfindToRaidGoal::new());
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));

            // Illusioner.java:65-73.
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                1,
                Box::new(IllusionerCastingSpellGoal::new(illusioner_weak.clone())),
            );
            goal_selector.add_goal(
                3,
                Box::new(AvoidEntityGoal::new(&EntityType::CREAKING, 8.0, 1.0, 1.2)),
            );
            goal_selector.add_goal(
                4,
                Box::new(IllusionerMirrorSpellGoal::new(illusioner_weak.clone())),
            );
            goal_selector.add_goal(
                5,
                Box::new(IllusionerBlindnessSpellGoal::new(illusioner_weak)),
            );
            goal_selector.add_goal(6, Box::new(RangedBowAttackGoal::new(20, 15.0)));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.6)));
            goal_selector.add_goal(
                9,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(10, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // `Illusioner.java:74` calls `setAlertOthers` after excluding `Raider` attackers.
            target_selector.add_goal(
                1,
                Box::new(RevengeGoal::new(true).exclude_raiders().alert_others()),
            );
            target_selector.add_goal(
                1,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::VILLAGER, true),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, true),
            );
        };

        mob_arc
    }
}

impl NBTStorage for IllusionerEntity {
    /// Vanilla: `SpellcasterIllager.addAdditionalSaveData` (`SpellTicks`).
    fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.mob_entity.living_entity.write_nbt(nbt);
        // `Raider.addAdditionalSaveData` (incl. `PatrollingMonster`'s patrol fields).
        self.write_raider_nbt(nbt);
        nbt.put_int("SpellTicks", self.spellcaster.casting_ticks_left());
    }

    /// Vanilla: `SpellcasterIllager.readAdditionalSaveData` (`SpellTicks`, default 0).
    fn read_nbt_non_mut(&self, nbt: &NbtCompound) {
        self.mob_entity.living_entity.read_nbt_non_mut(nbt);
        self.read_raider_nbt(nbt);
        self.spellcaster
            .set_casting_time(nbt.get_int("SpellTicks").unwrap_or(0));
    }
}

impl Mob for IllusionerEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla: `SpellcasterIllager.customServerAiStep`.
    fn mob_tick(&self, _caller: &Arc<dyn EntityBase>) {
        self.spellcaster.tick();
    }

    // Vanilla `Illusioner.applyRaidBuffs` is an empty override; the `Mob` trait's no-op default
    // is already correct parity.

    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }
}

impl PatrollingMonster for IllusionerEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for IllusionerEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityIllusionerAmbient
    }
}
