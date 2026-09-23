use std::sync::{Arc, Weak};

use pumpkin_data::{
    Block,
    damage::DamageType,
    entity::EntityType,
    sound::{Sound, SoundCategory},
    tracked_data,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;

use crate::entity::{
    Entity, EntityBase, NBTStorage,
    ai::goal::{
        active_target::ActiveTargetGoal, interact_with_door::InteractWithDoorGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity, piglin_shared,
        zombification::{self, ZombificationTimer},
        zombified_piglin::ZombifiedPiglinEntity,
    },
};
use crate::world::World;

pub struct PiglinBruteEntity {
    pub mob_entity: MobEntity,
    /// `AbstractPiglin.timeInOverworld`/`IsImmuneToZombification`
    /// (`AbstractPiglin.java:26-33`); brutes inherit the whole conversion path unchanged
    /// apart from the sound (`PiglinBrute.java:141-144`).
    zombification: ZombificationTimer,
}

impl PiglinBruteEntity {
    /// `PiglinBrute.xpReward` (`PiglinBrute.java:46`).
    pub const XP_REWARD: u32 = 20;

    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let piglin = Self {
            mob_entity,
            zombification: ZombificationTimer::new(),
        };
        let mob_arc = Arc::new(piglin);
        // `AbstractPiglin.applyOpenDoorsAbility` (`AbstractPiglin.java:43-47`).
        mob_arc
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_can_open_doors(true);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            // `InteractWithDoor.create()` in `PiglinBruteAi.initCoreActivity`
            // (`PiglinBruteAi.java:58`), with the villager's goal port of that behavior.
            goal_selector.add_goal(0, Box::new(InteractWithDoorGoal::new(true)));
            goal_selector.add_goal(2, Box::new(MeleeAttackGoal::new(1.0, true)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(
                1,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(
                    &mob_arc.mob_entity,
                    &EntityType::WITHER_SKELETON,
                    true,
                ),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::WITHER, true),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn is_immune_to_zombification(&self) -> bool {
        self.zombification.is_immune()
    }

    /// `AbstractPiglin.setImmuneToZombification`: updates the synced
    /// `DATA_IMMUNE_TO_ZOMBIFICATION`.
    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.zombification.set_immune(immune);
        self.send_immune_to_zombification();
    }

    fn send_immune_to_zombification(&self) {
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::piglin_brute::DATA_IMMUNE_TO_ZOMBIFICATION,
                self.zombification.is_immune(),
            )],
            None,
        );
    }

    /// `AbstractPiglin.isConverting` (`AbstractPiglin.java:103-107`).
    #[must_use]
    pub fn is_converting(&self) -> bool {
        self.zombification.is_converting(&self.mob_entity)
    }

    /// Mirrors `Piglin.checkPiglinSpawnRules` (never on a nether wart block). Vanilla registers
    /// no `SpawnPlacements` entry for brutes, which only spawn with bastion structures.
    #[must_use]
    pub fn check_piglin_brute_spawn_rules(world: &World, pos: &BlockPos) -> bool {
        world.get_block(&pos.down()) != &Block::NETHER_WART_BLOCK
    }
}

impl NBTStorage for PiglinBruteEntity {
    /// `AbstractPiglin.addAdditionalSaveData` (`AbstractPiglin.java:65-70`).
    fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.mob_entity.living_entity.write_nbt(nbt);
        self.zombification.write_nbt(nbt);
    }

    /// `AbstractPiglin.readAdditionalSaveData` (`AbstractPiglin.java:72-78`).
    fn read_nbt_non_mut(&self, nbt: &NbtCompound) {
        self.mob_entity.living_entity.read_nbt_non_mut(nbt);
        self.zombification.read_nbt(nbt);
        self.send_immune_to_zombification();
    }
}

impl Mob for PiglinBruteEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `AbstractPiglin.defineSynchedData`: `DATA_IMMUNE_TO_ZOMBIFICATION`.
    fn mob_init_data_tracker(&self) {
        self.send_immune_to_zombification();
    }

    fn get_base_experience_reward(&self) -> u32 {
        Self::XP_REWARD
    }

    /// `PiglinBrute.getAmbientSound` (`PiglinBrute.java:117-120`).
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPiglinBruteAmbient)
    }

    /// `PiglinBrute.getHurtSound` (`PiglinBrute.java:122-125`).
    fn get_hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPiglinBruteHurt)
    }

    /// `PiglinBrute.playStepSound` (`PiglinBrute.java:132-135`).
    fn get_step_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPiglinBruteStep)
    }

    /// `PiglinBruteAi.wasHurtBy`: unlike `Piglin`, brutes have no baby-flee or
    /// hoglin-outnumbered branch -- any non-piglin attacker is retaliated against
    /// directly via the same `maybeRetaliate`/`broadcastAngerTarget` piglins use.
    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if let Some(source) = source {
            if source.get_entity().entity_type.id == EntityType::PIGLIN.id
                || source.get_entity().entity_type.id == EntityType::PIGLIN_BRUTE.id
            {
                return;
            }
            piglin_shared::retaliate_and_alert_piglins(self, source);
        }
    }

    /// `AbstractPiglin.customServerAiStep` (`AbstractPiglin.java:80-96`): the overworld
    /// zombification timer, with `PiglinBrute.playConvertedSound`
    /// (`PiglinBrute.java:141-144`) for the conversion sound.
    fn mob_tick(&self, _caller: &Arc<dyn EntityBase>) {
        // `PiglinBruteAi.maybePlayActivitySound` (`PiglinBruteAi.java:150-161`) is
        // reached from `PiglinBrute.customServerAiStep` (`PiglinBrute.java:92-100`).
        if self.mob_entity.get_target().is_some() && rand::random::<f32>() < 0.0125 {
            let entity = &self.mob_entity.living_entity.entity;
            entity.world.load().play_sound_fine(
                Sound::EntityPiglinBruteAngry,
                SoundCategory::Hostile,
                &entity.pos.load(),
                1.0,
                1.0,
            );
        }

        if self.zombification.tick(&self.mob_entity) {
            if self
                .mob_entity
                .living_entity
                .entity
                .world
                .load()
                .level_info
                .load()
                .difficulty
                != Difficulty::Peaceful
            {
                zombification::play_converted_sound(
                    &self.mob_entity,
                    Sound::EntityPiglinBruteConvertedToZombified,
                );
            }
            zombification::convert_to(
                &self.mob_entity,
                &EntityType::ZOMBIFIED_PIGLIN,
                true,
                ZombifiedPiglinEntity::new,
            );
        }
    }
}
