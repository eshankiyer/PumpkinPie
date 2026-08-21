use std::sync::{Arc, Weak};

use pumpkin_data::{damage::DamageType, entity::EntityType, sound::Sound};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::Difficulty;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        active_target::ActiveTargetGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity, piglin_shared,
        zombification::{self, ZombificationTimer},
        zombified_piglin::ZombifiedPiglinEntity,
    },
};

pub struct PiglinBruteEntity {
    pub mob_entity: MobEntity,
    /// `AbstractPiglin.timeInOverworld`/`IsImmuneToZombification`
    /// (`AbstractPiglin.java:26-33`); brutes inherit the whole conversion path unchanged
    /// apart from the sound (`PiglinBrute.java:141-144`).
    zombification: ZombificationTimer,
}

impl PiglinBruteEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let piglin = Self {
            mob_entity,
            zombification: ZombificationTimer::new(),
        };
        let mob_arc = Arc::new(piglin);
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
}

impl NBTStorage for PiglinBruteEntity {
    /// `AbstractPiglin.addAdditionalSaveData` (`AbstractPiglin.java:65-70`).
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.zombification.write_nbt(nbt);
        })
    }

    /// `AbstractPiglin.readAdditionalSaveData` (`AbstractPiglin.java:72-78`).
    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.zombification.read_nbt(nbt);
            self.mob_entity.living_entity.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::piglin_brute::DATA_IMMUNE_TO_ZOMBIFICATION,
                    self.zombification.is_immune(),
                )],
                None,
            );
        })
    }
}

impl Mob for PiglinBruteEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `PiglinBruteAi.wasHurtBy`: unlike `Piglin`, brutes have no baby-flee or
    /// hoglin-outnumbered branch -- any non-piglin attacker is retaliated against
    /// directly via the same `maybeRetaliate`/`broadcastAngerTarget` piglins use.
    fn on_damage<'a>(
        &'a self,
        _damage_type: DamageType,
        source: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if let Some(source) = source {
                if source.get_entity().entity_type.id == EntityType::PIGLIN.id
                    || source.get_entity().entity_type.id == EntityType::PIGLIN_BRUTE.id
                {
                    return;
                }
                piglin_shared::retaliate_and_alert_piglins(self, source).await;
            }
        })
    }

    /// `AbstractPiglin.customServerAiStep` (`AbstractPiglin.java:80-96`): the overworld
    /// zombification timer, with `PiglinBrute.playConvertedSound`
    /// (`PiglinBrute.java:141-144`) for the conversion sound.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
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
                )
                .await;
            }
        })
    }
}
