use std::{
    fs::{self, File, OpenOptions},
    io::ErrorKind,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::error;

use num_traits::FromPrimitive;
use pumpkin_nbt::{
    compound::NbtCompound,
    nbt_compress::{read_gzip_compound_tag, write_gzip_compound_tag},
    tag::NbtTag,
};
use pumpkin_util::{Difficulty, world_seed::Seed};
use serde::{Deserialize, Serialize};

use crate::world_info::{
    DataPacks, MAXIMUM_SUPPORTED_LEVEL_VERSION, MAXIMUM_SUPPORTED_WORLD_DATA_VERSION,
    MINIMUM_SUPPORTED_LEVEL_VERSION, MINIMUM_SUPPORTED_WORLD_DATA_VERSION, WorldVersion,
    data_files::{
        minecraft_data_dir, read_game_rules, read_scoreboard, read_wandering_trader, read_weather,
        read_world_clocks, read_world_gen_settings, write_custom_boss_events_stub,
        write_game_rules, write_scheduled_events_stub, write_scoreboard, write_wandering_trader,
        write_weather, write_world_clocks, write_world_gen_settings,
    },
    default_data_packs,
};

use super::{LevelData, WorldInfoError, WorldInfoReader, WorldInfoWriter};

pub const LEVEL_DAT_FILE_NAME: &str = "level.dat";
pub const LEVEL_DAT_BACKUP_FILE_NAME: &str = "level.dat_old";

const LEVEL_DATA_TAG: &str = "Data";
const WORLD_GEN_SETTINGS_TAG: &str = "WorldGenSettings";

pub struct AnvilLevelInfo;

fn check_data_version(data: &NbtCompound) -> Result<(), WorldInfoError> {
    let Some(data_version) = data.get_int("DataVersion") else {
        error!(
            "The level.dat file does not have a data version! This means it is either corrupt or very old (read unsupported)"
        );
        return Err(WorldInfoError::DeserializationError(
            "Missing DataVersion".into(),
        ));
    };

    if (MINIMUM_SUPPORTED_WORLD_DATA_VERSION..=MAXIMUM_SUPPORTED_WORLD_DATA_VERSION)
        .contains(&data_version)
    {
        Ok(())
    } else {
        Err(WorldInfoError::UnsupportedDataVersion(data_version))
    }
}

fn check_level_version(data: &NbtCompound) -> Result<(), WorldInfoError> {
    let Some(level_version) = data.get_int("version") else {
        error!(
            "The level.dat file does not have a level version! This means it is either corrupt or very old (read unsupported)"
        );
        return Err(WorldInfoError::DeserializationError(
            "Missing version".into(),
        ));
    };

    if (MINIMUM_SUPPORTED_LEVEL_VERSION..=MAXIMUM_SUPPORTED_LEVEL_VERSION).contains(&level_version)
    {
        Ok(())
    } else {
        Err(WorldInfoError::UnsupportedLevelVersion(level_version))
    }
}

/// Reads one level.dat candidate and rejects it unless its `Data` compound carries a
/// supported `DataVersion`/`version` pair, so a corrupt current file falls through to the
/// `level.dat_old` backup instead of being accepted.
fn read_validated_level_dat(path: &Path) -> Result<NbtCompound, WorldInfoError> {
    let root = read_gzip_compound_tag(File::open(path)?)
        .map_err(|e| WorldInfoError::DeserializationError(e.to_string()))?;
    let Some(data) = root.get_compound(LEVEL_DATA_TAG) else {
        error!("The level.dat file has no {LEVEL_DATA_TAG} compound and is therefore corrupt");
        return Err(WorldInfoError::DeserializationError("Missing Data".into()));
    };
    check_data_version(data)?;
    check_level_version(data)?;
    Ok(root)
}

/// Restores a successfully read `level.dat_old` without exposing a partially
/// copied level.dat to the next startup. An unreadable current file is kept
/// under a unique name for diagnosis, matching vanilla's corrupted-file move.
fn restore_level_dat_from_backup(level_folder: &Path) -> Result<(), WorldInfoError> {
    let path = level_folder.join(LEVEL_DAT_FILE_NAME);
    let backup_path = level_folder.join(LEVEL_DAT_BACKUP_FILE_NAME);
    let restore_path = level_folder.join(format!(
        "level-restore-{}-{}.dat",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::copy(&backup_path, &restore_path)?;

    let corrupt_path = level_folder.join(format!(
        "level.dat_corrupt_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let had_current = path.exists();
    if had_current && let Err(error) = fs::rename(&path, &corrupt_path) {
        let _ = fs::remove_file(&restore_path);
        return Err(error.into());
    }

    if let Err(error) = fs::rename(&restore_path, &path) {
        if had_current && let Err(rollback_error) = fs::rename(&corrupt_path, &path) {
            error!("Failed to restore the unreadable level.dat: {rollback_error}");
        }
        let _ = fs::remove_file(&restore_path);
        return Err(error.into());
    }

    Ok(())
}

/// Reads the nested `spawn` compound (`LevelData.RespawnData.CODEC`, see
/// `net/minecraft/world/level/storage/PrimaryLevelData.java:126,139` and the record at
/// `net/minecraft/world/level/storage/LevelData.java:33-43`): a `GlobalPos` (`dimension`
/// string + `pos` 3-element int array, `net/minecraft/core/GlobalPos.java:13-15`) plus
/// float `yaw`/`pitch`.
///
/// Falls back to the legacy flat `SpawnX`/`SpawnY`/`SpawnZ`/`SpawnAngle`/`SpawnPitch` keys
/// that older Pumpkin builds wrote, so worlds saved by those builds still load correctly.
/// Split out of `read_world_info` to keep that function under the workspace line limit.
fn read_spawn(data: &pumpkin_nbt::compound::NbtCompound, level_data: &mut LevelData) {
    if let Some(spawn) = data.get_compound("spawn") {
        if let Some(pos) = spawn.get_int_array("pos")
            && let [x, y, z] = *pos
        {
            level_data.spawn_x = x;
            level_data.spawn_y = y;
            level_data.spawn_z = z;
        }
        if let Some(v) = spawn.get_float("yaw") {
            level_data.spawn_yaw = v;
        }
        if let Some(v) = spawn.get_float("pitch") {
            level_data.spawn_pitch = v;
        }
        return;
    }

    // Legacy Pumpkin schema (commit 76265bef), migrated on next save.
    if let Some(v) = data.get_int("SpawnX") {
        level_data.spawn_x = v;
    }
    if let Some(v) = data.get_int("SpawnY") {
        level_data.spawn_y = v;
    }
    if let Some(v) = data.get_int("SpawnZ") {
        level_data.spawn_z = v;
    }
    if let Some(v) = data.get_float("SpawnAngle") {
        level_data.spawn_yaw = v;
    }
    if let Some(v) = data.get_float("SpawnPitch") {
        level_data.spawn_pitch = v;
    }
}

/// Reads the nested `difficulty_settings` compound
/// (`LevelSettings.DifficultySettings.CODEC`, `net/minecraft/world/level/LevelSettings.java:58-67`):
/// `difficulty` as a lowercase string (`Difficulty.CODEC` is `StringRepresentable.fromEnum`,
/// `net/minecraft/world/Difficulty.java:12-18`), plus `hardcore` and `locked` booleans.
///
/// Falls back to the legacy flat root-level `Difficulty` byte / `DifficultyLocked` bool
/// that older Pumpkin builds wrote, so worlds saved by those builds still load correctly.
/// Pumpkin does not track hardcore mode in `LevelData`, so `hardcore` is read but discarded.
/// Split out of `read_world_info` to keep that function under the workspace line limit.
fn read_difficulty(data: &pumpkin_nbt::compound::NbtCompound, level_data: &mut LevelData) {
    if let Some(settings) = data.get_compound("difficulty_settings") {
        if let Some(v) = settings
            .get_string("difficulty")
            .and_then(|s| s.parse::<Difficulty>().ok())
        {
            level_data.difficulty = v;
        }
        if let Some(v) = settings.get_bool("locked") {
            level_data.difficulty_locked = v;
        }
        return;
    }

    // Legacy Pumpkin schema (commit 76265bef), migrated on next save.
    if let Some(v) = data.get_byte("Difficulty").and_then(Difficulty::from_i8) {
        level_data.difficulty = v;
    }
    if let Some(v) = data.get_bool("DifficultyLocked") {
        level_data.difficulty_locked = v;
    }
}

/// Reads the `DataPacks` sub-compound of level.dat's `Data` compound.
/// Split out of `read_world_info` to keep that function under the workspace line limit.
fn read_data_packs(data: &pumpkin_nbt::compound::NbtCompound, level_data: &mut LevelData) {
    let Some(packs) = data.get_compound("DataPacks") else {
        return;
    };
    let strings = |list: &[pumpkin_nbt::tag::NbtTag]| -> Vec<String> {
        list.iter()
            .filter_map(|t| match t {
                pumpkin_nbt::tag::NbtTag::String(s) => Some(s.to_string()),
                _ => None,
            })
            .collect()
    };
    if let Some(list) = packs.get_list("Disabled") {
        level_data.data_packs.disabled = strings(list);
    }
    if let Some(list) = packs.get_list("Enabled") {
        level_data.data_packs.enabled = strings(list);
    }
}

/// Builds the `DataPacks` sub-compound for level.dat.
/// Split out of `write_world_info` to keep that function under the workspace line limit.
fn write_data_packs(level_data: &LevelData) -> pumpkin_nbt::compound::NbtCompound {
    let as_tags = |v: &[String]| -> Vec<pumpkin_nbt::tag::NbtTag> {
        v.iter()
            .map(|s| pumpkin_nbt::tag::NbtTag::String(s.clone().into_boxed_str()))
            .collect()
    };
    let mut comp = pumpkin_nbt::compound::NbtCompound::new();
    comp.put_list("Disabled", as_tags(&level_data.data_packs.disabled));
    comp.put_list("Enabled", as_tags(&level_data.data_packs.enabled));
    comp
}

/// Builds the nested `spawn` compound. See [`read_spawn`] for the schema citation.
/// Pumpkin does not track a per-spawn dimension, so `dimension` is always written as the
/// overworld, matching the "minecraft:overworld" literal used elsewhere in this module.
fn write_spawn(level_data: &LevelData) -> pumpkin_nbt::compound::NbtCompound {
    let mut spawn = pumpkin_nbt::compound::NbtCompound::new();
    spawn.put_string("dimension", "minecraft:overworld".to_string());
    spawn.put(
        "pos",
        pumpkin_nbt::tag::NbtTag::IntArray(vec![
            level_data.spawn_x,
            level_data.spawn_y,
            level_data.spawn_z,
        ]),
    );
    spawn.put_float("yaw", level_data.spawn_yaw);
    spawn.put_float("pitch", level_data.spawn_pitch);
    spawn
}

/// Builds the nested `difficulty_settings` compound. See [`read_difficulty`] for the
/// schema citation. Pumpkin does not track hardcore mode in `LevelData`, so `hardcore` is
/// always written as `false`.
fn write_difficulty(level_data: &LevelData) -> pumpkin_nbt::compound::NbtCompound {
    let mut settings = pumpkin_nbt::compound::NbtCompound::new();
    settings.put_string("difficulty", level_data.difficulty.name().to_string());
    settings.put_bool("hardcore", false);
    settings.put_bool("locked", level_data.difficulty_locked);
    settings
}

const fn difficulty_from_id(id: i8) -> Option<Difficulty> {
    match id {
        0 => Some(Difficulty::Peaceful),
        1 => Some(Difficulty::Easy),
        2 => Some(Difficulty::Normal),
        3 => Some(Difficulty::Hard),
        _ => None,
    }
}

fn strings_from_nbt(tags: &[NbtTag]) -> Vec<String> {
    tags.iter()
        .filter_map(|tag| tag.extract_string().map(ToString::to_string))
        .collect()
}

fn strings_to_nbt(values: &[String]) -> Vec<NbtTag> {
    values
        .iter()
        .map(|value| NbtTag::from(value.as_str()))
        .collect()
}

fn world_version_from_nbt(version: &NbtCompound) -> WorldVersion {
    let mut world_version = WorldVersion::default();
    if let Some(name) = version.get_string("Name") {
        world_version.name = name.to_string();
    }
    if let Some(id) = version.get_int("Id") {
        world_version.id = id;
    }
    if let Some(snapshot) = version.get_bool("Snapshot") {
        world_version.snapshot = snapshot;
    }
    if let Some(series) = version.get_string("Series") {
        world_version.series = series.to_string();
    }
    world_version
}

fn world_version_to_nbt(version: &WorldVersion) -> NbtCompound {
    let mut compound = NbtCompound::new();
    compound.put_string("Name", version.name.clone());
    compound.put_int("Id", version.id);
    compound.put_bool("Snapshot", version.snapshot);
    compound.put_string("Series", version.series.clone());
    compound
}

fn data_packs_from_nbt(packs: &NbtCompound) -> DataPacks {
    let mut data_packs = default_data_packs();
    if let Some(disabled) = packs.get_list("Disabled") {
        data_packs.disabled = strings_from_nbt(disabled);
    }
    if let Some(enabled) = packs.get_list("Enabled") {
        data_packs.enabled = strings_from_nbt(enabled);
    }
    data_packs
}

fn data_packs_to_nbt(packs: &DataPacks) -> NbtCompound {
    let mut compound = NbtCompound::new();
    compound.put_list("Disabled", strings_to_nbt(&packs.disabled));
    compound.put_list("Enabled", strings_to_nbt(&packs.enabled));
    compound
}

fn stored_world_seed(level_folder: &Path, data: &NbtCompound) -> Option<i64> {
    read_world_gen_settings(level_folder)
        .map(|settings| settings.seed)
        .or_else(|| {
            data.get_compound(WORLD_GEN_SETTINGS_TAG)
                .and_then(|settings| settings.get_long("seed"))
        })
}

fn put_world_gen_settings_seed(data: &mut NbtCompound, seed: i64) {
    let mut world_gen_settings = data
        .get_compound(WORLD_GEN_SETTINGS_TAG)
        .cloned()
        .unwrap_or_default();
    world_gen_settings.put_long("seed", seed);
    data.put_compound(WORLD_GEN_SETTINGS_TAG, world_gen_settings);
}

fn update_world_border_from_nbt(level_data: &mut LevelData, data: &NbtCompound) {
    if let Some(border_center_x) = data.get_double("BorderCenterX") {
        level_data.border_center_x = border_center_x;
    }
    if let Some(border_center_z) = data.get_double("BorderCenterZ") {
        level_data.border_center_z = border_center_z;
    }
    if let Some(border_damage_per_block) = data.get_double("BorderDamagePerBlock") {
        level_data.border_damage_per_block = border_damage_per_block;
    }
    if let Some(border_size) = data.get_double("BorderSize") {
        level_data.border_size = border_size;
    }
    if let Some(border_safe_zone) = data.get_double("BorderSafeZone") {
        level_data.border_safe_zone = border_safe_zone;
    }
    if let Some(border_size_lerp_target) = data.get_double("BorderSizeLerpTarget") {
        level_data.border_size_lerp_target = border_size_lerp_target;
    }
    if let Some(border_size_lerp_time) = data.get_long("BorderSizeLerpTime") {
        level_data.border_size_lerp_time = border_size_lerp_time;
    }
    if let Some(border_warning_blocks) = data.get_double("BorderWarningBlocks") {
        level_data.border_warning_blocks = border_warning_blocks;
    }
    if let Some(border_warning_time) = data.get_double("BorderWarningTime") {
        level_data.border_warning_time = border_warning_time;
    }
}

fn update_spawn_from_nbt(level_data: &mut LevelData, data: &NbtCompound) {
    if let Some(spawn_x) = data.get_int("SpawnX") {
        level_data.spawn_x = spawn_x;
    }
    if let Some(spawn_y) = data.get_int("SpawnY") {
        level_data.spawn_y = spawn_y;
    }
    if let Some(spawn_z) = data.get_int("SpawnZ") {
        level_data.spawn_z = spawn_z;
    }
    if let Some(spawn_yaw) = data
        .get_float("SpawnAngle")
        .or_else(|| data.get_float("SpawnYaw"))
    {
        level_data.spawn_yaw = spawn_yaw;
    }
    if let Some(spawn_pitch) = data.get_float("SpawnPitch") {
        level_data.spawn_pitch = spawn_pitch;
    }
}

fn level_data_from_nbt(data: &NbtCompound, seed: i64) -> LevelData {
    let mut level_data = LevelData::default(Seed(seed as u64));

    update_world_border_from_nbt(&mut level_data, data);
    update_spawn_from_nbt(&mut level_data, data);

    if let Some(allow_commands) = data.get_bool("allowCommands") {
        level_data.allow_commands = allow_commands;
    }
    if let Some(data_packs) = data.get_compound("DataPacks") {
        level_data.data_packs = data_packs_from_nbt(data_packs);
    }
    if let Some(data_version) = data.get_int("DataVersion") {
        level_data.data_version = data_version;
    }
    if let Some(difficulty) = data.get_byte("Difficulty").and_then(difficulty_from_id) {
        level_data.difficulty = difficulty;
    }
    if let Some(difficulty_locked) = data.get_bool("DifficultyLocked") {
        level_data.difficulty_locked = difficulty_locked;
    }
    if let Some(last_played) = data.get_long("LastPlayed") {
        level_data.last_played = last_played;
    }
    if let Some(level_name) = data.get_string("LevelName") {
        level_data.level_name = level_name.to_string();
    }
    if let Some(world_version) = data.get_compound("Version") {
        level_data.world_version = world_version_from_nbt(world_version);
    }
    if let Some(level_version) = data.get_int("version") {
        level_data.level_version = level_version;
    }
    if let Some(map_id) = data.get_int("map_id") {
        level_data.map_id = map_id;
    }
    if let Some(day_time) = data.get_long("DayTime") {
        level_data.day_time = day_time;
    }
    if let Some(clear_weather_time) = data.get_int("clearWeatherTime") {
        level_data.clear_weather_time = clear_weather_time;
    }

    level_data
}

fn level_data_to_nbt(info: &LevelData, data: &mut NbtCompound) {
    data.put_bool("allowCommands", info.allow_commands);
    data.put_bool("initialized", info.initialized);
    data.put_double("BorderCenterX", info.border_center_x);
    data.put_double("BorderCenterZ", info.border_center_z);
    data.put_double("BorderDamagePerBlock", info.border_damage_per_block);
    data.put_double("BorderSize", info.border_size);
    data.put_double("BorderSafeZone", info.border_safe_zone);
    data.put_double("BorderSizeLerpTarget", info.border_size_lerp_target);
    data.put_long("BorderSizeLerpTime", info.border_size_lerp_time);
    data.put_double("BorderWarningBlocks", info.border_warning_blocks);
    data.put_double("BorderWarningTime", info.border_warning_time);
    data.put_compound("DataPacks", data_packs_to_nbt(&info.data_packs));
    data.put_int("DataVersion", info.data_version);
    data.put_byte("Difficulty", info.difficulty as i8);
    data.put_bool("DifficultyLocked", info.difficulty_locked);
    data.put_long("LastPlayed", info.last_played);
    data.put_string("LevelName", info.level_name.clone());
    data.put_int("SpawnX", info.spawn_x);
    data.put_int("SpawnY", info.spawn_y);
    data.put_int("SpawnZ", info.spawn_z);
    data.put_float("SpawnAngle", info.spawn_yaw);
    data.put_float("SpawnPitch", info.spawn_pitch);
    data.put_compound("Version", world_version_to_nbt(&info.world_version));
    data.put_int("version", info.level_version);
    data.put_int("map_id", info.map_id);
    put_world_gen_settings_seed(data, info.world_gen_settings.seed);
}

fn stamp_current_version(level_data: &mut LevelData) {
    level_data.data_version = MAXIMUM_SUPPORTED_WORLD_DATA_VERSION;
    level_data.level_version = MAXIMUM_SUPPORTED_LEVEL_VERSION;
    level_data.world_version = WorldVersion::default();
}

fn existing_level_dat_root(path: &Path) -> Result<NbtCompound, WorldInfoError> {
    match File::open(path) {
        Ok(file) => read_gzip_compound_tag(file)
            .map_err(|e| WorldInfoError::DeserializationError(e.to_string())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(NbtCompound::new()),
        Err(e) => Err(e.into()),
    }
}

impl WorldInfoReader for AnvilLevelInfo {
    fn read_world_info(&self, level_folder: &Path) -> Result<LevelData, WorldInfoError> {
        let path = level_folder.join(LEVEL_DAT_FILE_NAME);

        let root = match read_validated_level_dat(&path) {
            Ok(root) => root,
            Err(primary_error) => {
                let backup_path = level_folder.join(LEVEL_DAT_BACKUP_FILE_NAME);
                match read_validated_level_dat(&backup_path) {
                    Ok(root) => {
                        if let Err(error) = restore_level_dat_from_backup(level_folder) {
                            error!("Failed to restore level.dat from level.dat_old: {error}");
                        }
                        root
                    }
                    Err(WorldInfoError::InfoNotFound) => return Err(primary_error),
                    Err(backup_error) => return Err(backup_error),
                }
            }
        };
        let Some(data) = root.get_compound(LEVEL_DATA_TAG) else {
            error!("The level.dat file has no {LEVEL_DATA_TAG} compound and is therefore corrupt");
            return Err(WorldInfoError::DeserializationError("Missing Data".into()));
        };

        let Some(seed) = stored_world_seed(level_folder, data) else {
            return Err(WorldInfoError::MissingWorldSeed);
        };

        let mut level_data = level_data_from_nbt(data, seed);

        // Fields still stored directly in level.dat's "Data" compound (spawn point,
        // difficulty, allowCommands, level name, data packs). Everything else has moved to
        // the per-type save files under data/minecraft/*.dat, handled below. `WorldBorder`
        // and `map_id` are no longer read from level.dat at all -- vanilla keeps those in
        // separate SavedData files ("world_border" and "maps/last_id" respectively; see
        // net/minecraft/world/level/border/WorldBorder.java:24-26 and
        // net/minecraft/world/level/saveddata/maps/MapIndex.java:12-16) that Pumpkin does
        // not yet write, so those values stay at LevelData::default() until that lands.
        // Missing keys otherwise fall back to LevelData::default()'s values.
        // Read both the upstream flat schema and Pumpkin's nested schema. The helpers are
        // intentionally fallback-aware so legacy worlds remain loadable.
        // `initialized` defaults to true so a world written before Pumpkin tracked it is not
        // re-initialized on load.
        level_data.initialized = data.get_bool("initialized").unwrap_or(true);
        read_spawn(data, &mut level_data);
        read_difficulty(data, &mut level_data);
        read_data_packs(data, &mut level_data);

        // game_rules.dat – prefer the new file; fall back to level.dat values
        if minecraft_data_dir(level_folder)
            .join("game_rules.dat")
            .exists()
        {
            level_data.game_rules = read_game_rules(level_folder);
        }

        if minecraft_data_dir(level_folder)
            .join("world_clocks.dat")
            .exists()
        {
            let clocks = read_world_clocks(level_folder);
            if let Some(overworld) = clocks.clocks.get("minecraft:overworld") {
                level_data.day_time = overworld.total_ticks;
            }
        }

        // weather.dat
        if minecraft_data_dir(level_folder)
            .join("weather.dat")
            .exists()
        {
            let weather = read_weather(level_folder);
            level_data.clear_weather_time = weather.clear_weather_time;
            level_data.rain_time = weather.rain_time;
            level_data.raining = weather.raining;
            level_data.thundering = weather.thundering;
            level_data.thunder_time = weather.thunder_time;
        }

        // world_gen_settings.dat
        if minecraft_data_dir(level_folder)
            .join("world_gen_settings.dat")
            .exists()
            && let Some(wgs) = read_world_gen_settings(level_folder)
        {
            level_data.world_gen_settings = wgs;
        }

        // scoreboard.dat
        level_data.scoreboard_data = read_scoreboard(level_folder);

        // (wandering_trader.dat is not part of LevelData; stored separately when needed)

        Ok(level_data)
    }
}

impl WorldInfoWriter for AnvilLevelInfo {
    fn write_world_info(
        &self,
        info: &LevelData,
        level_folder: &Path,
    ) -> Result<(), WorldInfoError> {
        let start = SystemTime::now();
        let since_the_epoch = start.duration_since(UNIX_EPOCH).unwrap_or_default();
        let mut level_data = info.clone();
        level_data.last_played = since_the_epoch.as_millis() as i64;
        stamp_current_version(&mut level_data);

        // ── Write level.dat ───────────────────────────────────────────────────
        let path = level_folder.join(LEVEL_DAT_FILE_NAME);
        // Vanilla `LevelStorageSource.LevelStorageAccess.saveLevelData` writes to a temporary
        // file, rotates the previous level.dat to level.dat_old and only then renames the new
        // file into place, so an interrupted save never leaves a truncated level.dat.
        let temp_path = level_folder.join(format!(
            "level-{}-{}.dat",
            std::process::id(),
            since_the_epoch.as_nanos()
        ));
        let world_info_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;

        // Start from the existing document so unknown NBT keys survive the rewrite.
        let mut root = existing_level_dat_root(&path)?;
        let mut data_comp = root
            .get_compound(LEVEL_DATA_TAG)
            .cloned()
            .unwrap_or_default();
        level_data_to_nbt(&level_data, &mut data_comp);
        data_comp.put_compound("difficulty_settings", write_difficulty(&level_data));
        data_comp.put_compound("spawn", write_spawn(&level_data));
        // `WorldBorder` and `map_id` are deliberately not written: vanilla keeps them in
        // separate SavedData files, not level.dat (see the comment in read_world_info).

        data_comp.put_compound("DataPacks", write_data_packs(&level_data));
        root.put_compound(LEVEL_DATA_TAG, data_comp);

        if let Err(error) = write_gzip_compound_tag(root, world_info_file) {
            let _ = fs::remove_file(&temp_path);
            return Err(WorldInfoError::SerializationError(error.to_string()));
        }

        let backup_path = level_folder.join(LEVEL_DAT_BACKUP_FILE_NAME);
        let had_current = path.exists();
        if had_current {
            if backup_path.exists() {
                fs::remove_file(&backup_path)?;
            }
            fs::rename(&path, &backup_path)?;
        }

        if let Err(error) = fs::rename(&temp_path, &path) {
            if had_current && let Err(rollback_error) = fs::rename(&backup_path, &path) {
                error!("Failed to restore level.dat after replacement failure: {rollback_error}");
            }
            return Err(error.into());
        }

        let data_version = level_data.data_version;

        // ── Write data/minecraft/*.dat files ─────────────────────────────────

        // game_rules.dat
        if let Err(e) = write_game_rules(level_folder, &info.game_rules, data_version) {
            error!("Failed to write game_rules.dat: {e}");
        }

        // world_gen_settings.dat
        if let Err(e) =
            write_world_gen_settings(level_folder, &info.world_gen_settings, data_version)
        {
            error!("Failed to write world_gen_settings.dat: {e}");
        }

        // world_clocks.dat – persist the overworld day_time; preserve other
        let mut clocks = read_world_clocks(level_folder);
        clocks.data_version = data_version;
        clocks
            .clocks
            .entry("minecraft:overworld".to_string())
            .and_modify(|c| c.total_ticks = info.day_time)
            .or_insert(crate::world_info::data_files::DimensionClock {
                total_ticks: info.day_time,
            });

        if let Err(e) = write_world_clocks(level_folder, &clocks) {
            error!("Failed to write world_clocks.dat: {e}");
        }

        // weather.dat
        let weather = crate::world_info::data_files::WeatherData {
            rain_time: info.rain_time,
            raining: info.raining,
            thundering: info.thundering,
            thunder_time: info.thunder_time,
            clear_weather_time: info.clear_weather_time,
            data_version,
        };
        if let Err(e) = write_weather(level_folder, &weather) {
            error!("Failed to write weather.dat: {e}");
        }

        // scoreboard.dat
        if let Err(e) = write_scoreboard(level_folder, &info.scoreboard_data) {
            error!("Failed to write scoreboard.dat: {e}");
        }

        // wandering_trader.dat (stub / load-save)
        let mut wandering_trader = read_wandering_trader(level_folder);
        wandering_trader.data_version = data_version;
        if let Err(e) = write_wandering_trader(level_folder, &wandering_trader) {
            error!("Failed to write wandering_trader.dat: {e}");
        }

        // custom_boss_events.dat
        if let Err(e) = write_custom_boss_events_stub(level_folder, data_version) {
            error!("Failed to write custom_boss_events.dat: {e}");
        }

        // scheduled_events.dat
        if let Err(e) = write_scheduled_events_stub(level_folder, data_version) {
            error!("Failed to write scheduled_events.dat: {e}");
        }

        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
pub struct LevelDat {
    // This tag contains all the level data.
    #[serde(rename = "Data")]
    pub data: LevelData,
}

#[cfg(test)]
mod test {

    use pumpkin_data::game_rules::GameRuleRegistry;
    use pumpkin_nbt::{
        compound::NbtCompound,
        nbt_compress::{read_gzip_compound_tag, write_gzip_compound_tag},
        tag::NbtTag,
    };
    use pumpkin_util::{Difficulty, world_seed::Seed};
    use std::{
        fs::{self, File},
        path::Path,
        sync::LazyLock,
    };
    use tempfile::TempDir;

    use crate::{
        CURRENT_MC_VERSION,
        world_info::{
            DataPacks, LevelData, MAXIMUM_SUPPORTED_LEVEL_VERSION,
            MAXIMUM_SUPPORTED_WORLD_DATA_VERSION, MINIMUM_SUPPORTED_LEVEL_VERSION,
            MINIMUM_SUPPORTED_WORLD_DATA_VERSION, WorldGenSettings, WorldInfoError, WorldVersion,
            data_files::minecraft_data_dir,
        },
    };

    use super::{
        AnvilLevelInfo, LEVEL_DAT_BACKUP_FILE_NAME, LEVEL_DAT_FILE_NAME, LevelDat, WorldInfoReader,
        WorldInfoWriter,
    };
    use crate::world_info::data_files::ScoreboardData;

    const CONVERTED_LEVEL_NAME: &str = "Converted World";

    fn converted_level_dat(seed: Option<i64>) -> NbtCompound {
        let mut world_version = NbtCompound::new();
        world_version.put_string("Name", "1.21.9".to_string());
        world_version.put_int("Id", MINIMUM_SUPPORTED_WORLD_DATA_VERSION);

        let mut data_packs = NbtCompound::new();
        data_packs.put_list("Enabled", vec![NbtTag::from("vanilla")]);

        let mut data = NbtCompound::new();
        data.put_int("DataVersion", MINIMUM_SUPPORTED_WORLD_DATA_VERSION);
        data.put_int("version", MINIMUM_SUPPORTED_LEVEL_VERSION);
        data.put_string("LevelName", CONVERTED_LEVEL_NAME.to_string());
        data.put_bool("allowCommands", true);
        data.put_byte("Difficulty", Difficulty::Hard as i8);
        data.put_int("SpawnX", 128);
        data.put_int("SpawnY", 72);
        data.put_int("SpawnZ", -64);
        data.put_float("SpawnAngle", 90.0);
        data.put_double("BorderSize", 1024.0);
        data.put_long("DayTime", 5000);
        data.put_bool("hardcore", true);
        data.put_compound("Version", world_version);
        data.put_compound("DataPacks", data_packs);
        if let Some(seed) = seed {
            let mut world_gen_settings = NbtCompound::new();
            world_gen_settings.put_long("seed", seed);
            data.put_compound("WorldGenSettings", world_gen_settings);
        }

        let mut root = NbtCompound::new();
        root.put_compound("Data", data);
        root
    }

    fn write_level_dat(level_folder: &Path, root: NbtCompound) {
        let file = File::create(level_folder.join(LEVEL_DAT_FILE_NAME)).unwrap();
        write_gzip_compound_tag(root, file).unwrap();
    }

    fn read_level_dat(level_folder: &Path) -> NbtCompound {
        let file = File::open(level_folder.join(LEVEL_DAT_FILE_NAME)).unwrap();
        read_gzip_compound_tag(file).unwrap()
    }

    #[test]
    fn preserve_level_dat_seed() {
        let seed = 1337;

        let data = LevelData::default(Seed(1337));

        let temp_dir = TempDir::new().unwrap();

        AnvilLevelInfo
            .write_world_info(&data, temp_dir.path())
            .unwrap();

        let data = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap();

        assert_eq!(data.world_gen_settings.seed, seed);
    }

    #[test]
    fn read_level_dat_without_series() {
        let temp_dir = TempDir::new().unwrap();
        write_level_dat(temp_dir.path(), converted_level_dat(Some(987_654_321)));

        let data = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap();

        assert_eq!(data.world_gen_settings.seed, 987_654_321);
        assert_eq!((data.spawn_x, data.spawn_y, data.spawn_z), (128, 72, -64));
        assert_eq!(data.spawn_yaw, 90.0);
        assert_eq!(data.level_name, CONVERTED_LEVEL_NAME);
        assert_eq!(data.difficulty, Difficulty::Hard);
        assert!(data.allow_commands);
        assert_eq!(data.border_size, 1024.0);
        assert_eq!(data.border_safe_zone, 5.0);
        assert_eq!(data.day_time, 5000);
        assert_eq!(data.data_version, MINIMUM_SUPPORTED_WORLD_DATA_VERSION);
        assert_eq!(data.level_version, MINIMUM_SUPPORTED_LEVEL_VERSION);
        assert_eq!(data.world_version.name, "1.21.9");
        assert_eq!(data.world_version.id, MINIMUM_SUPPORTED_WORLD_DATA_VERSION);
        assert_eq!(data.world_version.series, "main");
        assert!(!data.world_version.snapshot);
        assert_eq!(data.data_packs.enabled, vec!["vanilla".to_string()]);
        assert!(data.data_packs.disabled.is_empty());
    }

    #[test]
    fn reject_level_dat_without_seed() {
        let temp_dir = TempDir::new().unwrap();
        write_level_dat(temp_dir.path(), converted_level_dat(None));

        let error = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap_err();

        assert!(matches!(error, WorldInfoError::MissingWorldSeed));
    }

    #[test]
    fn rewrite_level_dat_keeps_unmanaged_tags() {
        let temp_dir = TempDir::new().unwrap();
        write_level_dat(temp_dir.path(), converted_level_dat(Some(42)));

        let mut level_data = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap();
        level_data.level_name = "Renamed World".to_string();
        AnvilLevelInfo
            .write_world_info(&level_data, temp_dir.path())
            .unwrap();

        let root = read_level_dat(temp_dir.path());
        let data = root.get_compound("Data").unwrap();
        assert_eq!(data.get_bool("hardcore"), Some(true));
        assert_eq!(data.get_string("LevelName"), Some("Renamed World"));
        assert_eq!(
            data.get_compound("Version").unwrap().get_string("Series"),
            Some("main")
        );
    }

    #[test]
    fn rewrite_level_dat_stamps_current_version() {
        let temp_dir = TempDir::new().unwrap();
        write_level_dat(temp_dir.path(), converted_level_dat(Some(42)));

        let imported = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap();
        AnvilLevelInfo
            .write_world_info(&imported, temp_dir.path())
            .unwrap();

        let root = read_level_dat(temp_dir.path());
        let data = root.get_compound("Data").unwrap();
        assert_eq!(
            data.get_int("DataVersion"),
            Some(MAXIMUM_SUPPORTED_WORLD_DATA_VERSION)
        );
        assert_eq!(
            data.get_int("version"),
            Some(MAXIMUM_SUPPORTED_LEVEL_VERSION)
        );

        let version = data.get_compound("Version").unwrap();
        assert_eq!(version.get_string("Name"), Some(CURRENT_MC_VERSION));
        assert_eq!(
            version.get_int("Id"),
            Some(MAXIMUM_SUPPORTED_WORLD_DATA_VERSION)
        );

        let game_rules_path = minecraft_data_dir(temp_dir.path()).join("game_rules.dat");
        let game_rules = read_gzip_compound_tag(File::open(game_rules_path).unwrap()).unwrap();
        assert_eq!(
            game_rules
                .get_compound("data")
                .unwrap()
                .get_int("DataVersion"),
            Some(MAXIMUM_SUPPORTED_WORLD_DATA_VERSION)
        );
    }

    #[test]
    fn recover_seed_from_level_dat_after_losing_world_gen_settings() {
        let temp_dir = TempDir::new().unwrap();

        AnvilLevelInfo
            .write_world_info(&LevelData::default(Seed(8_675_309)), temp_dir.path())
            .unwrap();
        fs::remove_dir_all(temp_dir.path().join("data")).unwrap();

        let reloaded = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap();

        assert_eq!(reloaded.world_gen_settings.seed, 8_675_309);
    }

    #[test]
    fn round_trip_level_dat() {
        let temp_dir = TempDir::new().unwrap();

        let mut original = LevelData::default(Seed(4242));
        original.level_name = "Round Trip".to_string();
        original.difficulty = Difficulty::Easy;
        original.difficulty_locked = true;
        original.set_pos(16, -32);
        original.spawn_y = 64;
        original.spawn_yaw = 45.0;
        original.border_size = 2048.0;
        original.border_center_x = 8.0;
        original.day_time = 12_345;
        original.map_id = 3;

        AnvilLevelInfo
            .write_world_info(&original, temp_dir.path())
            .unwrap();

        let root = read_level_dat(temp_dir.path());
        let data = root.get_compound("Data").unwrap();
        assert_eq!(
            data.get_int("DataVersion"),
            Some(MAXIMUM_SUPPORTED_WORLD_DATA_VERSION)
        );
        assert_eq!(
            data.get_int("version"),
            Some(MAXIMUM_SUPPORTED_LEVEL_VERSION)
        );
        assert_eq!(data.get_string("LevelName"), Some("Round Trip"));
        assert_eq!(data.get_byte("Difficulty"), Some(Difficulty::Easy as i8));
        assert_eq!(data.get_bool("DifficultyLocked"), Some(true));
        assert_eq!(data.get_bool("allowCommands"), Some(true));
        assert_eq!(data.get_int("SpawnX"), Some(16));
        assert_eq!(data.get_int("SpawnY"), Some(64));
        assert_eq!(data.get_int("SpawnZ"), Some(-32));
        assert_eq!(data.get_float("SpawnAngle"), Some(45.0));
        assert_eq!(data.get_double("BorderSize"), Some(2048.0));
        assert_eq!(data.get_double("BorderCenterX"), Some(8.0));
        assert_eq!(data.get_int("map_id"), Some(3));
        assert!(data.get_long("LastPlayed").is_some_and(|played| played > 0));

        let version = data.get_compound("Version").unwrap();
        assert_eq!(version.get_string("Name"), Some(CURRENT_MC_VERSION));
        assert_eq!(
            version.get_int("Id"),
            Some(MAXIMUM_SUPPORTED_WORLD_DATA_VERSION)
        );
        assert_eq!(version.get_bool("Snapshot"), Some(false));
        assert_eq!(version.get_string("Series"), Some("main"));

        let data_packs = data.get_compound("DataPacks").unwrap();
        assert_eq!(
            data_packs.get_list("Enabled"),
            Some([NbtTag::from("vanilla")].as_slice())
        );
        assert!(
            data_packs
                .get_list("Disabled")
                .is_some_and(<[NbtTag]>::is_empty)
        );

        let mut reloaded = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap();
        assert!(reloaded.last_played > 0);
        reloaded.last_played = original.last_played;

        assert_eq!(reloaded, original);
    }

    static LEVEL_DAT: LazyLock<LevelDat> = LazyLock::new(|| LevelDat {
        data: LevelData {
            allow_commands: true,
            border_center_x: 0.0,
            border_center_z: 0.0,
            border_damage_per_block: 0.2,
            border_size: 59_999_968.0,
            border_safe_zone: 5.0,
            border_size_lerp_target: 59_999_968.0,
            border_size_lerp_time: 0,
            border_warning_blocks: 5.0,
            border_warning_time: 15.0,
            clear_weather_time: 0,
            rain_time: 0,
            raining: false,
            thundering: false,
            thunder_time: 0,
            data_packs: DataPacks {
                disabled: vec![
                    "minecart_improvements".to_string(),
                    "redstone_experiments".to_string(),
                    "trade_rebalance".to_string(),
                ],
                enabled: vec!["vanilla".to_string()],
            },
            data_version: 4189,
            day_time: 1727,
            difficulty: Difficulty::Normal,
            difficulty_locked: false,
            initialized: true,
            game_rules: GameRuleRegistry {
                block_explosion_drop_decay: true,
                command_block_output: true,
                drowning_damage: true,
                ender_pearls_vanish_on_death: true,
                fall_damage: true,
                fire_damage: true,
                forgive_dead_players: true,
                freeze_damage: true,
                global_sound_events: true,
                keep_inventory: false,
                lava_source_conversion: false,
                log_admin_commands: true,
                max_entity_cramming: 24,
                mob_explosion_drop_decay: true,
                mob_griefing: true,
                players_nether_portal_creative_delay: 0,
                players_nether_portal_default_delay: 80,
                players_sleeping_percentage: 100,
                projectiles_can_break_blocks: true,
                random_tick_speed: 3,
                reduced_debug_info: false,
                send_command_feedback: true,
                show_death_messages: true,
                spectators_generate_chunks: true,
                tnt_explosion_drop_decay: false,
                universal_anger: false,
                water_source_conversion: true,
                ..Default::default()
            },
            world_gen_settings: WorldGenSettings::new(Seed(1)),
            last_played: 1733847709327,
            level_name: "New World".to_string(),
            scoreboard_data: ScoreboardData::default(),
            spawn_x: 160,
            spawn_y: 70,
            spawn_z: 160,
            spawn_yaw: 0.0,
            spawn_pitch: 0.0,
            level_version: 19133,
            world_version: WorldVersion {
                name: "1.21.4".to_string(),
                id: 4189,
                snapshot: false,
                series: "main".to_string(),
            },
            map_id: 0,
        },
    });

    // #[test]
    // fn deserialize_level_dat() {
    //     let raw_compressed_nbt = fs::read("assets/level_1_21_4.dat").unwrap();
    //     assert!(!raw_compressed_nbt.is_empty());

    //     let mut decoder = GzDecoder::new(&raw_compressed_nbt[..]);
    //     let mut buf = Vec::new();
    //     decoder.read_to_end(&mut buf).unwrap();
    //     let level_dat: LevelDat = from_bytes(Cursor::new(buf)).expect("Failed to decode from file");

    //     assert_eq!(level_dat, *LEVEL_DAT);
    // }

    #[test]
    fn serialize_level_dat() {
        let mut data_comp = pumpkin_nbt::compound::NbtCompound::new();
        data_comp.put_int("DataVersion", LEVEL_DAT.data.data_version);
        data_comp.put_long("LastPlayed", LEVEL_DAT.data.last_played);

        let mut root = pumpkin_nbt::compound::NbtCompound::new();
        root.put_compound("Data", data_comp);

        let bytes = pumpkin_nbt::Nbt::from(root).write();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn preserves_world_clock_and_weather_data() {
        let temp_dir = TempDir::new().unwrap();
        let mut data = LevelData::default(Seed(42));
        data.day_time = 36_001;
        data.clear_weather_time = 1_200;
        data.rain_time = 9_001;
        data.raining = true;
        data.thundering = true;
        data.thunder_time = 4_501;

        AnvilLevelInfo
            .write_world_info(&data, temp_dir.path())
            .unwrap();

        let loaded = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap();
        assert_eq!(loaded.day_time, data.day_time);
        assert_eq!(loaded.clear_weather_time, data.clear_weather_time);
        assert_eq!(loaded.rain_time, data.rain_time);
        assert_eq!(loaded.raining, data.raining);
        assert_eq!(loaded.thundering, data.thundering);
        assert_eq!(loaded.thunder_time, data.thunder_time);
    }

    #[test]
    fn preserves_uninitialized_world_flag() {
        let temp_dir = TempDir::new().unwrap();
        let mut data = LevelData::default(Seed(42));
        data.initialized = false;

        AnvilLevelInfo
            .write_world_info(&data, temp_dir.path())
            .unwrap();

        let loaded = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap();
        assert!(!loaded.initialized);
    }

    #[test]
    fn replaces_level_dat_and_preserves_previous_backup() {
        let temp_dir = TempDir::new().unwrap();
        let mut first = LevelData::default(Seed(42));
        first.initialized = false;
        AnvilLevelInfo
            .write_world_info(&first, temp_dir.path())
            .unwrap();

        let mut second = first;
        second.initialized = true;
        AnvilLevelInfo
            .write_world_info(&second, temp_dir.path())
            .unwrap();

        let backup = File::open(temp_dir.path().join(LEVEL_DAT_BACKUP_FILE_NAME)).unwrap();
        let backup_root = pumpkin_nbt::nbt_compress::read_gzip_compound_tag(backup).unwrap();
        assert_eq!(
            backup_root
                .get_compound("Data")
                .and_then(|data| data.get_bool("initialized")),
            Some(false)
        );
        assert!(
            AnvilLevelInfo
                .read_world_info(temp_dir.path())
                .unwrap()
                .initialized
        );
    }

    #[test]
    fn restores_level_dat_from_backup_when_current_is_missing() {
        let temp_dir = TempDir::new().unwrap();
        let mut data = LevelData::default(Seed(42));
        data.initialized = false;
        AnvilLevelInfo
            .write_world_info(&data, temp_dir.path())
            .unwrap();

        let current = temp_dir.path().join(super::LEVEL_DAT_FILE_NAME);
        let backup = temp_dir.path().join(LEVEL_DAT_BACKUP_FILE_NAME);
        std::fs::rename(&current, &backup).unwrap();

        let loaded = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap();
        assert!(!loaded.initialized);
        assert!(current.is_file());
        assert!(backup.is_file());
    }

    #[test]
    fn restores_backup_when_current_level_data_version_is_invalid() {
        let temp_dir = TempDir::new().unwrap();
        let data = LevelData::default(Seed(42));
        AnvilLevelInfo
            .write_world_info(&data, temp_dir.path())
            .unwrap();

        let current = temp_dir.path().join(super::LEVEL_DAT_FILE_NAME);
        let backup = temp_dir.path().join(LEVEL_DAT_BACKUP_FILE_NAME);
        std::fs::copy(&current, &backup).unwrap();

        let mut data_tag = pumpkin_nbt::compound::NbtCompound::new();
        data_tag.put_int("DataVersion", 1);
        data_tag.put_int("version", 1);
        let mut root = pumpkin_nbt::compound::NbtCompound::new();
        root.put_compound("Data", data_tag);
        let file = File::create(&current).unwrap();
        pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, file).unwrap();

        let loaded = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap();
        assert_eq!(loaded.world_gen_settings.seed, 42);
        assert!(current.is_file());
        assert!(backup.is_file());
    }

    #[test]
    fn returns_backup_validation_error_when_current_is_missing() {
        let temp_dir = TempDir::new().unwrap();
        let data = LevelData::default(Seed(42));
        AnvilLevelInfo
            .write_world_info(&data, temp_dir.path())
            .unwrap();

        let current = temp_dir.path().join(super::LEVEL_DAT_FILE_NAME);
        let backup = temp_dir.path().join(LEVEL_DAT_BACKUP_FILE_NAME);
        std::fs::rename(&current, &backup).unwrap();

        let mut data_tag = pumpkin_nbt::compound::NbtCompound::new();
        data_tag.put_int("DataVersion", 1);
        data_tag.put_int("version", 1);
        let mut root = pumpkin_nbt::compound::NbtCompound::new();
        root.put_compound("Data", data_tag);
        let file = File::create(&backup).unwrap();
        pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, file).unwrap();

        let error = AnvilLevelInfo.read_world_info(temp_dir.path()).unwrap_err();
        assert!(matches!(
            error,
            super::WorldInfoError::UnsupportedDataVersion(1)
        ));
    }
}
