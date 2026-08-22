//! Mob spawn equipment system.
//!
//! This module handles the automatic equipping of mobs when they spawn, matching
//! vanilla Minecraft's `populateDefaultEquipmentSlots` and
//! `populateDefaultEquipmentEnchantments` behaviour. It features:
//!
//! - A data-driven `EQUIPMENT_REGISTRY` mapping 13 mob types to their weapon/armor
//!   configurations.
//! - Exact vanilla `RegionalDifficulty` computation (game time, chunk inhabited time,
//!   moon phase).
//! - Spawn enchantments through a faithful port of vanilla's
//!   `EnchantmentsByCostWithDifficulty` provider for
//!   `VanillaEnchantmentProviders.MOB_SPAWN_EQUIPMENT`: the pool is the generated
//!   `#minecraft:on_mob_spawn_equipment` tag, candidates are filtered with
//!   `Enchantment.isPrimaryItem`, and levels come from the
//!   `EnchantmentHelper.selectEnchantment` cost algorithm (difficulty-scaled cost,
//!   enchantability bonus, ±15% span, weighted picks with cost halving).
//! - Per-slot drop chances with looting bonus on death.
//!
//! Mobs not listed in the registry spawn with no equipment, matching vanilla
//! (not all mob types have equipment definitions).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;

use pumpkin_data::Enchantment;
use pumpkin_data::data_component_impl::{EnchantableImpl, EquipmentSlot};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::Enchantment as EnchantmentTag;
use pumpkin_util::difficulty::Difficulty;
use pumpkin_util::math::vector3::Vector3;
use rand::Rng;
use rand::RngExt;

use crate::entity::EntityBase;

// ══════════════════════════════════════════════════════════════════
// Global constants extracted from vanilla Minecraft 26.2
// Sources: Mob.java, DifficultyInstance.java, DropChances.java,
// EnchantmentsByCostWithDifficulty.java
// ══════════════════════════════════════════════════════════════════

/// Base chance (before `specialMultiplier` scaling) that a mob will wear armor.
/// From vanilla `Mob.MAX_WEARING_ARMOR_CHANCE`.
pub const WEARING_ARMOR_CHANCE: f32 = 0.15;

/// Chance per attempt to promote the armor tier to the next material.
/// From vanilla `Mob.WEARING_ARMOR_UPGRADE_MATERIAL_CHANCE`.
pub const ARMOR_UPGRADE_MATERIAL_CHANCE: f32 = 0.1087;

/// Maximum number of upgrade attempts for armor tier selection.
/// From vanilla `Mob.WEARING_ARMOR_UPGRADE_MATERIAL_ATTEMPTS`.
pub const ARMOR_UPGRADE_MATERIAL_ATTEMPTS: f32 = 3.0;

/// Default per-slot drop chance for equipment on mob death.
/// From vanilla `Mob.DEFAULT_EQUIPMENT_DROP_CHANCE`.
pub const DEFAULT_EQUIPMENT_DROP_CHANCE: f32 = 0.085;

/// Base chance (before `specialMultiplier`) for weapon enchantments at spawn.
/// From vanilla `Mob.MAX_ENCHANTED_WEAPON_CHANCE`.
pub const WEAPON_ENCHANT_CHANCE: f32 = 0.25;

/// Base chance (before `specialMultiplier`) for armor enchantments at spawn.
/// From vanilla `Mob.MAX_ENCHANTED_ARMOR_CHANCE`.
pub const ARMOR_ENCHANT_CHANCE: f32 = 0.5;

/// Minimum enchantment cost for mob spawn equipment.
/// From vanilla `mob_spawn_equipment.json`.
pub const MOB_SPAWN_ENCHANT_MIN_COST: i32 = 5;

/// Cost span added to the minimum, scaled by `specialMultiplier`.
/// From vanilla `mob_spawn_equipment.json`.
pub const MOB_SPAWN_ENCHANT_COST_SPAN: i32 = 17;

// ══════════════════════════════════════════════════════════════════
// Armor tiers — exact match to vanilla Mob.getEquipmentForSlot()
// Vanilla approximation: armor type selection (base 0-2 + 3 upgrade
// attempts at 10.87% per vanilla Mob.populateDefaultEquipmentSlots) and
// partial armor chance (0.1 on Hard / 0.25 otherwise).
// Type 0=Leather, 1=Copper, 2=Gold, 3=Chainmail, 4=Iron, 5=Diamond
// Slot order: HEAD, CHEST, LEGS, FEET
// ══════════════════════════════════════════════════════════════════

static ARMOR_TIERS: LazyLock<[[&'static Item; 4]; 6]> = LazyLock::new(|| {
    [
        [
            &Item::LEATHER_HELMET,
            &Item::LEATHER_CHESTPLATE,
            &Item::LEATHER_LEGGINGS,
            &Item::LEATHER_BOOTS,
        ],
        [
            &Item::COPPER_HELMET,
            &Item::COPPER_CHESTPLATE,
            &Item::COPPER_LEGGINGS,
            &Item::COPPER_BOOTS,
        ],
        [
            &Item::GOLDEN_HELMET,
            &Item::GOLDEN_CHESTPLATE,
            &Item::GOLDEN_LEGGINGS,
            &Item::GOLDEN_BOOTS,
        ],
        [
            &Item::CHAINMAIL_HELMET,
            &Item::CHAINMAIL_CHESTPLATE,
            &Item::CHAINMAIL_LEGGINGS,
            &Item::CHAINMAIL_BOOTS,
        ],
        [
            &Item::IRON_HELMET,
            &Item::IRON_CHESTPLATE,
            &Item::IRON_LEGGINGS,
            &Item::IRON_BOOTS,
        ],
        [
            &Item::DIAMOND_HELMET,
            &Item::DIAMOND_CHESTPLATE,
            &Item::DIAMOND_LEGGINGS,
            &Item::DIAMOND_BOOTS,
        ],
    ]
});

static ARMOR_POPULATION_ORDER: [EquipmentSlot; 4] = [
    EquipmentSlot::HEAD,
    EquipmentSlot::CHEST,
    EquipmentSlot::LEGS,
    EquipmentSlot::FEET,
];

// ══════════════════════════════════════════════════════════════════
// Enchantment system — faithful port of vanilla's
// `EnchantmentsByCostWithDifficulty` provider for
// `VanillaEnchantmentProviders.MOB_SPAWN_EQUIPMENT`
//
// The candidate pool is the generated `#minecraft:on_mob_spawn_equipment`
// tag (= `#minecraft:non_treasure`), not a hand-written list. Selection is
// `EnchantmentHelper.selectEnchantment`: difficulty-scaled base cost,
// enchantability bonus, ±15% random span, primary-item filtering, weighted
// picks by `Enchantment.weight`, exclusive-set compatibility filtering and
// cost halving.
// ══════════════════════════════════════════════════════════════════

/// The `#minecraft:on_mob_spawn_equipment` enchantment pool in tag order.
///
/// Resolves the generated tag (`VanillaEnchantmentProviders.java:24`: the
/// provider holds `enchantments.getOrThrow(EnchantmentTags.ON_MOB_SPAWN_EQUIPMENT)`)
/// into `&'static Enchantment` references. Order matters: it feeds both the
/// availability scan and vanilla `WeightedRandom` tie-breaking.
static MOB_SPAWN_EQUIPMENT_POOL: LazyLock<Vec<&'static Enchantment>> = LazyLock::new(|| {
    EnchantmentTag::MINECRAFT_ON_MOB_SPAWN_EQUIPMENT
        .1
        .iter()
        .filter_map(|id| {
            Enchantment::ALL
                .iter()
                .copied()
                .find(|enchantment| u16::from(enchantment.id) == *id)
        })
        .collect()
});

// ══════════════════════════════════════════════════════════════════
// Equipment Table Registry
// ══════════════════════════════════════════════════════════════════

/// A weighted entry in a weapon selection table.
/// Matches vanilla's weighted random selection in mob `populateDefaultEquipmentSlots`.
#[derive(Clone, Copy)]
pub struct WeaponEntry {
    /// The item to potentially give.
    pub item: &'static Item,
    /// Relative weight in the selection pool.
    pub weight: f32,
}

/// How a mob's main-hand weapon is selected on spawn.
#[derive(Clone, Copy)]
pub enum WeaponConfig {
    /// Always give this exact item (e.g. skeleton → bow).
    Always(&'static Item),
    /// Always give one of the weighted items (e.g. piglin weapons).
    AlwaysWeighted(&'static [WeaponEntry]),
    /// Give a weighted weapon with a difficulty-dependent chance.
    Chance {
        /// Chance when the base difficulty is Hard.
        on_hard: f32,
        /// Chance on all other difficulties.
        otherwise: f32,
        /// Weighted item pool to select from.
        items: &'static [WeaponEntry],
    },
    /// No weapon.
    None,
}

/// A per-slot armor entry with an independent spawn chance.
pub struct ArmorSlotEntry {
    /// Which equipment slot this armor occupies.
    pub slot: &'static EquipmentSlot,
    /// The armor item.
    pub item: &'static Item,
    /// Independent chance this slot receives armor.
    pub chance: f32,
}

/// How a mob's armor is selected on spawn.
#[derive(Clone, Copy)]
pub enum ArmorConfig {
    /// Use the vanilla algorithm: random tier (0-2 base + 3 upgrade attempts at
    /// 10.87% each), partial armor break chance (10% on Hard, 25% otherwise).
    /// See [`select_vanilla_armor`].
    Vanilla,
    /// Custom per-slot entries with independent chances (e.g. piglin golden armor).
    CustomPerSlot(&'static [ArmorSlotEntry]),
    /// No armor.
    None,
}

/// Equipment definition for a single mob type. All equipment is randomized at spawn
/// using [`RegionalDifficulty`] to compute per-world/per-chunk scaling factors.
pub struct MobEquipmentDef {
    /// The entity resource name (e.g. `"zombie"`, `"skeleton"`).
    pub entity_type: &'static str,
    /// Main-hand weapon configuration.
    pub weapon: WeaponConfig,
    /// Armor configuration.
    pub armor: ArmorConfig,
    /// Whether spawn-time enchantments can be applied.
    pub enchanted: bool,
    /// Whether this mob can randomly pick up loot from the ground.
    pub can_pick_up_loot: bool,
}

/// Registry of all mobs that receive equipment at spawn.
///
/// Maps entity resource names to their equipment definitions. Only mobs listed
/// here will receive weapons, armor, enchantments, and drop-chance settings.
/// Unlisted mobs spawn with no equipment (matching vanilla — not all mobs have
/// equipment tables).
pub static EQUIPMENT_REGISTRY: LazyLock<HashMap<&'static str, MobEquipmentDef>> =
    LazyLock::new(|| {
        static ZOMBIE_WEAPONS: [WeaponEntry; 3] = [
            WeaponEntry {
                item: &Item::IRON_SWORD,
                weight: 1.0,
            },
            WeaponEntry {
                item: &Item::IRON_SPEAR,
                weight: 1.0,
            },
            WeaponEntry {
                item: &Item::IRON_SHOVEL,
                weight: 4.0,
            },
        ];

        static DROWNED_WEAPONS: [WeaponEntry; 2] = [
            WeaponEntry {
                item: &Item::TRIDENT,
                weight: 10.0,
            },
            WeaponEntry {
                item: &Item::FISHING_ROD,
                weight: 6.0,
            },
        ];

        static PIGLIN_WEAPONS: [WeaponEntry; 3] = [
            WeaponEntry {
                item: &Item::CROSSBOW,
                weight: 5.0,
            },
            WeaponEntry {
                item: &Item::GOLDEN_SWORD,
                weight: 4.5,
            },
            WeaponEntry {
                item: &Item::GOLDEN_SPEAR,
                weight: 0.5,
            },
        ];

        static PIGLIN_ARMOR: [ArmorSlotEntry; 4] = [
            ArmorSlotEntry {
                slot: &EquipmentSlot::HEAD,
                item: &Item::GOLDEN_HELMET,
                chance: 0.1,
            },
            ArmorSlotEntry {
                slot: &EquipmentSlot::CHEST,
                item: &Item::GOLDEN_CHESTPLATE,
                chance: 0.1,
            },
            ArmorSlotEntry {
                slot: &EquipmentSlot::LEGS,
                item: &Item::GOLDEN_LEGGINGS,
                chance: 0.1,
            },
            ArmorSlotEntry {
                slot: &EquipmentSlot::FEET,
                item: &Item::GOLDEN_BOOTS,
                chance: 0.1,
            },
        ];

        static ZOMBIFIED_PIGLIN_WEAPONS: [WeaponEntry; 2] = [
            WeaponEntry {
                item: &Item::GOLDEN_SWORD,
                weight: 19.0,
            },
            WeaponEntry {
                item: &Item::GOLDEN_SPEAR,
                weight: 1.0,
            },
        ];

        let mut m = HashMap::new();

        // ─── Zombie ───
        m.insert(
            "zombie",
            MobEquipmentDef {
                entity_type: "zombie",
                weapon: WeaponConfig::Chance {
                    on_hard: 0.05,
                    otherwise: 0.01,
                    items: &ZOMBIE_WEAPONS,
                },
                armor: ArmorConfig::Vanilla,
                enchanted: true,
                can_pick_up_loot: true,
            },
        );

        // ─── Husk ───
        m.insert(
            "husk",
            MobEquipmentDef {
                entity_type: "husk",
                weapon: WeaponConfig::Chance {
                    on_hard: 0.05,
                    otherwise: 0.01,
                    items: &ZOMBIE_WEAPONS,
                },
                armor: ArmorConfig::Vanilla,
                enchanted: true,
                can_pick_up_loot: true,
            },
        );

        // ─── Zombie Villager ───
        m.insert(
            "zombie_villager",
            MobEquipmentDef {
                entity_type: "zombie_villager",
                weapon: WeaponConfig::Chance {
                    on_hard: 0.05,
                    otherwise: 0.01,
                    items: &ZOMBIE_WEAPONS,
                },
                armor: ArmorConfig::Vanilla,
                enchanted: true,
                can_pick_up_loot: true,
            },
        );

        // ─── Drowned ───
        m.insert(
            "drowned",
            MobEquipmentDef {
                entity_type: "drowned",
                weapon: WeaponConfig::Chance {
                    on_hard: 0.10,
                    otherwise: 0.10,
                    items: &DROWNED_WEAPONS,
                },
                armor: ArmorConfig::None,
                enchanted: true,
                can_pick_up_loot: true,
            },
        );

        // ─── Zombified Piglin ───
        m.insert(
            "zombified_piglin",
            MobEquipmentDef {
                entity_type: "zombified_piglin",
                weapon: WeaponConfig::AlwaysWeighted(&ZOMBIFIED_PIGLIN_WEAPONS),
                armor: ArmorConfig::None,
                enchanted: true,
                can_pick_up_loot: false,
            },
        );

        // ─── Skeleton ───
        m.insert(
            "skeleton",
            MobEquipmentDef {
                entity_type: "skeleton",
                weapon: WeaponConfig::Always(&Item::BOW),
                armor: ArmorConfig::Vanilla,
                enchanted: true,
                can_pick_up_loot: true,
            },
        );

        // ─── Stray ───
        m.insert(
            "stray",
            MobEquipmentDef {
                entity_type: "stray",
                weapon: WeaponConfig::Always(&Item::BOW),
                armor: ArmorConfig::Vanilla,
                enchanted: true,
                can_pick_up_loot: true,
            },
        );

        // ─── Bogged ───
        m.insert(
            "bogged",
            MobEquipmentDef {
                entity_type: "bogged",
                weapon: WeaponConfig::Always(&Item::BOW),
                armor: ArmorConfig::Vanilla,
                enchanted: true,
                can_pick_up_loot: true,
            },
        );

        // ─── Wither Skeleton ───
        m.insert(
            "wither_skeleton",
            MobEquipmentDef {
                entity_type: "wither_skeleton",
                weapon: WeaponConfig::Always(&Item::STONE_SWORD),
                armor: ArmorConfig::None,
                enchanted: false,
                can_pick_up_loot: false,
            },
        );

        // ─── Piglin ───
        m.insert(
            "piglin",
            MobEquipmentDef {
                entity_type: "piglin",
                weapon: WeaponConfig::AlwaysWeighted(&PIGLIN_WEAPONS),
                armor: ArmorConfig::CustomPerSlot(&PIGLIN_ARMOR),
                enchanted: true,
                can_pick_up_loot: false,
            },
        );

        // ─── Piglin Brute ───
        // `PiglinBrute.populateDefaultEquipmentSlots`: unconditional golden axe, no
        // armor roll, no enchant roll (unlike the regular Piglin).
        m.insert(
            "piglin_brute",
            MobEquipmentDef {
                entity_type: "piglin_brute",
                weapon: WeaponConfig::Always(&Item::GOLDEN_AXE),
                armor: ArmorConfig::None,
                enchanted: false,
                can_pick_up_loot: false,
            },
        );

        // ─── Pillager ───
        m.insert(
            "pillager",
            MobEquipmentDef {
                entity_type: "pillager",
                weapon: WeaponConfig::Always(&Item::CROSSBOW),
                armor: ArmorConfig::None,
                enchanted: false,
                can_pick_up_loot: false,
            },
        );

        // ─── Vindicator ───
        m.insert(
            "vindicator",
            MobEquipmentDef {
                entity_type: "vindicator",
                weapon: WeaponConfig::Always(&Item::IRON_AXE),
                armor: ArmorConfig::None,
                enchanted: true,
                can_pick_up_loot: false,
            },
        );

        m
    });

// ══════════════════════════════════════════════════════════════════
// Regional Difficulty — exact vanilla DifficultyInstance.java
// Vanilla approximation: identical formula to vanilla Minecraft 26.2's
// DifficultyInstance, including clamped regional difficulty, special
// multiplier (0-1 linear), and effective difficulty (2-4 range).
// ══════════════════════════════════════════════════════════════════

/// Computed difficulty values for a specific world chunk.
///
/// Mirrors Vanilla's `DifficultyInstance`. Used to scale equipment spawn rates,
/// enchantment costs, and loot-pickup flags.
#[derive(Clone, Copy)]
pub struct RegionalDifficulty {
    /// The world's base difficulty level (`Easy`, `Normal`, `Hard`).
    pub base_difficulty: Difficulty,
    /// Effective difficulty computed from game time, inhabited time, and moon phase.
    /// Clamped to the range `[2.0, 4.0]` (or `0.0` for Peaceful).
    pub effective_difficulty: f32,
    /// Linear multiplier in `[0.0, 1.0]` derived from `effective_difficulty`.
    /// When `0.0` (fresh chunk + early game), no equipment, enchantments, or
    /// loot-pickup flags are applied.
    pub special_multiplier: f32,
}

impl RegionalDifficulty {
    /// Computes difficulty at the given world position.
    ///
    /// Looks up the chunk's inhabited time and combines it with the world's
    /// difficulty, game time, and moon phase.
    pub fn at(world: &Arc<crate::world::World>, pos: Vector3<f64>) -> Self {
        let level_info = world.level_info.load();
        let difficulty = level_info.difficulty;
        let time_of_day = world.level_time.try_lock().map_or(0, |t| t.time_of_day);
        let inhabited_time = {
            let chunk_x = (pos.x / 16.0).floor() as i32;
            let chunk_z = (pos.z / 16.0).floor() as i32;
            world
                .level
                .loaded_chunks
                .get(&pumpkin_util::math::vector2::Vector2::new(chunk_x, chunk_z))
                .map_or(0, |c| {
                    c.inhabited_time.load(std::sync::atomic::Ordering::Relaxed)
                })
        };
        let moon_brightness = moon_brightness(time_of_day);

        Self::calculate(difficulty, time_of_day, inhabited_time, moon_brightness)
    }

    /// Direct calculation from raw inputs. Used by `at()` and for testing.
    #[must_use]
    pub fn calculate(
        difficulty: Difficulty,
        total_game_time: i64,
        chunk_inhabited_time: u64,
        moon_brightness: f32,
    ) -> Self {
        if difficulty == Difficulty::Peaceful {
            return Self {
                base_difficulty: difficulty,
                effective_difficulty: 0.0,
                special_multiplier: 0.0,
            };
        }

        let is_hard = difficulty == Difficulty::Hard;

        let mut scale = 0.75f32;
        let global_scale = ((total_game_time as f32 - 72000.0) / 1440000.0).clamp(0.0, 1.0) * 0.25;
        scale += global_scale;

        let mut local_scale = 0.0f32;
        local_scale += (chunk_inhabited_time as f32 / 3600000.0).clamp(0.0, 1.0)
            * if is_hard { 1.0 } else { 0.75 };
        local_scale += (moon_brightness * 0.25).clamp(0.0, global_scale);

        if difficulty == Difficulty::Easy {
            local_scale *= 0.5;
        }

        let difficulty_id = match difficulty {
            Difficulty::Peaceful => 0,
            Difficulty::Easy => 1,
            Difficulty::Normal => 2,
            Difficulty::Hard => 3,
        };

        let effective = difficulty_id as f32 * (scale + local_scale);

        let special_multiplier = if effective < 2.0 {
            0.0
        } else if effective > 4.0 {
            1.0
        } else {
            (effective - 2.0) / 2.0
        };

        Self {
            base_difficulty: difficulty,
            effective_difficulty: effective,
            special_multiplier,
        }
    }

    /// Random check scaled by `special_multiplier`.
    ///
    /// Returns `true` with probability `base_chance * special_multiplier`. When
    /// `special_multiplier` is `0.0` this always returns `false` (matching vanilla
    /// behaviour on fresh Normal/Easy worlds).
    #[must_use]
    pub fn should_happen(&self, base_chance: f32) -> bool {
        rand::random::<f32>() < base_chance * self.special_multiplier
    }
}

/// Moon brightness factor for the given time of day (0.0 to 1.0).
/// Full moon at phase 0, new moon at phase 4.
#[must_use]
fn moon_brightness(time_of_day: i64) -> f32 {
    let phase = (time_of_day / 24000 % 8) as i32;
    if phase == 0 {
        1.0
    } else {
        1.0 - (phase - 4).abs() as f32 / 4.0
    }
}

// ══════════════════════════════════════════════════════════════════
// Mob.enchantSpawnedArmor / enchantSpawnedWeapon — vanilla port
// Sources: Mob.java:1055-1081, EnchantmentsByCostWithDifficulty.java:31-38,
// EnchantmentHelper.java:547-578,597-612, WeightedRandom, ItemEnchantments.Mutable
// ══════════════════════════════════════════════════════════════════

/// Base provider cost before the enchantability bonus.
///
/// Vanilla `EnchantmentsByCostWithDifficulty.enchant`
/// (`EnchantmentsByCostWithDifficulty.java:33-34`):
///
/// ```java
/// int cost = Mth.randomBetweenInclusive(random, this.minCost,
///     this.minCost + (int) (difficultyModifier * this.maxCostSpan));
/// ```
///
/// The Java `(int)` cast truncates toward zero, so e.g. `0.999 * 17 = 16.983`
/// yields a span of 16 (not 17).
#[must_use]
fn mob_spawn_equipment_base_cost<R: Rng + ?Sized>(rng: &mut R, special_multiplier: f32) -> i32 {
    let span = (special_multiplier * MOB_SPAWN_ENCHANT_COST_SPAN as f32) as i32;
    MOB_SPAWN_ENCHANT_MIN_COST + rng.random_range(0..=span)
}

/// Vanilla `WeightedRandom.getRandomItem(random, entries, EnchantmentInstance::weight)`
/// (`EnchantmentInstance.weight` delegates to `Enchantment.getWeight`,
/// `EnchantmentInstance.java:8-10`). Integer weights and an exclusive upper hit test,
/// unlike a floating-point roll.
#[must_use]
fn weighted_random_enchantment<R: Rng + ?Sized>(
    rng: &mut R,
    entries: &[(&'static Enchantment, i32)],
) -> Option<(&'static Enchantment, i32)> {
    let total_weight: i32 = entries.iter().map(|(entry, _)| entry.weight).sum();
    if total_weight <= 0 {
        return None;
    }
    let mut roll = rng.random_range(0..total_weight);
    for entry in entries {
        roll -= entry.0.weight;
        if roll < 0 {
            return Some(*entry);
        }
    }
    None
}

/// Vanilla `EnchantmentHelper.getAvailableEnchantmentResults`
/// (`EnchantmentHelper.java:597-612`) restricted to the mob-spawn pool.
///
/// An enchantment is available at the highest level whose cost window
/// `[min_cost(level), max_cost(level)]` contains `cost`, and only when the item is
/// one of its primary items (`Enchantment.isPrimaryItem`, `Enchantment.java:130-131`).
/// The vanilla book branch (`itemStack.is(Items.BOOK)`) is unreachable here:
/// books carry no `minecraft:enchantable` component, so
/// [`select_spawn_enchantments`] bails out before reaching this scan.
#[must_use]
fn available_spawn_enchantment_results(
    cost: i32,
    item: &'static Item,
) -> Vec<(&'static Enchantment, i32)> {
    MOB_SPAWN_EQUIPMENT_POOL
        .iter()
        .filter_map(|enchantment| {
            if !enchantment.is_primary_item(item) {
                return None;
            }
            (1..=enchantment.max_level)
                .rev()
                .find(|level| {
                    cost >= enchantment.min_cost.calculate(*level)
                        && cost <= enchantment.max_cost.calculate(*level)
                })
                .map(|level| (*enchantment, level))
        })
        .collect()
}

/// Vanilla `EnchantmentHelper.selectEnchantment` (`EnchantmentHelper.java:547-578`)
/// for the mob-spawn equipment pool.
///
/// Raises `cost` by an enchantability bonus plus a ±15% random span, then repeatedly
/// picks weighted candidates while ``nextInt(50) <= cost``, halving the cost between
/// picks. Compatibility filtering mirrors `filterCompatibleEnchantments`
/// (`EnchantmentHelper.java:581-583`): everything incompatible with the most recent
/// pick — including itself — leaves the candidate list.
#[must_use]
fn select_spawn_enchantments<R: Rng + ?Sized>(
    rng: &mut R,
    stack: &ItemStack,
    mut cost: i32,
) -> Vec<(&'static Enchantment, i32)> {
    let Some(enchantable) = stack.get_data_component::<EnchantableImpl>() else {
        return Vec::new();
    };

    // `enchantmentCost += 1 + random.nextInt(value / 4 + 1) + random.nextInt(value / 4 + 1);`
    let quarter = enchantable.value / 4;
    cost += 1 + rng.random_range(0..=quarter) + rng.random_range(0..=quarter);

    // `randomSpan = (nextFloat() + nextFloat() - 1.0F) * 0.15F;` then
    // `Mth.clamp(Math.round(cost + cost * randomSpan), 1, Integer.MAX_VALUE)`.
    let random_span = (rng.random::<f32>() + rng.random::<f32>() - 1.0) * 0.15;
    let scaled = cost as f32 + cost as f32 * random_span;
    cost = ((scaled + 0.5).floor() as i64).clamp(1, i64::from(i32::MAX)) as i32;

    let mut available = available_spawn_enchantment_results(cost, stack.item);
    let mut results: Vec<(&'static Enchantment, i32)> = Vec::new();

    // First pick happens before any cost halving or repeat gate.
    let Some(first) = weighted_random_enchantment(rng, &available) else {
        return results;
    };
    results.push(first);

    while rng.random_range(0..50) <= cost {
        if let Some((last, _)) = results.last() {
            available.retain(|(candidate, _)| last.are_compatible(candidate));
        }

        if available.is_empty() {
            break;
        }

        if let Some(next) = weighted_random_enchantment(rng, &available) {
            results.push(next);
        } else {
            break;
        }
        cost /= 2;
    }

    results
}

/// Vanilla `EnchantmentsByCostWithDifficulty.enchant`
/// (`EnchantmentsByCostWithDifficulty.java:31-38`): draw a difficulty-scaled base
/// cost, run [`select_spawn_enchantments`], and upgrade the stack with every result.
///
/// `ItemStack::enchant` keeps `max(existing, new)` per enchantment, matching
/// `ItemEnchantments.Mutable.upgrade` (`ItemEnchantments.java:139-143`), so any
/// enchantments already on the equipped stack are preserved, never downgraded.
fn mob_spawn_equipment_provider_enchant<R: Rng + ?Sized>(
    rng: &mut R,
    stack: &mut ItemStack,
    special_multiplier: f32,
) {
    let cost = mob_spawn_equipment_base_cost(rng, special_multiplier);
    for (enchantment, level) in select_spawn_enchantments(rng, stack, cost) {
        stack.enchant(enchantment, level);
    }
}

/// Vanilla `Mob.enchantSpawnedEquipment` (`Mob.java:1073-1081`).
///
/// Rolls `nextFloat() < chance * difficulty.getSpecialMultiplier()` for a non-empty
/// stack, then applies the `MOB_SPAWN_EQUIPMENT` provider in place. Returns whether
/// the roll succeeded and enchanting was attempted.
fn enchant_spawned_equipment<R: Rng + ?Sized>(
    rng: &mut R,
    stack: &mut ItemStack,
    chance: f32,
    difficulty: &RegionalDifficulty,
) -> bool {
    if stack.is_empty() || rng.random::<f32>() >= chance * difficulty.special_multiplier {
        return false;
    }
    mob_spawn_equipment_provider_enchant(rng, stack, difficulty.special_multiplier);
    true
}

/// Vanilla `Mob.enchantSpawnedWeapon` (`Mob.java:1065-1067`): same shared helper as
/// armor with a `0.25F` base chance on the main-hand item.
fn enchant_spawned_weapon<R: Rng + ?Sized>(
    rng: &mut R,
    stack: &mut ItemStack,
    difficulty: &RegionalDifficulty,
) -> bool {
    enchant_spawned_equipment(rng, stack, WEAPON_ENCHANT_CHANCE, difficulty)
}

/// Vanilla `Mob.enchantSpawnedArmor` (`Mob.java:1069-1071`): enchants whatever item
/// currently occupies a humanoid-armor slot via the shared helper with a `0.5F` base
/// chance. The slot itself takes no further part — like vanilla, candidate filtering
/// is driven by the equipped stack's own items/primary-item tags.
fn enchant_spawned_armor<R: Rng + ?Sized>(
    rng: &mut R,
    stack: &mut ItemStack,
    difficulty: &RegionalDifficulty,
) -> bool {
    enchant_spawned_equipment(rng, stack, ARMOR_ENCHANT_CHANCE, difficulty)
}

// ══════════════════════════════════════════════════════════════════
// Equipment population
//
// Mirrors mob-specific `finalizeSpawn` / `populateDefaultEquipmentSlots`
// from Vanilla's Zombie, AbstractSkeleton, WitherSkeleton, Piglin,
// Pillager, Vindicator, Drowned, and ZombifiedPiglin.
// ══════════════════════════════════════════════════════════════════

/// Weighted random selection from a table of weapon entries.
#[must_use]
fn weighted_select_item(items: &[WeaponEntry]) -> &'static Item {
    let total: f32 = items.iter().map(|e| e.weight).sum();
    let mut rng = rand::rng();
    let mut roll: f32 = rng.random_range(0.0..total);
    for entry in items {
        roll -= entry.weight;
        if roll <= 0.0 {
            return entry.item;
        }
    }
    items.last().map_or(&Item::AIR, |e| e.item)
}

/// Selects armor using the vanilla algorithm.
///
/// 1. Random base tier (0-2) with up to 3 upgrade attempts at 10.87% each.
/// 2. Iterates HEAD→CHEST→LEGS→FEET, with a chance to stop early (10% Hard,
///    25% otherwise) — higher difficulty produces fewer pieces.
/// 3. Each piece gets the default equipment drop chance.
#[must_use]
fn select_vanilla_armor(difficulty: &RegionalDifficulty) -> Vec<(EquipmentSlot, ItemStack, f32)> {
    let mut rng = rand::rng();

    let mut armor_type = rng.random_range(0..3);
    let mut i = 1;
    while (i as f32) <= ARMOR_UPGRADE_MATERIAL_ATTEMPTS {
        if rng.random::<f32>() < ARMOR_UPGRADE_MATERIAL_CHANCE {
            armor_type += 1;
        }
        i += 1;
    }
    armor_type = armor_type.min(5);

    let tier = &ARMOR_TIERS[armor_type];

    let partial_chance = if difficulty.base_difficulty == Difficulty::Hard {
        0.1f32
    } else {
        0.25f32
    };

    let mut pieces = Vec::new();
    let mut first = true;
    for (i, slot) in ARMOR_POPULATION_ORDER.iter().enumerate() {
        if !first && rng.random::<f32>() < partial_chance {
            break;
        }
        first = false;
        pieces.push((
            slot.clone(),
            create_equipment_item(tier[i], difficulty),
            DEFAULT_EQUIPMENT_DROP_CHANCE,
        ));
    }
    pieces
}

/// Creates a fresh, full-durability `ItemStack` for mob equipment.
/// Vanilla mobs always spawn with equipment at full durability.
#[must_use]
fn create_equipment_item(item: &'static Item, _difficulty: &RegionalDifficulty) -> ItemStack {
    ItemStack::new(1, item)
}

/// Generates the equipment items, slots, and drop chances for a mob definition.
///
/// Handles the full weapon + armor selection logic. When `def.enchanted` is set
/// (mirroring the mobs that call `populateDefaultEquipmentEnchantments`, versus
/// those like `WitherSkeleton.java:79-80` and `PiglinBrute` that skip it), each
/// equipped piece goes through the vanilla enchant rolls —
/// [`enchant_spawned_weapon`] for the main hand, [`enchant_spawned_armor`] for
/// every armor slot — exactly as in `Mob.populateDefaultEquipmentEnchantments`
/// (`Mob.java:1055-1063`). All enchant rolls draw from a single RNG, like the
/// per-spawn `RandomSource` vanilla draws from `level.getRandom()`.
#[must_use]
fn equip_mob_from_def(
    def: &MobEquipmentDef,
    difficulty: &RegionalDifficulty,
) -> Vec<(EquipmentSlot, ItemStack, f32)> {
    let mut rng = rand::rng();
    let mut changes: Vec<(EquipmentSlot, ItemStack, f32)> = Vec::new();

    // ── Weapon ──
    match def.weapon {
        WeaponConfig::Always(item) => {
            let mut stack = create_equipment_item(item, difficulty);
            if def.enchanted {
                enchant_spawned_weapon(&mut rng, &mut stack, difficulty);
            }
            changes.push((
                EquipmentSlot::MAIN_HAND,
                stack,
                DEFAULT_EQUIPMENT_DROP_CHANCE,
            ));
        }
        WeaponConfig::AlwaysWeighted(items) => {
            let item = weighted_select_item(items);
            let mut stack = create_equipment_item(item, difficulty);
            if def.enchanted {
                enchant_spawned_weapon(&mut rng, &mut stack, difficulty);
            }
            changes.push((
                EquipmentSlot::MAIN_HAND,
                stack,
                DEFAULT_EQUIPMENT_DROP_CHANCE,
            ));
        }
        WeaponConfig::Chance {
            on_hard,
            otherwise,
            items,
        } => {
            let chance = if difficulty.base_difficulty == Difficulty::Hard {
                on_hard
            } else {
                otherwise
            };
            if rng.random::<f32>() < chance {
                let item = weighted_select_item(items);
                let mut stack = create_equipment_item(item, difficulty);
                if def.enchanted {
                    enchant_spawned_weapon(&mut rng, &mut stack, difficulty);
                }
                changes.push((
                    EquipmentSlot::MAIN_HAND,
                    stack,
                    DEFAULT_EQUIPMENT_DROP_CHANCE,
                ));
            }
        }
        WeaponConfig::None => {}
    }

    // ── Armor ──
    match def.armor {
        ArmorConfig::Vanilla => {
            if difficulty.should_happen(WEARING_ARMOR_CHANCE) {
                let armor_pieces = select_vanilla_armor(difficulty);
                for (slot, mut stack, drop_chance) in armor_pieces {
                    if def.enchanted {
                        enchant_spawned_armor(&mut rng, &mut stack, difficulty);
                    }
                    changes.push((slot, stack, drop_chance));
                }
            }
        }
        ArmorConfig::CustomPerSlot(entries) => {
            for entry in entries {
                if rng.random::<f32>() < entry.chance {
                    let mut stack = create_equipment_item(entry.item, difficulty);
                    if def.enchanted {
                        enchant_spawned_armor(&mut rng, &mut stack, difficulty);
                    }
                    changes.push((entry.slot.clone(), stack, DEFAULT_EQUIPMENT_DROP_CHANCE));
                }
            }
        }
        ArmorConfig::None => {}
    }

    changes
}

// ══════════════════════════════════════════════════════════════════
// Public entry point
// ══════════════════════════════════════════════════════════════════

/// Equips a mob with weapons/armor/enchantments when it spawns.
///
/// Called from the blanket `EntityBase::init_data_tracker` implementation for
/// all mob types. Looks up the mob's equipment definition in
/// [`EQUIPMENT_REGISTRY`], computes [`RegionalDifficulty`] at the mob's
/// position, generates equipment, stores it in the entity's equipment slots,
/// and broadcasts the changes to nearby players.
///
/// Mobs not listed in the registry silently receive no equipment.
pub async fn equip_mob_on_spawn(mob: &dyn EntityBase, world: &Arc<crate::world::World>) {
    let entity_type = mob.get_entity().entity_type;
    let pos = mob.get_entity().pos.load();
    let difficulty = RegionalDifficulty::at(world, pos);

    let Some(living) = mob.get_living_entity() else {
        return;
    };

    let entity_name = entity_type.resource_name;

    let Some(def) = EQUIPMENT_REGISTRY.get(entity_name) else {
        return;
    };

    let mut equipment = living.entity_equipment.lock().await;
    let mut drop_chances = living.equipment_drop_chances.lock().await;
    let changes_with_drops = equip_mob_from_def(def, &difficulty);

    let mut equipment_changes: Vec<(EquipmentSlot, ItemStack)> = Vec::new();

    for (slot, stack, drop_chance) in changes_with_drops {
        equipment.put(&slot, stack.clone());
        drop_chances.insert(slot.clone(), drop_chance);
        equipment_changes.push((slot, stack));
    }

    drop(equipment);
    drop(drop_chances);

    living.send_equipment_changes(&equipment_changes);
}

#[cfg(test)]
mod tests {
    use super::{
        ARMOR_ENCHANT_CHANCE, DEFAULT_EQUIPMENT_DROP_CHANCE, Enchantment, EnchantmentTag, Item,
        ItemStack, MOB_SPAWN_ENCHANT_COST_SPAN, MOB_SPAWN_ENCHANT_MIN_COST,
        MOB_SPAWN_EQUIPMENT_POOL, RegionalDifficulty, WEAPON_ENCHANT_CHANCE, WEARING_ARMOR_CHANCE,
        available_spawn_enchantment_results, enchant_spawned_armor, enchant_spawned_equipment,
        enchant_spawned_weapon, mob_spawn_equipment_base_cost,
        mob_spawn_equipment_provider_enchant, select_spawn_enchantments,
        weighted_random_enchantment,
    };
    use pumpkin_data::data_component_impl::{EnchantableImpl, EnchantmentsImpl};
    use pumpkin_util::difficulty::Difficulty;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    /// `RegionalDifficulty` at full strength: `DifficultyInstance.getSpecialMultiplier`
    /// returns 1.0 once the effective difficulty exceeds 4.0.
    fn max_difficulty() -> RegionalDifficulty {
        RegionalDifficulty {
            base_difficulty: Difficulty::Hard,
            effective_difficulty: 4.5,
            special_multiplier: 1.0,
        }
    }

    fn zero_difficulty() -> RegionalDifficulty {
        RegionalDifficulty {
            base_difficulty: Difficulty::Normal,
            effective_difficulty: 1.0,
            special_multiplier: 0.0,
        }
    }

    fn stack_of(item: &'static Item) -> ItemStack {
        ItemStack::new(1, item)
    }

    fn applied_enchantments(stack: &ItemStack) -> Vec<(&'static Enchantment, i32)> {
        stack
            .get_data_component::<EnchantmentsImpl>()
            .map(|data| data.enchantment.to_vec())
            .unwrap_or_default()
    }

    #[test]
    fn enchant_constants_match_vanilla() {
        // `Mob.java:1066,1070`.
        assert!((WEAPON_ENCHANT_CHANCE - 0.25).abs() < f32::EPSILON);
        assert!((ARMOR_ENCHANT_CHANCE - 0.5).abs() < f32::EPSILON);
        // `VanillaEnchantmentProviders.java:24` / `mob_spawn_equipment.json`.
        assert_eq!(MOB_SPAWN_ENCHANT_MIN_COST, 5);
        assert_eq!(MOB_SPAWN_ENCHANT_COST_SPAN, 17);
        assert!((DEFAULT_EQUIPMENT_DROP_CHANCE - 0.085).abs() < f32::EPSILON);
        assert!((WEARING_ARMOR_CHANCE - 0.15).abs() < f32::EPSILON);
    }

    #[test]
    fn base_cost_spans_difficulty_like_java_int_cast() {
        let mut rng = StdRng::seed_from_u64(0x0262);
        // Zero special multiplier collapses the range onto the bare minimum.
        for _ in 0..64 {
            assert_eq!(mob_spawn_equipment_base_cost(&mut rng, 0.0), 5);
        }
        // Full multiplier covers [min, min + span].
        for _ in 0..256 {
            assert!((5..=22).contains(&mob_spawn_equipment_base_cost(&mut rng, 1.0)));
        }
        // `(int) (0.999f * 17)` truncates to a span of 16 — a `.round()` here would
        // wrongly allow 22; this pins down the truncation semantics.
        for _ in 0..256 {
            assert!((5..=21).contains(&mob_spawn_equipment_base_cost(&mut rng, 0.999)));
        }
    }

    #[test]
    fn pool_resolves_the_on_mob_spawn_equipment_tag() {
        // The pool must mirror the generated tag one-to-one (same members, same
        // order), because order feeds vanilla WeightedRandom tie-breaking.
        let tag_ids = &EnchantmentTag::MINECRAFT_ON_MOB_SPAWN_EQUIPMENT.1;
        assert_eq!(MOB_SPAWN_EQUIPMENT_POOL.len(), tag_ids.len());
        for (enchantment, id) in MOB_SPAWN_EQUIPMENT_POOL.iter().zip(tag_ids.iter()) {
            assert_eq!(u16::from(enchantment.id), *id);
        }
    }

    #[test]
    fn pool_excludes_treasure_enchantments() {
        // `#on_mob_spawn_equipment` resolves to `#non_treasure`, so Mending, Swift
        // Sneak, Soul Speed, Frost Walker, and Wind Burst can never roll here — the
        // previous hand-written pools wrongly offered several of these on armor.
        for treasure in [
            &Enchantment::MENDING,
            &Enchantment::SWIFT_SNEAK,
            &Enchantment::SOUL_SPEED,
            &Enchantment::FROST_WALKER,
            &Enchantment::WIND_BURST,
        ] {
            assert!(
                !MOB_SPAWN_EQUIPMENT_POOL.contains(&treasure),
                "{} must not be in the mob spawn pool",
                treasure.registry_key
            );
        }
    }

    #[test]
    fn candidates_follow_primary_items_per_slot() {
        // Respiration/Aqua Affinity list helmets as primary items; Depth Strider is
        // boots-only. No cost may leak them across slots, and slot-appropriate
        // enchantments must appear somewhere in the reachable cost range.
        let helmet = &Item::DIAMOND_HELMET;
        let chestplate = &Item::DIAMOND_CHESTPLATE;
        let boots = &Item::DIAMOND_BOOTS;

        let mut respiration_seen = false;
        let mut depth_strider_seen = false;
        for cost in 0..=80i32 {
            let head: Vec<_> = available_spawn_enchantment_results(cost, helmet)
                .into_iter()
                .map(|(enchantment, _)| enchantment)
                .collect();
            if head.contains(&&Enchantment::RESPIRATION) {
                respiration_seen = true;
            }
            assert!(!head.contains(&&Enchantment::DEPTH_STRIDER));
            assert!(!head.contains(&&Enchantment::FEATHER_FALLING));

            let chest: Vec<_> = available_spawn_enchantment_results(cost, chestplate)
                .into_iter()
                .map(|(enchantment, _)| enchantment)
                .collect();
            assert!(!chest.contains(&&Enchantment::RESPIRATION));
            assert!(!chest.contains(&&Enchantment::AQUA_AFFINITY));

            let feet: Vec<_> = available_spawn_enchantment_results(cost, boots)
                .into_iter()
                .map(|(enchantment, _)| enchantment)
                .collect();
            assert!(!feet.contains(&&Enchantment::RESPIRATION));
            if feet.contains(&&Enchantment::DEPTH_STRIDER) {
                depth_strider_seen = true;
            }
        }
        assert!(respiration_seen, "helmets must be able to roll Respiration");
        assert!(
            depth_strider_seen,
            "boots must be able to roll Depth Strider"
        );
    }

    #[test]
    fn candidate_levels_sit_at_the_highest_affordable_cost_window() {
        // For every cost, each result must satisfy both window bounds
        // (`value >= getMinCost(level) && value <= getMaxCost(level)`,
        // `EnchantmentHelper.java:603-609`) and be the highest fitting level — the
        // scan runs from max level downwards and stops at the first match.
        for item in [
            &Item::LEATHER_HELMET,
            &Item::IRON_CHESTPLATE,
            &Item::DIAMOND_BOOTS,
        ] {
            for cost in 0..=80i32 {
                for (enchantment, level) in available_spawn_enchantment_results(cost, item) {
                    assert!((1..=enchantment.max_level).contains(&level));
                    assert!(cost >= enchantment.min_cost.calculate(level));
                    assert!(cost <= enchantment.max_cost.calculate(level));
                    let higher = level + 1;
                    assert!(
                        higher > enchantment.max_level
                            || cost < enchantment.min_cost.calculate(higher)
                            || cost > enchantment.max_cost.calculate(higher),
                        "{} L{level} is not the highest window fit for cost {cost}",
                        enchantment.registry_key
                    );
                }
            }
        }
    }

    #[test]
    fn weighted_pick_matches_vanilla_weight_ratios() {
        const DRAWS: u32 = 20_000;

        // Generated weights: protection 10 vs thorns 1 (`Enchantment.getWeight`).
        let entries: [(&Enchantment, i32); 2] =
            [(&Enchantment::PROTECTION, 1), (&Enchantment::THORNS, 1)];
        let mut rng = StdRng::seed_from_u64(7);
        let mut protection = 0u32;
        for _ in 0..DRAWS {
            let (picked, _) = weighted_random_enchantment(&mut rng, &entries).unwrap();
            if picked == &Enchantment::PROTECTION {
                protection += 1;
            }
        }
        let thorns = DRAWS - protection;
        // Expected ratio 10:1 with generous bounds so the seeded draw stays stable.
        assert!(thorns > 0);
        assert!(
            protection > thorns * 6 && protection < thorns * 15,
            "protection {protection} vs thorns {thorns}"
        );
    }

    #[test]
    fn weighted_pick_returns_none_for_degenerate_input() {
        let mut rng = StdRng::seed_from_u64(4);
        // An empty candidate list sums to a zero total weight, exercising the same
        // guard vanilla `WeightedRandom.getRandomItem` uses. A per-entry zero weight
        // cannot be constructed because weights come from the static registry.
        assert!(weighted_random_enchantment(&mut rng, &[]).is_none());
    }

    #[test]
    fn zero_special_multiplier_never_enchants() {
        let difficulty = zero_difficulty();
        for seed in 0..128u64 {
            let mut stack = stack_of(&Item::IRON_HELMET);
            let mut rng = StdRng::seed_from_u64(seed);
            assert!(!enchant_spawned_equipment(
                &mut rng,
                &mut stack,
                ARMOR_ENCHANT_CHANCE,
                &difficulty
            ));
            assert!(!stack.has_enchantments());
        }
    }

    #[test]
    fn full_multiplier_enchants_and_stays_inside_the_pool() {
        let difficulty = max_difficulty();
        let mut successes = 0;
        for seed in 0..256u64 {
            let mut stack = stack_of(&Item::IRON_HELMET);
            let mut rng = StdRng::seed_from_u64(seed);
            if enchant_spawned_armor(&mut rng, &mut stack, &difficulty) {
                successes += 1;
                let applied = applied_enchantments(&stack);
                assert!(!applied.is_empty());
                for (enchantment, level) in applied {
                    assert!(
                        MOB_SPAWN_EQUIPMENT_POOL.contains(&enchantment),
                        "{} leaked outside the provider pool",
                        enchantment.registry_key
                    );
                    assert!((1..=enchantment.max_level).contains(&level));
                }
            }
        }
        // P(no success in 256 rolls at p = 0.5) ≈ 5e-78.
        assert!(successes > 0);
    }

    #[test]
    fn preexisting_enchantments_are_upgraded_never_downgraded() {
        let difficulty = max_difficulty();
        for seed in 0..192u64 {
            let mut stack = stack_of(&Item::DIAMOND_HELMET);
            stack.enchant(&Enchantment::PROTECTION, 4);
            let mut rng = StdRng::seed_from_u64(seed);
            enchant_spawned_armor(&mut rng, &mut stack, &difficulty);
            assert!(
                stack.get_enchantment_level(&Enchantment::PROTECTION) >= 4,
                "seed {seed}: existing Protection IV was altered"
            );
        }
    }

    #[test]
    fn non_enchantable_items_are_left_untouched() {
        let difficulty = max_difficulty();
        for seed in 0..32u64 {
            let mut stick = stack_of(&Item::STICK);
            let mut rng = StdRng::seed_from_u64(seed);
            // A stick carries no `minecraft:enchantable` component, so whether or
            // not the gate passes, selection yields nothing and the stack survives.
            enchant_spawned_weapon(&mut rng, &mut stick, &difficulty);
            assert!(!stick.has_enchantments());
        }
    }

    #[test]
    fn empty_stacks_are_skipped_entirely() {
        let difficulty = max_difficulty();
        let mut stack = ItemStack::EMPTY.clone();
        let mut rng = StdRng::seed_from_u64(1);
        assert!(!enchant_spawned_equipment(
            &mut rng,
            &mut stack,
            ARMOR_ENCHANT_CHANCE,
            &difficulty
        ));
        assert!(!stack.has_enchantments());
    }

    #[test]
    fn provider_power_is_bounded_by_the_difficulty_scaled_cost() {
        // At multiplier 0 every base cost is exactly 5; after the enchantability
        // bonus (`+1 + nextInt(q+1) + nextInt(q+1)`) and the ±15% span the adjusted
        // cost can never exceed this ceiling derived from the item's own component.
        let mut rng = StdRng::seed_from_u64(99);
        let stack_item = &Item::LEATHER_HELMET;
        let enchantable = {
            let probe = stack_of(stack_item);
            probe
                .get_data_component::<EnchantableImpl>()
                .map_or(0, |e| e.value)
        };
        let quarter = enchantable / 4;
        let pre_span_max = MOB_SPAWN_ENCHANT_MIN_COST + 1 + quarter + quarter;
        let adjusted_ceiling = ((pre_span_max as f32 * 1.15 + 0.5).floor()) as i32;

        for _ in 0..256 {
            let mut stack = stack_of(stack_item);
            mob_spawn_equipment_provider_enchant(&mut rng, &mut stack, 0.0);
            let applied = applied_enchantments(&stack);
            assert!(
                !applied.is_empty(),
                "leather helmets always have an affordable candidate"
            );
            for (enchantment, level) in applied {
                let reachable = enchantment.min_cost.calculate(level) <= adjusted_ceiling
                    && enchantment.max_cost.calculate(level) >= 1;
                assert!(
                    reachable,
                    "{} L{level} unreachable within adjusted cost {adjusted_ceiling}",
                    enchantment.registry_key
                );
            }
        }
    }

    #[test]
    fn select_produces_nothing_without_an_enchantable_component() {
        let mut rng = StdRng::seed_from_u64(3);
        let stick = stack_of(&Item::STICK);
        assert!(select_spawn_enchantments(&mut rng, &stick, 30).is_empty());
    }

    #[test]
    fn registry_definitions_keep_their_vanilla_enchant_flags() {
        // `WitherSkeleton.java:79-80` overrides the enchant step to a no-op and
        // `PiglinBrute.finalizeSpawn` never calls it — both stay false.
        for name in ["wither_skeleton", "piglin_brute"] {
            let def = super::EQUIPMENT_REGISTRY.get(name).unwrap();
            assert!(
                !def.enchanted,
                "{name} must not gain spawn enchantments without a vanilla call"
            );
        }
        for name in ["zombie", "husk", "zombie_villager"] {
            // `Zombie.java:495-496`.
            let def = super::EQUIPMENT_REGISTRY.get(name).unwrap();
            assert!(
                def.enchanted,
                "{name} calls populateDefaultEquipmentEnchantments"
            );
        }
    }
}
