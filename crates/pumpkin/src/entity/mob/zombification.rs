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
    data_component_impl::EquipmentSlot,
    dimension::Dimension,
    effect::StatusEffect,
    entity::EntityType,
    item_stack::ItemStack,
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

/// Spawns `new_type` in place of `old`, optionally giving it 200 ticks of nausea.
///
/// `Mob.convertTo` as used by `AbstractPiglin.finishConversion` (`AbstractPiglin.java:109-115`)
/// and `Hoglin.finishConversion` (`Hoglin.java:251-256`). Conversions outside the zombification
/// path pass `nausea = false` -- `Tadpole.ageUp` (`Tadpole.java:238-247`) is one; the nausea
/// belongs to zombification, not to `convertTo` itself.
///
/// The copy itself is [`prepare_conversion`] and the spawn/discard is [`complete_conversion`];
/// callers that need to run `ConversionParams.AfterConversion` work between the two (the zombie
/// family's `handleAttributes`, for one) use them directly.
pub async fn convert_to<T>(
    old: &MobEntity,
    new_type: &'static EntityType,
    nausea: bool,
    build: impl FnOnce(Entity) -> Arc<T>,
) where
    T: EntityBase + Send + Sync + 'static,
{
    let converted = prepare_conversion(old, new_type, build).await;

    if nausea && let Some(new_living) = converted.get_living_entity() {
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

    complete_conversion(old, converted).await;
}

/// The first half of `Mob.convertTo` (`Mob.java:1229-1255`).
///
/// Builds the replacement and copies what `ConversionType.SINGLE.convert`
/// (`ConversionType.java:16-69`) and its `convertCommon` (`ConversionType.java:87-127`) carry
/// over, without spawning it or discarding `old`.
///
/// Carried over: position, velocity, rotation and body rotation, ground flag, fall distance,
/// invulnerability, custom name and its visibility, silent and no-gravity flags, scoreboard
/// tags, absorption, active effects, the equipment stacks together with their drop chances
/// (`ConversionParams.keepEquipment`), the persistence, left-handed and no-AI flags, and
/// (through [`complete_conversion`]) `canPickUpLoot` (`preserveCanPickUpLoot`).
///
/// `EntityType.create(level, CONVERSION)` never runs `finalizeSpawn`, so the replacement must not
/// re-roll spawn gear or the loot-pickup chance. `World::spawn_entity` runs
/// `Mob::init_data_tracker`, whose `finalizeSpawn` work is exactly what the entity-level
/// restored flag suppresses, so the flag is raised here. Anything else `finalizeSpawn` does that
/// a conversion keeps (for zombies, `handleAttributes`) is the caller's to run before
/// [`complete_conversion`].
///
/// Divergences: passengers and the vehicle (`ConversionType.java:20-38`), the leash holder,
/// the sleeping position, the team, the portal cooldown, the `ANGRY_AT` brain memory and the
/// `CUSTOM_DATA` component are not carried over -- Pumpkin has no start-riding transfer API,
/// and none of the others is modeled on the mobs that convert. The age is not copied:
/// vanilla copies it only when both mobs are `AgeableMob` (`ConversionType.java:82-86`), which
/// none of the converting pairs here is, so the replacement starts at zero.
pub async fn prepare_conversion<T>(
    old: &MobEntity,
    new_type: &'static EntityType,
    build: impl FnOnce(Entity) -> Arc<T>,
) -> Arc<T>
where
    T: EntityBase + Send + Sync + 'static,
{
    prepare_conversion_with_equipment(old, new_type, true, build).await
}

/// [`prepare_conversion`] with `ConversionParams.keepEquipment` (`ConversionType.java:40-47`)
/// selectable. With `keep_equipment` false the old mob's stacks are not transferred.
pub async fn prepare_conversion_with_equipment<T>(
    old: &MobEntity,
    new_type: &'static EntityType,
    keep_equipment: bool,
    build: impl FnOnce(Entity) -> Arc<T>,
) -> Arc<T>
where
    T: EntityBase + Send + Sync + 'static,
{
    let old_entity = &old.living_entity.entity;
    let world = old_entity.world.load().clone();
    let pos = old_entity.pos.load();

    let converted = build(Entity::new(world, pos, new_type));

    {
        let new_entity = converted.get_entity();
        new_entity.velocity.store(old_entity.velocity.load());
        new_entity.yaw.store(old_entity.yaw.load());
        new_entity.pitch.store(old_entity.pitch.load());
        new_entity.body_yaw.store(old_entity.body_yaw.load());
        new_entity
            .on_ground
            .store(old_entity.on_ground.load(Relaxed), Relaxed);
        new_entity
            .invulnerable
            .store(old_entity.invulnerable.load(Relaxed), Relaxed);
        // `setCustomNameVisible(from.isCustomNameVisible())` is unconditional
        // (`ConversionType.java:101`); the flag goes in first because `set_custom_name` reads it.
        new_entity
            .custom_name_visible
            .store(old_entity.custom_name_visible.load(Relaxed), Relaxed);
        if let Some(name) = &**old_entity.custom_name.load() {
            new_entity.set_custom_name(name.clone());
        }
        new_entity
            .persistence_required
            .store(old_entity.persistence_required.load(Relaxed), Relaxed);
        new_entity
            .silent
            .store(old_entity.silent.load(Relaxed), Relaxed);
        if old_entity.has_no_gravity() {
            new_entity.set_has_no_gravity(true);
        }
        let tags = old_entity.scoreboard_tags.lock().await.clone();
        if !tags.is_empty() {
            *new_entity.scoreboard_tags.lock().await = tags;
        }
    }

    if let Some(new_mob) = converted.get_mob() {
        let new_mob = new_mob.get_mob_entity();
        if old.is_left_handed() {
            new_mob.set_left_handed(true);
        }
        if old.is_no_ai() {
            new_mob.set_no_ai(true);
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
        new_living
            .absorption
            .store(old.living_entity.absorption.load());
        new_living
            .fall_distance
            .store(old.living_entity.fall_distance.load());
        for effect in effects {
            new_living.add_effect(effect).await;
        }

        // `ConversionType.java:40-47`: every non-empty stack moves across (`copyAndClear`) with
        // its slot's drop chance.
        let kept: Vec<(EquipmentSlot, ItemStack)> = if keep_equipment {
            let mut equipment = old.living_entity.entity_equipment.lock().await;
            let kept = equipment
                .equipment
                .iter()
                .filter(|(_, stack)| !stack.is_empty())
                .map(|(slot, stack)| (slot.clone(), stack.clone()))
                .collect();
            equipment.clear();
            kept
        } else {
            Vec::new()
        };
        if !kept.is_empty() {
            let old_chances = old
                .living_entity
                .equipment_drop_chances
                .lock()
                .await
                .clone();
            let mut new_equipment = new_living.entity_equipment.lock().await;
            let mut new_chances = new_living.equipment_drop_chances.lock().await;
            for (slot, stack) in kept {
                if let Some(chance) = old_chances.get(&slot) {
                    new_chances.insert(slot.clone(), *chance);
                }
                new_equipment.put(&slot, stack);
            }
        }
    }

    // `Mob.finalizeSpawn` work is not part of a conversion; see the doc comment.
    converted
        .get_entity()
        .restored_from_nbt
        .store(true, Relaxed);
    converted
}

/// The second half of `Mob.convertTo`: adds the replacement to the world and discards `old`
/// (`Mob.java:1246-1252`).
///
/// `preserveCanPickUpLoot` (`ConversionType.java:91-93`) is applied after the spawn: the
/// generic `Mob::init_data_tracker` re-rolls that flag for every mob it initializes, restored or
/// not, because `CanPickUpLoot` is not persisted yet.
pub async fn complete_conversion<T>(old: &MobEntity, converted: Arc<T>)
where
    T: EntityBase + Send + Sync + 'static,
{
    let old_entity = &old.living_entity.entity;
    let world = old_entity.world.load().clone();
    let can_pick_up_loot = old.can_pick_up_loot();

    world
        .spawn_entity(converted.clone() as Arc<dyn EntityBase>)
        .await;
    if let Some(new_mob) = converted.get_mob() {
        new_mob
            .get_mob_entity()
            .set_can_pick_up_loot(can_pick_up_loot);
    }
    old_entity.remove().await;
}

/// Plays a mob's conversion sound at its own position.
///
/// `playConvertedSound` (`PiglinBrute.java:141-144`, `Piglin.java`'s override, and the
/// inline `makeSound` in `Hoglin.java:152`). `makeSound` skips silent mobs and plays at the
/// mob's sound volume (1.0 for all three) and `getVoicePitch`, which callers pass as
/// `Mob::get_sound_pitch`. The category stays `Hostile`: the hoglin overrides
/// `getSoundSource` to it and both piglins are `Monster`s.
pub fn play_converted_sound(mob: &MobEntity, sound: Sound, pitch: f32) {
    let entity = &mob.living_entity.entity;
    if entity.is_silent() {
        return;
    }
    entity.world.load().play_sound_fine(
        sound,
        SoundCategory::Hostile,
        &entity.pos.load(),
        1.0,
        pitch,
    );
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
