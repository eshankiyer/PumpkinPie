//! Port of `net.minecraft.world.entity.monster.warden.WardenSpawnTracker`
//! (`world/entity/monster/warden/WardenSpawnTracker.java`).
//!
//! Vanilla hangs one tracker off every `ServerPlayer`, ticks it from the player's own tick
//! (`ServerPlayer.tick` -> `wardenSpawnTracker.tick()`) and serialises it into the player's
//! `.dat` under `warden_spawn_tracker` (the codec at WardenSpawnTracker.java:18-25).
//!
//! Two deliberate deviations, both forced by where the seams are in this codebase:
//!
//! 1. **Storage location.** `Player`'s `NBTStorage` impl has no extension point, so the
//!    trackers live in a side file, `<world root>/data/warden_spawn_tracker.dat`, keyed by
//!    player UUID. The per-player compound uses vanilla's own key names
//!    (`ticks_since_last_warning`, `warning_level`, `cooldown_ticks`,
//!    WardenSpawnTracker.java:20-22) so the values are directly comparable with a vanilla
//!    save, and so this can be lifted into `Player::write_nbt` unchanged later.
//! 2. **Lazy decay instead of a per-tick `tick()`.** There is no player-tick hook available
//!    here either, so each entry records the `world_age` it was last touched and is advanced
//!    on read. Over `E` elapsed ticks vanilla's loop performs `(t + E) / 12001` warning-level
//!    decrements, leaves `(t + E) % 12001` in `ticks_since_last_warning` and drops the
//!    cooldown to `max(0, c - E)`, which is what [`WardenSpawnTracker::advance`] computes. The observable difference is that offline time counts toward decay here,
//!    where vanilla only ticks a player who is in the world.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::nbt_compress::{read_gzip_compound_tag, write_gzip_compound_tag};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use tokio::sync::Mutex;
use tracing::warn;
use uuid::Uuid;

use crate::entity::player::Player;
use crate::world::World;
use pumpkin_data::entity::EntityType;

/// `WardenSpawnTracker.MAX_WARNING_LEVEL` (line 26).
pub const MAX_WARNING_LEVEL: i32 = 4;
/// `WardenSpawnTracker.PLAYER_SEARCH_RADIUS` (line 27).
const PLAYER_SEARCH_RADIUS: f64 = 16.0;
/// `WardenSpawnTracker.WARNING_CHECK_DIAMETER` (line 28).
const WARNING_CHECK_DIAMETER: f64 = 48.0;
/// `WardenSpawnTracker.DECREASE_WARNING_LEVEL_EVERY_INTERVAL` (line 29).
const DECREASE_WARNING_LEVEL_EVERY_INTERVAL: i64 = 12000;
/// `WardenSpawnTracker.WARNING_LEVEL_INCREASE_COOLDOWN` (line 30).
const WARNING_LEVEL_INCREASE_COOLDOWN: i32 = 200;

const FILE_NAME: &str = "warden_spawn_tracker.dat";
const TICKS_SINCE_LAST_WARNING: &str = "ticks_since_last_warning";
const WARNING_LEVEL: &str = "warning_level";
const COOLDOWN_TICKS: &str = "cooldown_ticks";
const ANCHOR: &str = "pumpkin_anchor_tick";

/// The three fields of `WardenSpawnTracker` (lines 31-33).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WardenSpawnTracker {
    ticks_since_last_warning: i32,
    warning_level: i32,
    cooldown_ticks: i32,
}

impl WardenSpawnTracker {
    #[must_use]
    pub const fn new(
        ticks_since_last_warning: i32,
        warning_level: i32,
        cooldown_ticks: i32,
    ) -> Self {
        Self {
            ticks_since_last_warning,
            warning_level,
            cooldown_ticks,
        }
    }

    /// `WardenSpawnTracker.tick` (lines 45-56).
    pub fn tick(&mut self) {
        if i64::from(self.ticks_since_last_warning) >= DECREASE_WARNING_LEVEL_EVERY_INTERVAL {
            self.decrease_warning_level();
            self.ticks_since_last_warning = 0;
        } else {
            self.ticks_since_last_warning += 1;
        }

        if self.cooldown_ticks > 0 {
            self.cooldown_ticks -= 1;
        }
    }

    /// Bulk form of [`Self::tick`] for `elapsed` ticks; see the module doc comment for why
    /// this replaces a per-tick call.
    ///
    /// The decay period is 12001 ticks, not 12000: the tick on which
    /// `ticks_since_last_warning` reaches the interval is spent on the decrement and the
    /// reset to zero rather than on another increment (lines 46-51). So over `E` ticks from
    /// `t`, with `X = t + E`, vanilla performs `X / 12001` decrements and leaves `X % 12001`
    /// behind. The leading `tick()` normalises a `t` that is already at or past the interval,
    /// which the closed form does not cover.
    pub fn advance(&mut self, elapsed: i64) {
        if elapsed <= 0 {
            return;
        }
        let mut remaining = elapsed;
        if i64::from(self.ticks_since_last_warning) >= DECREASE_WARNING_LEVEL_EVERY_INTERVAL {
            self.tick();
            remaining -= 1;
            if remaining <= 0 {
                return;
            }
        }

        let period = DECREASE_WARNING_LEVEL_EVERY_INTERVAL + 1;
        let total = i64::from(self.ticks_since_last_warning) + remaining;
        let decrements = i32::try_from(total / period).unwrap_or(i32::MAX);
        self.ticks_since_last_warning = (total % period) as i32;
        self.set_warning_level(self.warning_level.saturating_sub(decrements));
        let elapsed_ticks = i32::try_from(remaining).unwrap_or(i32::MAX);
        self.cooldown_ticks = self.cooldown_ticks.saturating_sub(elapsed_ticks).max(0);
    }

    /// `WardenSpawnTracker.reset` (lines 58-62).
    pub const fn reset(&mut self) {
        self.ticks_since_last_warning = 0;
        self.warning_level = 0;
        self.cooldown_ticks = 0;
    }

    /// `WardenSpawnTracker.onCooldown` (lines 91-93).
    #[must_use]
    pub const fn on_cooldown(&self) -> bool {
        self.cooldown_ticks > 0
    }

    /// `WardenSpawnTracker.increaseWarningLevel` (lines 105-111).
    fn increase_warning_level(&mut self) {
        if !self.on_cooldown() {
            self.ticks_since_last_warning = 0;
            self.cooldown_ticks = WARNING_LEVEL_INCREASE_COOLDOWN;
            self.set_warning_level(self.warning_level + 1);
        }
    }

    /// `WardenSpawnTracker.decreaseWarningLevel` (lines 113-115).
    fn decrease_warning_level(&mut self) {
        self.set_warning_level(self.warning_level - 1);
    }

    /// `WardenSpawnTracker.setWarningLevel` (lines 117-119): `Mth.clamp(level, 0, 4)`.
    pub fn set_warning_level(&mut self, warning_level: i32) {
        self.warning_level = warning_level.clamp(0, MAX_WARNING_LEVEL);
    }

    /// `WardenSpawnTracker.getWarningLevel` (lines 121-123).
    #[must_use]
    pub const fn warning_level(&self) -> i32 {
        self.warning_level
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int(TICKS_SINCE_LAST_WARNING, self.ticks_since_last_warning);
        nbt.put_int(WARNING_LEVEL, self.warning_level);
        nbt.put_int(COOLDOWN_TICKS, self.cooldown_ticks);
    }

    fn from_nbt(nbt: &NbtCompound) -> Self {
        // The codec uses NON_NEGATIVE_INT with a default of 0 for all three fields.
        Self {
            ticks_since_last_warning: nbt.get_int(TICKS_SINCE_LAST_WARNING).unwrap_or(0).max(0),
            warning_level: nbt
                .get_int(WARNING_LEVEL)
                .unwrap_or(0)
                .clamp(0, MAX_WARNING_LEVEL),
            cooldown_ticks: nbt.get_int(COOLDOWN_TICKS).unwrap_or(0).max(0),
        }
    }
}

#[derive(Clone, Copy)]
struct Entry {
    tracker: WardenSpawnTracker,
    /// `world_age` at which `tracker` was last brought up to date.
    anchor: i64,
}

#[derive(Default)]
struct Registry {
    entries: HashMap<Uuid, Entry>,
    path: Option<PathBuf>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));

/// Walks up from a dimension's `root_folder` to the directory holding `level.dat`, so every
/// dimension of one save resolves to the same tracker file (`Level::from_root_folder` hands
/// non-overworld dimensions a nested root).
fn world_root(level_root: &Path) -> PathBuf {
    let mut candidate = level_root;
    for _ in 0..4 {
        if candidate.join("level.dat").is_file() {
            return candidate.to_path_buf();
        }
        match candidate.parent() {
            Some(parent) => candidate = parent,
            None => break,
        }
    }
    level_root.to_path_buf()
}

impl Registry {
    fn ensure_loaded(&mut self, world: &World) {
        if self.path.is_some() {
            return;
        }
        let path = world_root(&world.level.level_folder.root_folder)
            .join("data")
            .join(FILE_NAME);
        self.path = Some(path.clone());

        let Ok(file) = std::fs::File::open(&path) else {
            return;
        };
        let Ok(root) = read_gzip_compound_tag(file) else {
            warn!(
                "Could not read {}; starting from empty trackers",
                path.display()
            );
            return;
        };
        for (key, tag) in &root.child_tags {
            let pumpkin_nbt::tag::NbtTag::Compound(compound) = tag else {
                continue;
            };
            let Ok(uuid) = Uuid::parse_str(key) else {
                continue;
            };
            self.entries.insert(
                uuid,
                Entry {
                    tracker: WardenSpawnTracker::from_nbt(compound),
                    anchor: compound.get_long(ANCHOR).unwrap_or(0),
                },
            );
        }
    }

    fn save(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        if let Some(parent) = path.parent()
            && std::fs::create_dir_all(parent).is_err()
        {
            return;
        }
        let mut root = NbtCompound::new();
        for (uuid, entry) in &self.entries {
            let mut compound = NbtCompound::new();
            entry.tracker.write_nbt(&mut compound);
            compound.put_long(ANCHOR, entry.anchor);
            root.put_compound(&uuid.to_string(), compound);
        }
        match std::fs::File::create(path) {
            Ok(file) => {
                if write_gzip_compound_tag(root, file).is_err() {
                    warn!("Could not write {}", path.display());
                }
            }
            Err(error) => warn!("Could not create {}: {error}", path.display()),
        }
    }

    /// Brings one player's tracker up to `now` and returns it.
    fn advanced(&mut self, uuid: Uuid, now: i64) -> WardenSpawnTracker {
        let entry = self.entries.entry(uuid).or_insert(Entry {
            tracker: WardenSpawnTracker::default(),
            anchor: now,
        });
        entry.tracker.advance(now.saturating_sub(entry.anchor));
        entry.anchor = now;
        entry.tracker
    }

    fn store(&mut self, uuid: Uuid, tracker: WardenSpawnTracker, now: i64) {
        self.entries.insert(
            uuid,
            Entry {
                tracker,
                anchor: now,
            },
        );
    }
}

/// `WardenSpawnTracker.hasNearbyWarden` (lines 95-98): any warden inside a 48-block cube
/// centred on `pos`.
fn has_nearby_warden(world: &Arc<World>, pos: &BlockPos) -> bool {
    let half = WARNING_CHECK_DIAMETER / 2.0;
    let center = Vector3::new(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y) + 0.5,
        f64::from(pos.0.z) + 0.5,
    );
    // The flat entity list is a sphere query; the corners are trimmed back to the cube below.
    let radius = half * 3.0f64.sqrt();
    world
        .get_nearby_entities(center, radius)
        .values()
        .any(|entity| {
            let base = entity.get_entity();
            if base.entity_type.id != EntityType::WARDEN.id {
                return false;
            }
            let entity_pos = base.pos.load();
            (entity_pos.x - center.x).abs() <= half
                && (entity_pos.y - center.y).abs() <= half
                && (entity_pos.z - center.z).abs() <= half
        })
}

/// `WardenSpawnTracker.getNearbyPlayers` (lines 100-103).
fn nearby_players(world: &Arc<World>, pos: &BlockPos) -> Vec<Arc<Player>> {
    let center = Vector3::new(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y) + 0.5,
        f64::from(pos.0.z) + 0.5,
    );
    world
        .get_nearby_players(center, PLAYER_SEARCH_RADIUS)
        .into_iter()
        .filter(|player| {
            !crate::entity::EntityBase::is_spectator(player.as_ref())
                && player.living_entity.entity.is_alive()
        })
        .collect()
}

/// `WardenSpawnTracker.tryWarn` (lines 64-89).
///
/// Returns the new shared warning level, or `None` when no warning was issued (a warden is
/// already nearby, or some nearby player is still inside the 200-tick cooldown).
pub async fn try_warn(world: &Arc<World>, pos: &BlockPos, trigger: &Arc<Player>) -> Option<i32> {
    if has_nearby_warden(world, pos) {
        return None;
    }

    let mut players = nearby_players(world, pos);
    if !players
        .iter()
        .any(|player| player.gameprofile.id == trigger.gameprofile.id)
    {
        players.push(trigger.clone());
    }

    let now = world.level_time.lock().await.world_age;
    let mut registry = REGISTRY.lock().await;
    registry.ensure_loaded(world);

    let mut trackers: Vec<(Uuid, WardenSpawnTracker)> = players
        .iter()
        .map(|player| {
            let uuid = player.gameprofile.id;
            (uuid, registry.advanced(uuid, now))
        })
        .collect();
    trackers.sort_unstable_by_key(|(uuid, _)| *uuid);
    trackers.dedup_by_key(|(uuid, _)| *uuid);

    if trackers.iter().any(|(_, tracker)| tracker.on_cooldown()) {
        registry.save();
        return None;
    }

    let mut highest = trackers
        .iter()
        .map(|(_, tracker)| *tracker)
        .max_by_key(WardenSpawnTracker::warning_level)?;
    highest.increase_warning_level();

    // `players.forEach(... copyData(spawnTracker))` (line 83).
    for (uuid, _) in &trackers {
        registry.store(*uuid, highest, now);
    }
    registry.save();

    Some(highest.warning_level())
}

/// Current warning level for one player, brought up to date. Exposed for tests and for a
/// future `/warden_spawn_tracker` command (`WardenSpawnTrackerCommand.java`).
pub async fn warning_level_of(world: &Arc<World>, uuid: Uuid) -> i32 {
    let now = world.level_time.lock().await.world_age;
    let mut registry = REGISTRY.lock().await;
    registry.ensure_loaded(world);
    registry.advanced(uuid, now).warning_level()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_matches_repeated_ticks() {
        let starts = [
            WardenSpawnTracker::new(5, 3, 200),
            WardenSpawnTracker::new(0, 4, 0),
            WardenSpawnTracker::new(11999, 2, 1),
            WardenSpawnTracker::new(12000, 2, 5),
        ];
        for start in starts {
            for elapsed in [
                0i64, 1, 2, 199, 200, 11999, 12000, 12001, 12002, 24002, 25000,
            ] {
                let mut ticked = start;
                for _ in 0..elapsed {
                    ticked.tick();
                }
                let mut advanced = start;
                advanced.advance(elapsed);
                assert_eq!(ticked, advanced, "start = {start:?}, elapsed = {elapsed}");
            }
        }
    }

    #[test]
    fn increase_is_gated_by_cooldown_and_clamped() {
        let mut tracker = WardenSpawnTracker::default();
        tracker.increase_warning_level();
        assert_eq!(tracker.warning_level(), 1);
        assert!(tracker.on_cooldown());

        // A second warning inside the 200-tick cooldown does nothing.
        tracker.increase_warning_level();
        assert_eq!(tracker.warning_level(), 1);

        for _ in 0..4 {
            tracker.advance(i64::from(WARNING_LEVEL_INCREASE_COOLDOWN));
            tracker.increase_warning_level();
        }
        assert_eq!(tracker.warning_level(), MAX_WARNING_LEVEL);
    }

    #[test]
    fn nbt_round_trip_uses_vanilla_keys() {
        let tracker = WardenSpawnTracker::new(7, 2, 42);
        let mut nbt = NbtCompound::new();
        tracker.write_nbt(&mut nbt);
        assert_eq!(nbt.get_int("ticks_since_last_warning"), Some(7));
        assert_eq!(nbt.get_int("warning_level"), Some(2));
        assert_eq!(nbt.get_int("cooldown_ticks"), Some(42));
        assert_eq!(WardenSpawnTracker::from_nbt(&nbt), tracker);
    }
}
