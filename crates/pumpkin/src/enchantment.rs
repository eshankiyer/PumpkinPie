//! Generic enchantment-effect framework.
//!
//! `pumpkin_data::Enchantment` (generated from vanilla's enchantment registry) carries no
//! numeric effect parameters at all — anvil cost, exclusive sets, slots, but nothing that
//! says "how much". Vanilla itself is data-driven here: every enchantment JSON attaches a list
//! of `EnchantmentEffectComponents` (see
//! `net/minecraft/world/item/enchantment/EnchantmentEffectComponents.java` in the decompiled
//! 26.2 source), each holding one or more `LevelBasedValue` formulas
//! (`net/minecraft/world/item/enchantment/LevelBasedValue.java`).
//!
//! This module ports that shape in hand-written Rust: [`LevelBasedValue`] mirrors vanilla's
//! formula variants exactly (`constant`/`linear`/`lookup`+fallback/`fraction`/`levels_squared`/
//! `clamped`), and [`EnchantmentEffect`] mirrors the effect-component kinds Pumpkin has (or is
//! expected to grow) call sites for. [`effects_for`] is the single dispatch path: look up an
//! enchantment's effects by `registry_key` and get back typed, level-parameterized data instead
//! of a per-name `if enchantment == Enchantment::X` chain at every call site.
//!
//! All numeric constants below are transcribed directly from
//! `/tmp/pumpkin-vanilla-26.2/decompiled/data/minecraft/enchantment/*.json` and cross-checked
//! against unit tests in this file. Where a JSON effect component has no Pumpkin evaluator yet
//! (e.g. a `ConditionalEffect` predicate type Pumpkin doesn't model), the enchantment's
//! *values* are still captured here for completeness and testing, but callers don't exist yet — see the
//! module-level doc on [`EnchantmentEffect`] for exactly which variants are wired vs. deferred.
//!
//! Three effects have an evaluator here but no caller, because the code that would drive them
//! lives outside this module: [`apply_frost_walker`] needs a `location_changed` hook in the
//! living-entity tick, [`location_based_item_damage`] needs the same hook (Soul Speed's boot
//! drain, next to `LivingEntity::tick_soul_speed`), and [`post_piercing_lunge`] needs the spear
//! piercing-attack path.

use crate::entity::attributes::{Modifier, ModifierOperation};
use crate::world::World;
use pumpkin_data::Block;
use pumpkin_data::Enchantment;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::{EnchantmentsImpl, EquipmentSlot};
use pumpkin_data::enchantment::AttributeModifierSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::Block as BlockTag;
use pumpkin_data::tag::{EntityType as EntityTypeTag, Taggable};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;

/// Mirrors vanilla `LevelBasedValue`.
///
/// See `net/minecraft/world/item/enchantment/LevelBasedValue.java`; `calculate` mirrors the Java
/// method of the same name exactly, including the float-only arithmetic vanilla uses throughout
/// this type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LevelBasedValue {
    Constant(f32),
    Linear {
        base: f32,
        per_level_above_first: f32,
    },
    LevelsSquared {
        added: f32,
    },
    /// 1-indexed: `values[level - 1]` for `level <= values.len()`, else `fallback`.
    Lookup {
        values: &'static [f32],
        fallback: &'static Self,
    },
    Fraction {
        numerator: &'static Self,
        denominator: &'static Self,
    },
    Clamped {
        value: &'static Self,
        min: f32,
        max: f32,
    },
}

impl LevelBasedValue {
    #[must_use]
    pub const fn linear(base: f32, per_level_above_first: f32) -> Self {
        Self::Linear {
            base,
            per_level_above_first,
        }
    }

    /// Mirrors `LevelBasedValue.calculate(int level)` for every variant.
    #[must_use]
    pub fn calculate(&self, level: i32) -> f32 {
        match self {
            Self::Constant(v) => *v,
            Self::Linear {
                base,
                per_level_above_first,
            } => base + per_level_above_first * (level - 1) as f32,
            Self::LevelsSquared { added } => (level * level) as f32 + added,
            Self::Lookup { values, fallback } => {
                if level >= 1 && (level as usize) <= values.len() {
                    values[level as usize - 1]
                } else {
                    fallback.calculate(level)
                }
            }
            Self::Fraction {
                numerator,
                denominator,
            } => {
                let d = denominator.calculate(level);
                if d == 0.0 {
                    0.0
                } else {
                    numerator.calculate(level) / d
                }
            }
            Self::Clamped { value, min, max } => value.calculate(level).clamp(*min, *max),
        }
    }
}

/// Mirrors vanilla `AttributeModifier.Operation`.
///
/// `add_value` -> `ADD_VALUE`, `add_multiplied_base` -> `ADD_MULTIPLIED_BASE`,
/// `add_multiplied_total` -> `ADD_MULTIPLIED_TOTAL`
/// (`net/minecraft/world/entity/ai/attributes/AttributeModifier.java`), which map one-to-one
/// onto Pumpkin's [`ModifierOperation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeOperation {
    AddValue,
    AddMultipliedBase,
    AddMultipliedTotal,
}

impl AttributeOperation {
    #[must_use]
    pub const fn to_modifier_operation(self) -> ModifierOperation {
        match self {
            Self::AddValue => ModifierOperation::Add,
            Self::AddMultipliedBase => ModifierOperation::MultiplyBase,
            Self::AddMultipliedTotal => ModifierOperation::MultiplyTotal,
        }
    }
}

/// Resolves a vanilla attribute id (without the `minecraft:` prefix) to Pumpkin's generated
///
/// attribute constant. Only the ids reachable from an enchantment's `minecraft:attributes`
/// component are listed; anything else returns `None` rather than silently applying to the
/// wrong attribute.
#[must_use]
pub fn attribute_by_key(key: &str) -> Option<&'static Attributes> {
    Some(match key {
        "burning_time" => &Attributes::BURNING_TIME,
        "explosion_knockback_resistance" => &Attributes::EXPLOSION_KNOCKBACK_RESISTANCE,
        "mining_efficiency" => &Attributes::MINING_EFFICIENCY,
        "movement_efficiency" => &Attributes::MOVEMENT_EFFICIENCY,
        "movement_speed" => &Attributes::MOVEMENT_SPEED,
        "oxygen_bonus" => &Attributes::OXYGEN_BONUS,
        "sneaking_speed" => &Attributes::SNEAKING_SPEED,
        "submerged_mining_speed" => &Attributes::SUBMERGED_MINING_SPEED,
        "sweeping_damage_ratio" => &Attributes::SWEEPING_DAMAGE_RATIO,
        "water_movement_efficiency" => &Attributes::WATER_MOVEMENT_EFFICIENCY,
        _ => return None,
    })
}

/// Mirrors `EquipmentSlotGroup.test` (`net/minecraft/world/entity/EquipmentSlotGroup.java:15-25`),
///
/// the predicate `Enchantment.matchingSlot` folds over an enchantment's declared slots
/// (`Enchantment.java:126`).
#[must_use]
pub const fn slot_group_matches(group: &AttributeModifierSlot, slot: &EquipmentSlot) -> bool {
    match group {
        AttributeModifierSlot::Any => true,
        AttributeModifierSlot::MainHand => matches!(slot, EquipmentSlot::MainHand(_)),
        AttributeModifierSlot::OffHand => matches!(slot, EquipmentSlot::OffHand(_)),
        AttributeModifierSlot::Hand => {
            matches!(slot, EquipmentSlot::MainHand(_) | EquipmentSlot::OffHand(_))
        }
        AttributeModifierSlot::Feet => matches!(slot, EquipmentSlot::Feet(_)),
        AttributeModifierSlot::Legs => matches!(slot, EquipmentSlot::Legs(_)),
        AttributeModifierSlot::Chest => matches!(slot, EquipmentSlot::Chest(_)),
        AttributeModifierSlot::Head => matches!(slot, EquipmentSlot::Head(_)),
        AttributeModifierSlot::Armor => slot.is_armor_slot(),
        AttributeModifierSlot::Body => matches!(slot, EquipmentSlot::Body(_)),
        AttributeModifierSlot::Saddle => matches!(slot, EquipmentSlot::Saddle(_)),
    }
}

/// Mirrors `EnchantmentAttributeEffect.idForSlot` (`effects/EnchantmentAttributeEffect.java:31`):
///
/// the JSON `id` (always `minecraft:enchantment.<registry key>`) suffixed with
/// `"/" + slot.getSerializedName()`.
///
/// The slot suffix is load-bearing: it is what lets the same enchantment on four different
/// armour pieces contribute four *separate* modifiers instead of overwriting each other.
#[must_use]
pub fn modifier_id_for_slot(enchantment: &'static Enchantment, slot: &EquipmentSlot) -> String {
    format!(
        "minecraft:enchantment.{}/{}",
        enchantment.registry_key,
        slot.to_name()
    )
}

/// Vanilla `EnchantmentHelper.forEachModifier(ItemStack, EquipmentSlot, BiConsumer)`
/// (`EnchantmentHelper.java:395-401`), collected into a vector instead of a callback.
///
/// Walks every enchantment on `stack`, keeps the ones whose declared slots match `slot`
/// (`Enchantment.matchingSlot`), and turns each of their `minecraft:attributes` effects into
/// a slot-scoped [`Modifier`] via `EnchantmentAttributeEffect.getModifier`.
///
/// `minecraft:location_changed` attribute effects (Soul Speed) are deliberately *not*
/// returned: vanilla only applies those while the entity stands on a soul-speed block.
#[must_use]
pub fn attribute_modifiers_for_slot(
    stack: &ItemStack,
    slot: &EquipmentSlot,
) -> Vec<(&'static Attributes, Modifier)> {
    let Some(enchantments) = stack.get_data_component::<EnchantmentsImpl>() else {
        return Vec::new();
    };

    let mut modifiers = Vec::new();
    for (enchantment, level) in enchantments.enchantment.iter() {
        if *level <= 0 {
            continue;
        }
        if !enchantment
            .slots
            .iter()
            .any(|group| slot_group_matches(group, slot))
        {
            continue;
        }
        for effect in effects_for(enchantment) {
            let EnchantmentEffect::AttributeBonus {
                attribute,
                amount,
                operation,
            } = effect
            else {
                continue;
            };
            let Some(target) = attribute_by_key(attribute) else {
                continue;
            };
            modifiers.push((
                target,
                Modifier {
                    id: modifier_id_for_slot(enchantment, slot),
                    amount: f64::from(amount.calculate(*level)),
                    operation: operation.to_modifier_operation(),
                },
            ));
        }
    }
    modifiers
}

/// The `minecraft:location_changed` attribute modifiers Soul Speed contributes while active,
///
/// scoped to the slot the enchanted boots occupy (`soul_speed.json` -> `minecraft:attribute`
/// effects nested under `minecraft:all_of`).
#[must_use]
pub fn location_based_attribute_modifiers_for_slot(
    stack: &ItemStack,
    slot: &EquipmentSlot,
) -> Vec<(&'static Attributes, Modifier)> {
    let Some(enchantments) = stack.get_data_component::<EnchantmentsImpl>() else {
        return Vec::new();
    };

    let mut modifiers = Vec::new();
    for (enchantment, level) in enchantments.enchantment.iter() {
        if *level <= 0 {
            continue;
        }
        if !enchantment
            .slots
            .iter()
            .any(|group| slot_group_matches(group, slot))
        {
            continue;
        }
        for effect in effects_for(enchantment) {
            let EnchantmentEffect::LocationBasedAttributeBonus {
                attribute,
                amount,
                operation,
            } = effect
            else {
                continue;
            };
            let Some(target) = attribute_by_key(attribute) else {
                continue;
            };
            modifiers.push((
                target,
                Modifier {
                    id: modifier_id_for_slot(enchantment, slot),
                    amount: f64::from(amount.calculate(*level)),
                    operation: operation.to_modifier_operation(),
                },
            ));
        }
    }
    modifiers
}

/// Gates a `minecraft:damage` effect the way vanilla's `entity_properties`/
/// `sensitive_to_*` entity-type-tag requirements do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageCondition {
    Always,
    SensitiveToBaneOfArthropods,
    SensitiveToSmite,
    SensitiveToImpaling,
    /// Power's `damage` requirement gates on the *direct attacker* (the arrow projectile) being
    /// tagged `#minecraft:arrows` — a different entity than the victim every other
    /// [`DamageCondition`] variant checks. [`Self::applies`] only ever receives the
    /// victim/target entity type (see its melee call sites in player.rs/mob/mod.rs), so it
    /// cannot evaluate this condition and always returns `false` here; a projectile-attack call
    /// site would need a separate check against the arrow's own entity type. Power's damage
    /// bonus is applied today via a pre-existing, unrelated multiplicative formula in
    /// `item/items/bow.rs`, not through this framework.
    DirectAttackerIsArrow,
}

impl DamageCondition {
    #[must_use]
    pub fn applies(&self, target_type: &'static EntityType) -> bool {
        match self {
            Self::Always => true,
            Self::SensitiveToBaneOfArthropods => {
                target_type.has_tag(&EntityTypeTag::MINECRAFT_SENSITIVE_TO_BANE_OF_ARTHROPODS)
            }
            Self::SensitiveToSmite => {
                target_type.has_tag(&EntityTypeTag::MINECRAFT_SENSITIVE_TO_SMITE)
            }
            Self::SensitiveToImpaling => {
                target_type.has_tag(&EntityTypeTag::MINECRAFT_SENSITIVE_TO_IMPALING)
            }
            Self::DirectAttackerIsArrow => false,
        }
    }
}

/// Gates a `minecraft:damage_protection` effect by damage-source tag.
///
/// Mirrors the `damage_source_properties`/`tags` requirements in `protection.json`/
/// `fire_protection.json`/`blast_protection.json`/`projectile_protection.json`/
/// `feather_falling.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionCondition {
    /// Protection: any damage not tagged `bypasses_invulnerability` (and not drown/starve/
    /// `generic_kill` per Pumpkin's existing `living.rs` gate).
    Always,
    IsFire,
    IsExplosion,
    IsProjectile,
    IsFall,
}

/// Gates a `minecraft:knockback` effect (Knockback enchantment applies unconditionally, Punch
/// only when the direct attacker is tagged `#minecraft:arrows`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnockbackCondition {
    Always,
    AttackerIsArrow,
}

/// One typed enchantment-effect-component, parameterized by [`LevelBasedValue`] where vanilla's
/// component holds a formula. Variants are grouped by wiring status:
///
/// **Wired** into a Pumpkin call site via this module: `Damage` (player.rs melee, mob/mod.rs
/// melee, trident.rs impaling — Power's `DirectAttackerIsArrow` condition always evaluates to
/// `false` at these melee sites by design, since Power's actual bonus stays on bow.rs's existing
/// formula; see [`DamageCondition::DirectAttackerIsArrow`]), `DamageProtection`
/// (living.rs armor-enchantment pass, also
/// fixing a pre-existing Feather Falling constant bug — see
/// `feather_falling_is_three_per_level_not_four`), `Knockback` (player.rs, mob/mod.rs),
/// `ArmorEffectiveness` (combat.rs `breach_armor_fraction`), `SmashDamagePerFallenBlock`
/// (combat.rs `density_extra_damage`), `PostAttackKnockbackMultiplier` (combat.rs
/// `wind_burst_knockback_multiplier`), `IgniteOnHit` (player.rs, mob/mod.rs fire aspect),
/// `CrossbowChargeTime` (crossbow.rs quick charge).
///
/// **Modeled, not yet wired** (values verified against vanilla JSON in this file's tests, but no
/// call site consumes them yet — see the final report / PARITY.md for the exact blocker per
/// enchantment): `ProjectileCount`, `ProjectileSpread` (multishot's existing call site already
/// hardcodes the same constants correctly since `max_level` is 1 — no behavior to gain from
/// wiring), `ProjectilePiercing`, `TridentReturnAcceleration` (trident has no return-to-owner
/// flight logic at all yet — a missing *feature*, not a wiring gap), `TridentSpinAttackStrength`,
/// `FishingTimeReduction`/`FishingLuckBonus` (existing call sites use different unit
/// conventions not yet verified against `FishingHook.java`), `RepairWithXpMultiply`,
/// `ThornsChance` (no reflect-damage-on-hit code path exists to hook into),
/// `EquipmentDropsChance` (looting's mob-drop-chance boost has no call site yet),
/// `AttributeBonus` (vanilla applies these via an equip-time attribute-modifier system Pumpkin
/// doesn't have a generic equivalent for yet), `AmmoUseSetZero`, `BlockExperienceSetZero`,
/// `ItemDamageRemoveBinomial`, `PreventEquipmentDrop`, `PreventArmorChange` (curses currently
/// have no enforcement code at all).
#[derive(Debug, Clone, Copy)]
pub enum EnchantmentEffect {
    Damage(DamageCondition, LevelBasedValue),
    DamageProtection(ProtectionCondition, LevelBasedValue),
    Knockback(KnockbackCondition, LevelBasedValue),
    ArmorEffectiveness(LevelBasedValue),
    SmashDamagePerFallenBlock(LevelBasedValue),
    /// Wind Burst's `post_attack` explode knockback multiplier.
    PostAttackKnockbackMultiplier(LevelBasedValue),
    /// Fire Aspect's `post_attack` ignite duration, in ticks (JSON is in seconds; ×20 applied
    /// at `calculate()` call sites the same way vanilla's `EnchantmentEntityEffects.Ignite`
    /// converts `duration` seconds to ticks).
    IgniteOnHit(LevelBasedValue),
    /// A `post_attack` `minecraft:apply_mob_effect` on the victim, gated by the same
    /// entity-type requirement the enchantment's `damage` component uses.
    ///
    /// Only Bane of Arthropods carries one today (`bane_of_arthropods.json`: Slowness,
    /// amplifier 3, duration `randomBetween(min, max)` seconds). Durations are in *seconds*,
    /// exactly as the JSON stores them; [`apply_mob_effect_on_hit`] does the ×20 and the
    /// rounding vanilla's `ApplyMobEffect.apply` does.
    ApplyMobEffectOnHit {
        condition: DamageCondition,
        min_duration_seconds: LevelBasedValue,
        max_duration_seconds: LevelBasedValue,
        min_amplifier: LevelBasedValue,
        max_amplifier: LevelBasedValue,
    },

    // --- modeled, not yet wired (see enum doc) ---
    ProjectileCount(LevelBasedValue),
    ProjectileSpread(LevelBasedValue),
    ProjectilePiercing(LevelBasedValue),
    TridentReturnAcceleration(LevelBasedValue),
    TridentSpinAttackStrength(LevelBasedValue),
    FishingTimeReduction(LevelBasedValue),
    FishingLuckBonus(LevelBasedValue),
    CrossbowChargeTime(LevelBasedValue),
    RepairWithXpMultiply(f32),
    /// Looting's `equipment_drops` chance-per-level bonus (gated on the attacker being a
    /// player), separate from `AttributeBonus` since vanilla's `equipment_drops` is a
    /// `TargetedConditionalEffect<EnchantmentValueEffect>` component, not an equip-time
    /// attribute modifier.
    EquipmentDropsChance(LevelBasedValue),
    /// Thorns: `post_attack` random-chance reflect. `chance = value.calculate(level)`, damage
    /// range and item-damage cost are vanilla constants (1..=5 damage, 2 durability) not
    /// level-scaled, so they're not modeled as a `LevelBasedValue`.
    ThornsChance(LevelBasedValue),
    /// Generic per-enchantment attribute-modifier bonus (vanilla's `minecraft:attributes`
    /// component, applied on equip rather than per-hit). The `&str` names the vanilla attribute
    /// id for documentation/test purposes only.
    AttributeBonus {
        /// Vanilla attribute id, without the `minecraft:` prefix.
        attribute: &'static str,
        amount: LevelBasedValue,
        operation: AttributeOperation,
    },
    /// An attribute modifier vanilla applies through `minecraft:location_changed` rather than
    /// the plain `minecraft:attributes` component, so it is *conditional* on where the entity
    /// is standing and must never be applied by the equip-time evaluator.
    ///
    /// Only Soul Speed carries these (`soul_speed.json`).
    LocationBasedAttributeBonus {
        attribute: &'static str,
        amount: LevelBasedValue,
        operation: AttributeOperation,
    },
    AmmoUseSetZero,
    BlockExperienceSetZero,
    ItemDamageRemoveBinomial(LevelBasedValue),
    PreventEquipmentDrop,
    PreventArmorChange,
    /// Frost Walker's `minecraft:location_changed` -> `minecraft:replace_disk`
    /// (`frost_walker.json`). Only the radius is level-scaled; height (1), offset (0,-1,0) and
    /// the placed state (`frosted_ice[age=0]`) are constants, so they live in
    /// [`apply_frost_walker`] rather than in the effect.
    ReplaceDiskRadius(LevelBasedValue),
    /// A `minecraft:location_changed` -> `minecraft:change_item_damage` gated by a
    /// `minecraft:random_chance` whose amount is a `minecraft:enchantment_level` value.
    ///
    /// Only Soul Speed carries one (`soul_speed.json`: 0.04 per level, 1 durability). Vanilla's
    /// `EnchantmentLevelProvider` multiplies the constant by the level, which is exactly what
    /// [`location_based_item_damage`] returns.
    LocationBasedItemDamage {
        chance_per_level: f32,
        amount: i32,
    },
    /// Lunge's `minecraft:post_piercing_attack` bundle (`lunge.json`): a forward impulse with
    /// coordinate scale (1,0,1), hunger exhaustion, and 1 point of item damage.
    PostPiercingLunge {
        magnitude: LevelBasedValue,
        exhaustion: LevelBasedValue,
        item_damage: i32,
    },
}

const BANE_DAMAGE: LevelBasedValue = LevelBasedValue::linear(2.5, 2.5);
const SMITE_DAMAGE: LevelBasedValue = LevelBasedValue::linear(2.5, 2.5);
const IMPALING_DAMAGE: LevelBasedValue = LevelBasedValue::linear(2.5, 2.5);
const SHARPNESS_DAMAGE: LevelBasedValue = LevelBasedValue::linear(1.0, 0.5);
const POWER_DAMAGE: LevelBasedValue = LevelBasedValue::linear(1.0, 0.5);

const PROTECTION_VALUE: LevelBasedValue = LevelBasedValue::linear(1.0, 1.0);
const FIRE_PROTECTION_VALUE: LevelBasedValue = LevelBasedValue::linear(2.0, 2.0);
const BLAST_PROTECTION_VALUE: LevelBasedValue = LevelBasedValue::linear(2.0, 2.0);
const PROJECTILE_PROTECTION_VALUE: LevelBasedValue = LevelBasedValue::linear(2.0, 2.0);
const FEATHER_FALLING_VALUE: LevelBasedValue = LevelBasedValue::linear(3.0, 3.0);

const KNOCKBACK_VALUE: LevelBasedValue = LevelBasedValue::linear(1.0, 1.0);
const PUNCH_VALUE: LevelBasedValue = LevelBasedValue::linear(1.0, 1.0);

const DENSITY_VALUE: LevelBasedValue = LevelBasedValue::linear(0.5, 0.5);
const BREACH_VALUE: LevelBasedValue = LevelBasedValue::linear(-0.15, -0.15);
const WIND_BURST_FALLBACK: LevelBasedValue = LevelBasedValue::linear(1.5, 0.35);
const WIND_BURST_VALUES: &[f32] = &[1.2, 1.75, 2.2];
const WIND_BURST_VALUE: LevelBasedValue = LevelBasedValue::Lookup {
    values: WIND_BURST_VALUES,
    fallback: &WIND_BURST_FALLBACK,
};

const FIRE_ASPECT_DURATION_SECONDS: LevelBasedValue = LevelBasedValue::linear(4.0, 4.0);

const BANE_SLOWNESS_MIN_DURATION: LevelBasedValue = LevelBasedValue::Constant(1.5);
const BANE_SLOWNESS_MAX_DURATION: LevelBasedValue = LevelBasedValue::linear(1.5, 0.5);
const BANE_SLOWNESS_AMPLIFIER: LevelBasedValue = LevelBasedValue::Constant(3.0);

const LOOTING_VALUE: LevelBasedValue = LevelBasedValue::linear(0.01, 0.01);
const MULTISHOT_COUNT: LevelBasedValue = LevelBasedValue::Constant(2.0);
const MULTISHOT_SPREAD: LevelBasedValue = LevelBasedValue::Constant(10.0);
const PIERCING_VALUE: LevelBasedValue = LevelBasedValue::linear(1.0, 1.0);
const LOYALTY_VALUE: LevelBasedValue = LevelBasedValue::linear(1.0, 1.0);
const LUCK_OF_THE_SEA_VALUE: LevelBasedValue = LevelBasedValue::linear(1.0, 1.0);
const LURE_VALUE: LevelBasedValue = LevelBasedValue::linear(5.0, 5.0);
const QUICK_CHARGE_VALUE: LevelBasedValue = LevelBasedValue::linear(-0.25, -0.25);
const RIPTIDE_SPIN_VALUE: LevelBasedValue = LevelBasedValue::linear(1.5, 0.75);
const MENDING_FACTOR: f32 = 2.0;
const THORNS_CHANCE: LevelBasedValue = LevelBasedValue::linear(0.15, 0.15);

const AQUA_AFFINITY_ATTR: LevelBasedValue = LevelBasedValue::linear(4.0, 4.0);
const DEPTH_STRIDER_ATTR: LevelBasedValue = LevelBasedValue::linear(0.333_333_34, 0.333_333_34);
const EFFICIENCY_ATTR: LevelBasedValue = LevelBasedValue::LevelsSquared { added: 1.0 };
const RESPIRATION_ATTR: LevelBasedValue = LevelBasedValue::linear(1.0, 1.0);
const SWIFT_SNEAK_ATTR: LevelBasedValue = LevelBasedValue::linear(0.15, 0.15);
const BLAST_PROTECTION_ATTR: LevelBasedValue = LevelBasedValue::linear(0.15, 0.15);
const SWEEPING_EDGE_NUMERATOR: LevelBasedValue = LevelBasedValue::linear(1.0, 1.0);
const SWEEPING_EDGE_DENOMINATOR: LevelBasedValue = LevelBasedValue::linear(2.0, 1.0);
const SWEEPING_EDGE_RATIO: LevelBasedValue = LevelBasedValue::Fraction {
    numerator: &SWEEPING_EDGE_NUMERATOR,
    denominator: &SWEEPING_EDGE_DENOMINATOR,
};
const SOUL_SPEED_SPEED_ATTR: LevelBasedValue = LevelBasedValue::linear(0.0405, 0.0105);
const SOUL_SPEED_EFFICIENCY_ATTR: LevelBasedValue = LevelBasedValue::Constant(1.0);
const FIRE_PROTECTION_BURNING_TIME_ATTR: LevelBasedValue = LevelBasedValue::linear(-0.15, -0.15);

/// `frost_walker.json`: `clamped(linear(3, 1), 0, 16)`.
const FROST_WALKER_RADIUS_INNER: LevelBasedValue = LevelBasedValue::linear(3.0, 1.0);
const FROST_WALKER_RADIUS: LevelBasedValue = LevelBasedValue::Clamped {
    value: &FROST_WALKER_RADIUS_INNER,
    min: 0.0,
    max: 16.0,
};
/// `frost_walker.json`: `"height": 1.0` and `"offset": [0, -1, 0]`, both level-independent.
const FROST_WALKER_HEIGHT: i32 = 1;
const FROST_WALKER_Y_OFFSET: i32 = -1;

/// `soul_speed.json`, second `location_changed` entry: `random_chance` of
/// `enchantment_level(0.04)`, `change_item_damage` amount 1.
const SOUL_SPEED_DAMAGE_CHANCE_PER_LEVEL: f32 = 0.04;

/// `lunge.json`: `linear(0.458, 0.458)` magnitude, `linear(4, 4)` exhaustion, 1 item damage.
const LUNGE_MAGNITUDE: LevelBasedValue = LevelBasedValue::linear(0.458, 0.458);
const LUNGE_EXHAUSTION: LevelBasedValue = LevelBasedValue::linear(4.0, 4.0);

const UNBREAKING_ARMOR_NUMERATOR: LevelBasedValue = LevelBasedValue::linear(2.0, 2.0);
const UNBREAKING_ARMOR_DENOMINATOR: LevelBasedValue = LevelBasedValue::linear(10.0, 5.0);
const UNBREAKING_ARMOR_CHANCE: LevelBasedValue = LevelBasedValue::Fraction {
    numerator: &UNBREAKING_ARMOR_NUMERATOR,
    denominator: &UNBREAKING_ARMOR_DENOMINATOR,
};
const UNBREAKING_OTHER_NUMERATOR: LevelBasedValue = LevelBasedValue::linear(1.0, 1.0);
const UNBREAKING_OTHER_DENOMINATOR: LevelBasedValue = LevelBasedValue::linear(2.0, 1.0);
const UNBREAKING_OTHER_CHANCE: LevelBasedValue = LevelBasedValue::Fraction {
    numerator: &UNBREAKING_OTHER_NUMERATOR,
    denominator: &UNBREAKING_OTHER_DENOMINATOR,
};

/// Single dispatch path: look up an enchantment's effects by `registry_key`.
///
/// Replaces per-call-site `if enchantment == Enchantment::X` chains with one match keyed on the
/// stable registry key (not `id`, which is a generated ordinal that shifts as Mojang adds
/// enchantments).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn effects_for(enchantment: &'static Enchantment) -> &'static [EnchantmentEffect] {
    use EnchantmentEffect as E;
    match enchantment.registry_key {
        "aqua_affinity" => &[E::AttributeBonus {
            attribute: "submerged_mining_speed",
            amount: AQUA_AFFINITY_ATTR,
            operation: AttributeOperation::AddMultipliedTotal,
        }],
        "bane_of_arthropods" => &[
            E::Damage(DamageCondition::SensitiveToBaneOfArthropods, BANE_DAMAGE),
            E::ApplyMobEffectOnHit {
                condition: DamageCondition::SensitiveToBaneOfArthropods,
                min_duration_seconds: BANE_SLOWNESS_MIN_DURATION,
                max_duration_seconds: BANE_SLOWNESS_MAX_DURATION,
                min_amplifier: BANE_SLOWNESS_AMPLIFIER,
                max_amplifier: BANE_SLOWNESS_AMPLIFIER,
            },
        ],
        "binding_curse" => &[E::PreventArmorChange],
        "blast_protection" => &[
            E::DamageProtection(ProtectionCondition::IsExplosion, BLAST_PROTECTION_VALUE),
            E::AttributeBonus {
                attribute: "explosion_knockback_resistance",
                amount: BLAST_PROTECTION_ATTR,
                operation: AttributeOperation::AddValue,
            },
        ],
        "breach" => &[E::ArmorEffectiveness(BREACH_VALUE)],
        // "channeling" has no numeric formula (boolean-gated lightning summon only) — falls
        // through to the wildcard arm below.
        "density" => &[E::SmashDamagePerFallenBlock(DENSITY_VALUE)],
        "depth_strider" => &[E::AttributeBonus {
            attribute: "water_movement_efficiency",
            amount: DEPTH_STRIDER_ATTR,
            operation: AttributeOperation::AddValue,
        }],
        "efficiency" => &[E::AttributeBonus {
            attribute: "mining_efficiency",
            amount: EFFICIENCY_ATTR,
            operation: AttributeOperation::AddValue,
        }],
        "feather_falling" => &[E::DamageProtection(
            ProtectionCondition::IsFall,
            FEATHER_FALLING_VALUE,
        )],
        "fire_aspect" => &[E::IgniteOnHit(FIRE_ASPECT_DURATION_SECONDS)],
        "fire_protection" => &[
            E::DamageProtection(ProtectionCondition::IsFire, FIRE_PROTECTION_VALUE),
            E::AttributeBonus {
                attribute: "burning_time",
                amount: FIRE_PROTECTION_BURNING_TIME_ATTR,
                operation: AttributeOperation::AddMultipliedBase,
            },
        ],
        // "flame" (constant 100-tick ignite, not level-scaled) and "fortune" (loot-table only,
        // no data component) fall through to the wildcard arm below.
        "frost_walker" => &[E::ReplaceDiskRadius(FROST_WALKER_RADIUS)],
        "lunge" => &[E::PostPiercingLunge {
            magnitude: LUNGE_MAGNITUDE,
            exhaustion: LUNGE_EXHAUSTION,
            item_damage: 1,
        }],
        "impaling" => &[E::Damage(
            DamageCondition::SensitiveToImpaling,
            IMPALING_DAMAGE,
        )],
        "infinity" => &[E::AmmoUseSetZero],
        "knockback" => &[E::Knockback(KnockbackCondition::Always, KNOCKBACK_VALUE)],
        "looting" => &[E::EquipmentDropsChance(LOOTING_VALUE)],
        "loyalty" => &[E::TridentReturnAcceleration(LOYALTY_VALUE)],
        "luck_of_the_sea" => &[E::FishingLuckBonus(LUCK_OF_THE_SEA_VALUE)],
        // "lunge" is post_piercing_attack only (spear-specific push/exhaustion); falls through
        // to the wildcard arm below.
        "lure" => &[E::FishingTimeReduction(LURE_VALUE)],
        "mending" => &[E::RepairWithXpMultiply(MENDING_FACTOR)],
        "multishot" => &[
            E::ProjectileCount(MULTISHOT_COUNT),
            E::ProjectileSpread(MULTISHOT_SPREAD),
        ],
        "piercing" => &[E::ProjectilePiercing(PIERCING_VALUE)],
        "power" => &[E::Damage(
            DamageCondition::DirectAttackerIsArrow,
            POWER_DAMAGE,
        )],
        "projectile_protection" => &[E::DamageProtection(
            ProtectionCondition::IsProjectile,
            PROJECTILE_PROTECTION_VALUE,
        )],
        "protection" => &[E::DamageProtection(
            ProtectionCondition::Always,
            PROTECTION_VALUE,
        )],
        "punch" => &[E::Knockback(
            KnockbackCondition::AttackerIsArrow,
            PUNCH_VALUE,
        )],
        "quick_charge" => &[E::CrossbowChargeTime(QUICK_CHARGE_VALUE)],
        "respiration" => &[E::AttributeBonus {
            attribute: "oxygen_bonus",
            amount: RESPIRATION_ATTR,
            operation: AttributeOperation::AddValue,
        }],
        "riptide" => &[E::TridentSpinAttackStrength(RIPTIDE_SPIN_VALUE)],
        "sharpness" => &[E::Damage(DamageCondition::Always, SHARPNESS_DAMAGE)],
        "silk_touch" => &[E::BlockExperienceSetZero],
        "smite" => &[E::Damage(DamageCondition::SensitiveToSmite, SMITE_DAMAGE)],
        "soul_speed" => &[
            E::LocationBasedAttributeBonus {
                attribute: "movement_speed",
                amount: SOUL_SPEED_SPEED_ATTR,
                operation: AttributeOperation::AddValue,
            },
            E::LocationBasedAttributeBonus {
                attribute: "movement_efficiency",
                amount: SOUL_SPEED_EFFICIENCY_ATTR,
                operation: AttributeOperation::AddValue,
            },
            E::LocationBasedItemDamage {
                chance_per_level: SOUL_SPEED_DAMAGE_CHANCE_PER_LEVEL,
                amount: 1,
            },
        ],
        "sweeping_edge" => &[E::AttributeBonus {
            attribute: "sweeping_damage_ratio",
            amount: SWEEPING_EDGE_RATIO,
            operation: AttributeOperation::AddValue,
        }],
        "swift_sneak" => &[E::AttributeBonus {
            attribute: "sneaking_speed",
            amount: SWIFT_SNEAK_ATTR,
            operation: AttributeOperation::AddValue,
        }],
        "thorns" => &[E::ThornsChance(THORNS_CHANCE)],
        "unbreaking" => &[
            E::ItemDamageRemoveBinomial(UNBREAKING_ARMOR_CHANCE),
            E::ItemDamageRemoveBinomial(UNBREAKING_OTHER_CHANCE),
        ],
        "vanishing_curse" => &[E::PreventEquipmentDrop],
        "wind_burst" => &[E::PostAttackKnockbackMultiplier(WIND_BURST_VALUE)],
        _ => &[],
    }
}

/// Sums every `Damage` effect of `enchantment` that applies to `target_type`.
///
/// Convenience for the common case used by melee-attack call sites (player/mob damage
/// calculation), mirroring `EnchantmentHelper.modifyDamage` iterating every equipped/held
/// enchantment.
#[must_use]
pub fn damage_bonus(
    enchantment: &'static Enchantment,
    level: i32,
    target_type: &'static EntityType,
) -> f32 {
    effects_for(enchantment)
        .iter()
        .map(|effect| match effect {
            EnchantmentEffect::Damage(condition, value) if condition.applies(target_type) => {
                value.calculate(level)
            }
            _ => 0.0,
        })
        .sum()
}

/// Resolves an [`EnchantmentEffect::ApplyMobEffectOnHit`] into `(duration_ticks, amplifier)`.
///
/// Mirrors `ApplyMobEffect.apply` (`effects/ApplyMobEffect.java:36-49`): both duration and
/// amplifier are `Mth.randomBetween(min, max)`, duration is converted seconds -> ticks by ×20
/// and `Math.round`ed, and the amplifier is `Math.round`ed then floored at 0. `duration_roll`
/// and `amplifier_roll` stand in for `random.nextFloat()` and must be in `[0, 1)`.
#[must_use]
pub fn apply_mob_effect_on_hit(
    min_duration_seconds: LevelBasedValue,
    max_duration_seconds: LevelBasedValue,
    min_amplifier: LevelBasedValue,
    max_amplifier: LevelBasedValue,
    level: i32,
    duration_roll: f32,
    amplifier_roll: f32,
) -> (i32, u8) {
    let min_d = min_duration_seconds.calculate(level);
    let max_d = max_duration_seconds.calculate(level);
    let seconds = duration_roll.mul_add(max_d - min_d, min_d);
    let ticks = (seconds * 20.0).round() as i32;

    let min_a = min_amplifier.calculate(level);
    let max_a = max_amplifier.calculate(level);
    let amplifier = amplifier_roll.mul_add(max_a - min_a, min_a).round();
    let amplifier = amplifier.clamp(0.0, f32::from(u8::MAX)) as u8;

    (ticks.max(0), amplifier)
}

/// Frost Walker's `minecraft:location_changed` -> `minecraft:replace_disk` effect.
///
/// Ports `ReplaceDisk.apply`
/// (`net/minecraft/world/item/enchantment/effects/ReplaceDisk.java:43-55`) with the constants
/// `frost_walker.json` fixes for this enchantment: offset `(0, -1, 0)`, height `1` (so the y
/// loop collapses to the single layer below the entity, because vanilla's upper bound is
/// `min(height - 1, 0)`), and the placed state `frosted_ice[age=0]`.
///
/// The `block_predicate` is the `all_of` from the same JSON: air directly above, water block,
/// water fluid, and unobstructed (`UnobstructedPredicate.test` -> `isUnobstructed(null, ...)`,
/// i.e. no entity intersects the full block cube).
///
/// `source_entity` is the walker: vanilla passes it as the game-event context for the
/// `trigger_game_event: minecraft:block_place` the JSON fires per placed block
/// (`ReplaceDisk.java:52-54`).
///
/// The caller is responsible for the effect's *requirements* (`is_on_ground`, no vehicle) and
/// for having a nonzero Frost Walker level on the boots.
pub async fn apply_frost_walker(
    world: &Arc<World>,
    position: Vector3<f64>,
    level: i32,
    source_entity: Option<Arc<dyn crate::entity::EntityBase>>,
) {
    if level <= 0 {
        return;
    }
    let radius = FROST_WALKER_RADIUS.calculate(level) as i32;
    if radius <= 0 {
        return;
    }
    let center = BlockPos::new(
        position.x.floor() as i32,
        position.y.floor() as i32 + FROST_WALKER_Y_OFFSET,
        position.z.floor() as i32,
    );
    // `Math.min(height - 1, 0)` — with height 1 this is 0, so exactly one layer.
    let max_dy = (FROST_WALKER_HEIGHT - 1).min(0);
    let radius_sq = f64::from(radius * radius);
    let frosted_ice = Block::FROSTED_ICE.default_state.id;

    for dy in 0..=max_dy {
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let pos = BlockPos::new(center.0.x + dx, center.0.y + dy, center.0.z + dz);
                // `BlockPos.distToCenterSqr(x, pos.getY() + 0.5, z)`: the y term cancels, so
                // this is the horizontal distance from the block centre to the entity.
                let ox = f64::from(pos.0.x) + 0.5 - position.x;
                let oz = f64::from(pos.0.z) + 0.5 - position.z;
                if ox * ox + oz * oz >= radius_sq {
                    continue;
                }
                if !world.get_block(&pos.up()).has_tag(&BlockTag::MINECRAFT_AIR) {
                    continue;
                }
                if world.get_block(&pos) != &Block::WATER {
                    continue;
                }
                if world.get_fluid(&pos) != &Fluid::WATER {
                    continue;
                }
                if !world
                    .get_entities_at_box(&BoundingBox::from_block(&pos))
                    .is_empty()
                {
                    continue;
                }
                let replaced = world
                    .set_block_state(&pos, frosted_ice, BlockFlags::NOTIFY_ALL)
                    .await;
                if replaced == frosted_ice {
                    continue;
                }
                crate::world::game_event::emit_game_event(
                    world,
                    pumpkin_data::game_event::GameEvent::BlockPlace,
                    Vector3::new(
                        f64::from(pos.0.x) + 0.5,
                        f64::from(pos.0.y) + 0.5,
                        f64::from(pos.0.z) + 0.5,
                    ),
                    source_entity.clone().map_or_else(
                        crate::world::game_event::GameEventContext::none,
                        crate::world::game_event::GameEventContext::of_entity,
                    ),
                )
                .await;
            }
        }
    }
}

/// The `minecraft:location_changed` -> `change_item_damage` roll an enchantment contributes,
/// as `(chance, damage_amount)`.
///
/// Only Soul Speed has one (`soul_speed.json`, second `location_changed` entry): a
/// `random_chance` of `enchantment_level(0.04)` — so `0.04 * level`, vanilla's
/// `EnchantmentLevelProvider` — damaging the boots by 1. Note this is a `location_changed`
/// effect, *not* part of the `minecraft:tick` component, which is only particles and sound.
#[must_use]
pub fn location_based_item_damage(
    enchantment: &'static Enchantment,
    level: i32,
) -> Option<(f32, i32)> {
    if level <= 0 {
        return None;
    }
    effects_for(enchantment)
        .iter()
        .find_map(|effect| match effect {
            EnchantmentEffect::LocationBasedItemDamage {
                chance_per_level,
                amount,
            } => Some((chance_per_level * level as f32, *amount)),
            _ => None,
        })
}

/// Lunge's `minecraft:post_piercing_attack` payload, as
/// `(impulse_magnitude, exhaustion, item_damage)` (`lunge.json`).
///
/// The impulse direction is the attacker's look vector with coordinate scale `(1, 0, 1)`, i.e.
/// horizontal only; the caller applies it, since the spear piercing-attack path lives outside
/// this module.
#[must_use]
pub fn post_piercing_lunge(
    enchantment: &'static Enchantment,
    level: i32,
) -> Option<(f32, f32, i32)> {
    if level <= 0 {
        return None;
    }
    effects_for(enchantment)
        .iter()
        .find_map(|effect| match effect {
            EnchantmentEffect::PostPiercingLunge {
                magnitude,
                exhaustion,
                item_damage,
            } => Some((
                magnitude.calculate(level),
                exhaustion.calculate(level),
                *item_damage,
            )),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enchanted(
        item: &'static pumpkin_data::item::Item,
        ench: &'static Enchantment,
        level: i32,
    ) -> ItemStack {
        let mut stack = ItemStack::new(1, item);
        stack.enchant(ench, level);
        stack
    }

    #[test]
    fn equip_time_attribute_formulas_match_vanilla_json() {
        // respiration.json: linear(1.0, 1.0), add_value.
        assert_eq!(RESPIRATION_ATTR.calculate(1), 1.0);
        assert_eq!(RESPIRATION_ATTR.calculate(3), 3.0);
        // swift_sneak.json: linear(0.15, 0.15), add_value.
        assert!((SWIFT_SNEAK_ATTR.calculate(1) - 0.15).abs() < 1e-6);
        assert!((SWIFT_SNEAK_ATTR.calculate(3) - 0.45).abs() < 1e-6);
        // blast_protection.json: linear(0.15, 0.15) on explosion_knockback_resistance.
        assert!((BLAST_PROTECTION_ATTR.calculate(4) - 0.60).abs() < 1e-6);
        // depth_strider.json: linear(0.33333334, 0.33333334).
        assert!((DEPTH_STRIDER_ATTR.calculate(3) - 1.0).abs() < 1e-6);
        // fire_protection.json: linear(-0.15, -0.15) on burning_time, add_multiplied_base.
        assert!((FIRE_PROTECTION_BURNING_TIME_ATTR.calculate(1) + 0.15).abs() < 1e-6);
        assert!((FIRE_PROTECTION_BURNING_TIME_ATTR.calculate(4) + 0.60).abs() < 1e-6);
        // soul_speed.json: linear(0.0405, 0.0105) speed, constant 1.0 efficiency.
        assert!((SOUL_SPEED_SPEED_ATTR.calculate(1) - 0.0405).abs() < 1e-6);
        assert!((SOUL_SPEED_SPEED_ATTR.calculate(3) - 0.0615).abs() < 1e-6);
        assert_eq!(SOUL_SPEED_EFFICIENCY_ATTR.calculate(3), 1.0);
    }

    #[test]
    fn attribute_operations_match_the_json_operation_field() {
        let ops: Vec<(&str, AttributeOperation)> = [
            "aqua_affinity",
            "blast_protection",
            "depth_strider",
            "efficiency",
            "fire_protection",
            "respiration",
            "sweeping_edge",
            "swift_sneak",
        ]
        .iter()
        .flat_map(|key| {
            let enchantment = Enchantment::ALL
                .iter()
                .find(|e| e.registry_key == *key)
                .expect("enchantment exists");
            effects_for(enchantment)
                .iter()
                .filter_map(move |effect| match effect {
                    EnchantmentEffect::AttributeBonus {
                        attribute,
                        operation,
                        ..
                    } => Some((*attribute, *operation)),
                    _ => None,
                })
        })
        .collect();

        assert!(ops.contains(&(
            "submerged_mining_speed",
            AttributeOperation::AddMultipliedTotal
        )));
        assert!(ops.contains(&("burning_time", AttributeOperation::AddMultipliedBase)));
        assert!(ops.contains(&(
            "explosion_knockback_resistance",
            AttributeOperation::AddValue
        )));
        assert!(ops.contains(&("water_movement_efficiency", AttributeOperation::AddValue)));
        assert!(ops.contains(&("mining_efficiency", AttributeOperation::AddValue)));
        assert!(ops.contains(&("oxygen_bonus", AttributeOperation::AddValue)));
        assert!(ops.contains(&("sneaking_speed", AttributeOperation::AddValue)));
        assert!(ops.contains(&("sweeping_damage_ratio", AttributeOperation::AddValue)));
    }

    #[test]
    fn modifier_ids_are_scoped_per_slot_so_four_armour_pieces_stack() {
        // EnchantmentAttributeEffect.idForSlot: id + "/" + slot.getSerializedName().
        let ids: Vec<String> = [
            EquipmentSlot::HEAD,
            EquipmentSlot::CHEST,
            EquipmentSlot::LEGS,
            EquipmentSlot::FEET,
        ]
        .iter()
        .map(|slot| modifier_id_for_slot(&Enchantment::BLAST_PROTECTION, slot))
        .collect();

        assert_eq!(ids[0], "minecraft:enchantment.blast_protection/head");
        assert_eq!(ids[3], "minecraft:enchantment.blast_protection/feet");
        let mut unique = ids;
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            4,
            "slot suffix must keep the four ids distinct"
        );
    }

    #[test]
    fn evaluator_emits_one_modifier_per_matching_slot() {
        let boots = enchanted(
            &pumpkin_data::item::Item::DIAMOND_BOOTS,
            &Enchantment::DEPTH_STRIDER,
            2,
        );
        let applied = attribute_modifiers_for_slot(&boots, &EquipmentSlot::FEET);
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0.id, Attributes::WATER_MOVEMENT_EFFICIENCY.id);
        assert!((applied[0].1.amount - 0.666_666_68).abs() < 1e-6);
        assert_eq!(applied[0].1.id, "minecraft:enchantment.depth_strider/feet");
    }

    #[test]
    fn evaluator_respects_the_enchantment_slot_group() {
        // depth_strider.json declares slots: ["feet"], so the same boots held in the main hand
        // must contribute nothing (Enchantment.matchingSlot).
        let boots = enchanted(
            &pumpkin_data::item::Item::DIAMOND_BOOTS,
            &Enchantment::DEPTH_STRIDER,
            3,
        );
        assert!(attribute_modifiers_for_slot(&boots, &EquipmentSlot::MAIN_HAND).is_empty());
        assert!(attribute_modifiers_for_slot(&boots, &EquipmentSlot::HEAD).is_empty());

        // blast_protection.json declares slots: ["armor"], which matches all four pieces.
        let helmet = enchanted(
            &pumpkin_data::item::Item::DIAMOND_HELMET,
            &Enchantment::BLAST_PROTECTION,
            2,
        );
        assert_eq!(
            attribute_modifiers_for_slot(&helmet, &EquipmentSlot::HEAD).len(),
            1
        );
        assert!(attribute_modifiers_for_slot(&helmet, &EquipmentSlot::OFF_HAND).is_empty());
    }

    #[test]
    fn fire_protection_contributes_a_negative_multiply_base_burning_time_modifier() {
        let chest = enchanted(
            &pumpkin_data::item::Item::DIAMOND_CHESTPLATE,
            &Enchantment::FIRE_PROTECTION,
            4,
        );
        let applied = attribute_modifiers_for_slot(&chest, &EquipmentSlot::CHEST);
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0.id, Attributes::BURNING_TIME.id);
        assert!((applied[0].1.amount + 0.60).abs() < 1e-6);
        assert!(matches!(
            applied[0].1.operation,
            ModifierOperation::MultiplyBase
        ));
    }

    #[test]
    fn soul_speed_is_location_based_and_never_applied_on_equip() {
        let boots = enchanted(
            &pumpkin_data::item::Item::DIAMOND_BOOTS,
            &Enchantment::SOUL_SPEED,
            3,
        );
        assert!(
            attribute_modifiers_for_slot(&boots, &EquipmentSlot::FEET).is_empty(),
            "soul speed is a location_changed effect; applying it on equip is a permanent speed buff"
        );

        let conditional = location_based_attribute_modifiers_for_slot(&boots, &EquipmentSlot::FEET);
        assert_eq!(conditional.len(), 2);
        let speed = conditional
            .iter()
            .find(|(attribute, _)| attribute.id == Attributes::MOVEMENT_SPEED.id)
            .expect("movement_speed modifier");
        assert!((speed.1.amount - 0.0615).abs() < 1e-6);
        assert_eq!(speed.1.id, "minecraft:enchantment.soul_speed/feet");
    }

    #[test]
    fn unenchanted_and_zero_level_stacks_contribute_nothing() {
        let plain = ItemStack::new(1, &pumpkin_data::item::Item::DIAMOND_BOOTS);
        assert!(attribute_modifiers_for_slot(&plain, &EquipmentSlot::FEET).is_empty());
        let zero = enchanted(
            &pumpkin_data::item::Item::DIAMOND_BOOTS,
            &Enchantment::DEPTH_STRIDER,
            0,
        );
        assert!(attribute_modifiers_for_slot(&zero, &EquipmentSlot::FEET).is_empty());
    }

    #[test]
    fn every_attribute_key_used_by_an_effect_resolves() {
        for enchantment in Enchantment::ALL {
            for effect in effects_for(enchantment) {
                match effect {
                    EnchantmentEffect::AttributeBonus { attribute, .. }
                    | EnchantmentEffect::LocationBasedAttributeBonus { attribute, .. } => {
                        assert!(
                            attribute_by_key(attribute).is_some(),
                            "unmapped attribute id {attribute} on {}",
                            enchantment.registry_key
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    /// `frost_walker.json`: `clamped(linear(3, 1), 0, 16)`, truncated to an int radius by
    /// `ReplaceDisk.apply` (`ReplaceDisk.java:46`).
    #[test]
    fn frost_walker_radius_matches_vanilla() {
        assert_eq!(FROST_WALKER_RADIUS.calculate(1), 3.0);
        assert_eq!(FROST_WALKER_RADIUS.calculate(2), 4.0);
        // Clamp bites well above max_level=2, but NBT/command levels can reach it.
        assert_eq!(FROST_WALKER_RADIUS.calculate(50), 16.0);
        // height 1 collapses vanilla's `min(height - 1, 0)` y range to a single layer.
        assert_eq!((FROST_WALKER_HEIGHT - 1).min(0), 0);
        assert_eq!(FROST_WALKER_Y_OFFSET, -1);
    }

    /// `soul_speed.json`, second `location_changed` entry: `enchantment_level(0.04)` chance of
    /// 1 point of item damage. This is the boot-durability drain, not the `minecraft:tick`
    /// component (particles and sound only).
    #[test]
    fn soul_speed_item_damage_scales_with_level() {
        let (chance, amount) =
            location_based_item_damage(&Enchantment::SOUL_SPEED, 3).expect("soul speed drain");
        assert!((chance - 0.12).abs() < 1e-6);
        assert_eq!(amount, 1);
        assert!(location_based_item_damage(&Enchantment::SOUL_SPEED, 0).is_none());
        assert!(location_based_item_damage(&Enchantment::DEPTH_STRIDER, 3).is_none());
    }

    /// `lunge.json`: impulse `linear(0.458, 0.458)`, exhaustion `linear(4, 4)`, 1 item damage.
    #[test]
    fn lunge_values_match_vanilla() {
        let (magnitude, exhaustion, damage) =
            post_piercing_lunge(&Enchantment::LUNGE, 2).expect("lunge effect");
        assert!((magnitude - 0.916).abs() < 1e-6);
        assert!((exhaustion - 8.0).abs() < 1e-6);
        assert_eq!(damage, 1);
        assert!(post_piercing_lunge(&Enchantment::SHARPNESS, 2).is_none());
    }

    #[test]
    fn linear_matches_vanilla_formula() {
        // base + per_level_above_first * (level - 1)
        assert_eq!(SHARPNESS_DAMAGE.calculate(1), 1.0);
        assert_eq!(SHARPNESS_DAMAGE.calculate(5), 3.0);
        assert_eq!(BANE_DAMAGE.calculate(1), 2.5);
        assert_eq!(BANE_DAMAGE.calculate(5), 12.5);
    }

    #[test]
    fn lookup_uses_table_within_range_and_fallback_beyond_it() {
        // wind_burst.json: values [1.2, 1.75, 2.2], fallback linear(1.5, 0.35).
        assert_eq!(WIND_BURST_VALUE.calculate(1), 1.2);
        assert_eq!(WIND_BURST_VALUE.calculate(2), 1.75);
        assert_eq!(WIND_BURST_VALUE.calculate(3), 2.2);
        // Levels above max_level=3 (reachable via NBT/commands) must fall back, not clamp to
        // the last table entry.
        assert_eq!(WIND_BURST_VALUE.calculate(4), 1.5 + 0.35 * 3.0);
    }

    #[test]
    fn density_matches_old_free_function_for_all_levels() {
        // Behavior-preserving-refactor check against the pre-framework `density_extra_damage`.
        for level in 1..=5 {
            let fall_distance = 4.0f32;
            let old = 0.5 * f64::from(level as u32) * f64::from(fall_distance);
            let new = f64::from(DENSITY_VALUE.calculate(level) * fall_distance);
            assert!(
                (old - new).abs() < 1e-9,
                "level {level}: old={old} new={new}"
            );
        }
    }

    #[test]
    fn breach_matches_old_free_function_for_all_levels() {
        for level in 1..=4 {
            let base = 0.8f32;
            let old = (base - 0.15 * level as f32).clamp(0.0, 1.0);
            let new = (base + BREACH_VALUE.calculate(level)).clamp(0.0, 1.0);
            assert!(
                (old - new).abs() < 1e-6,
                "level {level}: old={old} new={new}"
            );
        }
    }

    #[test]
    fn levels_squared_matches_efficiency_json() {
        // efficiency.json: levels_squared, added 1.0
        assert_eq!(EFFICIENCY_ATTR.calculate(1), 2.0);
        assert_eq!(EFFICIENCY_ATTR.calculate(5), 26.0);
    }

    #[test]
    fn fraction_matches_sweeping_edge_json() {
        // sweeping_edge.json: numerator linear(1,1), denominator linear(2,1)
        assert!((SWEEPING_EDGE_RATIO.calculate(1) - 0.5).abs() < 1e-6);
        assert!((SWEEPING_EDGE_RATIO.calculate(2) - (2.0 / 3.0)).abs() < 1e-6);
        assert!((SWEEPING_EDGE_RATIO.calculate(3) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn fraction_returns_zero_for_zero_denominator() {
        const ZERO_DENOMINATOR: LevelBasedValue = LevelBasedValue::Constant(0.0);
        const NUMERATOR: LevelBasedValue = LevelBasedValue::Constant(5.0);
        const FRACTION: LevelBasedValue = LevelBasedValue::Fraction {
            numerator: &NUMERATOR,
            denominator: &ZERO_DENOMINATOR,
        };
        assert_eq!(FRACTION.calculate(1), 0.0);
    }

    #[test]
    fn clamped_bounds_the_inner_value() {
        const INNER: LevelBasedValue = LevelBasedValue::linear(-1.0, -1.0);
        const CLAMPED: LevelBasedValue = LevelBasedValue::Clamped {
            value: &INNER,
            min: 0.0,
            max: 10.0,
        };
        assert_eq!(CLAMPED.calculate(5), 0.0);
    }

    #[test]
    fn feather_falling_is_three_per_level_not_four() {
        // feather_falling.json: linear(base=3.0, per_level_above_first=3.0). Pumpkin's
        // pre-framework `living.rs` hardcoded `level * 4`, which over-reduces fall damage.
        assert_eq!(FEATHER_FALLING_VALUE.calculate(1), 3.0);
        assert_eq!(FEATHER_FALLING_VALUE.calculate(4), 12.0);
    }

    #[test]
    fn protection_family_matches_json_constants() {
        assert_eq!(PROTECTION_VALUE.calculate(4), 4.0);
        assert_eq!(FIRE_PROTECTION_VALUE.calculate(4), 8.0);
        assert_eq!(BLAST_PROTECTION_VALUE.calculate(4), 8.0);
        assert_eq!(PROJECTILE_PROTECTION_VALUE.calculate(4), 8.0);
    }

    #[test]
    fn knockback_and_punch_share_the_same_linear_formula() {
        assert_eq!(KNOCKBACK_VALUE.calculate(2), 2.0);
        assert_eq!(PUNCH_VALUE.calculate(2), 2.0);
    }

    #[test]
    fn sharpness_and_power_share_the_same_linear_formula() {
        assert_eq!(SHARPNESS_DAMAGE.calculate(5), 3.0);
        assert_eq!(POWER_DAMAGE.calculate(5), 3.0);
    }

    #[test]
    fn fire_aspect_duration_in_ticks_matches_existing_call_sites() {
        // fire_aspect.json linear(4,4) seconds * 20 ticks/sec == the pre-framework
        // `level * 80` used in player.rs / mob/mod.rs.
        for level in 1..=2 {
            let ticks = FIRE_ASPECT_DURATION_SECONDS.calculate(level) * 20.0;
            assert_eq!(ticks, level as f32 * 80.0);
        }
    }

    #[test]
    fn unbreaking_fractions_match_json() {
        // Armor: numerator linear(2,2), denominator linear(10,5) -> level1: 2/10=0.2
        assert!((UNBREAKING_ARMOR_CHANCE.calculate(1) - 0.2).abs() < 1e-6);
        // Other tools: numerator linear(1,1), denominator linear(2,1) -> level1: 1/2=0.5
        assert!((UNBREAKING_OTHER_CHANCE.calculate(1) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn bane_of_arthropods_slowness_matches_json() {
        // bane_of_arthropods.json post_attack: slowness, min/max_amplifier 3.0,
        // min_duration 1.5s, max_duration linear(1.5, 0.5).
        let roll = |level, duration_roll| {
            apply_mob_effect_on_hit(
                BANE_SLOWNESS_MIN_DURATION,
                BANE_SLOWNESS_MAX_DURATION,
                BANE_SLOWNESS_AMPLIFIER,
                BANE_SLOWNESS_AMPLIFIER,
                level,
                duration_roll,
                0.0,
            )
        };
        // Level 1: min == max == 1.5s, so the roll cannot matter -> 30 ticks.
        assert_eq!(roll(1, 0.0), (30, 3));
        assert_eq!(roll(1, 0.999), (30, 3));
        // Level 5: max duration is 1.5 + 0.5*4 = 3.5s -> 30..=70 ticks.
        assert_eq!(roll(5, 0.0), (30, 3));
        assert_eq!(roll(5, 1.0), (70, 3));
        assert_eq!(roll(5, 0.5), (50, 3));
    }

    #[test]
    fn bane_of_arthropods_effect_is_gated_on_the_same_tag_as_its_damage() {
        let effects = effects_for(&Enchantment::BANE_OF_ARTHROPODS);
        let gated = effects.iter().any(|effect| {
            matches!(
                effect,
                EnchantmentEffect::ApplyMobEffectOnHit {
                    condition: DamageCondition::SensitiveToBaneOfArthropods,
                    ..
                }
            )
        });
        assert!(gated, "bane must carry a tag-gated post_attack effect");
        let spider = &pumpkin_data::entity::EntityType::SPIDER;
        let cow = &pumpkin_data::entity::EntityType::COW;
        assert!(DamageCondition::SensitiveToBaneOfArthropods.applies(spider));
        assert!(!DamageCondition::SensitiveToBaneOfArthropods.applies(cow));
    }

    #[test]
    fn effects_for_dispatches_every_enchantment_without_panicking() {
        for enchantment in Enchantment::all() {
            let _ = effects_for(enchantment);
        }
    }

    #[test]
    fn damage_bonus_gates_on_target_tag() {
        let skeleton = &pumpkin_data::entity::EntityType::SKELETON;
        let cow = &pumpkin_data::entity::EntityType::COW;
        assert_eq!(damage_bonus(&Enchantment::SMITE, 1, skeleton), 2.5);
        assert_eq!(damage_bonus(&Enchantment::SMITE, 1, cow), 0.0);
        assert_eq!(damage_bonus(&Enchantment::SHARPNESS, 1, cow), 1.0);
    }

    #[test]
    fn power_never_contributes_at_melee_call_sites() {
        // Power's damage requirement gates on the *direct attacker* being tagged
        // `#minecraft:arrows` (power.json), not on the victim's type. A melee call site (the
        // only kind `damage_bonus`/`DamageCondition::applies` can evaluate today) must never
        // apply it, regardless of level or victim — otherwise punching someone while holding a
        // Power bow silently adds bonus melee damage that vanilla never grants.
        let cow = &pumpkin_data::entity::EntityType::COW;
        assert_eq!(damage_bonus(&Enchantment::POWER, 5, cow), 0.0);
    }
}
