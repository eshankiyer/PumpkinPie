//! Shared port of vanilla's piglin/hoglin overworld-zombification timer.
//!
//! Vanilla drives this from two near-identical copies of the same logic:
//! `AbstractPiglin.customServerAiStep` (`AbstractPiglin.java:80-96`) for `Piglin` and
//! `PiglinBrute`, and `Hoglin.customServerAiStep` (`Hoglin.java:144-158`) for `Hoglin`.
//! Both count ticks spent where the `gameplay/piglins_zombify` environment attribute is
//! set, and convert once the counter passes `CONVERSION_TIME = 300`
//! (`AbstractPiglin.java:29`, `Hoglin.java:69`).
//!
//! Pumpkin has no Brain/Activity system, but this behaviour never needed one -- it lives
//! in `customServerAiStep`, so it maps directly onto `Mob::mob_tick`.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering::Relaxed},
};

use pumpkin_data::{
    dimension::Dimension,
    effect::StatusEffect,
    entity::EntityType,
    potion::Effect,
    sound::{Sound, SoundCategory},
};
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::{Entity, EntityBase, mob::MobEntity};
use crate::world::World;

/// `AbstractPiglin.CONVERSION_TIME` (`AbstractPiglin.java:29`) and `Hoglin.CONVERSION_TIME`
/// (`Hoglin.java:69`); both compare with `>`, so conversion lands on tick 301.
pub const CONVERSION_TIME_TICKS: i32 = 300;

/// `MobEffectInstance(MobEffects.NAUSEA, 200, 0)` handed to the converted mob by
/// `AbstractPiglin.finishConversion` (`AbstractPiglin.java:109-115`) and
/// `Hoglin.finishConversion` (`Hoglin.java:251-256`).
const NAUSEA_DURATION_TICKS: i32 = 200;

/// `isConverting`'s environment check (`AbstractPiglin.java:106`, `Hoglin.java:299`):
/// `environmentAttributes().getValue(EnvironmentAttributes.PIGLINS_ZOMBIFY, position)`.
///
/// That attribute defaults to `true` (`EnvironmentAttributes.java:135-137`) and is set to
/// `false` by exactly one built-in dimension type, `minecraft:the_nether`
/// (`DimensionTypes.java:94`). Pumpkin has no environment-attribute map, so this tests the
/// dimension directly: equivalent for the vanilla dimensions, but a datapack that overrides
/// the attribute (or a custom nether-like dimension) is not honoured.
fn dimension_zombifies_piglins(world: &World) -> bool {
    world.dimension.minecraft_name != Dimension::THE_NETHER.minecraft_name
}

/// The `timeInOverworld` counter plus the `IsImmuneToZombification` flag, as carried by
/// `AbstractPiglin` (`AbstractPiglin.java:26-33`) and duplicated on `Hoglin`
/// (`Hoglin.java:69-71`).
pub struct ZombificationTimer {
    time_in_overworld: AtomicI32,
    immune: AtomicBool,
}

impl Default for ZombificationTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl ZombificationTimer {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            // `DEFAULT_TIME_IN_OVERWORLD = 0`, `DEFAULT_IMMUNE_TO_ZOMBIFICATION = false`
            // (`AbstractPiglin.java:30-32`).
            time_in_overworld: AtomicI32::new(0),
            immune: AtomicBool::new(false),
        }
    }

    #[must_use]
    pub fn is_immune(&self) -> bool {
        self.immune.load(Relaxed)
    }

    pub fn set_immune(&self, immune: bool) {
        self.immune.store(immune, Relaxed);
    }

    /// `isConverting` (`AbstractPiglin.java:103-107`, `Hoglin.java:296-300`).
    #[must_use]
    pub fn is_converting(&self, mob: &MobEntity) -> bool {
        !self.is_immune()
            && !mob.is_no_ai()
            && dimension_zombifies_piglins(&mob.living_entity.entity.world.load())
    }

    /// Advances the counter one tick, returning `true` on the single tick where vanilla
    /// would call `finishConversion`. Not converting resets the counter to zero, matching
    /// the `else` branch both vanilla copies share.
    pub fn tick(&self, mob: &MobEntity) -> bool {
        if !self.is_converting(mob) {
            self.time_in_overworld.store(0, Relaxed);
            return false;
        }
        if self.time_in_overworld.fetch_add(1, Relaxed) + 1 > CONVERSION_TIME_TICKS {
            // Vanilla never needs this: `convertTo` discards the entity inside the same
            // `customServerAiStep` call. Here the conversion is async, so reset the counter
            // so a mob that is still ticked while `convert_to` runs cannot fire twice and
            // spawn two replacements.
            self.time_in_overworld.store(0, Relaxed);
            return true;
        }
        false
    }

    /// `addAdditionalSaveData` (`AbstractPiglin.java:65-70`, `Hoglin.java:273-277`). Both
    /// classes use the same two keys.
    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("IsImmuneToZombification", self.is_immune());
        nbt.put_int("TimeInOverworld", self.time_in_overworld.load(Relaxed));
    }

    /// `readAdditionalSaveData` (`AbstractPiglin.java:72-78`, `Hoglin.java:280-285`), with
    /// vanilla's defaults for both keys.
    pub fn read_nbt(&self, nbt: &NbtCompound) {
        self.set_immune(nbt.get_bool("IsImmuneToZombification").unwrap_or(false));
        self.time_in_overworld
            .store(nbt.get_int("TimeInOverworld").unwrap_or(0), Relaxed);
    }
}

/// Spawns `new_type` in place of `old` and gives it 200 ticks of nausea.
///
/// `Mob.convertTo` as used by `AbstractPiglin.finishConversion` (`AbstractPiglin.java:109-115`)
/// and `Hoglin.finishConversion` (`Hoglin.java:251-256`).
///
/// Simplifications, matching the copy set `ZombieEntity::finish_conversion` already uses for
/// zombie -> drowned: position, velocity, rotation, ground flag, age, invulnerability, custom
/// name and active effects carry over; equipment, leash and passengers do not. That is a real
/// divergence for `PiglinBrute`, whose vanilla `ConversionParams.single(this, true, true)`
/// keeps the golden axe -- here the converted zombified piglin arrives empty-handed. Pumpkin
/// has no generic `Mob::convertTo`, and building equipment transfer here would duplicate work
/// that belongs in one.
pub async fn convert_to<T>(
    old: &MobEntity,
    new_type: &'static EntityType,
    build: impl FnOnce(Entity) -> Arc<T>,
) where
    T: EntityBase + Send + Sync + 'static,
{
    let old_entity = &old.living_entity.entity;
    let world = old_entity.world.load().clone();
    let pos = old_entity.pos.load();

    let converted = build(Entity::new(world.clone(), pos, new_type));

    {
        let new_entity = converted.get_entity();
        new_entity.velocity.store(old_entity.velocity.load());
        new_entity.yaw.store(old_entity.yaw.load());
        new_entity.pitch.store(old_entity.pitch.load());
        new_entity
            .on_ground
            .store(old_entity.on_ground.load(Relaxed), Relaxed);
        new_entity.age.store(old_entity.age.load(Relaxed), Relaxed);
        new_entity
            .invulnerable
            .store(old_entity.invulnerable.load(Relaxed), Relaxed);
        if let Some(name) = &**old_entity.custom_name.load() {
            new_entity.set_custom_name(name.clone());
            new_entity
                .custom_name_visible
                .store(old_entity.custom_name_visible.load(Relaxed), Relaxed);
        }
    }

    let effects: Vec<_> = old
        .living_entity
        .active_effects
        .lock()
        .await
        .values()
        .cloned()
        .collect();
    if let Some(new_living) = converted.get_living_entity() {
        for effect in effects {
            new_living.add_effect(effect).await;
        }
        new_living
            .add_effect(Effect {
                effect_type: &StatusEffect::NAUSEA,
                duration: NAUSEA_DURATION_TICKS,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            })
            .await;
    }

    world.spawn_entity(converted as Arc<dyn EntityBase>).await;
    old_entity.remove().await;
}

/// Plays a mob's conversion sound at its own position.
///
/// `playConvertedSound` (`PiglinBrute.java:141-144`, `Piglin.java`'s override, and the
/// inline `makeSound` in `Hoglin.java:152`). Vanilla's `makeSound` uses the mob's own sound
/// volume and voice pitch; Pumpkin's mobs have no `getVoicePitch` equivalent, so this plays
/// at the unmodified 1.0/1.0 that `Entity::play_sound` also uses.
pub fn play_converted_sound(mob: &MobEntity, sound: Sound) {
    let entity = &mob.living_entity.entity;
    entity
        .world
        .load()
        .play_sound(sound, SoundCategory::Hostile, &entity.pos.load());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_nether_suppresses_zombification() {
        // `DimensionTypes.java:94` sets `PIGLINS_ZOMBIFY` false for the nether only; the
        // attribute default is `true` (`EnvironmentAttributes.java:135-137`).
        assert_ne!(
            Dimension::OVERWORLD.minecraft_name,
            Dimension::THE_NETHER.minecraft_name
        );
        assert_ne!(
            Dimension::THE_END.minecraft_name,
            Dimension::THE_NETHER.minecraft_name
        );
    }

    #[test]
    fn conversion_time_matches_vanilla() {
        assert_eq!(CONVERSION_TIME_TICKS, 300);
    }
}
