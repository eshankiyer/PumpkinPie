// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::BlockEntity;
use pumpkin_data::block_properties::{
    BlockProperties, TrialSpawnerLikeProperties, TrialSpawnerState,
};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockStateId, world::WorldEvent};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::GameMode;
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos, vector3::Vector3};
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::MutexGuard as StdMutexGuard;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::entity::NBTStorage;
use crate::entity::item::ItemEntity;
use crate::entity::{Entity, ominous_item_spawner::OminousItemSpawnerEntity};
use crate::plugin::api::events::entity::item_spawn::ItemSpawnEvent;
use crate::world::game_event::{GameEventContext, emit_game_event};
use crate::world::natural_spawner::spawn_dimensions;
use crate::world::{BlockFlags, World};

// TrialSpawnerConfig.java:92-97 (Builder defaults)
const DEFAULT_OMINOUS_ITEMS_LOOT_TABLE: &str =
    "minecraft:spawners/trial_chamber/items_to_drop_when_ominous";

#[derive(Clone)]
pub struct TrialSpawnerConfig {
    pub spawn_range: i32,
    pub total_mobs: f32,
    pub simultaneous_mobs: f32,
    pub total_mobs_added_per_player: f32,
    pub simultaneous_mobs_added_per_player: f32,
    pub ticks_between_spawn: i64,
    pub spawn_potentials: Vec<(&'static EntityType, i32, NbtCompound)>,
    // TrialSpawnerConfig.java:47-49 stores the weighted reward-table list.
    pub loot_tables_to_eject: Vec<(String, i32)>,
    // TrialSpawnerConfig.java:18-27, 50-52 stores the ominous item loot-table key.
    pub items_to_drop_when_ominous: String,
}

impl Default for TrialSpawnerConfig {
    fn default() -> Self {
        Self {
            spawn_range: 4,
            total_mobs: 6.0,
            simultaneous_mobs: 2.0,
            total_mobs_added_per_player: 2.0,
            simultaneous_mobs_added_per_player: 1.0,
            ticks_between_spawn: 40,
            spawn_potentials: Vec::new(),
            // TrialSpawnerConfig.java:98-102 supplies the normal two-table default.
            loot_tables_to_eject: default_loot_tables(),
            items_to_drop_when_ominous: DEFAULT_OMINOUS_ITEMS_LOOT_TABLE.to_owned(),
        }
    }
}

impl TrialSpawnerConfig {
    // TrialSpawnerConfig.CODEC (TrialSpawnerConfig.java:53) is a RegistryFileCodec:
    // structure-baked NBT stores a bare resource-key string (e.g.
    // "minecraft:trial_chamber/melee/zombie/normal") pointing at the built-in
    // trial_spawner_config registry (TrialSpawnerConfigs.java), not an inline compound.
    // An inline compound is still accepted for hand-authored / test NBT.
    fn from_nbt(nbt: Option<&NbtTag>) -> Self {
        match nbt {
            Some(NbtTag::String(key)) => built_in_config(key).unwrap_or_default(),
            Some(NbtTag::Compound(nbt)) => Self::from_compound(nbt),
            _ => Self::default(),
        }
    }

    fn from_compound(nbt: &NbtCompound) -> Self {
        let mut config = Self::default();
        if let Some(v) = nbt.get_int("spawn_range") {
            config.spawn_range = v;
        }
        if let Some(v) = nbt.get_float("total_mobs") {
            config.total_mobs = v;
        }
        if let Some(v) = nbt.get_float("simultaneous_mobs") {
            config.simultaneous_mobs = v;
        }
        if let Some(v) = nbt.get_float("total_mobs_added_per_player") {
            config.total_mobs_added_per_player = v;
        }
        if let Some(v) = nbt.get_float("simultaneous_mobs_added_per_player") {
            config.simultaneous_mobs_added_per_player = v;
        }
        if let Some(v) = nbt.get_int("ticks_between_spawn") {
            config.ticks_between_spawn = i64::from(v);
        }
        if let Some(list) = nbt.get_list("spawn_potentials") {
            for entry in list {
                let NbtTag::Compound(entry) = entry else {
                    continue;
                };
                let weight = entry.get_int("weight").unwrap_or(1);
                let Some(data) = entry.get_compound("data") else {
                    continue;
                };
                let Some(entity) = data.get_compound("entity") else {
                    continue;
                };
                let Some(id) = entity.get_string("id") else {
                    continue;
                };
                let name = id.strip_prefix("minecraft:").unwrap_or(id);
                if let Some(entity_type) = EntityType::from_name(name) {
                    config
                        .spawn_potentials
                        .push((entity_type, weight, data.clone()));
                }
            }
        }
        // TrialSpawnerConfig.java:30-56 decodes lootTablesToEject as weighted data/table pairs.
        if let Some(list) = nbt.get_list("loot_tables_to_eject") {
            config.loot_tables_to_eject = list
                .iter()
                .filter_map(|entry| {
                    let NbtTag::Compound(entry) = entry else {
                        return None;
                    };
                    Some((
                        entry.get_string("data")?.to_owned(),
                        entry.get_int("weight").unwrap_or(1),
                    ))
                })
                .collect();
        }
        if let Some(key) = nbt.get_string("items_to_drop_when_ominous") {
            key.clone_into(&mut config.items_to_drop_when_ominous);
        }
        config
    }

    // TrialSpawnerConfig.DIRECT_CODEC (TrialSpawnerConfig.java:30-55) encoded inline, the
    // way `Holder.direct` configs (`FullConfig.overrideEntity`) are stored.
    fn to_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_int("spawn_range", self.spawn_range);
        nbt.put_float("total_mobs", self.total_mobs);
        nbt.put_float("simultaneous_mobs", self.simultaneous_mobs);
        nbt.put_float(
            "total_mobs_added_per_player",
            self.total_mobs_added_per_player,
        );
        nbt.put_float(
            "simultaneous_mobs_added_per_player",
            self.simultaneous_mobs_added_per_player,
        );
        nbt.put_int(
            "ticks_between_spawn",
            i32::try_from(self.ticks_between_spawn).unwrap_or(i32::MAX),
        );
        let potentials = self
            .spawn_potentials
            .iter()
            .map(|(_, weight, data)| {
                let mut entry = NbtCompound::new();
                entry.put_compound("data", data.clone());
                entry.put_int("weight", *weight);
                NbtTag::Compound(entry)
            })
            .collect();
        nbt.put_list("spawn_potentials", potentials);
        let tables = self
            .loot_tables_to_eject
            .iter()
            .map(|(table, weight)| {
                let mut entry = NbtCompound::new();
                entry.put_string("data", table.clone());
                entry.put_int("weight", *weight);
                NbtTag::Compound(entry)
            })
            .collect();
        nbt.put_list("loot_tables_to_eject", tables);
        nbt.put_string(
            "items_to_drop_when_ominous",
            self.items_to_drop_when_ominous.clone(),
        );
        nbt
    }

    // TrialSpawnerConfig.java:50-52; TrialSpawnerStateData.java:273-294.
    fn items_to_drop_when_ominous(&self) -> &str {
        &self.items_to_drop_when_ominous
    }

    // TrialSpawnerConfig.java:58-60
    fn calculate_target_total_mobs(&self, additional_players: i32) -> i32 {
        (self.total_mobs + self.total_mobs_added_per_player * additional_players as f32).floor()
            as i32
    }

    // TrialSpawnerConfig.java:62-64
    fn calculate_target_simultaneous_mobs(&self, additional_players: i32) -> i32 {
        (self.simultaneous_mobs
            + self.simultaneous_mobs_added_per_player * additional_players as f32)
            .floor() as i32
    }

    fn pick_random_spawn_data(&self) -> Option<(&'static EntityType, NbtCompound)> {
        let total_weight: i32 = self.spawn_potentials.iter().map(|(_, w, _)| *w).sum();
        if total_weight <= 0 {
            return self
                .spawn_potentials
                .first()
                .map(|(entity, _, data)| (*entity, data.clone()));
        }
        let mut roll = rand::random_range(0..total_weight);
        for (entity, weight, data) in &self.spawn_potentials {
            if roll < *weight {
                return Some((*entity, data.clone()));
            }
            roll -= weight;
        }
        None
    }

    // TrialSpawnerConfig.java:47-49 and TrialSpawnerState.java:132-137: select one configured
    // reward table using its weighted-list entry before the ejection cycle begins.
    fn pick_random_loot_table(&self) -> Option<String> {
        let total_weight: i32 = self
            .loot_tables_to_eject
            .iter()
            .map(|(_, weight)| *weight)
            .sum();
        if total_weight <= 0 {
            return self
                .loot_tables_to_eject
                .first()
                .map(|(table, _)| table.clone());
        }
        let mut roll = rand::random_range(0..total_weight);
        for (table, weight) in &self.loot_tables_to_eject {
            if roll < *weight {
                return Some(table.clone());
            }
            roll -= weight;
        }
        None
    }
}

fn default_loot_tables() -> Vec<(String, i32)> {
    // TrialSpawnerConfig.java:98-102: normal rewards use equal consumables/key weights.
    vec![
        ("minecraft:spawners/trial_chamber/consumables".to_owned(), 1),
        ("minecraft:spawners/trial_chamber/key".to_owned(), 1),
    ]
}

// TrialSpawnerConfigs.java:22-269 (bootstrap registry). The entity compound is
// retained because SpawnData carries more than the registry id (for example the
// baby-zombie and slime-size modifiers).
#[allow(clippy::too_many_lines)]
fn built_in_config(key: &str) -> Option<TrialSpawnerConfig> {
    const D_SIM: f32 = 2.0;
    const D_TOTAL: f32 = 6.0;
    const D_TOTAL_ADD: f32 = 2.0;

    let key = key.strip_prefix("minecraft:").unwrap_or(key);
    let (path, variant) = key.rsplit_once('/')?;
    let is_ominous = match variant {
        "normal" => false,
        "ominous" => true,
        _ => return None,
    };

    // (simultaneous_mobs, simultaneous_mobs_added_per_player, ticks_between_spawn,
    // total_mobs, total_mobs_added_per_player, mob)
    let (sim, sim_add, ticks, total, total_add, mob): (f32, f32, i64, f32, f32, &str) =
        match (path, is_ominous) {
            ("trial_chamber/breeze", false) => (1.0, 0.5, 20, 2.0, 1.0, "breeze"),
            ("trial_chamber/breeze", true) => (D_SIM, 0.5, 20, 4.0, 1.0, "breeze"),
            ("trial_chamber/melee/husk", false | true) => {
                (3.0, 0.5, 20, D_TOTAL, D_TOTAL_ADD, "husk")
            }
            ("trial_chamber/melee/spider", false) => (3.0, 0.5, 20, D_TOTAL, D_TOTAL_ADD, "spider"),
            ("trial_chamber/melee/spider", true) => (4.0, 0.5, 20, 12.0, D_TOTAL_ADD, "spider"),
            ("trial_chamber/melee/zombie", false | true) => {
                (3.0, 0.5, 20, D_TOTAL, D_TOTAL_ADD, "zombie")
            }
            ("trial_chamber/ranged/poison_skeleton", false | true) => {
                (3.0, 0.5, 20, D_TOTAL, D_TOTAL_ADD, "bogged")
            }
            ("trial_chamber/ranged/skeleton", false | true) => {
                (3.0, 0.5, 20, D_TOTAL, D_TOTAL_ADD, "skeleton")
            }
            ("trial_chamber/ranged/stray", false | true) => {
                (3.0, 0.5, 20, D_TOTAL, D_TOTAL_ADD, "stray")
            }
            ("trial_chamber/slow_ranged/poison_skeleton", false | true) => {
                (4.0, 2.0, 160, D_TOTAL, D_TOTAL_ADD, "bogged")
            }
            ("trial_chamber/slow_ranged/skeleton", false | true) => {
                (4.0, 2.0, 160, D_TOTAL, D_TOTAL_ADD, "skeleton")
            }
            ("trial_chamber/slow_ranged/stray", false | true) => {
                (4.0, 2.0, 160, D_TOTAL, D_TOTAL_ADD, "stray")
            }
            ("trial_chamber/small_melee/baby_zombie", false | true) => {
                (D_SIM, 0.5, 20, D_TOTAL, D_TOTAL_ADD, "zombie")
            }
            ("trial_chamber/small_melee/cave_spider", false) => {
                (3.0, 0.5, 20, D_TOTAL, D_TOTAL_ADD, "cave_spider")
            }
            ("trial_chamber/small_melee/cave_spider", true) => {
                (4.0, 0.5, 20, 12.0, D_TOTAL_ADD, "cave_spider")
            }
            ("trial_chamber/small_melee/silverfish", false) => {
                (3.0, 0.5, 20, D_TOTAL, D_TOTAL_ADD, "silverfish")
            }
            ("trial_chamber/small_melee/silverfish", true) => {
                (4.0, 0.5, 20, 12.0, D_TOTAL_ADD, "silverfish")
            }
            ("trial_chamber/small_melee/slime", false) => {
                (3.0, 0.5, 20, D_TOTAL, D_TOTAL_ADD, "slime")
            }
            ("trial_chamber/small_melee/slime", true) => (4.0, 0.5, 20, 12.0, D_TOTAL_ADD, "slime"),
            _ => return None,
        };

    let entity_type = EntityType::from_name(mob)?;
    let mut entity = NbtCompound::new();
    entity.put_string("id", format!("minecraft:{mob}"));
    let mut potentials = Vec::new();
    if mob == "zombie" && path == "trial_chamber/small_melee/baby_zombie" {
        entity.put_bool("IsBaby", true);
        let mut data = NbtCompound::new();
        data.put_compound("entity", entity);
        potentials.push((entity_type, 1, data));
    } else if mob == "slime" {
        for (size, weight) in [(1i8, 3i32), (2i8, 1i32)] {
            let mut entity = NbtCompound::new();
            entity.put_string("id", "minecraft:slime".to_string());
            entity.put_byte("Size", size);
            let mut data = NbtCompound::new();
            data.put_compound("entity", entity);
            potentials.push((entity_type, weight, data));
        }
    } else {
        let mut data = NbtCompound::new();
        data.put_compound("entity", entity);
        if is_ominous {
            let equipment_table = if matches!(
                path,
                "trial_chamber/melee/husk"
                    | "trial_chamber/melee/zombie"
                    | "trial_chamber/small_melee/baby_zombie"
            ) {
                "minecraft:equipment/trial_chamber_melee"
            } else if path.contains("ranged") {
                "minecraft:equipment/trial_chamber_ranged"
            } else {
                "minecraft:equipment/trial_chamber"
            };
            let mut equipment = NbtCompound::new();
            equipment.put_string("loot_table", equipment_table.to_string());
            equipment.put_float("slot_drop_chances", 0.0);
            data.put_compound("equipment", equipment);
        }
        potentials.push((entity_type, 1, data));
    }
    Some(TrialSpawnerConfig {
        spawn_range: 4,
        total_mobs: total,
        simultaneous_mobs: sim,
        total_mobs_added_per_player: total_add,
        simultaneous_mobs_added_per_player: sim_add,
        ticks_between_spawn: ticks,
        spawn_potentials: potentials,
        // TrialSpawnerConfigs.java:39-317: ominous rewards weight the key 3 and consumables 7.
        loot_tables_to_eject: if is_ominous {
            vec![
                ("minecraft:spawners/ominous/trial_chamber/key".to_owned(), 3),
                (
                    "minecraft:spawners/ominous/trial_chamber/consumables".to_owned(),
                    7,
                ),
            ]
        } else {
            default_loot_tables()
        },
        // TrialSpawnerConfig.java:103: the default ominous item table is shared by
        // the built-in normal and ominous configurations.
        items_to_drop_when_ominous: DEFAULT_OMINOUS_ITEMS_LOOT_TABLE.to_owned(),
    })
}

pub struct TrialSpawnerBlockEntity {
    pub position: BlockPos,
    normal_config_nbt: Mutex<Option<NbtTag>>,
    ominous_config_nbt: Mutex<Option<NbtTag>>,
    normal_config: StdMutex<TrialSpawnerConfig>,
    ominous_config: StdMutex<TrialSpawnerConfig>,
    target_cooldown_length: i64,
    required_player_range: f64,
    detected_players: Mutex<HashSet<Uuid>>,
    current_mobs: Mutex<HashSet<Uuid>>,
    cooldown_ends_at: AtomicI64,
    next_mob_spawns_at: AtomicI64,
    total_mobs_spawned: AtomicI32,
    next_spawn_entity: StdMutex<Option<&'static EntityType>>,
    next_spawn_data: StdMutex<Option<NbtCompound>>,
    ejecting_loot_table: StdMutex<Option<String>>,
}

// TrialSpawner.java:56-58
const DEFAULT_TARGET_COOLDOWN_LENGTH: i64 = 36000;
const DEFAULT_REQUIRED_PLAYER_RANGE: f64 = 14.0;
// TrialSpawnerState.java:41-42
const DELAY_BEFORE_EJECT_AFTER_KILLING_LAST_MOB: i64 = 40;
const TIME_BETWEEN_EACH_EJECTION: i64 = 30;
// TrialSpawner.java:59
const MAX_MOB_TRACKING_DISTANCE_SQR: i32 = 47 * 47;
// TrialSpawnerStateData.java:45 and TrialSpawnerConfig.java:66-68
const TRIAL_OMEN_PER_BAD_OMEN_LEVEL: i32 = 18_000;
const OMINOUS_ITEM_SPAWNER_INTERVAL: i64 = 160;

impl TrialSpawnerBlockEntity {
    pub const ID: &'static str = "minecraft:trial_spawner";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            normal_config_nbt: Mutex::const_new(None),
            ominous_config_nbt: Mutex::const_new(None),
            normal_config: StdMutex::new(TrialSpawnerConfig::default()),
            ominous_config: StdMutex::new(TrialSpawnerConfig::default()),
            target_cooldown_length: DEFAULT_TARGET_COOLDOWN_LENGTH,
            required_player_range: DEFAULT_REQUIRED_PLAYER_RANGE,
            detected_players: Mutex::const_new(HashSet::new()),
            current_mobs: Mutex::const_new(HashSet::new()),
            cooldown_ends_at: AtomicI64::new(0),
            next_mob_spawns_at: AtomicI64::new(0),
            total_mobs_spawned: AtomicI32::new(0),
            next_spawn_entity: StdMutex::new(None),
            next_spawn_data: StdMutex::new(None),
            ejecting_loot_table: StdMutex::new(None),
        }
    }

    fn active_config(&self, is_ominous: bool) -> StdMutexGuard<'_, TrialSpawnerConfig> {
        if is_ominous {
            self.ominous_config.lock().unwrap()
        } else {
            self.normal_config.lock().unwrap()
        }
    }

    async fn reset_statistics(&self) {
        self.detected_players.lock().await.clear();
        self.total_mobs_spawned.store(0, Ordering::Relaxed);
        self.next_mob_spawns_at.store(0, Ordering::Relaxed);
        self.cooldown_ends_at.store(0, Ordering::Relaxed);
    }

    // TrialSpawnerStateData.java:82-86. `ejectingLootTable` is deliberately left alone:
    // only the EJECTING_REWARD state clears it (TrialSpawnerState.java:129).
    async fn reset(&self) {
        self.current_mobs.lock().await.clear();
        *self.next_spawn_entity.lock().unwrap() = None;
        *self.next_spawn_data.lock().unwrap() = None;
        self.reset_statistics().await;
    }

    // TrialSpawnerConfig.java:74-87 (`withSpawning`): replaces the spawn potentials
    // with a single weight-1 entry whose SpawnData carries only the entity id.
    fn with_spawning(
        config: &TrialSpawnerConfig,
        entity_type: &'static EntityType,
    ) -> TrialSpawnerConfig {
        let mut entity = NbtCompound::new();
        entity.put_string("id", format!("minecraft:{}", entity_type.resource_name));
        let mut data = NbtCompound::new();
        data.put_compound("entity", entity);
        let mut overridden = config.clone();
        overridden.spawn_potentials = vec![(entity_type, 1, data)];
        overridden
    }

    /// Vanilla `TrialSpawnerBlockEntity#setEntityId` (TrialSpawnerBlockEntity.java:57-65):
    /// delegates to `TrialSpawner#overrideEntityToSpawn` (TrialSpawner.java:340-344),
    /// which resets the state data, swaps the spawn entity in both configs through
    /// `FullConfig#overrideEntity` (TrialSpawner.java:394-401), and forces the block
    /// state back to INACTIVE.
    pub async fn set_entity_id(&self, world: &Arc<World>, entity_type: &'static EntityType) {
        self.reset().await;
        let normal = Self::with_spawning(&self.active_config(false), entity_type);
        let ominous = Self::with_spawning(&self.active_config(true), entity_type);
        // `FullConfig.overrideEntity` builds `Holder.direct` configs, which `store` writes
        // inline; without refreshing the saved compounds the override was lost on reload.
        *self.normal_config_nbt.lock().await = Some(NbtTag::Compound(normal.to_nbt()));
        *self.ominous_config_nbt.lock().await = Some(NbtTag::Compound(ominous.to_nbt()));
        *self.normal_config.lock().unwrap() = normal;
        *self.ominous_config.lock().unwrap() = ominous;

        // TrialSpawner.java:343 (`setState(level, TrialSpawnerState.INACTIVE)`)
        let state_id = world.get_block_state_id(&self.position);
        let block = Block::from_state_id(state_id);
        if TrialSpawnerLikeProperties::handles_block_id(block.id) {
            let mut props = TrialSpawnerLikeProperties::from_state_id(state_id, block);
            props.trial_spawner_state = TrialSpawnerState::Inactive;
            world
                .set_block_state(
                    &self.position,
                    props.to_state_id(block),
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
        }
    }

    async fn count_additional_players(&self) -> i32 {
        // StateData.java:113-119
        (self.detected_players.lock().await.len() as i32 - 1).max(0)
    }

    fn trial_omen_duration(amplifier: u8) -> i32 {
        TRIAL_OMEN_PER_BAD_OMEN_LEVEL * (i32::from(amplifier) + 1)
    }

    // TrialSpawnerBlockEntity.java:87-91
    fn mark_updated(&self, world: &Arc<World>) {
        if let Some(block_entity) = world.get_block_entity(&self.position) {
            world.update_block_entity(&block_entity);
        }
    }

    // TrialSpawnerStateData.java:127-137, 180-200 and TrialSpawner.java:102-107
    async fn apply_ominous(
        &self,
        world: &Arc<World>,
        player: &Arc<crate::entity::player::Player>,
        bad_omen: Option<Effect>,
        game_time: i64,
    ) {
        if let Some(effect) = bad_omen {
            player.remove_effect(&StatusEffect::BAD_OMEN).await;
            player
                .add_effect(Effect {
                    effect_type: &StatusEffect::TRIAL_OMEN,
                    duration: Self::trial_omen_duration(effect.amplifier),
                    amplifier: 0,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                })
                .await;
        }

        world.sync_world_event(
            WorldEvent::ParticlesTrialSpawnerBecomeOminous,
            BlockPos::floored(
                player.eye_position().x,
                player.eye_position().y,
                player.eye_position().z,
            ),
            0,
        );

        let state_id = world.get_block_state_id(&self.position);
        let block = Block::from_state_id(state_id);
        if TrialSpawnerLikeProperties::handles_block_id(block.id) {
            let mut props = TrialSpawnerLikeProperties::from_state_id(state_id, block);
            props.ominous = true;
            world
                .set_block_state(
                    &self.position,
                    props.to_state_id(block),
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
        }
        world.sync_world_event(
            WorldEvent::ParticlesTrialSpawnerBecomeOminous,
            self.position,
            1,
        );

        let mobs = {
            let mut current_mobs = self.current_mobs.lock().await;
            let mobs = current_mobs.iter().copied().collect::<Vec<_>>();
            current_mobs.clear();
            mobs
        };
        for id in mobs {
            if let Some(entity) = world.get_entity_by_uuid(id) {
                // TrialSpawnerStateData.java:180-189 and Mob.java:923-938
                world.sync_world_event(
                    WorldEvent::ParticlesTrialSpawnerSpawnMobAt,
                    entity.get_entity().block_pos.load(),
                    0,
                );
                if let Some(mob) = entity.get_mob() {
                    mob.drop_preserved_equipment().await;
                }
                entity.get_entity().remove().await;
            }
        }

        if !self.active_config(true).spawn_potentials.is_empty() {
            *self.next_spawn_entity.lock().unwrap() = None;
            *self.next_spawn_data.lock().unwrap() = None;
        }
        self.total_mobs_spawned.store(0, Ordering::Relaxed);
        self.next_mob_spawns_at.store(
            game_time + self.active_config(true).ticks_between_spawn,
            Ordering::Relaxed,
        );
        self.cooldown_ends_at
            .store(game_time + OMINOUS_ITEM_SPAWNER_INTERVAL, Ordering::Relaxed);
        self.mark_updated(world);
    }

    // TrialSpawnerState.java:147-150 and TrialSpawner.java:109-112
    async fn remove_ominous(&self, world: &Arc<World>) {
        let state_id = world.get_block_state_id(&self.position);
        let block = Block::from_state_id(state_id);
        if TrialSpawnerLikeProperties::handles_block_id(block.id) {
            let mut props = TrialSpawnerLikeProperties::from_state_id(state_id, block);
            if props.ominous {
                props.ominous = false;
                world
                    .set_block_state(
                        &self.position,
                        props.to_state_id(block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
            }
        }
    }

    // TrialSpawner.java:150-158. overridePeacefulAndMobSpawnRule is a
    // @VisibleForTesting-only escape hatch, never set by gameplay code, so it
    // is omitted.
    fn can_spawn_in_level(world: &Arc<World>) -> bool {
        let level_data = world.level_info.load();
        level_data.game_rules.spawner_blocks_work
            && level_data.difficulty != pumpkin_util::Difficulty::Peaceful
            && level_data.game_rules.spawn_mobs
    }

    // TrialSpawnerStateData.java:121: the scan runs when `(pos.asLong() + gameTime) % 20 == 0`.
    fn detection_tick_due(position: BlockPos, game_time: i64) -> bool {
        position.as_long().wrapping_add(game_time) % 20 == 0
    }

    // PlayerDetector.NO_CREATIVE_PLAYERS (PlayerDetector.java:24-30) selects players whose
    // block position is `closerThan` the spawner block; Vec3i.closerThan compares the
    // squared block distance strictly (Vec3i.java:193-195).
    fn is_within_player_range(&self, player_block: BlockPos) -> bool {
        let dx = player_block.0.x - self.position.0.x;
        let dy = player_block.0.y - self.position.0.y;
        let dz = player_block.0.z - self.position.0.z;
        f64::from(dx * dx + dy * dy + dz * dz)
            < self.required_player_range * self.required_player_range
    }

    // TrialSpawnerStateData.java:123: an ominous spawner in COOLDOWN never rescans.
    const fn scans_for_players(state: TrialSpawnerState, is_ominous: bool) -> bool {
        !matches!(state, TrialSpawnerState::Cooldown) || !is_ominous
    }

    // TrialSpawnerStateData.java:143: while in COOLDOWN, scanned players are only
    // registered when the spawner turned ominous during this scan.
    const fn registers_players(state: TrialSpawnerState, became_ominous: bool) -> bool {
        !matches!(state, TrialSpawnerState::Cooldown) || became_ominous
    }

    // TrialSpawnerStateData.java:120-158; PlayerDetector clips with
    // `ClipContext.Block.VISUAL` (`World::raycast_visual`).
    #[allow(clippy::too_many_lines)]
    async fn try_detect_players(
        &self,
        world: &Arc<World>,
        state: TrialSpawnerState,
        mut is_ominous: bool,
    ) {
        let game_time = world.get_world_age().await;
        if !Self::detection_tick_due(self.position, game_time)
            || !Self::scans_for_players(state, is_ominous)
        {
            return;
        }
        let nearby = world.get_nearby_players(
            self.position.to_centered_f64(),
            self.required_player_range + 1.0,
        );
        let mut eligible = Vec::new();
        let mut visible = Vec::new();
        for player in nearby {
            if matches!(
                player.gamemode.load(),
                GameMode::Spectator | GameMode::Creative
            ) || !self.is_within_player_range(player.living_entity.entity.block_pos.load())
            {
                continue;
            }
            if world
                .raycast_visual(
                    self.position.to_centered_f64(),
                    player.eye_position(),
                    async |block_pos, world| !world.get_block_state(block_pos).is_air(),
                )
                .await
                .is_none_or(|(hit, _)| hit == self.position)
            {
                visible.push(player.clone());
            }
            eligible.push(player);
        }

        let mut became_ominous = false;
        if !is_ominous {
            let mut bad_omen = None;
            let mut ominous_player = None;
            for player in &visible {
                if player.get_effect(&StatusEffect::TRIAL_OMEN).await.is_some() {
                    ominous_player = Some((player.clone(), None));
                    break;
                }
                if bad_omen.is_none()
                    && let Some(effect) = player.get_effect(&StatusEffect::BAD_OMEN).await
                {
                    bad_omen = Some((player.clone(), Some(effect)));
                }
            }
            if let Some((player, effect)) = ominous_player.or(bad_omen) {
                self.apply_ominous(world, &player, effect, game_time).await;
                is_ominous = true;
                became_ominous = true;
            }
        }
        if !Self::registers_players(state, became_ominous) {
            return;
        }

        let searching_for_first_player = self.detected_players.lock().await.is_empty();
        let found: HashSet<Uuid> = (if searching_for_first_player {
            visible
        } else {
            eligible
        })
        .iter()
        .map(|p| p.gameprofile.id)
        .collect();

        let mut detected = self.detected_players.lock().await;
        let before = detected.len();
        detected.extend(found);
        if detected.len() != before {
            self.next_mob_spawns_at
                .fetch_max(game_time + 40, Ordering::Relaxed);
            let event = if is_ominous {
                WorldEvent::ParticlesTrialSpawnerDetectPlayerOminous
            } else {
                WorldEvent::ParticlesTrialSpawnerDetectPlayer
            };
            if !became_ominous {
                world.sync_world_event(event, self.position, detected.len() as i32);
            }
        }
    }

    // TrialSpawnerStateData.java:95-98. `getOrCreateNextSpawnData` runs first, so the next
    // mob is rolled (and synced to clients as the display entity) as soon as the spawner
    // looks for mobs to spawn.
    fn has_mob_to_spawn(&self, world: &Arc<World>, config: &TrialSpawnerConfig) -> bool {
        self.get_or_create_next_spawn_data(world, config).is_some()
            || !config.spawn_potentials.is_empty()
    }

    // TrialSpawnerStateData.java:226-236: rolls the next spawn data when none is stored and
    // calls `markUpdated` so clients receive `spawn_data`.
    fn get_or_create_next_spawn_data(
        &self,
        world: &Arc<World>,
        config: &TrialSpawnerConfig,
    ) -> Option<(&'static EntityType, NbtCompound)> {
        let (result, created) = {
            let mut next = self.next_spawn_entity.lock().unwrap();
            let mut data = self.next_spawn_data.lock().unwrap();
            let created = next.is_none();
            if created {
                let (entity, spawn_data) = config.pick_random_spawn_data()?;
                *next = Some(entity);
                *data = Some(spawn_data);
            }
            let entity = (*next)?;
            let spawn_data = data.clone().unwrap_or_else(|| {
                let mut entity_data = NbtCompound::new();
                entity_data.put_string("id", format!("minecraft:{}", entity.resource_name));
                let mut spawn_data = NbtCompound::new();
                spawn_data.put_compound("entity", entity_data);
                spawn_data
            });
            ((entity, spawn_data), created)
        };
        if created {
            self.mark_updated(world);
        }
        Some(result)
    }

    // TrialSpawner.java:288-291 and PlayerDetector.java:55-58: clip from the spawn position
    // to the spawner centre; the line is clear when nothing but the spawner block is hit.
    // `ClipContext.Block.VISUAL` uses `getVisualShape` (`World::raycast_visual`).
    async fn in_line_of_sight(&self, world: &Arc<World>, dest: Vector3<f64>) -> bool {
        world
            .raycast_visual(
                dest,
                self.position.to_centered_f64(),
                async |block_pos, world| !world.get_block_state(block_pos).is_air(),
            )
            .await
            .is_none_or(|(hit, _)| hit == self.position)
    }

    // TrialSpawner.java:161-234, simplified: no `Pos` override, spawn-placement rules,
    // `checkSpawnObstruction`, `finalizeSpawn` or equipment (all need spawn-reason plumbing
    // that lives in entity/); collision, line of sight and custom spawn rules are kept.
    async fn spawn_mob(
        &self,
        world: &Arc<World>,
        config: &TrialSpawnerConfig,
        is_ominous: bool,
    ) -> Option<Uuid> {
        let (entity_type, spawn_data) = self.get_or_create_next_spawn_data(world, config)?;
        let pos = self.position.0;
        let spawn_range = f64::from(config.spawn_range);
        let spawn_pos = Vector3::new(
            pos.x as f64 + (rand::random::<f64>() - rand::random::<f64>()) * spawn_range + 0.5,
            (pos.y + rand::random_range(0..3) - 1) as f64,
            pos.z as f64 + (rand::random::<f64>() - rand::random::<f64>()) * spawn_range + 0.5,
        );
        // TrialSpawner.java:182 tests `getSpawnAABB`, which applies the spawn dimension scale.
        if !world.is_space_empty(BoundingBox::new_from_pos(
            spawn_pos.x,
            spawn_pos.y,
            spawn_pos.z,
            &spawn_dimensions(entity_type),
        )) {
            return None;
        }
        if !self.in_line_of_sight(world, spawn_pos).await {
            return None;
        }
        let spawn_block_pos = BlockPos::floored(spawn_pos.x, spawn_pos.y, spawn_pos.z);
        if !custom_spawn_rules_allow(world, &spawn_block_pos, &spawn_data) {
            return None;
        }
        let uuid = uuid::Uuid::new_v4();
        let entity = crate::entity::r#type::from_type(entity_type, spawn_pos, world, uuid);
        if let Some(entity_nbt) = spawn_data.get_compound("entity") {
            if let Some(living) = entity.get_living_entity() {
                living.read_nbt_non_mut(entity_nbt).await;
            } else {
                entity.get_entity().read_nbt_non_mut(entity_nbt).await;
            }
            entity.read_nbt_non_mut(entity_nbt).await;
        }
        // TrialSpawner.java:203 `snapTo` runs after the NBT is read and randomises the yaw.
        entity
            .get_entity()
            .set_rotation(rand::random::<f32>() * 360.0, 0.0);
        // TrialSpawner.java:220: trial spawner mobs never despawn.
        if let Some(mob) = entity.get_mob() {
            mob.set_persistence_required();
        }
        world.spawn_entity(entity.clone()).await;
        // TrialSpawner.java:228-230: FlameParticle.encode() is the ordinal, OMINOUS = 1.
        let flame = i32::from(is_ominous);
        world.sync_world_event(WorldEvent::ParticlesTrialSpawnerSpawn, self.position, flame);
        world.sync_world_event(
            WorldEvent::ParticlesTrialSpawnerSpawnMobAt,
            spawn_block_pos,
            flame,
        );
        emit_game_event(
            world,
            GameEvent::EntityPlace,
            spawn_block_pos.to_centered_f64(),
            GameEventContext::of_entity(entity),
        )
        .await;
        {
            let mut next = self.next_spawn_entity.lock().unwrap();
            let mut next_data = self.next_spawn_data.lock().unwrap();
            if let Some((next_entity, spawn_data)) = config.pick_random_spawn_data() {
                *next = Some(next_entity);
                *next_data = Some(spawn_data);
            } else {
                *next = None;
                *next_data = None;
            }
        }
        self.mark_updated(world);
        Some(uuid)
    }

    // TrialSpawner.java:271-290
    async fn untrack_dead_mobs(&self, world: &Arc<World>) -> bool {
        let mut mobs = self.current_mobs.lock().await;
        let before = mobs.len();
        mobs.retain(|id| {
            world.get_entity_by_uuid(*id).is_some_and(|e| {
                e.get_entity().is_alive()
                    && e.get_entity()
                        .block_pos
                        .load()
                        .0
                        .squared_distance_to_vec(&self.position.0)
                        <= MAX_MOB_TRACKING_DISTANCE_SQR
            })
        });
        mobs.len() != before
    }

    // Eject one item from the loot table picked for this reward cycle.
    // TrialSpawner.java:236-247
    async fn eject_reward(&self, world: &Arc<World>, table: &str) {
        // An empty roll ejects nothing and sends no event (TrialSpawner.java:240).
        let Some(item) = spawner_ejection_item(table) else {
            return;
        };
        // `DefaultDispenseItemBehavior.spawnItem(level, item, 2, UP, atBottomCenterOf(pos)
        // .relative(UP, 1.2))` (DefaultDispenseItemBehavior.java:30-49): the stack starts
        // 0.125 below that point and is thrown upwards, rather than dropped inside the block.
        let pos = self.position.0;
        let spawn_pos = Vector3::new(
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 1.2 - 0.125,
            f64::from(pos.z) + 0.5,
        );
        let spread = 0.017_227_5 * 2.0;
        let velocity = Vector3::new(
            triangle(0.0, spread),
            triangle(0.2, spread),
            triangle(0.0, spread),
        );
        let entity = Entity::new(world.clone(), spawn_pos, &EntityType::ITEM);
        // Not vanilla: `World::drop_stack`, which this replaced, fired the cancellable plugin
        // `ItemSpawnEvent`, so keep firing it for rewards.
        let mut item_event = ItemSpawnEvent::new(
            entity.entity_id,
            spawn_pos,
            item.item.registry_key.to_string(),
        );
        if let Some(server) = world.server.upgrade() {
            server.plugin_manager.fire(&server, &mut item_event).await;
        }
        if !item_event.cancelled {
            // `spawnItem` never calls `setDefaultPickUpDelay`, so the stack is pickable at once.
            world
                .spawn_entity(Arc::new(ItemEntity::new_with_velocity(
                    entity, item, velocity, 0,
                )))
                .await;
        }
        world.sync_world_event(WorldEvent::AnimationTrialSpawnerEjectItem, self.position, 0);
    }

    #[allow(clippy::too_many_lines)]
    async fn tick_server(&self, world: &Arc<World>) {
        let state_id = world.get_block_state_id(&self.position);
        let block = Block::from_state_id(state_id);
        if !TrialSpawnerLikeProperties::handles_block_id(block.id) {
            return;
        }
        let props = TrialSpawnerLikeProperties::from_state_id(state_id, block);
        let is_ominous = props.ominous;
        let game_time = world.get_world_age().await;

        if self.untrack_dead_mobs(world).await {
            self.next_mob_spawns_at.store(
                game_time + self.active_config(is_ominous).ticks_between_spawn,
                Ordering::Relaxed,
            );
        }

        let config = self.active_config(is_ominous).clone();
        let next_state = self
            .tick_state_machine(
                world,
                props.trial_spawner_state,
                is_ominous,
                &config,
                game_time,
            )
            .await;

        if next_state != props.trial_spawner_state {
            // TrialSpawnerBlockEntity.setState (TrialSpawnerBlockEntity.java:81-84) applies the
            // state to the block's *current* state. The state machine may have flipped
            // `ominous` since `props` was read (apply_ominous / remove_ominous), and writing the
            // stale copy back would undo that.
            let state_id = world.get_block_state_id(&self.position);
            let block = Block::from_state_id(state_id);
            if TrialSpawnerLikeProperties::handles_block_id(block.id) {
                let mut current = TrialSpawnerLikeProperties::from_state_id(state_id, block);
                current.trial_spawner_state = next_state;
                world
                    .set_block_state(
                        &self.position,
                        current.to_state_id(block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
            }
        }
    }

    // TrialSpawnerState.java:63-155
    async fn tick_state_machine(
        &self,
        world: &Arc<World>,
        current: TrialSpawnerState,
        is_ominous: bool,
        config: &TrialSpawnerConfig,
        game_time: i64,
    ) -> TrialSpawnerState {
        match current {
            // TrialSpawnerState.java:69: the spawner only wakes once a display entity can be
            // created, i.e. once the next spawn data names a mob. Waking unconditionally made an
            // unconfigured spawner flip between INACTIVE and WAITING_FOR_PLAYERS every tick.
            TrialSpawnerState::Inactive => {
                if self.get_or_create_next_spawn_data(world, config).is_some() {
                    TrialSpawnerState::WaitingForPlayers
                } else {
                    TrialSpawnerState::Inactive
                }
            }
            TrialSpawnerState::WaitingForPlayers => {
                if !Self::can_spawn_in_level(world) {
                    self.reset_statistics().await;
                    return TrialSpawnerState::WaitingForPlayers;
                }
                if !self.has_mob_to_spawn(world, config) {
                    return TrialSpawnerState::Inactive;
                }
                self.try_detect_players(world, current, is_ominous).await;
                if self.detected_players.lock().await.is_empty() {
                    TrialSpawnerState::WaitingForPlayers
                } else {
                    TrialSpawnerState::Active
                }
            }
            TrialSpawnerState::Active => {
                self.tick_active_state(world, is_ominous, config, game_time)
                    .await
            }
            TrialSpawnerState::WaitingForRewardEjection => {
                // StateData.java:213-216
                let cooldown_started_at =
                    self.cooldown_ends_at.load(Ordering::Relaxed) - self.target_cooldown_length;
                if game_time >= cooldown_started_at + DELAY_BEFORE_EJECT_AFTER_KILLING_LAST_MOB {
                    world.play_block_sound(
                        Sound::BlockTrialSpawnerOpenShutter,
                        SoundCategory::Blocks,
                        self.position,
                    );
                    TrialSpawnerState::EjectingReward
                } else {
                    TrialSpawnerState::WaitingForRewardEjection
                }
            }
            TrialSpawnerState::EjectingReward => {
                let cooldown_started_at =
                    self.cooldown_ends_at.load(Ordering::Relaxed) - self.target_cooldown_length;
                if (game_time - cooldown_started_at) % TIME_BETWEEN_EACH_EJECTION != 0 {
                    return TrialSpawnerState::EjectingReward;
                }
                if self.detected_players.lock().await.is_empty() {
                    *self.ejecting_loot_table.lock().unwrap() = None;
                    world.play_block_sound(
                        Sound::BlockTrialSpawnerCloseShutter,
                        SoundCategory::Blocks,
                        self.position,
                    );
                    TrialSpawnerState::Cooldown
                } else {
                    // TrialSpawnerState.java:132-134 rolls the configured weighted table
                    // (TrialSpawnerConfig.java:47-49) on the first ejection of a reward cycle.
                    let table = {
                        let mut ejecting = self.ejecting_loot_table.lock().unwrap();
                        if ejecting.is_none() {
                            *ejecting = config.pick_random_loot_table();
                        }
                        ejecting.clone()
                    };
                    if let Some(table) = table.as_deref() {
                        self.eject_reward(world, table).await;
                    }
                    let mut detected = self.detected_players.lock().await;
                    if let Some(&first) = detected.iter().next() {
                        detected.remove(&first);
                    }
                    TrialSpawnerState::EjectingReward
                }
            }
            TrialSpawnerState::Cooldown => {
                self.try_detect_players(world, current, is_ominous).await;
                if !self.detected_players.lock().await.is_empty() {
                    self.total_mobs_spawned.store(0, Ordering::Relaxed);
                    self.next_mob_spawns_at.store(0, Ordering::Relaxed);
                    TrialSpawnerState::Active
                } else if game_time >= self.cooldown_ends_at.load(Ordering::Relaxed) {
                    self.remove_ominous(world).await;
                    self.reset().await;
                    TrialSpawnerState::WaitingForPlayers
                } else {
                    TrialSpawnerState::Cooldown
                }
            }
        }
    }

    /// `TrialSpawnerState.ACTIVE.tick` (`TrialSpawnerState.java`, `ACTIVE` case): counts nearby
    /// players, spawns mobs up to the simultaneous/total caps, and transitions to
    /// `WaitingForRewardEjection` once the total cap is hit and all spawned mobs are dead.
    async fn tick_active_state(
        &self,
        world: &Arc<World>,
        is_ominous: bool,
        config: &TrialSpawnerConfig,
        game_time: i64,
    ) -> TrialSpawnerState {
        if !Self::can_spawn_in_level(world) {
            self.reset_statistics().await;
            return TrialSpawnerState::WaitingForPlayers;
        }
        if !self.has_mob_to_spawn(world, config) {
            return TrialSpawnerState::Inactive;
        }
        let additional_players = self.count_additional_players().await;
        self.try_detect_players(world, TrialSpawnerState::Active, is_ominous)
            .await;
        if is_ominous {
            self.spawn_ominous_item_spawner(world, config, game_time)
                .await;
        }

        let total_spawned = self.total_mobs_spawned.load(Ordering::Relaxed);
        if total_spawned >= config.calculate_target_total_mobs(additional_players) {
            if self.current_mobs.lock().await.is_empty() {
                self.cooldown_ends_at
                    .store(game_time + self.target_cooldown_length, Ordering::Relaxed);
                self.total_mobs_spawned.store(0, Ordering::Relaxed);
                self.next_mob_spawns_at.store(0, Ordering::Relaxed);
                return TrialSpawnerState::WaitingForRewardEjection;
            }
        } else if game_time >= self.next_mob_spawns_at.load(Ordering::Relaxed)
            && self.current_mobs.lock().await.len()
                < config.calculate_target_simultaneous_mobs(additional_players) as usize
            && let Some(uuid) = self.spawn_mob(world, config, is_ominous).await
        {
            self.current_mobs.lock().await.insert(uuid);
            self.total_mobs_spawned.fetch_add(1, Ordering::Relaxed);
            self.next_mob_spawns_at
                .store(game_time + config.ticks_between_spawn, Ordering::Relaxed);
        }
        TrialSpawnerState::Active
    }

    // TrialSpawnerState.java:158-171 and OminousItemSpawner.java:37-42 create one
    // delayed item-spawner above a nearby detected entity at the configured cadence.
    async fn spawn_ominous_item_spawner(
        &self,
        world: &Arc<World>,
        config: &TrialSpawnerConfig,
        game_time: i64,
    ) {
        if game_time < self.cooldown_ends_at.load(Ordering::Relaxed) {
            return;
        }
        let Some(item) = ominous_spawner_item(config.items_to_drop_when_ominous()) else {
            return;
        };
        let Some(spawn_pos) = self.calculate_position_to_spawn_spawner(world).await else {
            return;
        };
        let entity = Entity::new(world.clone(), spawn_pos, &EntityType::OMINOUS_ITEM_SPAWNER);
        let item_spawner = OminousItemSpawnerEntity::create(entity, item);
        world.spawn_entity(item_spawner).await;
        let pitch = (rand::random::<f32>() - rand::random::<f32>()).mul_add(0.2, 1.0);
        world.play_sound_fine(
            Sound::BlockTrialSpawnerSpawnItemBegin,
            SoundCategory::Blocks,
            &BlockPos::floored(spawn_pos.x, spawn_pos.y, spawn_pos.z).to_centered_f64(),
            1.0,
            pitch,
        );
        self.cooldown_ends_at
            .store(game_time + OMINOUS_ITEM_SPAWNER_INTERVAL, Ordering::Relaxed);
    }

    // TrialSpawnerState.java:176-221: pick a non-creative, non-spectator living detected
    // player in range (none -> no spawn), then on a coin flip either a living tracked mob in
    // range or one of those players, and place the item spawner above it. Only the target's
    // position and bounding-box height are needed, so candidates are kept as that pair.
    async fn calculate_position_to_spawn_spawner(
        &self,
        world: &Arc<World>,
    ) -> Option<Vector3<f64>> {
        let center = self.position.to_centered_f64();
        let range_sq = self.required_player_range * self.required_player_range;
        let in_range = |pos: Vector3<f64>| pos.squared_distance_to_vec(&center) <= range_sq;
        let height_of = |entity: &Entity| f64::from(entity.entity_dimension.load().height);

        let nearby_players: Vec<(Vector3<f64>, f64)> = self
            .detected_players
            .lock()
            .await
            .iter()
            .filter_map(|id| world.get_player_by_uuid(*id))
            .filter(|player| {
                let entity = &player.living_entity.entity;
                !matches!(
                    player.gamemode.load(),
                    GameMode::Creative | GameMode::Spectator
                ) && !entity.is_removed()
                    && !player.living_entity.is_dead_or_dying()
                    && in_range(entity.pos.load())
            })
            .map(|player| {
                let entity = &player.living_entity.entity;
                (entity.pos.load(), height_of(entity))
            })
            .collect();
        if nearby_players.is_empty() {
            return None;
        }

        let eligible = if rand::random::<bool>() {
            self.current_mobs
                .lock()
                .await
                .iter()
                .filter_map(|id| world.get_entity_by_uuid(*id))
                .filter(|mob| {
                    let entity = mob.get_entity();
                    !entity.is_removed()
                        && mob
                            .get_living_entity()
                            .is_none_or(|living| !living.is_dead_or_dying())
                        && in_range(entity.pos.load())
                })
                .map(|mob| {
                    let entity = mob.get_entity();
                    (entity.pos.load(), height_of(entity))
                })
                .collect()
        } else {
            nearby_players
        };
        let (target_pos, target_height) = match eligible.len() {
            0 => return None,
            1 => eligible.first().copied()?,
            len => eligible.get(rand::random_range(0..len)).copied()?,
        };
        Self::calculate_position_above(world, target_pos, target_height).await
    }

    // TrialSpawnerState.java:198-205: clip (VISUAL, no fluids) from the entity up to
    // `height + 2 + nextInt(4)` above it and sit one block below the centre of the block that
    // was hit (or of the end block on a miss); refused when that spot has a collision shape.
    async fn calculate_position_above(
        world: &Arc<World>,
        entity_pos: Vector3<f64>,
        height: f64,
    ) -> Option<Vector3<f64>> {
        let try_spawn_pos = Vector3::new(
            entity_pos.x,
            entity_pos.y + height + 2.0 + f64::from(rand::random_range(0..4u8)),
            entity_pos.z,
        );
        let hit_block = world
            .raycast_visual(
                entity_pos,
                try_spawn_pos,
                async |block_pos, world| !world.get_block_state(block_pos).is_air(),
            )
            .await
            .map_or_else(
                || BlockPos::floored(try_spawn_pos.x, try_spawn_pos.y, try_spawn_pos.z),
                |(hit, _)| hit,
            );
        let center = hit_block.to_centered_f64();
        let down = Vector3::new(center.x, center.y - 1.0, center.z);
        let down_block = BlockPos::floored(down.x, down.y, down.z);
        world
            .get_block_state(&down_block)
            .collision_shapes
            .is_empty()
            .then_some(down)
    }
}

// `RandomSource.triangle(min, max)`: `min + max * (nextDouble - nextDouble)`.
fn triangle(min: f64, max: f64) -> f64 {
    (rand::random::<f64>() - rand::random::<f64>()).mul_add(max, min)
}

fn custom_spawn_rules_allow(world: &World, pos: &BlockPos, spawn_data: &NbtCompound) -> bool {
    let Some(rules) = spawn_data.get_compound("custom_spawn_rules") else {
        return true;
    };

    let in_range = |name: &str, value: u8| {
        let Some(range) = rules.get_compound(name) else {
            return true;
        };
        let min = range.get_int("min_inclusive").unwrap_or(0).clamp(0, 15) as u8;
        let max = range.get_int("max_inclusive").unwrap_or(15).clamp(0, 15) as u8;
        (min..=max).contains(&value)
    };

    in_range(
        "block_light_limit",
        world.get_block_light_level(pos).unwrap_or(0),
    ) && in_range(
        "sky_light_limit",
        world
            .get_sky_light_level(pos)
            .saturating_sub(world.sky_darken.load(Ordering::Relaxed)),
    )
}

impl BlockEntity for TrialSpawnerBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn tick<'a>(&'a self, world: &'a Arc<World>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move { self.tick_server(world).await })
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let normal_config_nbt = nbt.get("normal_config").cloned();
        let ominous_config_nbt = nbt.get("ominous_config").cloned();
        let normal_config = TrialSpawnerConfig::from_nbt(normal_config_nbt.as_ref());
        let ominous_config = TrialSpawnerConfig::from_nbt(ominous_config_nbt.as_ref());
        let target_cooldown_length = nbt
            .get_int("target_cooldown_length")
            .map_or(DEFAULT_TARGET_COOLDOWN_LENGTH, i64::from);
        let required_player_range = nbt
            .get_int("required_player_range")
            .map_or(DEFAULT_REQUIRED_PLAYER_RANGE, f64::from);

        // 26.2 stores TrialSpawnerStateData.Packed directly at the block-entity
        // root. Accept the old nested form so existing Pumpkin worlds upgrade
        // without losing their cooldown or tracked entities.
        let packed = nbt.get_compound("spawner_data").unwrap_or(nbt);
        let detected_players = packed
            .get_list("registered_players")
            .map(parse_uuid_list)
            .unwrap_or_default();
        let current_mobs = packed
            .get_list("current_mobs")
            .map(parse_uuid_list)
            .unwrap_or_default();
        let cooldown_ends_at = packed.get_long("cooldown_ends_at").unwrap_or(0);
        let next_mob_spawns_at = packed.get_long("next_mob_spawns_at").unwrap_or(0);
        let total_mobs_spawned = packed.get_int("total_mobs_spawned").unwrap_or(0);
        let next_spawn_data = packed.get_compound("spawn_data").cloned();
        let next_spawn_entity = next_spawn_data
            .as_ref()
            .and_then(|data| data.get_compound("entity"))
            .and_then(|entity| entity.get_string("id"))
            .and_then(|id| EntityType::from_name(id.strip_prefix("minecraft:").unwrap_or(id)));
        let ejecting_loot_table = packed
            .get_string("ejecting_loot_table")
            .map(ToOwned::to_owned);

        Self {
            position,
            normal_config_nbt: Mutex::new(normal_config_nbt),
            ominous_config_nbt: Mutex::new(ominous_config_nbt),
            normal_config: StdMutex::new(normal_config),
            ominous_config: StdMutex::new(ominous_config),
            target_cooldown_length,
            required_player_range,
            detected_players: Mutex::new(detected_players),
            current_mobs: Mutex::new(current_mobs),
            cooldown_ends_at: AtomicI64::new(cooldown_ends_at),
            next_mob_spawns_at: AtomicI64::new(next_mob_spawns_at),
            total_mobs_spawned: AtomicI32::new(total_mobs_spawned),
            next_spawn_entity: StdMutex::new(next_spawn_entity),
            next_spawn_data: StdMutex::new(next_spawn_data),
            ejecting_loot_table: StdMutex::new(ejecting_loot_table),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if let Some(cfg) = self.normal_config_nbt.lock().await.as_ref() {
                nbt.put("normal_config", cfg.clone());
            }
            if let Some(cfg) = self.ominous_config_nbt.lock().await.as_ref() {
                nbt.put("ominous_config", cfg.clone());
            }
            nbt.put_int(
                "target_cooldown_length",
                i32::try_from(self.target_cooldown_length).unwrap_or(i32::MAX),
            );
            nbt.put_int("required_player_range", self.required_player_range as i32);

            let players: Vec<NbtTag> = self
                .detected_players
                .lock()
                .await
                .iter()
                .map(|u| uuid_to_int_array(*u))
                .collect();
            nbt.put_list("registered_players", players);
            let mobs: Vec<NbtTag> = self
                .current_mobs
                .lock()
                .await
                .iter()
                .map(|u| uuid_to_int_array(*u))
                .collect();
            nbt.put_list("current_mobs", mobs);
            nbt.put_long(
                "cooldown_ends_at",
                self.cooldown_ends_at.load(Ordering::Relaxed),
            );
            nbt.put_long(
                "next_mob_spawns_at",
                self.next_mob_spawns_at.load(Ordering::Relaxed),
            );
            nbt.put_int(
                "total_mobs_spawned",
                self.total_mobs_spawned.load(Ordering::Relaxed),
            );
            if let Some(spawn_data) = self.next_spawn_data.lock().unwrap().as_ref() {
                nbt.put_compound("spawn_data", spawn_data.clone());
            }
            if let Some(table) = self.ejecting_loot_table.lock().unwrap().as_ref() {
                nbt.put_string("ejecting_loot_table", table.clone());
            }
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        Some(NbtCompound::new())
    }

    fn chunk_data_nbt_with_state(&self, block_state: BlockStateId) -> Option<NbtCompound> {
        // TrialSpawnerBlockEntity.getUpdateTag sends TrialSpawnerStateData. The
        // configs are server-save data and contain registry-backed codecs that
        // the client must never decode from this update packet.
        let mut nbt = NbtCompound::new();
        let block = Block::from_state_id(block_state);
        if TrialSpawnerLikeProperties::handles_block_id(block.id)
            && TrialSpawnerLikeProperties::from_state_id(block_state, block).trial_spawner_state
                == TrialSpawnerState::Active
        {
            nbt.put_long(
                "next_mob_spawns_at",
                self.next_mob_spawns_at.load(Ordering::Relaxed),
            );
        }

        let next_data = self.next_spawn_data.lock().unwrap();
        if let Some(spawn_data) = next_data.as_ref() {
            nbt.put_compound("spawn_data", spawn_data.clone());
        } else {
            drop(next_data);
            let next = self.next_spawn_entity.lock().unwrap();
            let Some(entity_type) = *next else {
                return Some(nbt);
            };
            let mut entity = NbtCompound::new();
            entity.put_string("id", format!("minecraft:{}", entity_type.resource_name));
            let mut spawn_data = NbtCompound::new();
            spawn_data.put_compound("entity", entity);
            nbt.put_compound("spawn_data", spawn_data);
        }
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn potion_item(
    item: &'static pumpkin_data::item::Item,
    potion_name: &str,
) -> pumpkin_data::item_stack::ItemStack {
    let mut stack = pumpkin_data::item_stack::ItemStack::new(1, item);
    // SetPotionFunction.run (SetPotionFunction.java:32) is `itemStack.update(POTION_CONTENTS,
    // EMPTY, potion, PotionContents::withPotion)`, which replaces the component in place
    // (ItemStack.update -> set). `ItemStack::new(POTION)` already carries a WATER
    // `PotionContents` patch entry and `get_data_component` returns the first match, so
    // pushing a second entry left the ejected potion reading as water.
    if let Some(potion) = pumpkin_data::potion::Potion::from_name(potion_name) {
        let potion_id = Some(potion.id as i32);
        if let Some(contents) =
            stack.get_data_component_mut::<pumpkin_data::data_component_impl::PotionContentsImpl>()
        {
            contents.potion_id = potion_id;
        } else {
            stack.patch.push((
                pumpkin_data::data_component::DataComponent::PotionContents,
                Some(Box::new(
                    pumpkin_data::data_component_impl::PotionContentsImpl {
                        potion_id,
                        custom_color: None,
                        custom_effects: Vec::new(),
                        custom_name: None,
                    },
                )),
            ));
        }
    }
    stack
}

// items_to_drop_when_ominous.json:1-179 contains one uniform roll from each
// pool; the generated server loot tables do not include this spawner namespace.
fn ominous_spawner_item(table: &str) -> Option<pumpkin_data::item_stack::ItemStack> {
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;

    if table != DEFAULT_OMINOUS_ITEMS_LOOT_TABLE {
        return None;
    }

    let first_pool = match rand::random_range(0..7u8) {
        0 => potion_item(&Item::LINGERING_POTION, "wind_charged"),
        1 => potion_item(&Item::LINGERING_POTION, "oozing"),
        2 => potion_item(&Item::LINGERING_POTION, "weaving"),
        3 => potion_item(&Item::LINGERING_POTION, "infested"),
        4 => potion_item(&Item::LINGERING_POTION, "strength"),
        5 => potion_item(&Item::LINGERING_POTION, "swiftness"),
        _ => potion_item(&Item::LINGERING_POTION, "slow_falling"),
    };
    let second_pool = match rand::random_range(0..5u8) {
        0 => ItemStack::new(1, &Item::ARROW),
        1 => potion_item(&Item::TIPPED_ARROW, "poison"),
        2 => potion_item(&Item::TIPPED_ARROW, "strong_slowness"),
        3 => ItemStack::new(1u8 + rand::random_range(0..3u8), &Item::FIRE_CHARGE),
        _ => ItemStack::new(1u8 + rand::random_range(0..3u8), &Item::WIND_CHARGE),
    };

    let total_weight = u16::from(first_pool.item_count) + u16::from(second_pool.item_count);
    if rand::random_range(0..total_weight) < u16::from(first_pool.item_count) {
        Some(first_pool)
    } else {
        Some(second_pool)
    }
}

// Hand-ported (no generic loot-table registry entry exists for the
// "spawners/*" namespace, only "chests/*"), keyed by the full table id so the ominous
// variants are not mistaken for the normal ones:
// data/minecraft/loot_table/spawners/trial_chamber/{consumables,key}.json and
// data/minecraft/loot_table/spawners/ominous/trial_chamber/{consumables,key}.json.
fn spawner_ejection_item(table: &str) -> Option<pumpkin_data::item_stack::ItemStack> {
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;

    // `set_count` with a `uniform` provider of whole numbers.
    fn uniform_stack(item: &'static Item, min: u8, max: u8) -> ItemStack {
        ItemStack::new(rand::random_range(min..=max), item)
    }

    let entries: [(i32, fn() -> ItemStack); 5] =
        match table.strip_prefix("minecraft:").unwrap_or(table) {
            "spawners/trial_chamber/consumables" => [
                (3, || ItemStack::new(1, &Item::COOKED_CHICKEN)),
                (3, || uniform_stack(&Item::BREAD, 1, 3)),
                (2, || uniform_stack(&Item::BAKED_POTATO, 1, 3)),
                (1, || potion_item(&Item::POTION, "regeneration")),
                (1, || potion_item(&Item::POTION, "swiftness")),
            ],
            "spawners/ominous/trial_chamber/consumables" => [
                (3, || uniform_stack(&Item::COOKED_BEEF, 1, 2)),
                (3, || uniform_stack(&Item::BAKED_POTATO, 2, 4)),
                (2, || uniform_stack(&Item::GOLDEN_CARROT, 1, 2)),
                (1, || potion_item(&Item::POTION, "regeneration")),
                (1, || potion_item(&Item::POTION, "strength")),
            ],
            "spawners/trial_chamber/key" => {
                return Some(ItemStack::new(1, &Item::TRIAL_KEY));
            }
            "spawners/ominous/trial_chamber/key" => {
                return Some(ItemStack::new(1, &Item::OMINOUS_TRIAL_KEY));
            }
            _ => return None,
        };
    let total: i32 = entries.iter().map(|(weight, _)| *weight).sum();
    let mut roll = rand::random_range(0..total);
    for (weight, make) in entries {
        if roll < weight {
            return Some(make());
        }
        roll -= weight;
    }
    None
}

fn parse_uuid_list(list: &[NbtTag]) -> HashSet<Uuid> {
    list.iter()
        .filter_map(|tag| {
            let NbtTag::IntArray(v) = tag else {
                return None;
            };
            let &[a, b, c, d] = v.as_slice() else {
                return None;
            };
            Some(Uuid::from_u128(
                ((a as u32 as u128) << 96)
                    | ((b as u32 as u128) << 64)
                    | ((c as u32 as u128) << 32)
                    | (d as u32 as u128),
            ))
        })
        .collect()
}

fn uuid_to_int_array(u: Uuid) -> NbtTag {
    let v = u.as_u128();
    NbtTag::IntArray(vec![
        (v >> 96) as i32,
        ((v >> 64) & 0xFFFF_FFFF) as i32,
        ((v >> 32) & 0xFFFF_FFFF) as i32,
        (v & 0xFFFF_FFFF) as i32,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> TrialSpawnerConfig {
        TrialSpawnerConfig::default()
    }

    #[test]
    fn target_total_mobs_scales_with_players() {
        let c = config();
        assert_eq!(c.calculate_target_total_mobs(0), 6);
        assert_eq!(c.calculate_target_total_mobs(1), 8);
        assert_eq!(c.calculate_target_total_mobs(3), 12);
    }

    #[test]
    fn target_simultaneous_mobs_scales_with_players() {
        let c = config();
        assert_eq!(c.calculate_target_simultaneous_mobs(0), 2);
        assert_eq!(c.calculate_target_simultaneous_mobs(2), 4);
    }

    #[test]
    fn cooldown_start_time_derivation() {
        let target_cooldown_length = DEFAULT_TARGET_COOLDOWN_LENGTH;
        let cooldown_ends_at = 100_000 + target_cooldown_length;
        let cooldown_started_at = cooldown_ends_at - target_cooldown_length;
        assert_eq!(cooldown_started_at, 100_000);
        assert_eq!(
            cooldown_started_at + DELAY_BEFORE_EJECT_AFTER_KILLING_LAST_MOB,
            100_000 + DELAY_BEFORE_EJECT_AFTER_KILLING_LAST_MOB
        );
    }

    #[test]
    fn bad_omen_duration_scales_with_amplifier() {
        // TrialSpawnerStateData.java:202-209 converts Bad Omen to Trial Omen for
        // 18000 ticks per one-based Bad Omen amplifier.
        assert_eq!(TrialSpawnerBlockEntity::trial_omen_duration(0), 18_000);
        assert_eq!(TrialSpawnerBlockEntity::trial_omen_duration(2), 54_000);
    }

    #[test]
    fn eject_items_cadence_matches_time_between_ejections() {
        let cooldown_started_at: i64 = 1000;
        for offset in 0..90 {
            let game_time = cooldown_started_at + offset;
            let is_eject_tick = (game_time - cooldown_started_at) % TIME_BETWEEN_EACH_EJECTION == 0;
            assert_eq!(is_eject_tick, offset % TIME_BETWEEN_EACH_EJECTION == 0);
        }
    }

    #[test]
    fn default_config_matches_vanilla_builder() {
        let c = config();
        assert_eq!(c.spawn_range, 4);
        assert!((c.total_mobs - 6.0).abs() < f32::EPSILON);
        assert!((c.simultaneous_mobs - 2.0).abs() < f32::EPSILON);
        assert_eq!(c.ticks_between_spawn, 40);
        // TrialSpawnerConfig.java:98-102 supplies the two default reward tables with weight 1.
        assert_eq!(c.loot_tables_to_eject.len(), 2);
        assert!(
            c.loot_tables_to_eject
                .iter()
                .all(|(_, weight)| *weight == 1)
        );
        assert_eq!(
            c.items_to_drop_when_ominous,
            DEFAULT_OMINOUS_ITEMS_LOOT_TABLE
        );
    }

    // TrialSpawnerConfig.java:50-52 accepts an overridable ominous item table key.
    #[test]
    fn ominous_item_table_key_is_loaded_and_resolved() {
        let mut nbt = NbtCompound::new();
        nbt.put_string(
            "items_to_drop_when_ominous",
            "minecraft:custom/table".to_string(),
        );
        let config = TrialSpawnerConfig::from_compound(&nbt);
        assert_eq!(config.items_to_drop_when_ominous, "minecraft:custom/table");
        assert!(ominous_spawner_item(DEFAULT_OMINOUS_ITEMS_LOOT_TABLE).is_some());
        assert!(ominous_spawner_item("minecraft:custom/table").is_none());
    }

    #[test]
    fn built_in_config_resolves_structure_baked_resource_key() {
        let cfg = built_in_config("minecraft:trial_chamber/melee/zombie/normal")
            .expect("known key must resolve");
        assert!(!cfg.spawn_potentials.is_empty());
        assert_eq!(cfg.spawn_potentials[0].0.id, EntityType::ZOMBIE.id);
        assert_eq!(cfg.ticks_between_spawn, 20);
    }

    #[test]
    fn built_in_config_rejects_unknown_key() {
        assert!(built_in_config("minecraft:not_a_real_config/normal").is_none());
    }

    // TrialSpawnerStateData.java:121 throttles on `pos.asLong() + gameTime`, whose packed
    // layout (x << 38 | z << 12 | y) is not the coordinate sum.
    #[test]
    fn player_scan_throttle_uses_packed_block_position() {
        let due = TrialSpawnerBlockEntity::detection_tick_due;
        assert!(due(BlockPos::new(0, 0, 0), 0));
        assert!(!due(BlockPos::new(0, 0, 0), 1));
        assert!(due(BlockPos::new(0, 3, 0), 17));
        // x = 1 packs to 1 << 38, which is 4 (mod 20).
        assert!(due(BlockPos::new(1, 0, 0), 16));
        assert!(!due(BlockPos::new(1, 0, 0), 19));
        // z = 1 packs to 1 << 12, which is 16 (mod 20).
        assert!(due(BlockPos::new(0, 0, 1), 4));
        // x = -1 packs to -(1 << 38); Java's `%` keeps the dividend's sign, as Rust's does.
        assert!(due(BlockPos::new(-1, 0, 0), 4));
        assert!(!due(BlockPos::new(-1, 0, 0), 0));
    }

    // TrialSpawnerStateData.java:123 and :143.
    #[test]
    fn cooldown_only_registers_players_when_turning_ominous() {
        use TrialSpawnerState::{Active, Cooldown, WaitingForPlayers};
        type Entity = TrialSpawnerBlockEntity;
        assert!(!Entity::scans_for_players(Cooldown, true));
        assert!(Entity::scans_for_players(Cooldown, false));
        assert!(Entity::scans_for_players(Active, true));
        assert!(!Entity::registers_players(Cooldown, false));
        assert!(Entity::registers_players(Cooldown, true));
        assert!(Entity::registers_players(WaitingForPlayers, false));
        assert!(Entity::registers_players(Active, false));
    }

    // PlayerDetector.NO_CREATIVE_PLAYERS compares block positions with the strict
    // `Vec3i.closerThan`.
    #[test]
    fn player_range_is_a_strict_block_distance() {
        let spawner = TrialSpawnerBlockEntity::new(BlockPos::new(0, 0, 0));
        assert!(spawner.is_within_player_range(BlockPos::new(13, 0, 0)));
        assert!(!spawner.is_within_player_range(BlockPos::new(14, 0, 0)));
        assert!(!spawner.is_within_player_range(BlockPos::new(10, 10, 0)));
        assert!(spawner.is_within_player_range(BlockPos::new(-8, 8, 8)));
    }

    #[test]
    fn overridden_config_round_trips_through_inline_nbt() {
        // `FullConfig.overrideEntity` results are stored inline, so `to_nbt` must reload to
        // the same spawn entity.
        let overridden = TrialSpawnerBlockEntity::with_spawning(
            &TrialSpawnerConfig::default(),
            &EntityType::HUSK,
        );
        let restored = TrialSpawnerConfig::from_nbt(Some(&NbtTag::Compound(overridden.to_nbt())));
        assert_eq!(restored.spawn_potentials.len(), 1);
        assert_eq!(restored.spawn_potentials[0].0.id, EntityType::HUSK.id);
        assert_eq!(restored.spawn_potentials[0].1, 1);
        assert_eq!(
            restored.loot_tables_to_eject,
            overridden.loot_tables_to_eject
        );
        assert_eq!(restored.ticks_between_spawn, overridden.ticks_between_spawn);
    }

    #[test]
    fn built_in_ominous_config_round_trips_through_inline_nbt() {
        let config = built_in_config("minecraft:trial_chamber/melee/zombie/ominous")
            .expect("known key must resolve");
        let restored = TrialSpawnerConfig::from_nbt(Some(&NbtTag::Compound(config.to_nbt())));
        assert!((restored.total_mobs - config.total_mobs).abs() < f32::EPSILON);
        assert!(
            (restored.simultaneous_mobs_added_per_player
                - config.simultaneous_mobs_added_per_player)
                .abs()
                < f32::EPSILON
        );
        assert_eq!(restored.loot_tables_to_eject, config.loot_tables_to_eject);
        assert!(
            restored.spawn_potentials[0]
                .2
                .get_compound("equipment")
                .is_some()
        );
    }

    // spawners/ominous/trial_chamber/key.json rewards `ominous_trial_key`; the ominous
    // tables were previously matched by their `/key` and `/consumables` suffixes.
    #[test]
    fn ejection_tables_are_matched_by_full_id() {
        use pumpkin_data::item::Item;
        let normal = spawner_ejection_item("minecraft:spawners/trial_chamber/key")
            .expect("normal key table");
        assert_eq!(normal.item.id, Item::TRIAL_KEY.id);
        let ominous = spawner_ejection_item("minecraft:spawners/ominous/trial_chamber/key")
            .expect("ominous key table");
        assert_eq!(ominous.item.id, Item::OMINOUS_TRIAL_KEY.id);
        assert!(spawner_ejection_item("minecraft:custom/key").is_none());
        assert!(spawner_ejection_item("minecraft:custom/consumables").is_none());
    }

    #[test]
    fn ominous_consumables_use_the_ominous_pool() {
        use pumpkin_data::item::Item;
        let allowed = [
            Item::COOKED_BEEF.id,
            Item::BAKED_POTATO.id,
            Item::GOLDEN_CARROT.id,
            Item::POTION.id,
        ];
        for _ in 0..200 {
            let stack =
                spawner_ejection_item("minecraft:spawners/ominous/trial_chamber/consumables")
                    .expect("ominous consumables table");
            assert!(allowed.contains(&stack.item.id));
            if stack.item.id == Item::POTION.id {
                assert!(!stack.patch.is_empty(), "set_potion must be applied");
            }
        }
    }

    // `Potion::from_name` takes the bare registry path; a namespaced name silently produced
    // an empty potion.
    #[test]
    fn potion_item_applies_the_named_potion() {
        use pumpkin_data::item::Item;
        let stack = potion_item(&Item::POTION, "regeneration");
        // set_potion replaces the default WATER contents: one component, naming regeneration.
        assert_eq!(stack.patch.len(), 1);
        assert_eq!(
            stack
                .get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
                .and_then(|contents| contents.potion_id),
            Some(pumpkin_data::potion::Potion::REGENERATION.id as i32)
        );
    }

    // `ClipContext.Block.VISUAL` clips through `Shapes.empty()` visual shapes (TransparentBlock,
    // IronBarsBlock, PowderSnowBlock) but stops at blocks that keep the default collision shape.
    #[test]
    fn visual_line_of_sight_passes_through_empty_visual_shapes() {
        for block in [
            &Block::GLASS,
            &Block::WHITE_STAINED_GLASS,
            &Block::TINTED_GLASS,
            &Block::GLASS_PANE,
            &Block::BLACK_STAINED_GLASS_PANE,
            &Block::IRON_BARS,
            &Block::COPPER_BARS,
            &Block::WAXED_OXIDIZED_COPPER_BARS,
            &Block::COPPER_GRATE,
            &Block::WAXED_WEATHERED_COPPER_GRATE,
            &Block::POWDER_SNOW,
        ] {
            assert!(crate::world::has_empty_visual_shape(block), "{}", block.name);
        }
        for block in [
            &Block::STONE,
            &Block::ICE,
            &Block::SLIME_BLOCK,
            &Block::OAK_FENCE,
            &Block::POWDER_SNOW_CAULDRON,
            &Block::COPPER_BLOCK,
            &Block::COPPER_TRAPDOOR,
        ] {
            assert!(!crate::world::has_empty_visual_shape(block), "{}", block.name);
        }
    }

    #[test]
    fn from_nbt_falls_back_to_empty_default_for_unresolvable_string() {
        let config = TrialSpawnerConfig::from_nbt(Some(&NbtTag::String("nope".into())));
        assert!(config.spawn_potentials.is_empty());
    }
}
