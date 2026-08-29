//! Raid/Raider wave system.
// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! Ports `net.minecraft.world.entity.raid.{Raids,Raid,Raider}` (vanilla 26.2 decompile).
//!
//! Trigger path: `BadOmenMobEffect`/`RaidOmenMobEffect` (`crate::entity::living::LivingEntity`
//! `should_apply_effect_tick`/`apply_effect_tick`, `BAD_OMEN`/`RAID_OMEN` branches) convert
//! `BAD_OMEN` into `RAID_OMEN` on entering a real village (`World::is_close_to_village`, backed
//! by `world::village_poi`'s POI-density tracker) and, on `RAID_OMEN` expiry, call
//! `RaidManager::create_or_extend_raid` - which computes the raid center from real occupied
//! `village`-tag POIs, mirroring `Raids.createOrExtendRaid`.
//!
//! Scope reductions (no vanilla-parity infrastructure exists yet for these; see
//! `pumpkin/src/world/mod.rs` `RaidManager` field and `CLAUDE.md`/`PARITY.md`):
//! - The raid center never re-centers onto a real village once triggered
//!   (`moveRaidCenterToNearbyVillageSection` is not ported), and a raid is never lost for
//!   "no longer a village" (`Raid.java` lines 287-297).
//! - `village_poi`'s job-site/meeting-point POIs are never claimed by anything in Pumpkin (see
//!   that module's docs), so in practice only claimed beds (`HOME`) register as "occupied" -
//!   village detection is real but currently under-populated relative to vanilla.
//! - `EnvironmentAttributes.CAN_START_RAID` (`Raids.createOrExtendRaid`) is not checked - no
//!   such attribute system exists in Pumpkin.
//! - `ServerPlayer.raidOmenPosition` (`Player::raid_omen_position`) is not persisted to NBT;
//!   a relog while `RAID_OMEN` is active drops the pending raid trigger instead of resuming it.
//! - `Raid.findRandomSpawnPos` drops the `isVillage`/chunk-loaded/`SpawnPlacements` legality
//!   checks and only checks the vertical-distance bound (`Raid.java` line 675).
//! - No banner-pattern rendering: the wave leader is equipped with a plain white banner
//!   instead of the ominous banner's exact pattern layers (`Raid.getBannerComponentPatch`).
//! - `ObtainRaidLeaderBannerGoal`/`RaiderMoveThroughVillageGoal`/`RaiderCelebration` AI goals
//!   (`Raider.java`) are not ported: raiders fight using their existing hostile-mob goals only.
//! - Advancement/statistics hooks (`CriteriaTriggers.RAID_OMEN`/`RAID_WIN`, `Stats.RAID_TRIGGER`/
//!   `RAID_WIN`) are not ported.
//! - `Raid.getEnchantOdds` (bonus-enchant chance on raider gear by omen level) is not ported;
//!   nothing in Pumpkin currently varies raider loot by raid state.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rand::RngExt;
use uuid::Uuid;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_util::difficulty::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;

use crate::entity::EntityBase;
use crate::entity::living::RaidMembership;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::raider::create_ominous_banner;
use crate::entity::r#type::from_type;
use crate::world::World;
use crate::world::bossbar::{Bossbar, BossbarColor, BossbarDivisions};
use crate::world::natural_spawner::{is_spawn_position_ok, is_valid_empty_spawn_block};

const RAID_TIMEOUT_TICKS: i64 = 48000;
const NUM_SPAWN_ATTEMPTS: i32 = 5;
const DEFAULT_PRE_RAID_TICKS: i32 = 300;
const MAX_CELEBRATION_TICKS: i32 = 600;
pub(crate) const DEFAULT_MAX_RAID_OMEN_LEVEL: i32 = 5;
const HERO_OF_THE_VILLAGE_DURATION: i32 = 48000;
const POST_RAID_TICK_LIMIT: i32 = 40;
const VALID_RAID_RADIUS_SQR: f64 = 9216.0;
const RAID_REMOVAL_THRESHOLD_SQR: f64 = 12544.0;
const PATROL_SPAWN_INTERVAL: i32 = 12000;
const PATROL_SPAWN_INTERVAL_JITTER: i32 = 1200;
const BRIGHT_OUTSIDE_SKY_DARKEN: u8 = 4;
const PATROL_MAX_BLOCK_LIGHT: u8 = 8;
const PATROL_REQUIRED_LOADED_DELTA: i32 = 10;
const VINDICATOR_SPAWNS: [i32; 8] = [0, 0, 2, 0, 1, 4, 2, 5];
const EVOKER_SPAWNS: [i32; 8] = [0, 0, 0, 0, 0, 1, 1, 2];
const PILLAGER_SPAWNS: [i32; 8] = [0, 4, 3, 3, 4, 4, 4, 2];
const WITCH_SPAWNS: [i32; 8] = [0, 0, 0, 0, 3, 0, 0, 1];
const RAVAGER_SPAWNS: [i32; 8] = [0, 0, 0, 1, 0, 1, 0, 2];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaidStatus {
    Ongoing,
    Victory,
    Loss,
    Stopped,
}

impl RaidStatus {
    #[must_use]
    pub const fn get_serialized_name(self) -> &'static str {
        match self {
            Self::Ongoing => "ongoing",
            Self::Victory => "victory",
            Self::Loss => "loss",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaiderType {
    Vindicator,
    Evoker,
    Pillager,
    Witch,
    Ravager,
}

impl RaiderType {
    pub const VALUES: [Self; 5] = [
        Self::Vindicator,
        Self::Evoker,
        Self::Pillager,
        Self::Witch,
        Self::Ravager,
    ];

    #[must_use]
    pub const fn entity_type(self) -> &'static EntityType {
        match self {
            Self::Vindicator => &EntityType::VINDICATOR,
            Self::Evoker => &EntityType::EVOKER,
            Self::Pillager => &EntityType::PILLAGER,
            Self::Witch => &EntityType::WITCH,
            Self::Ravager => &EntityType::RAVAGER,
        }
    }

    #[must_use]
    pub const fn spawns_per_wave(self) -> &'static [i32; 8] {
        match self {
            Self::Vindicator => &[0, 0, 2, 0, 1, 4, 2, 5],
            Self::Evoker => &[0, 0, 0, 0, 0, 1, 1, 2],
            Self::Pillager => &[0, 4, 3, 3, 4, 4, 4, 2],
            Self::Witch => &[0, 0, 0, 0, 3, 0, 0, 1],
            Self::Ravager => &[0, 0, 0, 1, 0, 1, 0, 2],
        }
    }
}

/// Raid.java:786-793.
pub(crate) const fn num_groups_for_difficulty(difficulty: Difficulty) -> i32 {
    Raid::get_num_groups(difficulty)
}

/// Raid.java:747-780.
fn potential_bonus_spawns(
    raider_type: RaiderType,
    wave: i32,
    difficulty: Difficulty,
    is_bonus_wave: bool,
) -> i32 {
    let is_easy = difficulty == Difficulty::Easy;
    let is_normal = difficulty == Difficulty::Normal;
    let mut rng = rand::rng();
    let bonus_spawns = match raider_type {
        RaiderType::Vindicator | RaiderType::Pillager => {
            if is_easy { rng.random_range(0..2) } else if is_normal { 1 } else { 2 }
        }
        RaiderType::Evoker => return 0,
        RaiderType::Witch => {
            if is_easy || wave <= 2 || wave == 4 { return 0; }
            1
        }
        RaiderType::Ravager => i32::from(!is_easy && is_bonus_wave),
    };
    if bonus_spawns > 0 { rng.random_range(0..bonus_spawns + 1) } else { 0 }
}

pub struct Raid {
    pub id: i32,
    pub center: BlockPos,
    pub status: RaidStatus,
    pub active: bool,
    pub started: bool,
    pub ticks_active: u64,
    pub raid_omen_level: i32,
    pub groups_spawned: i32,
    pub num_groups: i32,
    pub raid_cooldown_ticks: i32,
    pub post_raid_ticks: i32,
    pub celebration_ticks: u32,
    pub total_health: f32,
    pub bossbar: Bossbar,
    pub group_to_leader_map: HashMap<i32, Uuid>,
    pub group_raider_map: HashMap<i32, HashSet<Uuid>>,
    pub heroes_of_the_village: HashSet<Uuid>,
    pub players_in_raid: HashSet<Uuid>,
    pub wave_spawn_pos: Option<BlockPos>,
}

impl Raid {
    pub const VALID_RAID_RADIUS: f64 = 96.0;
    pub const VALID_RAID_RADIUS_SQR: f64 = 9216.0;
    pub const RAID_REMOVAL_THRESHOLD_SQR: f64 = 12544.0;
    pub const MAX_RAID_OMEN_LEVEL: i32 = 5;

    #[must_use]
    pub fn new(id: i32, center: BlockPos, difficulty: Difficulty) -> Self {
        let mut bossbar = Bossbar::new(TextComponent::translate("event.minecraft.raid", []));
        bossbar.color = BossbarColor::Red;
        bossbar.division = BossbarDivisions::Notches10;
        bossbar.health = 0.0;

        let num_groups = Self::get_num_groups(difficulty);

        Self {
            id,
            center,
            status: RaidStatus::Ongoing,
            active: true,
            started: false,
            ticks_active: 0,
            raid_omen_level: 0,
            groups_spawned: 0,
            num_groups,
            raid_cooldown_ticks: 300,
            post_raid_ticks: 0,
            celebration_ticks: 0,
            total_health: 0.0,
            bossbar,
            group_to_leader_map: HashMap::new(),
            group_raider_map: HashMap::new(),
            heroes_of_the_village: HashSet::new(),
            players_in_raid: HashSet::new(),
            wave_spawn_pos: None,
        }
    }

    #[must_use]
    pub const fn get_num_groups(difficulty: Difficulty) -> i32 {
        match difficulty {
            Difficulty::Peaceful => 0,
            Difficulty::Easy => 3,
            Difficulty::Normal => 5,
            Difficulty::Hard => 7,
        }
    }

    #[must_use]
    pub const fn is_over(&self) -> bool {
        self.is_victory() || self.is_loss()
    }

    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        matches!(self.status, RaidStatus::Stopped)
    }

    #[must_use]
    pub const fn is_victory(&self) -> bool {
        matches!(self.status, RaidStatus::Victory)
    }

    #[must_use]
    pub const fn is_loss(&self) -> bool {
        matches!(self.status, RaidStatus::Loss)
    }

    #[must_use]
    pub const fn is_started(&self) -> bool {
        self.started
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    #[must_use]
    pub const fn get_groups_spawned(&self) -> i32 {
        self.groups_spawned
    }

    /// `Raid.getGroupsSpawned` (`Raid.java:207`).
    #[must_use]
    pub(crate) const fn groups_spawned(&self) -> i32 {
        self.groups_spawned
    }

    #[must_use]
    pub const fn get_raid_omen_level(&self) -> i32 {
        self.raid_omen_level
    }

    pub fn set_raid_omen_level(&mut self, level: i32) {
        self.raid_omen_level = level.clamp(0, Self::MAX_RAID_OMEN_LEVEL);
    }

    /// `Raid.isBetweenWaves` (`Raid.java:169-171`).
    #[must_use]
    pub fn is_between_waves(&self) -> bool {
        self.groups_spawned > 0 && self.get_total_raiders_alive() == 0 && self.raid_cooldown_ticks > 0
    }

    #[must_use]
    pub const fn has_first_wave_spawned(&self) -> bool {
        self.groups_spawned > 0
    }

    #[must_use]
    pub const fn center(&self) -> BlockPos {
        self.center
    }

    #[must_use]
    pub const fn raid_omen_level(&self) -> i32 {
        self.raid_omen_level
    }

    /// Vanilla `Raid.getEnchantOdds` (`Raid.java:795-806`) feeds the chance used by
    /// `Vindicator.applyRaidBuffs` (`Vindicator.java:171`).
    #[must_use]
    pub const fn enchant_odds(&self) -> f32 {
        match self.raid_omen_level {
            2 => 0.1,
            3 => 0.25,
            4 => 0.5,
            5 => 0.75,
            _ => 0.0,
        }
    }

    #[must_use]
    pub const fn has_more_waves(&self) -> bool {
        if self.has_bonus_wave() {
            !self.has_spawned_bonus_wave()
        } else {
            !self.is_final_wave()
        }
    }

    #[must_use]
    pub const fn is_final_wave(&self) -> bool {
        self.groups_spawned == self.num_groups
    }

    #[must_use]
    pub const fn has_bonus_wave(&self) -> bool {
        self.raid_omen_level > 1
    }

    #[must_use]
    pub const fn has_spawned_bonus_wave(&self) -> bool {
        self.groups_spawned > self.num_groups
    }

    #[must_use]
    pub fn should_spawn_bonus_group(&self) -> bool {
        self.is_final_wave() && self.get_total_raiders_alive() == 0 && self.has_bonus_wave()
    }

    #[must_use]
    pub fn should_spawn_group(&self) -> bool {
        self.raid_cooldown_ticks == 0
            && (self.groups_spawned < self.num_groups || self.should_spawn_bonus_group())
            && self.get_total_raiders_alive() == 0
    }

    #[must_use]
    pub fn get_total_raiders_alive(&self) -> usize {
        self.group_raider_map.values().map(HashSet::len).sum()
    }

    #[must_use]
    pub fn get_all_raiders(&self) -> HashSet<Uuid> {
        let mut set = HashSet::new();
        for wave_set in self.group_raider_map.values() {
            set.extend(wave_set);
        }
        set
    }

    pub fn stop(&mut self, world: &World) {
        self.active = false;
        self.status = RaidStatus::Stopped;
        self.remove_all_players(world);
    }

    pub fn remove_all_players(&mut self, world: &World) {
        let players = world.players.load();
        for player in players.iter() {
            if self.players_in_raid.contains(&player.gameprofile.id) {
                player.remove_bossbar(self.bossbar.uuid);
            }
        }
        self.players_in_raid.clear();
    }

    pub fn absorb_raid_omen(&mut self) -> bool {
        self.raid_omen_level = (self.raid_omen_level + 1).clamp(1, Self::MAX_RAID_OMEN_LEVEL);
        true
    }

    pub fn add_hero_of_the_village(&mut self, killer_uuid: Uuid) {
        self.heroes_of_the_village.insert(killer_uuid);
    }

    const fn get_default_num_spawns(
        &self,
        raider_type: RaiderType,
        wave: i32,
        is_bonus_wave: bool,
    ) -> i32 {
        let spawns = raider_type.spawns_per_wave();
        if is_bonus_wave {
            spawns[self.num_groups as usize]
        } else {
            spawns[wave as usize]
        }
    }

    fn get_potential_bonus_spawns(
        raider_type: RaiderType,
        wave: i32,
        difficulty: Difficulty,
        is_bonus_wave: bool,
    ) -> i32 {
        let is_easy = difficulty == Difficulty::Easy;
        let is_normal = difficulty == Difficulty::Normal;
        let bonus_spawns = match raider_type {
            RaiderType::Vindicator | RaiderType::Pillager => {
                if is_easy {
                    rand::random::<u32>() % 2
                } else if is_normal {
                    1
                } else {
                    2
                }
            }
            RaiderType::Evoker => 0,
            RaiderType::Witch => u32::from(!(is_easy || wave <= 2 || wave == 4)),
            RaiderType::Ravager => u32::from(!is_easy && is_bonus_wave),
        };
        if bonus_spawns > 0 {
            (rand::random::<u32>() % (bonus_spawns + 1)) as i32
        } else {
            0
        }
    }

    pub fn spawn_group(&mut self, world: &Arc<World>, pos: BlockPos) {
        let mut leader_set = false;
        let group_number = self.groups_spawned + 1;
        self.total_health = 0.0;
        let is_bonus_group = self.should_spawn_bonus_group();

        let difficulty = world.level_info.load().difficulty;

        for raider_type in RaiderType::VALUES {
            let num_spawns = self.get_default_num_spawns(raider_type, group_number, is_bonus_group)
                + Self::get_potential_bonus_spawns(
                    raider_type,
                    group_number,
                    difficulty,
                    is_bonus_group,
                );
            let mut ravagers_spawned = 0;

            for _ in 0..num_spawns {
                let uuid = Uuid::new_v4();
                let spawn_pos = Vector3::new(
                    f64::from(pos.0.x) + 0.5,
                    f64::from(pos.0.y) + 1.0,
                    f64::from(pos.0.z) + 0.5,
                );
                let entity_base = from_type(raider_type.entity_type(), spawn_pos, world, uuid);

                let mut is_patrol_leader = false;
                if let Some(mob) = entity_base.get_mob()
                    && let Some(raider) = mob.as_raider()
                {
                    if !leader_set && raider.can_be_leader() {
                        is_patrol_leader = true;
                        raider.set_patrol_leader(true);
                        let banner = create_ominous_banner();
                        let living = &mob.get_mob_entity().living_entity;
                        if let Ok(mut equipment) = living.entity_equipment.try_lock() {
                            equipment.put(&EquipmentSlot::HEAD, banner.clone());
                        }
                        living.send_equipment_changes(&[(EquipmentSlot::HEAD, banner)]);
                        self.group_to_leader_map.insert(group_number, uuid);
                        leader_set = true;
                    }
                    raider.set_wave(group_number);
                    raider.set_can_join_raid(true);
                    raider.apply_raid_buffs(group_number, false);
                }

                if let Some(mob) = entity_base.get_mob() {
                    self.join_raid(
                        group_number,
                        uuid,
                        mob.get_mob_entity(),
                        is_patrol_leader,
                    );
                }
                world.spawn_entity_non_save(entity_base.clone());

                if *raider_type.entity_type() == EntityType::RAVAGER {
                    let mut riding_type: Option<&'static EntityType> = None;
                    if group_number == Self::get_num_groups(Difficulty::Normal) {
                        riding_type = Some(&EntityType::PILLAGER);
                    } else if group_number >= Self::get_num_groups(Difficulty::Hard) {
                        if ravagers_spawned == 0 {
                            riding_type = Some(&EntityType::EVOKER);
                        } else {
                            riding_type = Some(&EntityType::VINDICATOR);
                        }
                    }
                    ravagers_spawned += 1;

                    if let Some(riding_type) = riding_type {
                        let rider_uuid = Uuid::new_v4();
                        let rider_base = from_type(riding_type, spawn_pos, world, rider_uuid);
                        if let Some(mob) = rider_base.get_mob()
                            && let Some(raider) = mob.as_raider()
                        {
                            raider.set_wave(group_number);
                            raider.set_can_join_raid(true);
                            raider.apply_raid_buffs(group_number, false);
                        }
                        if let Some(mob) = rider_base.get_mob() {
                            self.join_raid(
                                group_number,
                                rider_uuid,
                                mob.get_mob_entity(),
                                false,
                            );
                        }
                        world.spawn_entity_non_save(rider_base);
                    }
                }
            }
        }

        self.wave_spawn_pos = None;
        self.groups_spawned += 1;
        self.update_bossbar(world);
    }

    pub(crate) fn join_raid(
        &mut self,
        wave: i32,
        uuid: Uuid,
        mob_entity: &crate::entity::mob::MobEntity,
        is_patrol_leader: bool,
    ) {
        self.group_raider_map.entry(wave).or_default().insert(uuid);

        let living = &mob_entity.living_entity;
        self.total_health += living.health.load();
        living.can_join_raid.store(true, std::sync::atomic::Ordering::Relaxed);
        living.raid_membership.store(Some(RaidMembership {
            raid_id: self.id,
            wave,
            is_patrol_leader,
        }));
    }

    /// Raid.java:613-627.
    pub fn remove_from_raid(
        &mut self,
        wave: i32,
        uuid: Uuid,
        health: f32,
        remove_from_total_health: bool,
    ) {
        if let Some(raiders) = self.group_raider_map.get_mut(&wave)
            && raiders.remove(&uuid)
            && remove_from_total_health
        {
            self.total_health -= health;
        }
    }

    #[must_use]
    pub fn find_random_spawn_pos(&self, world: &Arc<World>, max_tries: usize) -> Option<BlockPos> {
        let seconds_remaining = self.raid_cooldown_ticks / 20;
        let how_far = 0.22 * seconds_remaining as f32 - 0.24;
        let mut rng = rand::rng();
        let start_angle = rng.random::<f32>() * std::f32::consts::TAU;

        for i in 0..max_tries {
            let angle = start_angle + std::f32::consts::PI * i as f32 / 8.0;
            let jitter = rng.random_range(0..3) as f32 * how_far.floor();
            let spawn_x = self.center.0.x
                + (angle.cos() * 32.0 * how_far).floor() as i32
                + jitter.floor() as i32;
            let spawn_z = self.center.0.z
                + (angle.sin() * 32.0 * how_far).floor() as i32
                + jitter.floor() as i32;
            let spawn_y = world.get_top_block(pumpkin_util::math::vector2::Vector2::new(spawn_x, spawn_z));

            if (spawn_y - self.center.0.y).abs() <= 96 {
                let spawn_pos = BlockPos(Vector3::new(spawn_x, spawn_y, spawn_z));
                return Some(spawn_pos);
            }
        }
        None
    }

    pub fn play_sound(&self, world: &World, pos: BlockPos) {
        let sound_pos = pos.to_centered_f64();
        world.play_sound_fine(
            Sound::EventRaidHorn,
            pumpkin_data::sound::SoundCategory::Neutral,
            &sound_pos,
            64.0,
            1.0,
        );
    }

    pub fn update_players(&mut self, world: &World) {
        let center_f64 = self.center.to_f64();
        let players = world.players.load();
        let nearby_players: HashSet<Uuid> = players
            .iter()
            .filter(|player| {
                let pos = player.get_entity().pos.load();
                pos.squared_distance_to_vec(&center_f64) <= Self::VALID_RAID_RADIUS_SQR
            })
            .map(|player| player.gameprofile.id)
            .collect();

        for player in players.iter() {
            let id = player.gameprofile.id;
            let was_in = self.players_in_raid.contains(&id);
            let is_in = nearby_players.contains(&id);

            if !was_in && is_in {
                player.send_bossbar(&self.bossbar);
                self.players_in_raid.insert(id);
            } else if was_in && !is_in {
                player.remove_bossbar(self.bossbar.uuid);
                self.players_in_raid.remove(&id);
            }
        }
    }

    /// Raid.java:466-499, including removal of dead or stray raiders.
    pub fn update_living_raiders(&mut self, world: &World) {
        struct Stale {
            wave: i32,
            uuid: Uuid,
            health: f32,
        }
        let mut stale = Vec::new();
        for (&wave, raiders) in &self.group_raider_map {
            for &uuid in raiders {
                match world.get_entity_by_uuid(uuid) {
                    None => stale.push(Stale { wave, uuid, health: 0.0 }),
                    Some(entity) => {
                        let too_far = self.center.squared_distance(&BlockPos::floored_v(
                            entity.get_entity().pos.load(),
                        )) as f64 >= RAID_REMOVAL_THRESHOLD_SQR;
                        if !entity.get_entity().is_alive() || too_far {
                            let health = entity.get_living_entity().map_or(0.0, |l| l.health.load());
                            stale.push(Stale { wave, uuid, health });
                        }
                    }
                }
            }
        }
        for stale in stale {
            if let Some(raiders) = self.group_raider_map.get_mut(&stale.wave)
                && raiders.remove(&stale.uuid)
            {
                self.total_health -= stale.health;
            }
            if self.group_to_leader_map.get(&stale.wave) == Some(&stale.uuid) {
                self.group_to_leader_map.remove(&stale.wave);
            }
            if let Some(entity) = world.get_entity_by_uuid(stale.uuid)
                && let Some(living) = entity.get_living_entity()
                && living.raid_membership.load().is_some_and(|m| m.raid_id == self.id)
            {
                living.raid_membership.store(None);
            }
        }
    }

    pub fn update_bossbar(&mut self, world: &World) {
        let living_health = self.get_health_of_living_raiders(world);
        let progress = if self.total_health > 0.0 {
            (living_health / self.total_health).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.bossbar.health = progress;

        let players = world.players.load();
        for player in players.iter() {
            if self.players_in_raid.contains(&player.gameprofile.id) {
                player.update_bossbar_health(&self.bossbar.uuid, self.bossbar.health);
                player.update_bossbar_title(&self.bossbar.uuid, self.bossbar.title.clone());
            }
        }
    }

    #[must_use]
    pub fn get_health_of_living_raiders(&self, world: &World) -> f32 {
        let mut health = 0.0;
        for raiders in self.group_raider_map.values() {
            for &uuid in raiders {
                if let Some(e) = world.get_entity_by_uuid(uuid)
                    && let Some(living) = e.get_living_entity()
                {
                    health += living.health.load();
                }
            }
        }
        health
    }

    pub fn tick(&mut self, world: &Arc<World>) {
        if self.is_stopped() {
            return;
        }

        if self.status == RaidStatus::Ongoing {
            if world.level_info.load().difficulty == Difficulty::Peaceful {
                self.stop(world);
                return;
            }

            self.ticks_active += 1;
            if self.ticks_active >= RAID_TIMEOUT_TICKS as u64 {
                self.stop(world);
                return;
            }

            let raiders_alive = self.get_total_raiders_alive();
            if raiders_alive == 0 && self.has_more_waves() {
                if self.raid_cooldown_ticks <= 0 {
                    if self.groups_spawned > 0 {
                        self.raid_cooldown_ticks = 300;
                        self.bossbar.title = TextComponent::translate("event.minecraft.raid", []);
                    }
                } else {
                    if self.wave_spawn_pos.is_none() && self.raid_cooldown_ticks % 5 == 0 {
                        self.wave_spawn_pos = self.find_random_spawn_pos(world, 8);
                    }

                    if self.raid_cooldown_ticks == 300 || self.raid_cooldown_ticks % 20 == 0 {
                        self.update_players(world);
                    }

                    self.raid_cooldown_ticks -= 1;
                    self.bossbar.health =
                        ((300 - self.raid_cooldown_ticks) as f32 / 300.0).clamp(0.0, 1.0);
                }
            }

            if self.ticks_active.is_multiple_of(20) {
                self.update_players(world);
                self.update_living_raiders(world);
                let alive = self.get_total_raiders_alive();
                if alive > 0 && alive <= 2 {
                    self.bossbar.title = TextComponent::translate(
                        "event.minecraft.raid.raiders_remaining",
                        [TextComponent::text(alive.to_string())],
                    );
                } else {
                    self.bossbar.title = TextComponent::translate("event.minecraft.raid", []);
                }
                self.update_bossbar(world);
            }

            let mut attempts = 0;
            while self.should_spawn_group() {
                let Some(spawn_pos) = self
                    .wave_spawn_pos
                    .or_else(|| self.find_random_spawn_pos(world, 20))
                else {
                    attempts += 1;
                    if attempts > NUM_SPAWN_ATTEMPTS {
                        self.stop(world);
                        break;
                    }
                    continue;
                };

                self.started = true;
                self.spawn_group(world, spawn_pos);
                self.play_sound(world, spawn_pos);
            }

            let raiders_alive = self.get_total_raiders_alive();
            if self.is_started() && !self.has_more_waves() && raiders_alive == 0 {
                if self.post_raid_ticks < POST_RAID_TICK_LIMIT {
                    self.post_raid_ticks += 1;
                } else {
                    self.status = RaidStatus::Victory;
                    let effect = Effect {
                        effect_type: &StatusEffect::HERO_OF_THE_VILLAGE,
                        duration: HERO_OF_THE_VILLAGE_DURATION,
                        amplifier: (self.raid_omen_level - 1).max(0) as u8,
                        ambient: false,
                        show_particles: true,
                        show_icon: true,
                        blend: true,
                    };
                    let players = world.players.load();
                    for player in players.iter() {
                        if self.heroes_of_the_village.contains(&player.gameprofile.id) {
                            player.add_effect(effect.clone());
                        }
                    }
                }
            } else if raiders_alive == 0
                && self.has_first_wave_spawned()
                && !self.has_more_waves()
                && self.post_raid_ticks == 0
            {
                self.status = RaidStatus::Loss;
            }
        } else if self.is_over() {
            self.celebration_ticks += 1;
            if self.celebration_ticks >= MAX_CELEBRATION_TICKS as u32 {
                self.stop(world);
                return;
            }

            if self.is_loss() && self.celebration_ticks == 1 {
                self.bossbar.title = TextComponent::translate("event.minecraft.raid.defeat", []);
            }

            if self.celebration_ticks.is_multiple_of(20) {
                self.update_players(world);
                if self.is_victory() {
                    self.bossbar.health = 0.0;
                    self.bossbar.title =
                        TextComponent::translate("event.minecraft.raid.victory.full", []);
                } else {
                    self.bossbar.title =
                        TextComponent::translate("event.minecraft.raid.defeat.full", []);
                }
                self.update_bossbar(world);
            }
        }
    }
}

#[derive(Default)]
pub struct PatrolSpawner {
    next_tick: i32,
}

impl PatrolSpawner {
    fn is_bright_outside(world: &Arc<World>) -> bool {
        world.dimension.has_skylight
            && world.sky_darken.load(std::sync::atomic::Ordering::Relaxed) < BRIGHT_OUTSIDE_SKY_DARKEN
    }

    fn has_chunks_at(world: &Arc<World>, pos: BlockPos) -> bool {
        let min_x = (pos.0.x - PATROL_REQUIRED_LOADED_DELTA) >> 4;
        let max_x = (pos.0.x + PATROL_REQUIRED_LOADED_DELTA) >> 4;
        let min_z = (pos.0.z - PATROL_REQUIRED_LOADED_DELTA) >> 4;
        let max_z = (pos.0.z + PATROL_REQUIRED_LOADED_DELTA) >> 4;
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                if world.level.loaded_chunks.get(&pumpkin_util::math::vector2::Vector2::new(x, z)).is_none() {
                    return false;
                }
            }
        }
        true
    }

    fn spawn_patrol_member(world: &Arc<World>, pos: BlockPos, is_leader: bool) -> bool {
        if !is_valid_empty_spawn_block(world.get_block_state(&pos), &EntityType::PILLAGER)
            || world.get_block_light_level(&pos).unwrap_or(0) > PATROL_MAX_BLOCK_LIGHT
            || !is_spawn_position_ok(world, &pos, &EntityType::PILLAGER)
        {
            return false;
        }
        let spawn_pos = Vector3::new(f64::from(pos.0.x) + 0.5, f64::from(pos.0.y), f64::from(pos.0.z) + 0.5);
        let pillager = crate::entity::r#type::from_type(&EntityType::PILLAGER, spawn_pos, world, Uuid::new_v4());
        if is_leader && let Some(mob) = pillager.get_mob() {
            let banner = create_ominous_banner();
            let living = &mob.get_mob_entity().living_entity;
            if let Ok(mut equipment) = living.entity_equipment.try_lock() {
                equipment.put(&EquipmentSlot::HEAD, banner.clone());
            }
            living.send_equipment_changes(&[(EquipmentSlot::HEAD, banner)]);
        }
        world.spawn_entity_non_save(pillager);
        true
    }

    fn tick(&mut self, world: &Arc<World>) {
        let level_info = world.level_info.load();
        let game_rules = &level_info.game_rules;
        if !(game_rules.spawn_mobs && game_rules.spawn_monsters && level_info.difficulty != Difficulty::Peaceful)
            || !game_rules.spawn_patrols
        {
            return;
        }
        self.next_tick -= 1;
        if self.next_tick > 0 {
            return;
        }
        self.next_tick += PATROL_SPAWN_INTERVAL + rand::random_range(0..PATROL_SPAWN_INTERVAL_JITTER);
        if !Self::is_bright_outside(world) || rand::random_range(0..5) != 0 {
            return;
        }
        let players = world.players.load();
        if players.is_empty() {
            return;
        }
        let player = players[rand::random_range(0..players.len())].clone();
        drop(players);
        if player.gamemode.load() == pumpkin_util::GameMode::Spectator {
            return;
        }
        let player_pos = player.get_entity().block_pos.load();
        if world.is_close_to_village(player_pos, 2) {
            return;
        }
        let offset = || (24 + rand::random_range(0..24)) * if rand::random_range(0..2) == 0 { -1 } else { 1 };
        let mut spawn_pos = player_pos.add(offset(), 0, offset());
        if !Self::has_chunks_at(world, spawn_pos) {
            return;
        }
        let difficulty = RegionalDifficulty::at(world, spawn_pos.to_centered_f64());
        let group_size = difficulty.effective_difficulty.ceil() as i32 + 1;
        for i in 0..group_size {
            spawn_pos = BlockPos::new(
                spawn_pos.0.x,
                world.get_top_block(pumpkin_util::math::vector2::Vector2::new(spawn_pos.0.x, spawn_pos.0.z)) + 1,
                spawn_pos.0.z,
            );
            if i == 0 && !Self::spawn_patrol_member(world, spawn_pos, true) {
                break;
            }
            if i > 0 {
                Self::spawn_patrol_member(world, spawn_pos, false);
            }
            spawn_pos = spawn_pos.add(
                rand::random_range(0..5) - rand::random_range(0..5),
                0,
                rand::random_range(0..5) - rand::random_range(0..5),
            );
        }
    }
}

#[derive(Default)]
pub struct Raids {
    pub raid_map: HashMap<i32, Raid>,
    pub next_id: i32,
    pub tick_counter: u64,
    patrol_spawner: PatrolSpawner,
}

impl Raids {
    #[must_use]
    pub fn get(&self, raid_id: i32) -> Option<&Raid> {
        self.raid_map.get(&raid_id)
    }

    pub fn get_mut(&mut self, raid_id: i32) -> Option<&mut Raid> {
        self.raid_map.get_mut(&raid_id)
    }

    pub fn raid_mut(&mut self, raid_id: i32) -> Option<&mut Raid> {
        self.get_mut(raid_id)
    }

    #[must_use]
    pub fn raid(&self, raid_id: i32) -> Option<&Raid> {
        self.get(raid_id)
    }

    /// `VillagerGoalPackages.getPreRaidPackage` (`VillagerGoalPackages.java:231-245`).
    #[must_use]
    pub fn is_pre_raid_at(&self, pos: BlockPos) -> bool {
        self.raid_map.values().filter(|raid| raid.is_active()).any(|raid| {
            (raid.center.squared_distance(&pos) as f64) < VALID_RAID_RADIUS_SQR
                && (!raid.has_first_wave_spawned() || raid.is_between_waves())
        })
    }

    fn find_active_raid_near(&self, pos: BlockPos) -> Option<i32> {
        self.raid_map
            .iter()
            .filter(|(_, raid)| raid.is_active())
            .map(|(&id, raid)| (id, raid.center.squared_distance(&pos) as f64))
            .filter(|&(_, distance)| distance < VALID_RAID_RADIUS_SQR)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    #[must_use]
    pub fn raid_omen_level_near(&self, pos: BlockPos) -> Option<i32> {
        self.find_active_raid_near(pos)
            .map(|id| self.raid_map[&id].raid_omen_level())
    }

    #[must_use]
    pub fn get_raid_at(&self, pos: &BlockPos) -> Option<&Raid> {
        let pos_f64 = pos.to_f64();
        self.raid_map.values().find(|r| {
            r.is_active()
                && r.center.to_f64().squared_distance_to_vec(&pos_f64)
                    <= Raid::VALID_RAID_RADIUS_SQR
        })
    }

    pub fn get_raid_at_mut(&mut self, pos: &BlockPos) -> Option<&mut Raid> {
        let pos_f64 = pos.to_f64();
        self.raid_map.values_mut().find(|r| {
            r.is_active()
                && r.center.to_f64().squared_distance_to_vec(&pos_f64)
                    <= Raid::VALID_RAID_RADIUS_SQR
        })
    }

    #[must_use]
    pub fn get_nearby_raid(&self, pos: &BlockPos, max_dist_sqr: f64) -> Option<&Raid> {
        let pos_f64 = pos.to_f64();
        let mut closest = None;
        let mut closest_dist = max_dist_sqr;

        for raid in self.raid_map.values() {
            let dist = raid.center.to_f64().squared_distance_to_vec(&pos_f64);
            if raid.is_active() && dist < closest_dist {
                closest = Some(raid);
                closest_dist = dist;
            }
        }
        closest
    }

    pub fn create_or_extend_raid(
        &mut self,
        raid_position: BlockPos,
        world: &Arc<World>,
    ) -> Option<i32> {
        if !world.level_info.load().game_rules.raids {
            return None;
        }
        let poi_positions = world.village_poi_positions_in_range(raid_position, 64);
        let raid_center_pos = if poi_positions.is_empty() {
            raid_position
        } else {
            let count = poi_positions.len() as f64;
            let (sum_x, sum_y, sum_z) = poi_positions.iter().fold((0.0, 0.0, 0.0), |acc, pos| {
                (
                    acc.0 + f64::from(pos.0.x),
                    acc.1 + f64::from(pos.0.y),
                    acc.2 + f64::from(pos.0.z),
                )
            });
            BlockPos::new(
                (sum_x / count).floor() as i32,
                (sum_y / count).floor() as i32,
                (sum_z / count).floor() as i32,
            )
        };

        let existing_id = self.raid_map.iter().find_map(|(&id, r)| {
            let dist = r
                .center
                .to_f64()
                .squared_distance_to_vec(&raid_center_pos.to_f64());
            (r.is_active() && dist <= Raid::VALID_RAID_RADIUS_SQR).then_some(id)
        });

        if let Some(id) = existing_id {
            if let Some(raid) = self.raid_map.get_mut(&id) {
                raid.absorb_raid_omen();
            }
            Some(id)
        } else {
            self.next_id += 1;
            let id = self.next_id;
            let mut raid = Raid::new(id, raid_center_pos, world.level_info.load().difficulty);
            raid.absorb_raid_omen();
            self.raid_map.insert(id, raid);
            Some(id)
        }
    }

    pub fn tick(&mut self, world: &Arc<World>) {
        self.tick_counter += 1;
        self.patrol_spawner.tick(world);

        if !world.level_info.load().game_rules.raids {
            for raid in self.raid_map.values_mut() {
                raid.stop(world);
            }
        }

        let ids: Vec<i32> = self.raid_map.keys().copied().collect();
        for id in ids {
            if let Some(raid) = self.raid_map.get_mut(&id) && !raid.is_stopped() {
                raid.tick(world);
            }
        }

        let stopped: Vec<i32> = self
            .raid_map
            .iter()
            .filter(|(_, raid)| raid.is_stopped())
            .map(|(&id, _)| id)
            .collect();
        for id in stopped {
            self.raid_map.remove(&id);
        }
    }
}


#[cfg(test)]
mod tests {
    use super::{Difficulty, Raid, RaiderType, num_groups_for_difficulty, potential_bonus_spawns};
    use pumpkin_util::math::position::BlockPos;

    #[test]
    fn num_groups_matches_vanilla_difficulty_table() {
        assert_eq!(num_groups_for_difficulty(Difficulty::Peaceful), 0);
        assert_eq!(num_groups_for_difficulty(Difficulty::Easy), 3);
        assert_eq!(num_groups_for_difficulty(Difficulty::Normal), 5);
        assert_eq!(num_groups_for_difficulty(Difficulty::Hard), 7);
    }

    #[test]
    fn enchant_odds_match_vanilla_omen_table() {
        // Raid.java:795-806.
        let mut raid = Raid::new(0, BlockPos::new(0, 0, 0), Difficulty::Normal);
        for (omen, odds) in [(0, 0.0), (1, 0.0), (2, 0.1), (3, 0.25), (4, 0.5), (5, 0.75), (6, 0.0)] {
            raid.raid_omen_level = omen;
            assert!((raid.enchant_odds() - odds).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn default_num_spawns_matches_vanilla_wave_tables() {
        let raid = Raid::new(0, BlockPos::new(0, 0, 0), Difficulty::Normal);
        assert_eq!(raid.get_default_num_spawns(RaiderType::Pillager, 5, false), 4);
        assert_eq!(raid.get_default_num_spawns(RaiderType::Vindicator, 5, false), 4);
        assert_eq!(raid.get_default_num_spawns(RaiderType::Evoker, 5, false), 1);
        assert_eq!(raid.get_default_num_spawns(RaiderType::Witch, 5, false), 0);
        assert_eq!(raid.get_default_num_spawns(RaiderType::Ravager, 5, false), 1);
    }

    #[test]
    fn bonus_spawns_use_vanilla_tables() {
        for wave in 1..=7 {
            for difficulty in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard] {
                assert_eq!(potential_bonus_spawns(RaiderType::Evoker, wave, difficulty, false), 0);
            }
        }
    }

    #[test]
    fn patrol_interval_bounds_match_vanilla() {
        // PatrolSpawner.java:26.
        assert_eq!(super::PATROL_SPAWN_INTERVAL, 12000);
        assert_eq!(super::PATROL_SPAWN_INTERVAL_JITTER, 1200);
    }
}
