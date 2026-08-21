use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
};

use pumpkin_data::game_rules::{GameRule, GameRuleRegistry, GameRuleValue};
use pumpkin_nbt::{compound::NbtCompound, nbt_compress::read_gzip_compound_tag, tag::NbtTag};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use tracing::warn;

use crate::world_info::{
    BiomeSource, Dimension, Dimensions, Generator, GeneratorSettings, WorldGenSettings,
    WorldInfoError,
};

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct DataFileRoot<T> {
    #[serde(rename = "data")]
    pub data: T,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WeatherData {
    #[serde(rename = "rain_time", default)]
    pub rain_time: i32,
    #[serde(rename = "raining", default)]
    pub raining: bool,
    #[serde(rename = "thundering", default)]
    pub thundering: bool,
    #[serde(rename = "thunder_time", default)]
    pub thunder_time: i32,
    #[serde(rename = "clear_weather_time", default)]
    pub clear_weather_time: i32,
    #[serde(rename = "DataVersion", default)]
    pub data_version: i32,
}

impl Default for WeatherData {
    fn default() -> Self {
        Self {
            rain_time: 0,
            raining: false,
            thundering: false,
            thunder_time: 0,
            clear_weather_time: -1,
            data_version: 0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WorldGenSettingsData {
    #[serde(flatten)]
    pub settings: WorldGenSettings,
    #[serde(rename = "DataVersion", default)]
    pub data_version: i32,
    #[serde(rename = "bonus_chest", default)]
    pub bonus_chest: bool,
    #[serde(rename = "generate_structures", default = "default_true")]
    pub generate_structures: bool,
}

const fn default_true() -> bool {
    true
}

impl WorldGenSettingsData {
    #[must_use]
    pub const fn new(settings: WorldGenSettings, data_version: i32) -> Self {
        Self {
            settings,
            data_version,
            bonus_chest: false,
            generate_structures: true,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DimensionClock {
    pub total_ticks: i64,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct WorldClocksData {
    pub clocks: std::collections::HashMap<String, DimensionClock>,
    pub data_version: i32,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WanderingTraderData {
    #[serde(rename = "spawn_delay", default = "default_wandering_trader_delay")]
    pub spawn_delay: i32,
    #[serde(rename = "spawn_chance", default = "default_wandering_trader_chance")]
    pub spawn_chance: i32,
    #[serde(rename = "DataVersion", default)]
    pub data_version: i32,
}

const fn default_wandering_trader_delay() -> i32 {
    24_000
}
const fn default_wandering_trader_chance() -> i32 {
    25
}

impl Default for WanderingTraderData {
    fn default() -> Self {
        Self {
            spawn_delay: default_wandering_trader_delay(),
            spawn_chance: default_wandering_trader_chance(),
            data_version: 0,
        }
    }
}

#[must_use]
pub fn minecraft_data_dir(level_folder: &Path) -> PathBuf {
    level_folder.join("data").join("minecraft")
}

/// Ensures the `<world>/data/minecraft/` directory exists.
pub fn ensure_minecraft_data_dir(level_folder: &Path) -> Result<PathBuf, WorldInfoError> {
    let dir = minecraft_data_dir(level_folder);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Root of the `<world>/data/` tree, vanilla `SavedDataStorage.dataFolder`.
#[must_use]
pub fn saved_data_dir(level_folder: &Path) -> PathBuf {
    level_folder.join("data")
}

/// Resolves a `SavedDataType` id to `<world>/data/<namespace>/<path>.dat`.
///
/// Mirrors vanilla `SavedDataStorage.getDataFile` (`SavedDataStorage.java:57-64`),
/// which appends `.dat` and calls `Identifier.resolveAgainst`
/// (`Identifier.java:152-163`). Both sides reject an id that escapes the data folder.
pub fn saved_data_file(level_folder: &Path, id: &str) -> Result<PathBuf, WorldInfoError> {
    let (namespace, path) = id.split_once(':').unwrap_or(("minecraft", id));
    if namespace.is_empty() || path.is_empty() {
        return Err(WorldInfoError::SerializationError(format!(
            "invalid saved data id {id:?}"
        )));
    }
    let root = saved_data_dir(level_folder);
    let mut resolved = root.clone();
    // `PathBuf::starts_with` compares components without normalising, so a `..`
    // anywhere would slip past the containment check below. Reject those segments
    // outright, on the namespace as well as the path.
    for segment in std::iter::once(namespace).chain(path.split('/')) {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains(std::path::MAIN_SEPARATOR)
        {
            return Err(WorldInfoError::SerializationError(format!(
                "saved data id {id:?} tried to escape the data directory"
            )));
        }
        resolved.push(segment);
    }
    let mut file = resolved.into_os_string();
    file.push(".dat");
    let file = PathBuf::from(file);
    if !file.starts_with(&root) {
        return Err(WorldInfoError::SerializationError(format!(
            "saved data id {id:?} tried to escape the data directory"
        )));
    }
    Ok(file)
}

/// Reads the inner `data` compound of a saved-data file.
///
/// `None` when the file is absent or unreadable. Vanilla
/// `SavedDataStorage.readSavedData` parses `tag.get("data")`
/// (`SavedDataStorage.java:84-100`).
#[must_use]
pub fn read_saved_data(level_folder: &Path, id: &str) -> Option<NbtCompound> {
    let path = saved_data_file(level_folder, id).ok()?;
    if !path.exists() {
        return None;
    }
    match File::open(&path) {
        Ok(f) => match read_gzip_compound_tag(f) {
            Ok(root) => root.get_compound("data").cloned(),
            Err(e) => {
                warn!("Failed to deserialize saved data {id}: {e}");
                None
            }
        },
        Err(e) => {
            warn!("Failed to open saved data {id}: {e}");
            None
        }
    }
}

/// Writes a saved-data file in vanilla's envelope.
///
/// A root compound holds the payload under `data` with `DataVersion` as its
/// *sibling*, not nested inside (vanilla `SavedDataStorage.encodeUnchecked`,
/// `SavedDataStorage.java:190-196`).
pub fn write_saved_data(
    level_folder: &Path,
    id: &str,
    data: NbtCompound,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let path = saved_data_file(level_folder, id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(&path)?;
    let mut root = NbtCompound::new();
    root.put_compound("data", data);
    root.put_int("DataVersion", data_version);
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, BufWriter::new(file))
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

/// Vanilla `WorldBorder.Settings` (`WorldBorder.java:456-473`).
///
/// Persisted as the `minecraft:world_border` saved data (`WorldBorder.java:25-27`);
/// before 26.2 these lived in level.dat as `Border*`.
#[derive(Clone, PartialEq, Debug)]
pub struct WorldBorderData {
    pub center_x: f64,
    pub center_z: f64,
    pub damage_per_block: f64,
    pub safe_zone: f64,
    pub warning_blocks: i32,
    pub warning_time: i32,
    pub size: f64,
    pub lerp_time: i64,
    pub lerp_target: f64,
}

impl Default for WorldBorderData {
    /// Vanilla `WorldBorder.Settings.DEFAULT` (`WorldBorder.java:459`).
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_z: 0.0,
            damage_per_block: 0.2,
            safe_zone: 5.0,
            warning_blocks: 5,
            warning_time: 300,
            size: 5.999_997E7,
            lerp_time: 0,
            lerp_target: 0.0,
        }
    }
}

pub const WORLD_BORDER_ID: &str = "world_border";

/// Reads `data/minecraft/world_border.dat`, or `None` when it is absent so the
/// caller can fall back to the legacy level.dat `Border*` keys.
#[must_use]
pub fn read_world_border(level_folder: &Path) -> Option<WorldBorderData> {
    let c = read_saved_data(level_folder, WORLD_BORDER_ID)?;
    let d = WorldBorderData::default();
    Some(WorldBorderData {
        center_x: c.get_double("center_x").unwrap_or(d.center_x),
        center_z: c.get_double("center_z").unwrap_or(d.center_z),
        damage_per_block: c
            .get_double("damage_per_block")
            .unwrap_or(d.damage_per_block),
        safe_zone: c.get_double("safe_zone").unwrap_or(d.safe_zone),
        warning_blocks: c.get_int("warning_blocks").unwrap_or(d.warning_blocks),
        warning_time: c.get_int("warning_time").unwrap_or(d.warning_time),
        size: c.get_double("size").unwrap_or(d.size),
        lerp_time: c.get_long("lerp_time").unwrap_or(d.lerp_time),
        lerp_target: c.get_double("lerp_target").unwrap_or(d.lerp_target),
    })
}

pub fn write_world_border(
    level_folder: &Path,
    border: &WorldBorderData,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let mut c = NbtCompound::new();
    c.put_double("center_x", border.center_x);
    c.put_double("center_z", border.center_z);
    c.put_double("damage_per_block", border.damage_per_block);
    c.put_double("safe_zone", border.safe_zone);
    c.put_int("warning_blocks", border.warning_blocks);
    c.put_int("warning_time", border.warning_time);
    c.put_double("size", border.size);
    c.put_long("lerp_time", border.lerp_time);
    c.put_double("lerp_target", border.lerp_target);
    write_saved_data(level_folder, WORLD_BORDER_ID, c, data_version)
}

/// Vanilla `MapIndex` (`MapIndex.java`).
///
/// Stored as `minecraft:maps/last_id` (`MapIndex.java:15-17`), i.e.
/// `data/minecraft/maps/last_id.dat`.
pub const MAP_INDEX_ID: &str = "maps/last_id";

/// Reads the next map id to hand out.
///
/// Vanilla `MapIndex` stores the LAST issued map id, defaulting to -1, and
/// `getNextMapId` pre-increments (`MapIndex.java:12,27-31`), so the first map on a
/// fresh world is 0. Pumpkin's `LevelData::map_id` is the NEXT id to hand out, hence
/// the +1 here and the -1 on write.
#[must_use]
pub fn read_next_map_id(level_folder: &Path) -> Option<i32> {
    let c = read_saved_data(level_folder, MAP_INDEX_ID)?;
    Some(c.get_int("map").unwrap_or(-1).saturating_add(1))
}

pub fn write_next_map_id(
    level_folder: &Path,
    next_map_id: i32,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let mut c = NbtCompound::new();
    c.put_int("map", next_map_id.saturating_sub(1));
    write_saved_data(level_folder, MAP_INDEX_ID, c, data_version)
}

pub fn read_weather(level_folder: &Path) -> WeatherData {
    let path = minecraft_data_dir(level_folder).join("weather.dat");
    if !path.exists() {
        return WeatherData::default();
    }
    match File::open(&path) {
        Ok(f) => match read_gzip_compound_tag(f) {
            Ok(compound) => {
                let data_compound = compound.get_compound("data");
                let c = data_compound.as_ref().map_or(&compound, |v| v);
                WeatherData {
                    clear_weather_time: c.get_int("clear_weather_time").unwrap_or(0),
                    rain_time: c.get_int("rain_time").unwrap_or(0),
                    thunder_time: c.get_int("thunder_time").unwrap_or(0),
                    raining: c.get_bool("raining").unwrap_or(false),
                    thundering: c.get_bool("thundering").unwrap_or(false),
                    data_version: c.get_int("DataVersion").unwrap_or(0),
                }
            }
            Err(e) => {
                warn!("Failed to deserialize weather.dat, using defaults: {e}");
                WeatherData::default()
            }
        },
        Err(e) => {
            warn!("Failed to open weather.dat, using defaults: {e}");
            WeatherData::default()
        }
    }
}

pub fn write_weather(level_folder: &Path, data: &WeatherData) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("weather.dat");
    let file = File::create(&path)?;
    let mut data_comp = NbtCompound::new();
    data_comp.put_int("clear_weather_time", data.clear_weather_time);
    data_comp.put_int("rain_time", data.rain_time);
    data_comp.put_int("thunder_time", data.thunder_time);
    data_comp.put_bool("raining", data.raining);
    data_comp.put_bool("thundering", data.thundering);
    let mut root = NbtCompound::new();
    root.put_compound("data", data_comp);
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, BufWriter::new(file))
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

pub fn read_world_gen_settings(level_folder: &Path) -> Option<WorldGenSettings> {
    let path = minecraft_data_dir(level_folder).join("world_gen_settings.dat");
    if !path.exists() {
        return None;
    }
    match File::open(&path) {
        Ok(f) => match read_gzip_compound_tag(f) {
            Ok(compound) => {
                let data = compound.get_compound("data")?;
                let Some(seed) = data.get_long("seed") else {
                    warn!("world_gen_settings.dat has no seed");
                    return None;
                };
                let dimensions = dimensions_from_nbt(data)?;
                Some(WorldGenSettings {
                    seed,
                    generate_structures: data.get_bool("generate_structures").unwrap_or(true),
                    bonus_chest: data.get_bool("bonus_chest").unwrap_or(false),
                    legacy_custom_options: data
                        .get_string("legacy_custom_options")
                        .map(str::to_string),
                    dimensions,
                })
            }
            Err(e) => {
                warn!("Failed to deserialize world_gen_settings.dat: {e}");
                None
            }
        },
        Err(e) => {
            warn!("Failed to open world_gen_settings.dat: {e}");
            None
        }
    }
}

pub fn write_world_gen_settings(
    level_folder: &Path,
    settings: &WorldGenSettings,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("world_gen_settings.dat");
    let file = File::create(&path)?;
    let mut inner = NbtCompound::new();
    inner.put_long("seed", settings.seed);
    inner.put_bool("generate_structures", settings.generate_structures);
    inner.put_bool("bonus_chest", settings.bonus_chest);
    if let Some(options) = &settings.legacy_custom_options {
        inner.put_string("legacy_custom_options", options.clone());
    }
    inner.put_compound("dimensions", dimensions_to_nbt(&settings.dimensions));

    let mut root = NbtCompound::new();
    root.put_compound("data", inner);
    root.put_int("DataVersion", data_version);
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, BufWriter::new(file))
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

fn json_to_nbt(value: &Value) -> Option<NbtTag> {
    match value {
        Value::Null => None,
        Value::Bool(value) => Some(NbtTag::Byte(i8::from(*value))),
        Value::Number(value) => value.as_i64().map_or_else(
            || value.as_f64().map(NbtTag::Double),
            |value| {
                i32::try_from(value).map_or_else(
                    |_| Some(NbtTag::Long(value)),
                    |value| Some(NbtTag::Int(value)),
                )
            },
        ),
        Value::String(value) => Some(NbtTag::String(value.clone().into_boxed_str())),
        Value::Array(values) => Some(NbtTag::List(
            values.iter().filter_map(json_to_nbt).collect(),
        )),
        Value::Object(values) => Some(NbtTag::Compound(
            values
                .iter()
                .filter_map(|(key, value)| json_to_nbt(value).map(|value| (key.clone(), value)))
                .collect(),
        )),
    }
}

fn nbt_to_json(tag: &NbtTag) -> Option<Value> {
    match tag {
        NbtTag::End => None,
        NbtTag::Byte(value) => Some(Value::Number(Number::from(*value))),
        NbtTag::Short(value) => Some(Value::Number(Number::from(*value))),
        NbtTag::Int(value) => Some(Value::Number(Number::from(*value))),
        NbtTag::Long(value) => Some(Value::Number(Number::from(*value))),
        NbtTag::Float(value) => Number::from_f64(f64::from(*value)).map(Value::Number),
        NbtTag::Double(value) => Number::from_f64(*value).map(Value::Number),
        NbtTag::ByteArray(values) => Some(Value::Array(
            values
                .iter()
                .map(|value| Value::Number(Number::from(*value)))
                .collect(),
        )),
        NbtTag::String(value) => Some(Value::String(value.to_string())),
        NbtTag::List(values) => Some(Value::Array(
            values.iter().filter_map(nbt_to_json).collect(),
        )),
        NbtTag::Compound(value) => Some(Value::Object(
            value
                .child_tags
                .iter()
                .filter_map(|(key, value)| nbt_to_json(value).map(|value| (key.to_string(), value)))
                .collect(),
        )),
        NbtTag::IntArray(values) => Some(Value::Array(
            values
                .iter()
                .map(|value| Value::Number(Number::from(*value)))
                .collect(),
        )),
        NbtTag::LongArray(values) => Some(Value::Array(
            values
                .iter()
                .map(|value| Value::Number(Number::from(*value)))
                .collect(),
        )),
    }
}

fn dimension_to_nbt(dimension: &Dimension) -> NbtCompound {
    let mut generator = NbtCompound::new();
    if let Some(settings) = &dimension.generator.settings {
        match settings {
            GeneratorSettings::Reference(value) => {
                generator.put_string("settings", value.clone());
            }
            GeneratorSettings::Compound(value) => {
                if let Some(tag) = json_to_nbt(value) {
                    generator.put("settings", tag);
                }
            }
        }
    }
    if let Some(biome_source) = &dimension.generator.biome_source {
        let mut source = NbtCompound::new();
        match biome_source {
            BiomeSource::WithPreset { preset, biome_type } => {
                source.put_string("preset", preset.clone());
                source.put_string("type", biome_type.clone());
            }
            BiomeSource::Simple { biome_type } => source.put_string("type", biome_type.clone()),
            BiomeSource::Compound(value) => {
                if let Some(NbtTag::Compound(compound)) = json_to_nbt(value) {
                    generator.put_compound("biome_source", compound);
                }
            }
        }
        if !matches!(biome_source, BiomeSource::Compound(_)) {
            generator.put_compound("biome_source", source);
        }
    }
    generator.put_string("type", dimension.generator.generator_type.clone());

    let mut result = NbtCompound::new();
    result.put_compound("generator", generator);
    result.put_string("type", dimension.dimension_type.clone());
    result
}

fn dimensions_to_nbt(dimensions: &Dimensions) -> NbtCompound {
    dimensions
        .iter()
        .map(|(name, dimension)| (name.clone(), NbtTag::Compound(dimension_to_nbt(dimension))))
        .collect()
}

fn dimension_from_nbt(value: &NbtCompound) -> Option<Dimension> {
    let generator = value.get_compound("generator")?;
    let settings = match generator.get("settings") {
        Some(NbtTag::String(value)) => Some(GeneratorSettings::Reference(value.to_string())),
        Some(value) => nbt_to_json(value).map(GeneratorSettings::Compound),
        None => None,
    };
    let biome_source = generator.get_compound("biome_source").and_then(|source| {
        let biome_type = source.get_string("type")?.to_string();
        if let Some(preset) = source.get_string("preset") {
            Some(BiomeSource::WithPreset {
                preset: preset.to_string(),
                biome_type,
            })
        } else if source.child_tags.len() > 1 {
            nbt_to_json(&NbtTag::Compound(source.clone())).map(BiomeSource::Compound)
        } else {
            Some(BiomeSource::Simple { biome_type })
        }
    });
    let generator_type = generator.get_string("type")?.to_string();
    if generator_type.is_empty()
        || (generator_type == "minecraft:noise" && (settings.is_none() || biome_source.is_none()))
    {
        return None;
    }
    let dimension_type = value.get_string("type")?;
    if dimension_type.is_empty() {
        return None;
    }
    Some(Dimension {
        generator: Generator {
            settings,
            biome_source,
            generator_type,
        },
        dimension_type: dimension_type.to_string(),
    })
}

fn dimensions_from_nbt(data: &NbtCompound) -> Option<Dimensions> {
    let dimensions = data.get_compound("dimensions")?;
    let dimensions: Option<Dimensions> = dimensions
        .child_tags
        .iter()
        .map(|(name, value)| {
            value
                .extract_compound()
                .and_then(dimension_from_nbt)
                .map(|dimension| (name.to_string(), dimension))
        })
        .collect();
    let dimensions = dimensions?;
    dimensions
        .contains_key("minecraft:overworld")
        .then_some(dimensions)
}

#[must_use]
pub fn game_rules_to_nbt(rules: &GameRuleRegistry, data_version: i32) -> NbtCompound {
    let mut inner = NbtCompound::new();
    for rule in GameRule::all() {
        let key = format!("minecraft:{rule}");
        match rules.get(rule) {
            GameRuleValue::Bool(b) => inner.put(&key, NbtTag::Byte(i8::from(*b))),
            GameRuleValue::Int(i) => inner.put(&key, NbtTag::Int(*i as i32)),
        }
    }
    inner.put_int("DataVersion", data_version);

    let mut root = NbtCompound::new();
    root.put_compound("data", inner);
    root
}

pub fn game_rules_from_nbt(root: &NbtCompound) -> GameRuleRegistry {
    let mut registry = GameRuleRegistry::default();

    let Some(inner) = root.get_compound("data") else {
        warn!("game_rules.dat missing 'data' compound, using defaults");
        return registry;
    };

    for rule in GameRule::all() {
        let key = format!("minecraft:{rule}");
        match registry.get_mut(rule) {
            GameRuleValue::Bool(b) => {
                if let Some(v) = inner.get_byte(&key) {
                    *b = v != 0;
                }
            }
            GameRuleValue::Int(i) => {
                if let Some(v) = inner.get_int(&key) {
                    *i = i64::from(v);
                }
            }
        }
    }

    registry
}

pub fn read_game_rules(level_folder: &Path) -> GameRuleRegistry {
    let path = minecraft_data_dir(level_folder).join("game_rules.dat");
    if !path.exists() {
        return GameRuleRegistry::default();
    }

    match File::open(&path) {
        Ok(f) => match read_gzip_compound_tag(f) {
            Ok(compound) => game_rules_from_nbt(&compound),
            Err(e) => {
                warn!("Failed to parse game_rules.dat: {e}");
                GameRuleRegistry::default()
            }
        },
        Err(e) => {
            warn!("Failed to open game_rules.dat: {e}");
            GameRuleRegistry::default()
        }
    }
}

pub fn write_game_rules(
    level_folder: &Path,
    rules: &GameRuleRegistry,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("game_rules.dat");

    let compound = game_rules_to_nbt(rules, data_version);
    let file = File::create(&path)?;

    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(compound, file)
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

pub fn read_world_clocks(level_folder: &Path) -> WorldClocksData {
    let path = minecraft_data_dir(level_folder).join("world_clocks.dat");
    if !path.exists() {
        return WorldClocksData::default();
    }

    match File::open(&path) {
        Ok(f) => match read_gzip_compound_tag(f) {
            Ok(compound) => world_clocks_from_nbt(&compound),
            Err(e) => {
                warn!("Failed to parse world_clocks.dat: {e}");
                WorldClocksData::default()
            }
        },
        Err(e) => {
            warn!("Failed to open world_clocks.dat: {e}");
            WorldClocksData::default()
        }
    }
}

fn world_clocks_from_nbt(root: &NbtCompound) -> WorldClocksData {
    let mut result = WorldClocksData::default();

    let Some(inner) = root.get_compound("data") else {
        return result;
    };

    result.data_version = inner.get_int("DataVersion").unwrap_or(0);

    for (key, tag) in &inner.child_tags {
        if key.as_ref() == "DataVersion" {
            continue;
        }
        if let NbtTag::Compound(dim_compound) = tag {
            let total_ticks = dim_compound.get_long("total_ticks").unwrap_or(0);
            result
                .clocks
                .insert(key.to_string(), DimensionClock { total_ticks });
        }
    }

    result
}

pub fn write_world_clocks(
    level_folder: &Path,
    clocks: &WorldClocksData,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("world_clocks.dat");

    let mut inner = NbtCompound::new();
    for (dim_name, clock) in &clocks.clocks {
        let mut dim_compound = NbtCompound::new();
        dim_compound.put_long("total_ticks", clock.total_ticks);
        inner.put_compound(dim_name, dim_compound);
    }
    inner.put_int("DataVersion", clocks.data_version);

    let mut root = NbtCompound::new();
    root.put_compound("data", inner);

    let file = File::create(&path)?;

    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, file)
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

pub fn read_wandering_trader(level_folder: &Path) -> WanderingTraderData {
    let path = minecraft_data_dir(level_folder).join("wandering_trader.dat");
    if !path.exists() {
        return WanderingTraderData::default();
    }
    match File::open(&path) {
        Ok(f) => match read_gzip_compound_tag(f) {
            Ok(compound) => {
                let data_compound = compound.get_compound("data");
                let c = data_compound.as_ref().map_or(&compound, |v| v);
                WanderingTraderData {
                    spawn_delay: c.get_int("WanderingTraderSpawnDelay").unwrap_or(24_000),
                    spawn_chance: c.get_int("WanderingTraderSpawnChance").unwrap_or(25),
                    data_version: c.get_int("DataVersion").unwrap_or(0),
                }
            }
            Err(e) => {
                warn!("Failed to deserialize wandering_trader.dat, using defaults: {e}");
                WanderingTraderData::default()
            }
        },
        Err(e) => {
            warn!("Failed to open wandering_trader.dat: {e}");
            WanderingTraderData::default()
        }
    }
}

pub fn write_wandering_trader(
    level_folder: &Path,
    data: &WanderingTraderData,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("wandering_trader.dat");
    let file = File::create(&path)?;
    let mut data_comp = NbtCompound::new();
    data_comp.put_int("WanderingTraderSpawnDelay", data.spawn_delay);
    data_comp.put_int("WanderingTraderSpawnChance", data.spawn_chance);
    let mut root = NbtCompound::new();
    root.put_compound("data", data_comp);
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, BufWriter::new(file))
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

pub fn write_custom_boss_events_stub(
    level_folder: &Path,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("custom_boss_events.dat");
    // Only create if absent; actual boss-bar persistence lives elsewhere.
    if path.exists() {
        return Ok(());
    }

    let mut inner = NbtCompound::new();
    inner.put_int("DataVersion", data_version);
    let mut root = NbtCompound::new();
    root.put_compound("data", inner);

    let file = File::create(&path)?;

    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, file)
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

pub fn write_scheduled_events_stub(
    level_folder: &Path,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("scheduled_events.dat");
    if path.exists() {
        return Ok(());
    }

    let mut inner = NbtCompound::new();
    inner.put("events", NbtTag::List(vec![]));
    inner.put_int("DataVersion", data_version);
    let mut root = NbtCompound::new();
    root.put_compound("data", inner);

    let file = File::create(&path)?;

    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, file)
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

fn default_criteria_name() -> String {
    "dummy".to_string()
}

fn default_render_type() -> String {
    "integer".to_string()
}

fn default_visibility() -> String {
    "always".to_string()
}

fn default_collision_rule() -> String {
    "always".to_string()
}

fn default_empty_text_component() -> pumpkin_util::text::TextComponentBase {
    pumpkin_util::text::TextComponent::empty().0
}

/// Serializable scoreboard data for `data/minecraft/scoreboard.dat`.
///
/// Field names and structure mirror vanilla's `ScoreboardSaveData.Packed` record
/// exactly (see `net.minecraft.world.scores.ScoreboardSaveData`, decompiled 26.2
/// source), so a `scoreboard.dat` written here loads in vanilla and vice versa.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ScoreboardData {
    #[serde(rename = "Objectives", default)]
    pub objectives: Vec<SerializableObjective>,
    #[serde(rename = "PlayerScores", default)]
    pub scores: Vec<SerializableScore>,
    /// Display slot bindings: slot id (e.g. "list", "sidebar", "sidebar.team.red",
    /// matching `net.minecraft.world.scores.DisplaySlot#getSerializedName`) -> objective name.
    #[serde(rename = "DisplaySlots", default)]
    pub display_slots: std::collections::HashMap<String, String>,
    #[serde(rename = "Teams", default)]
    pub teams: Vec<SerializableTeam>,
}

/// Mirrors `net.minecraft.world.scores.Objective.Packed`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SerializableObjective {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "CriteriaName", default = "default_criteria_name")]
    pub criteria_name: String,
    #[serde(rename = "DisplayName")]
    pub display_name: pumpkin_util::text::TextComponentBase,
    #[serde(rename = "RenderType", default = "default_render_type")]
    pub render_type: String,
    /// NOTE: vanilla's key here is genuinely `snake_case` (`display_auto_update`)
    /// while the surrounding keys are `PascalCase`; this is not a typo.
    #[serde(rename = "display_auto_update", default)]
    pub display_auto_update: bool,
    /// Number format override, JSON-encoded. Vanilla encodes this as a proper
    /// `NumberFormatTypes` sum type (blank/styled/fixed); modeling that fully is
    /// deferred, so this field is Pumpkin-internal only and will not round-trip
    /// through vanilla. See `net.minecraft.network.chat.numbers.NumberFormatTypes`.
    #[serde(rename = "format", default, skip_serializing_if = "Option::is_none")]
    pub number_format: Option<String>,
}

/// Mirrors `net.minecraft.world.scores.Scoreboard.PackedScore` + `Score.Packed`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SerializableScore {
    #[serde(rename = "Name")]
    pub entity_name: String,
    #[serde(rename = "Objective")]
    pub objective_name: String,
    #[serde(rename = "Score", default)]
    pub value: i32,
    #[serde(rename = "Locked", default)]
    pub locked: bool,
    #[serde(rename = "display", default, skip_serializing_if = "Option::is_none")]
    pub display: Option<pumpkin_util::text::TextComponentBase>,
    /// See the note on `SerializableObjective::number_format`.
    #[serde(rename = "format", default, skip_serializing_if = "Option::is_none")]
    pub number_format: Option<String>,
}

/// Mirrors `net.minecraft.world.scores.PlayerTeam.Packed`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SerializableTeam {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(
        rename = "DisplayName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub display_name: Option<pumpkin_util::text::TextComponentBase>,
    /// One of vanilla's `TeamColor` serialized names (`black`, `dark_blue`, ...),
    /// absent when the team has no color.
    #[serde(rename = "TeamColor", default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(rename = "AllowFriendlyFire", default = "default_true")]
    pub friendly_fire: bool,
    #[serde(rename = "SeeFriendlyInvisibles", default = "default_true")]
    pub see_friendly_invisibles: bool,
    #[serde(rename = "MemberNamePrefix", default = "default_empty_text_component")]
    pub player_prefix: pumpkin_util::text::TextComponentBase,
    #[serde(rename = "MemberNameSuffix", default = "default_empty_text_component")]
    pub player_suffix: pumpkin_util::text::TextComponentBase,
    #[serde(rename = "NameTagVisibility", default = "default_visibility")]
    pub nametag_visibility: String,
    #[serde(rename = "DeathMessageVisibility", default = "default_visibility")]
    pub death_message_visibility: String,
    #[serde(rename = "CollisionRule", default = "default_collision_rule")]
    pub collision_rule: String,
    #[serde(rename = "Players", default)]
    pub players: Vec<String>,
}

/// Manual NBT codec for `scoreboard.dat`.
///
/// Upstream `7350fba3` removed the serde bridge from `pumpkin-nbt`, so the
/// `DataFileRoot<ScoreboardData>` round-trip has to be spelled out. The key
/// names below are unchanged from the serde attributes on the structs above.
mod scoreboard_nbt {
    use super::{ScoreboardData, SerializableObjective, SerializableScore, SerializableTeam};
    use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
    use pumpkin_util::text::TextComponentBase;

    fn text(compound: &NbtCompound, key: &str) -> Option<TextComponentBase> {
        compound
            .get_compound(key)
            .map(TextComponentBase::from_nbt_compound)
    }

    fn compounds<'a>(compound: &'a NbtCompound, key: &str) -> Vec<&'a NbtCompound> {
        compound.get_list(key).map_or_else(Vec::new, |list| {
            list.iter()
                .filter_map(|tag| match tag {
                    NbtTag::Compound(c) => Some(c),
                    _ => None,
                })
                .collect()
        })
    }

    fn put_opt_string(compound: &mut NbtCompound, key: &str, value: Option<&String>) {
        if let Some(value) = value {
            compound.put_string(key, value.clone());
        }
    }

    fn put_opt_text(compound: &mut NbtCompound, key: &str, value: Option<&TextComponentBase>) {
        if let Some(value) = value {
            compound.put_compound(key, value.to_nbt_compound());
        }
    }

    fn objective_to_nbt(objective: &SerializableObjective) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.put_string("Name", objective.name.clone());
        compound.put_string("CriteriaName", objective.criteria_name.clone());
        compound.put_compound("DisplayName", objective.display_name.to_nbt_compound());
        compound.put_string("RenderType", objective.render_type.clone());
        compound.put_bool("display_auto_update", objective.display_auto_update);
        put_opt_string(&mut compound, "format", objective.number_format.as_ref());
        compound
    }

    fn objective_from_nbt(compound: &NbtCompound) -> Option<SerializableObjective> {
        Some(SerializableObjective {
            name: compound.get_string("Name")?.to_string(),
            criteria_name: compound
                .get_string("CriteriaName")
                .unwrap_or("dummy")
                .to_string(),
            display_name: text(compound, "DisplayName")?,
            render_type: compound
                .get_string("RenderType")
                .unwrap_or("integer")
                .to_string(),
            display_auto_update: compound.get_bool("display_auto_update").unwrap_or_default(),
            number_format: compound.get_string("format").map(ToString::to_string),
        })
    }

    fn score_to_nbt(score: &SerializableScore) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.put_string("Name", score.entity_name.clone());
        compound.put_string("Objective", score.objective_name.clone());
        compound.put_int("Score", score.value);
        compound.put_bool("Locked", score.locked);
        put_opt_text(&mut compound, "display", score.display.as_ref());
        put_opt_string(&mut compound, "format", score.number_format.as_ref());
        compound
    }

    fn score_from_nbt(compound: &NbtCompound) -> Option<SerializableScore> {
        Some(SerializableScore {
            entity_name: compound.get_string("Name")?.to_string(),
            objective_name: compound.get_string("Objective")?.to_string(),
            value: compound.get_int("Score").unwrap_or_default(),
            locked: compound.get_bool("Locked").unwrap_or_default(),
            display: text(compound, "display"),
            number_format: compound.get_string("format").map(ToString::to_string),
        })
    }

    fn team_to_nbt(team: &SerializableTeam) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.put_string("Name", team.name.clone());
        put_opt_text(&mut compound, "DisplayName", team.display_name.as_ref());
        put_opt_string(&mut compound, "TeamColor", team.color.as_ref());
        compound.put_bool("AllowFriendlyFire", team.friendly_fire);
        compound.put_bool("SeeFriendlyInvisibles", team.see_friendly_invisibles);
        compound.put_compound("MemberNamePrefix", team.player_prefix.to_nbt_compound());
        compound.put_compound("MemberNameSuffix", team.player_suffix.to_nbt_compound());
        compound.put_string("NameTagVisibility", team.nametag_visibility.clone());
        compound.put_string(
            "DeathMessageVisibility",
            team.death_message_visibility.clone(),
        );
        compound.put_string("CollisionRule", team.collision_rule.clone());
        compound.put_list(
            "Players",
            team.players
                .iter()
                .map(|p| NbtTag::String(p.clone().into_boxed_str()))
                .collect(),
        );
        compound
    }

    fn team_from_nbt(compound: &NbtCompound) -> Option<SerializableTeam> {
        Some(SerializableTeam {
            name: compound.get_string("Name")?.to_string(),
            display_name: text(compound, "DisplayName"),
            color: compound.get_string("TeamColor").map(ToString::to_string),
            friendly_fire: compound.get_bool("AllowFriendlyFire").unwrap_or(true),
            see_friendly_invisibles: compound.get_bool("SeeFriendlyInvisibles").unwrap_or(true),
            player_prefix: text(compound, "MemberNamePrefix")
                .unwrap_or_else(|| pumpkin_util::text::TextComponent::text(String::new()).0),
            player_suffix: text(compound, "MemberNameSuffix")
                .unwrap_or_else(|| pumpkin_util::text::TextComponent::text(String::new()).0),
            nametag_visibility: compound
                .get_string("NameTagVisibility")
                .unwrap_or("always")
                .to_string(),
            death_message_visibility: compound
                .get_string("DeathMessageVisibility")
                .unwrap_or("always")
                .to_string(),
            collision_rule: compound
                .get_string("CollisionRule")
                .unwrap_or("always")
                .to_string(),
            players: compound.get_list("Players").map_or_else(Vec::new, |list| {
                list.iter()
                    .filter_map(|tag| match tag {
                        NbtTag::String(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .collect()
            }),
        })
    }

    pub fn to_nbt(data: &ScoreboardData) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.put_list(
            "Objectives",
            data.objectives
                .iter()
                .map(|o| NbtTag::Compound(objective_to_nbt(o)))
                .collect(),
        );
        compound.put_list(
            "PlayerScores",
            data.scores
                .iter()
                .map(|s| NbtTag::Compound(score_to_nbt(s)))
                .collect(),
        );
        let mut display_slots = NbtCompound::new();
        for (slot, objective) in &data.display_slots {
            display_slots.put_string(slot, objective.clone());
        }
        compound.put_compound("DisplaySlots", display_slots);
        compound.put_list(
            "Teams",
            data.teams
                .iter()
                .map(|t| NbtTag::Compound(team_to_nbt(t)))
                .collect(),
        );
        compound
    }

    pub fn from_nbt(compound: &NbtCompound) -> ScoreboardData {
        ScoreboardData {
            objectives: compounds(compound, "Objectives")
                .into_iter()
                .filter_map(objective_from_nbt)
                .collect(),
            scores: compounds(compound, "PlayerScores")
                .into_iter()
                .filter_map(score_from_nbt)
                .collect(),
            display_slots: compound.get_compound("DisplaySlots").map_or_else(
                Default::default,
                |slots| {
                    slots
                        .child_tags
                        .iter()
                        .filter_map(|(slot, tag)| match tag {
                            NbtTag::String(objective) => {
                                Some((slot.to_string(), objective.to_string()))
                            }
                            _ => None,
                        })
                        .collect()
                },
            ),
            teams: compounds(compound, "Teams")
                .into_iter()
                .filter_map(team_from_nbt)
                .collect(),
        }
    }
}

pub fn read_scoreboard(level_folder: &Path) -> ScoreboardData {
    let path = minecraft_data_dir(level_folder).join("scoreboard.dat");
    if !path.exists() {
        return ScoreboardData::default();
    }
    match File::open(&path) {
        Ok(f) => match read_gzip_compound_tag(std::io::BufReader::new(f)) {
            Ok(root) => root
                .get_compound("data")
                .map_or_else(ScoreboardData::default, scoreboard_nbt::from_nbt),
            Err(e) => {
                warn!("Failed to deserialize scoreboard.dat, using defaults: {e}");
                ScoreboardData::default()
            }
        },
        Err(e) => {
            warn!("Failed to open scoreboard.dat, using defaults: {e}");
            ScoreboardData::default()
        }
    }
}

pub fn write_scoreboard(level_folder: &Path, data: &ScoreboardData) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("scoreboard.dat");
    let file = File::create(&path)?;
    let mut root = NbtCompound::new();
    root.put_compound("data", scoreboard_nbt::to_nbt(data));
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, BufWriter::new(file))
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

#[cfg(test)]
mod scoreboard_test {
    use super::{ScoreboardData, SerializableObjective, SerializableScore, SerializableTeam};
    use pumpkin_util::text::TextComponent;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn sample_data() -> ScoreboardData {
        let mut display_slots = HashMap::new();
        display_slots.insert("sidebar".to_string(), "test_objective".to_string());
        display_slots.insert("sidebar.team.red".to_string(), "red_objective".to_string());

        ScoreboardData {
            objectives: vec![SerializableObjective {
                name: "test_objective".to_string(),
                criteria_name: "dummy".to_string(),
                display_name: TextComponent::text("Test Objective").0,
                render_type: "integer".to_string(),
                display_auto_update: false,
                number_format: None,
            }],
            scores: vec![SerializableScore {
                entity_name: "Steve".to_string(),
                objective_name: "test_objective".to_string(),
                value: 42,
                locked: false,
                display: None,
                number_format: None,
            }],
            display_slots,
            teams: vec![SerializableTeam {
                name: "red_team".to_string(),
                display_name: Some(TextComponent::text("Red Team").0),
                color: Some("red".to_string()),
                friendly_fire: true,
                see_friendly_invisibles: true,
                player_prefix: TextComponent::empty().0,
                player_suffix: TextComponent::empty().0,
                nametag_visibility: "always".to_string(),
                death_message_visibility: "hideForOtherTeams".to_string(),
                collision_rule: "pushOwnTeam".to_string(),
                players: vec!["Steve".to_string()],
            }],
        }
    }

    /// A `ScoreboardData` written to `scoreboard.dat` and read back must be
    /// byte-for-byte equivalent, since this file is meant to be cross-compatible
    /// with vanilla's own `scoreboard.dat` (see `ScoreboardSaveData` in the
    /// decompiled 26.2 source).
    #[test]
    fn scoreboard_dat_round_trip() {
        let temp_dir = TempDir::new().unwrap();
        let data = sample_data();

        super::write_scoreboard(temp_dir.path(), &data).unwrap();
        let loaded = super::read_scoreboard(temp_dir.path());

        assert_eq!(loaded, data);
    }

    /// Missing `scoreboard.dat` (a fresh world) must not error, and must yield an
    /// empty (default) scoreboard.
    #[test]
    fn missing_scoreboard_dat_yields_default() {
        let temp_dir = TempDir::new().unwrap();
        let loaded = super::read_scoreboard(temp_dir.path());
        assert_eq!(loaded, ScoreboardData::default());
    }

    /// Vanilla NBT field names are exact and case-sensitive; verify the on-disk
    /// keys match `ScoreboardSaveData.Packed`'s codec field names, not just that
    /// our own round-trip is internally consistent.
    #[test]
    fn field_names_match_vanilla() {
        let temp_dir = TempDir::new().unwrap();
        let data = sample_data();
        super::write_scoreboard(temp_dir.path(), &data).unwrap();

        let path = super::minecraft_data_dir(temp_dir.path()).join("scoreboard.dat");
        let file = std::fs::File::open(&path).unwrap();
        let root = pumpkin_nbt::nbt_compress::read_gzip_compound_tag(file).unwrap();
        let inner = root.get_compound("data").expect("missing 'data' compound");

        assert!(inner.get_list("Objectives").is_some());
        assert!(inner.get_list("PlayerScores").is_some());
        assert!(inner.get_compound("DisplaySlots").is_some());
        assert!(inner.get_list("Teams").is_some());
    }
}

#[cfg(test)]
mod saved_data_tests {
    use super::{
        MAP_INDEX_ID, WORLD_BORDER_ID, WorldBorderData, read_next_map_id, read_saved_data,
        read_world_border, saved_data_file, write_next_map_id, write_saved_data,
        write_world_border,
    };
    use pumpkin_nbt::compound::NbtCompound;
    use tempfile::TempDir;

    #[test]
    fn ids_resolve_like_identifier_resolve_against() {
        let root = std::path::Path::new("/w");
        assert_eq!(
            saved_data_file(root, "world_border").unwrap(),
            root.join("data/minecraft/world_border.dat")
        );
        assert_eq!(
            saved_data_file(root, "maps/last_id").unwrap(),
            root.join("data/minecraft/maps/last_id.dat")
        );
        assert_eq!(
            saved_data_file(root, "mypack:sub/thing").unwrap(),
            root.join("data/mypack/sub/thing.dat")
        );
    }

    #[test]
    fn ids_cannot_escape_the_data_directory() {
        let root = std::path::Path::new("/w");
        assert!(saved_data_file(root, "../../etc/passwd").is_err());
        assert!(saved_data_file(root, "maps/../../escape").is_err());
        assert!(saved_data_file(root, "..:escape").is_err());
        assert!(saved_data_file(root, "minecraft:.").is_err());
        assert!(saved_data_file(root, "").is_err());
    }

    /// Vanilla `SavedDataStorage.encodeUnchecked` puts the payload under `data`
    /// and `DataVersion` beside it at the root, not inside the payload.
    #[test]
    fn envelope_has_data_version_as_a_sibling_of_data() {
        let dir = TempDir::new().unwrap();
        let mut payload = NbtCompound::new();
        payload.put_int("x", 7);
        write_saved_data(dir.path(), "test/thing", payload, 4567).unwrap();

        let path = saved_data_file(dir.path(), "test/thing").unwrap();
        let root =
            pumpkin_nbt::nbt_compress::read_gzip_compound_tag(std::fs::File::open(path).unwrap())
                .unwrap();
        assert_eq!(root.get_int("DataVersion"), Some(4567));
        let data = root.get_compound("data").unwrap();
        assert_eq!(data.get_int("x"), Some(7));
        assert_eq!(data.get_int("DataVersion"), None);

        assert_eq!(
            read_saved_data(dir.path(), "test/thing")
                .unwrap()
                .get_int("x"),
            Some(7)
        );
    }

    #[test]
    fn missing_saved_data_reads_as_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_saved_data(dir.path(), "world_border").is_none());
        assert!(read_world_border(dir.path()).is_none());
        assert!(read_next_map_id(dir.path()).is_none());
    }

    /// Field names come from `WorldBorder.Settings.CODEC` (`WorldBorder.java:460-473`).
    #[test]
    fn world_border_round_trip_uses_vanilla_field_names() {
        let dir = TempDir::new().unwrap();
        let border = WorldBorderData {
            center_x: 12.5,
            center_z: -8.25,
            damage_per_block: 0.4,
            safe_zone: 3.0,
            warning_blocks: 9,
            warning_time: 42,
            size: 2048.0,
            lerp_time: 1234,
            lerp_target: 4096.0,
        };
        write_world_border(dir.path(), &border, 4567).unwrap();
        assert_eq!(read_world_border(dir.path()).unwrap(), border);

        let root = pumpkin_nbt::nbt_compress::read_gzip_compound_tag(
            std::fs::File::open(saved_data_file(dir.path(), WORLD_BORDER_ID).unwrap()).unwrap(),
        )
        .unwrap();
        let d = root.get_compound("data").unwrap();
        for key in [
            "center_x",
            "center_z",
            "damage_per_block",
            "safe_zone",
            "size",
            "lerp_target",
        ] {
            assert!(d.get_double(key).is_some(), "missing double {key}");
        }
        assert_eq!(d.get_int("warning_blocks"), Some(9));
        assert_eq!(d.get_int("warning_time"), Some(42));
        assert_eq!(d.get_long("lerp_time"), Some(1234));
    }

    /// `MapIndex` stores the LAST issued id and defaults to -1, so a fresh world
    /// hands out 0 first (`MapIndex.java:12,27-31`).
    #[test]
    fn map_index_stores_last_id_not_next_id() {
        let dir = TempDir::new().unwrap();
        write_next_map_id(dir.path(), 0, 4567).unwrap();
        let root = pumpkin_nbt::nbt_compress::read_gzip_compound_tag(
            std::fs::File::open(saved_data_file(dir.path(), MAP_INDEX_ID).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(root.get_compound("data").unwrap().get_int("map"), Some(-1));
        assert_eq!(read_next_map_id(dir.path()), Some(0));

        write_next_map_id(dir.path(), 5, 4567).unwrap();
        assert_eq!(read_next_map_id(dir.path()), Some(5));
    }
}
