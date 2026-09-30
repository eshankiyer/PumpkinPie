use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::zombie::{
    ZombieEntityBase, ZombieFamily, ignite_target_on_hit, is_eye_in_water,
    prepare_zombie_conversion, publish_under_water_conversion, zombie::ZombieEntity,
    zombie_killed_entity,
};
use crate::entity::mob::zombification;
use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    mob::{Mob, MobEntity},
};

/// `Zombie::inWaterTime` threshold (`Zombie.java` `tick`): ticks the eyes must be submerged
/// before underwater conversion starts.
const WATER_TICKS_TO_START_CONVERSION: i32 = 600;
/// `Zombie::startUnderWaterConversion`'s fixed countdown (`Zombie.java` `tick`,
/// `startUnderWaterConversion(300)`).
const CONVERSION_TICKS: i32 = 300;

/// `Husk::doHurtTarget`: `140 * (int) difficulty`. Vanilla truncates `difficulty` to `int`
/// *before* multiplying, so `effective_difficulty` (clamped to `[2.0, 4.0]`) only ever produces
/// 280, 420, or 560 ticks.
const fn husk_hunger_duration(effective_difficulty: f32) -> i32 {
    140 * (effective_difficulty as i32)
}

pub struct HuskEntity {
    entity: Arc<ZombieEntityBase>,
    /// Vanilla `Zombie::inWaterTime`, counted while the eyes are under water.
    in_water_time: AtomicI32,
    /// Vanilla `Zombie::conversionTime`. `-1` while not converting, matching the
    /// `DrownedConversionTime` NBT sentinel `Zombie::readAdditionalSaveData` uses.
    conversion_time: AtomicI32,
}

impl HuskEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = ZombieEntityBase::new(entity);
        let husk = Self {
            entity,
            in_water_time: AtomicI32::new(0),
            conversion_time: AtomicI32::new(-1),
        };
        Arc::new(husk)
    }

    /// Vanilla `Zombie::doUnderWaterConversion` + `Husk::doUnderWaterConversion`
    /// (`Husk.java:82-88`): replaces this husk with a plain zombie at the same position through
    /// `convertToZombieType`, keeping its equipment, baby state and the other
    /// `ConversionType.SINGLE` state (see [`zombification::prepare_conversion`]).
    async fn finish_conversion(&self) {
        let old_entity = self.get_entity();
        let zombie = prepare_zombie_conversion(self, &EntityType::ZOMBIE, ZombieEntity::new).await;
        // A husk never breaks doors, so only a new leader can turn the flag on.
        zombie.settle_conversion_door_breaking(false).await;
        zombification::complete_conversion(&self.entity.mob_entity, zombie).await;
        if !old_entity.silent.load(Ordering::Relaxed) {
            old_entity.world.load().sync_world_event(
                WorldEvent::SoundHuskToZombie,
                old_entity.block_pos.load(),
                0,
            );
        }
    }
}

impl ZombieFamily for HuskEntity {
    fn zombie_base(&self) -> &ZombieEntityBase {
        &self.entity
    }
}

impl NBTStorage for HuskEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.entity.write_nbt(nbt).await;
            nbt.put_int(
                "DrownedConversionTime",
                self.conversion_time.load(Ordering::Relaxed),
            );
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.entity.read_nbt_non_mut(nbt).await;
            let time = nbt.get_int("DrownedConversionTime").unwrap_or(-1);
            self.conversion_time.store(time, Ordering::Relaxed);
            if time != -1 {
                // `startUnderWaterConversion(conversionTime)` (`Zombie.java:414-415`).
                publish_under_water_conversion(self.get_entity());
            }
        })
    }
}

impl Mob for HuskEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    /// `Husk.getStepSound` (`Husk.java:60-63`), played by `Zombie.playStepSound` at volume
    /// `0.15F` and pitch `1.0F` -- the `Mob` defaults.
    fn get_step_sound(&self) -> Option<Sound> {
        Some(Sound::EntityHuskStep)
    }

    /// `Zombie::getBaseExperienceReward` (`Zombie.java:178-185`).
    fn get_base_experience_reward(&self) -> u32 {
        self.entity.base_experience_reward()
    }

    /// `LivingEntity::getVoicePitch`, reading the husk's own baby flag.
    fn get_sound_pitch(&self) -> f32 {
        self.entity.voice_pitch()
    }

    /// `Zombie::killedEntity` (`Zombie.java:421-435`), inherited by `Husk`.
    fn killed_entity<'a>(&'a self, victim: &'a dyn EntityBase) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move { zombie_killed_entity(&self.entity, victim).await })
    }

    /// Delegates to `ZombieEntityBase`, which carries `Zombie::finalizeSpawn`'s
    /// `handleAttributes` roll (`Zombie.java:505`) that every zombie variant inherits.
    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move { self.entity.mob_init_data_tracker().await })
    }

    /// `Zombie::hurtServer`'s reinforcement half (`Zombie.java:288-340`), inherited by `Husk`.
    fn on_damage<'a>(
        &'a self,
        _damage_type: DamageType,
        source: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            crate::entity::mob::zombie::try_spawn_reinforcements(&self.entity.mob_entity, source)
                .await;
        })
    }

    /// Vanilla `Husk::doHurtTarget` (`Husk.java:65-75`): `super.doHurtTarget` first -- so the
    /// ignite half of `Zombie::doHurtTarget` -- then an unarmed husk hit applies Hunger for
    /// `140 * getEffectiveDifficulty()` ticks.
    fn on_successful_attack<'a>(&'a self, target: &'a dyn EntityBase) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            ignite_target_on_hit(&self.entity.mob_entity, target).await;
            let entity = self.get_entity();
            let held_item = self.entity.mob_entity.living_entity.held_item(entity).await;
            let main_hand_empty = held_item.is_empty();
            if !main_hand_empty {
                return;
            }
            let Some(target_living) = target.get_living_entity() else {
                return;
            };

            let difficulty = RegionalDifficulty::at(&entity.world.load(), entity.pos.load());
            let duration = husk_hunger_duration(difficulty.effective_difficulty);
            target_living
                .add_effect(Effect {
                    effect_type: &StatusEffect::HUNGER,
                    duration,
                    amplifier: 0,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                })
                .await;
        })
    }

    /// Vanilla `Zombie::tick`'s underwater-conversion timer (`convertsInWater` is `true` for the
    /// base `Zombie`, and `Husk` doesn't override it, so husks convert to zombies just like any
    /// other zombie submerged for long enough).
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if self
                .entity
                .mob_entity
                .living_entity
                .dead
                .load(Ordering::Relaxed)
            {
                return;
            }

            let converting_time = self.conversion_time.load(Ordering::Relaxed);
            if converting_time >= 0 {
                let new_time = converting_time - 1;
                self.conversion_time.store(new_time, Ordering::Relaxed);
                if new_time < 0 {
                    self.finish_conversion().await;
                }
            } else if is_eye_in_water(self.get_entity()) {
                let new_time = self.in_water_time.fetch_add(1, Ordering::Relaxed) + 1;
                if new_time >= WATER_TICKS_TO_START_CONVERSION {
                    self.in_water_time.store(0, Ordering::Relaxed);
                    self.conversion_time
                        .store(CONVERSION_TICKS, Ordering::Relaxed);
                    publish_under_water_conversion(self.get_entity());
                }
            } else {
                self.in_water_time.store(-1, Ordering::Relaxed);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::husk_hunger_duration;

    #[test]
    fn hunger_duration_truncates_difficulty_before_multiplying() {
        assert_eq!(husk_hunger_duration(2.0), 280);
        // 2.7 truncates to 2, not 2.7 * 140 = 378.
        assert_eq!(husk_hunger_duration(2.7), 280);
        assert_eq!(husk_hunger_duration(3.5), 420);
        assert_eq!(husk_hunger_duration(4.0), 560);
    }
}
