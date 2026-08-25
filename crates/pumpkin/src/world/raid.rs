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
use tokio::sync::Mutex;
use uuid::Uuid;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;

use super::World;
use super::bossbar::{Bossbar, BossbarColor, BossbarDivisions, BossbarFlags};
use crate::entity::EntityBase;
use crate::entity::living::RaidMembership;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::player::Player;
use crate::world::natural_spawner::{is_spawn_position_ok, is_valid_empty_spawn_block};

const RAID_TIMEOUT_TICKS: i64 = 48000; // Raid.java:88
const NUM_SPAWN_ATTEMPTS: i32 = 5; // Raid.java:89
const DEFAULT_PRE_RAID_TICKS: i32 = 300; // Raid.java:94
const MAX_CELEBRATION_TICKS: i32 = 600; // Raid.java:96
pub(crate) const DEFAULT_MAX_RAID_OMEN_LEVEL: i32 = 5; // Raid.java:98
const HERO_OF_THE_VILLAGE_DURATION: i32 = 48000; // Raid.java:103
const POST_RAID_TICK_LIMIT: i32 = 40; // Raid.java:93
const VALID_RAID_RADIUS_SQR: f64 = 9216.0; // Raid.java:105
const RAID_REMOVAL_THRESHOLD_SQR: f64 = 12544.0; // Raid.java:106

// PatrolSpawner.java:26 - base cooldown and jitter between patrol-spawn attempts.
const PATROL_SPAWN_INTERVAL: i32 = 12000;
const PATROL_SPAWN_INTERVAL_JITTER: i32 = 1200;
/// `Level.isBrightOutside` (Level.java:384-386): `skyDarken < 4`.
const BRIGHT_OUTSIDE_SKY_DARKEN: u8 = 4;
/// `PatrollingMonster.checkPatrollingMonsterSpawnRules` (PatrollingMonster.java:87-95) refuses
/// any position whose block light exceeds 8.
const PATROL_MAX_BLOCK_LIGHT: u8 = 8;
/// `PatrolSpawner.tick` (PatrolSpawner.java:37-38): the half-extent of the square of chunks that
/// must already be loaded around the chosen spawn column.
const PATROL_REQUIRED_LOADED_DELTA: i32 = 10;

// Raid.java:831-846, RaiderType.spawnsPerWaveBeforeBonus, indexed by wave number
// (index 0 unused; index == numGroups is the bonus-wave row).
const VINDICATOR_SPAWNS: [i32; 8] = [0, 0, 2, 0, 1, 4, 2, 5];
const EVOKER_SPAWNS: [i32; 8] = [0, 0, 0, 0, 0, 1, 1, 2];
const PILLAGER_SPAWNS: [i32; 8] = [0, 4, 3, 3, 4, 4, 4, 2];
const WITCH_SPAWNS: [i32; 8] = [0, 0, 0, 0, 3, 0, 0, 1];
const RAVAGER_SPAWNS: [i32; 8] = [0, 0, 0, 1, 0, 1, 0, 2];

#[derive(Clone, Copy, PartialEq, Eq)]
enum RaiderType {
    Vindicator,
    Evoker,
    Pillager,
    Witch,
    Ravager,
}

impl RaiderType {
    const ALL: [Self; 5] = [
        Self::Vindicator,
        Self::Evoker,
        Self::Pillager,
        Self::Witch,
        Self::Ravager,
    ];

    const fn entity_type(self) -> &'static EntityType {
        match self {
            Self::Vindicator => &EntityType::VINDICATOR,
            Self::Evoker => &EntityType::EVOKER,
            Self::Pillager => &EntityType::PILLAGER,
            Self::Witch => &EntityType::WITCH,
            Self::Ravager => &EntityType::RAVAGER,
        }
    }

    const fn spawn_table(self) -> &'static [i32; 8] {
        match self {
            Self::Vindicator => &VINDICATOR_SPAWNS,
            Self::Evoker => &EVOKER_SPAWNS,
            Self::Pillager => &PILLAGER_SPAWNS,
            Self::Witch => &WITCH_SPAWNS,
            Self::Ravager => &RAVAGER_SPAWNS,
        }
    }
}

/// Raid.java:786-793.
pub(crate) const fn num_groups_for_difficulty(difficulty: Difficulty) -> i32 {
    match difficulty {
        Difficulty::Peaceful => 0,
        Difficulty::Easy => 3,
        Difficulty::Normal => 5,
        Difficulty::Hard => 7,
    }
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
            if is_easy {
                rng.random_range(0..2)
            } else if is_normal {
                1
            } else {
                2
            }
        }
        RaiderType::Evoker => return 0,
        RaiderType::Witch => {
            if is_easy || wave <= 2 || wave == 4 {
                return 0;
            }
            1
        }
        RaiderType::Ravager => i32::from(!is_easy && is_bonus_wave),
    };

    if bonus_spawns > 0 {
        rng.random_range(0..bonus_spawns + 1)
    } else {
        0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RaidStatus {
    Ongoing,
    Victory,
    Loss,
    Stopped,
}

pub struct Raid {
    id: i32,
    center: BlockPos,
    started: bool,
    active: bool,
    ticks_active: i64,
    omen_level: i32,
    groups_spawned: i32,
    post_raid_ticks: i32,
    cooldown_ticks: i32,
    total_health: f32,
    num_groups: i32,
    status: RaidStatus,
    heroes_of_the_village: HashSet<Uuid>,
    group_leaders: HashMap<i32, Uuid>,
    group_raiders: HashMap<i32, HashSet<Uuid>>,
    celebration_ticks: i32,
    bossbar_uuid: Uuid,
    bossbar_players: Vec<Uuid>,
}

impl Raid {
    fn new(id: i32, center: BlockPos, difficulty: Difficulty) -> Self {
        Self {
            id,
            center,
            started: false,
            active: true,
            ticks_active: 0,
            omen_level: 0,
            groups_spawned: 0,
            post_raid_ticks: 0,
            cooldown_ticks: DEFAULT_PRE_RAID_TICKS,
            total_health: 0.0,
            num_groups: num_groups_for_difficulty(difficulty),
            status: RaidStatus::Ongoing,
            heroes_of_the_village: HashSet::new(),
            group_leaders: HashMap::new(),
            group_raiders: HashMap::new(),
            celebration_ticks: 0,
            bossbar_uuid: Uuid::new_v4(),
            bossbar_players: Vec::new(),
        }
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    #[must_use]
    pub const fn is_started(&self) -> bool {
        self.started
    }

    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.status == RaidStatus::Stopped
    }

    #[must_use]
    pub fn is_victory(&self) -> bool {
        self.status == RaidStatus::Victory
    }

    #[must_use]
    pub fn is_loss(&self) -> bool {
        self.status == RaidStatus::Loss
    }

    #[must_use]
    pub fn is_over(&self) -> bool {
        self.is_victory() || self.is_loss()
    }

    #[must_use]
    pub const fn has_first_wave_spawned(&self) -> bool {
        self.groups_spawned > 0
    }

    /// `Raid.isBetweenWaves` (`Raid.java:169-171`): a spawned raid with no living raiders and a
    /// positive inter-wave cooldown is still in the pre-raid activity package.
    #[must_use]
    pub fn is_between_waves(&self) -> bool {
        self.has_first_wave_spawned() && self.total_raiders_alive() == 0 && self.cooldown_ticks > 0
    }

    #[must_use]
    pub const fn center(&self) -> BlockPos {
        self.center
    }

    #[must_use]
    pub const fn raid_omen_level(&self) -> i32 {
        self.omen_level
    }

    /// Vanilla `Raid.getEnchantOdds` (`Raid.java:795-806`): the chance ceiling fed
    /// into raider weapon-enchant rolls (`Vindicator.applyRaidBuffs`,
    /// `Vindicator.java:171`), scaled by Bad Omen level above one.
    #[must_use]
    pub const fn enchant_odds(&self) -> f32 {
        match self.omen_level {
            2 => 0.1,
            3 => 0.25,
            4 => 0.5,
            5 => 0.75,
            _ => 0.0,
        }
    }

    fn total_raiders_alive(&self) -> i32 {
        self.group_raiders.values().map(HashSet::len).sum::<usize>() as i32
    }

    const fn is_final_wave(&self) -> bool {
        self.groups_spawned == self.num_groups
    }

    const fn has_bonus_wave(&self) -> bool {
        self.omen_level > 1
    }

    const fn has_spawned_bonus_wave(&self) -> bool {
        self.groups_spawned > self.num_groups
    }

    const fn has_more_waves(&self) -> bool {
        if self.has_bonus_wave() {
            !self.has_spawned_bonus_wave()
        } else {
            !self.is_final_wave()
        }
    }

    fn should_spawn_bonus_group(&self) -> bool {
        self.is_final_wave() && self.total_raiders_alive() == 0 && self.has_bonus_wave()
    }

    fn should_spawn_group(&self) -> bool {
        self.cooldown_ticks == 0
            && (self.groups_spawned < self.num_groups || self.should_spawn_bonus_group())
            && self.total_raiders_alive() == 0
    }

    const fn default_num_spawns(
        &self,
        raider_type: RaiderType,
        wave: i32,
        is_bonus_wave: bool,
    ) -> i32 {
        let table = raider_type.spawn_table();
        if is_bonus_wave {
            table[self.num_groups as usize]
        } else {
            table[wave as usize]
        }
    }

    const fn stop(&mut self) {
        self.active = false;
        self.status = RaidStatus::Stopped;
    }

    /// Raid.java:247-261. Reads `RAID_OMEN` (not `BAD_OMEN` - see
    /// `RaidOmenMobEffect.applyEffectTick`, which triggers this on `RAID_OMEN` expiry after
    /// `BadOmenMobEffect` converted `BAD_OMEN` into `RAID_OMEN` on village entry).
    async fn absorb_raid_omen(&mut self, player: &Player) -> bool {
        let Some(effect) = player
            .living_entity
            .get_effect(&StatusEffect::RAID_OMEN)
            .await
        else {
            return false;
        };
        self.omen_level = (self.omen_level + i32::from(effect.amplifier) + 1)
            .clamp(0, DEFAULT_MAX_RAID_OMEN_LEVEL);
        true
    }

    pub fn remove_leader(&mut self, wave: i32) {
        self.group_leaders.remove(&wave);
    }

    pub fn add_hero_of_the_village(&mut self, uuid: Uuid) {
        self.heroes_of_the_village.insert(uuid);
    }

    /// Raid.java:613-627. `health` is only used when `remove_from_total_health` is set (the
    /// vanilla stray-raider cleanup path); the death path passes `false`/`0.0` since a raider's
    /// health at the moment of death does not affect the boss-bar total.
    pub fn remove_from_raid(
        &mut self,
        wave: i32,
        uuid: Uuid,
        health: f32,
        remove_from_total_health: bool,
    ) {
        if let Some(raiders) = self.group_raiders.get_mut(&wave)
            && raiders.remove(&uuid)
            && remove_from_total_health
        {
            self.total_health -= health;
        }
    }

    fn find_random_spawn_pos(&self, world: &Arc<World>, max_tries: i32) -> Option<BlockPos> {
        let seconds_remaining = self.cooldown_ticks / 20;
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
            let spawn_y = world.get_top_block(Vector2::new(spawn_x, spawn_z));
            if (spawn_y - self.center.0.y).abs() <= 96 {
                return Some(BlockPos::new(spawn_x, spawn_y, spawn_z));
            }
        }
        None
    }

    /// Raid.java:519-570.
    async fn spawn_group(&mut self, world: &Arc<World>, pos: BlockPos) {
        let group_number = self.groups_spawned + 1;
        self.total_health = 0.0;
        let difficulty = world.level_info.load().difficulty;
        let is_bonus_group = self.should_spawn_bonus_group();
        let mut leader_set = false;
        let spawn_pos = pos.to_centered_f64();

        for raider_type in RaiderType::ALL {
            let num_spawns = self.default_num_spawns(raider_type, group_number, is_bonus_group)
                + potential_bonus_spawns(raider_type, group_number, difficulty, is_bonus_group);
            let mut ravagers_spawned = 0;

            for _ in 0..num_spawns {
                let uuid = Uuid::new_v4();
                let raider = crate::entity::r#type::from_type(
                    raider_type.entity_type(),
                    spawn_pos,
                    world,
                    uuid,
                );
                let Some(mob) = raider.get_mob() else {
                    continue;
                };
                let is_leader = !leader_set && mob.can_be_raid_leader();
                if is_leader {
                    leader_set = true;
                    self.group_leaders.insert(group_number, uuid);
                    equip_ominous_banner(mob.get_mob_entity()).await;
                }
                self.join_raid(group_number, uuid, mob.get_mob_entity(), is_leader);
                // Raid.java:582 passes `false` unconditionally, regardless of leader status.
                mob.apply_raid_buffs(group_number, false).await;

                if raider_type == RaiderType::Ravager {
                    let rider_type =
                        if group_number == num_groups_for_difficulty(Difficulty::Normal) {
                            Some(&EntityType::PILLAGER)
                        } else if group_number >= num_groups_for_difficulty(Difficulty::Hard) {
                            Some(if ravagers_spawned == 0 {
                                &EntityType::EVOKER
                            } else {
                                &EntityType::VINDICATOR
                            })
                        } else {
                            None
                        };
                    ravagers_spawned += 1;

                    if let Some(rider_type) = rider_type {
                        let rider_uuid = Uuid::new_v4();
                        let rider = crate::entity::r#type::from_type(
                            rider_type, spawn_pos, world, rider_uuid,
                        );
                        if let Some(rider_mob) = rider.get_mob() {
                            self.join_raid(
                                group_number,
                                rider_uuid,
                                rider_mob.get_mob_entity(),
                                false,
                            );
                            rider_mob.apply_raid_buffs(group_number, false).await;
                        }
                        world.spawn_entity(rider.clone()).await;
                        raider
                            .get_entity()
                            .add_passenger(raider.clone(), rider)
                            .await;
                    }
                }

                world.spawn_entity(raider).await;
            }
        }

        self.groups_spawned += 1;
        self.update_bossbar_health(world).await;
    }

    /// `Raid.joinRaid` (`Raid.java`). `pub(crate)` so raider-side AI (`PathfindToRaidGoal`'s
    /// `recruitNearby`, and `Raider.aiStep`'s self-recruitment path) can enlist mid-raid.
    pub(crate) fn join_raid(
        &mut self,
        wave: i32,
        uuid: Uuid,
        mob_entity: &crate::entity::mob::MobEntity,
        is_patrol_leader: bool,
    ) {
        self.group_raiders.entry(wave).or_default().insert(uuid);
        self.total_health += mob_entity.living_entity.health.load();
        mob_entity
            .living_entity
            .can_join_raid
            .store(true, std::sync::atomic::Ordering::Relaxed);
        mob_entity
            .living_entity
            .raid_membership
            .store(Some(RaidMembership {
                raid_id: self.id,
                wave,
                is_patrol_leader,
            }));
    }

    /// `Raid.getGroupsSpawned` (`Raid.java:207`).
    #[must_use]
    pub(crate) const fn groups_spawned(&self) -> i32 {
        self.groups_spawned
    }

    fn health_of_living_raiders(&self, world: &Arc<World>) -> f32 {
        let mut health = 0.0;
        for raiders in self.group_raiders.values() {
            for uuid in raiders {
                if let Some(entity) = world.get_entity_by_uuid(*uuid)
                    && let Some(living) = entity.get_living_entity()
                {
                    health += living.health.load();
                }
            }
        }
        health
    }

    async fn update_bossbar_health(&self, world: &Arc<World>) {
        let fraction = if self.total_health > 0.0 {
            (self.health_of_living_raiders(world) / self.total_health).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let players = world.players.load();
        for uuid in &self.bossbar_players {
            if let Some(player) = players.iter().find(|p| p.gameprofile.id == *uuid) {
                player
                    .update_bossbar_health(&self.bossbar_uuid, fraction)
                    .await;
            }
        }
    }

    fn make_bossbar(&self) -> Bossbar {
        Bossbar {
            uuid: self.bossbar_uuid,
            title: pumpkin_util::text::TextComponent::translate("event.minecraft.raid", []),
            health: 0.0,
            color: BossbarColor::Red,
            division: BossbarDivisions::Notches10,
            flags: BossbarFlags::empty(),
        }
    }

    /// Raid.java:218-233, approximated with a fixed-radius player scan instead of the exact
    /// per-position raid lookup (`level.getRaidAt(pos) == this`).
    async fn update_players(&mut self, world: &Arc<World>) {
        let center = self.center.to_centered_f64();
        let nearby = world.get_nearby_players(center, 64.0);
        let current: Vec<Uuid> = nearby.iter().map(|p| p.gameprofile.id).collect();

        for player in &nearby {
            if !self.bossbar_players.contains(&player.gameprofile.id) {
                player.send_bossbar(&self.make_bossbar()).await;
                self.bossbar_players.push(player.gameprofile.id);
            }
        }

        let world_players = world.players.load();
        let stale: Vec<Uuid> = self
            .bossbar_players
            .iter()
            .filter(|u| !current.contains(u))
            .copied()
            .collect();
        for uuid in stale {
            if let Some(player) = world_players.iter().find(|p| p.gameprofile.id == uuid) {
                player.remove_bossbar(self.bossbar_uuid).await;
            }
            self.bossbar_players.retain(|u| *u != uuid);
        }
    }

    async fn remove_all_bossbars(&mut self, world: &Arc<World>) {
        let players = world.players.load();
        for uuid in self.bossbar_players.drain(..) {
            if let Some(player) = players.iter().find(|p| p.gameprofile.id == uuid) {
                player.remove_bossbar(self.bossbar_uuid).await;
            }
        }
    }

    /// Raid.java:466-499, without the `isVillage`/`getNoActionTime` timeout branch (no village
    /// system to check against).
    fn update_raiders(&mut self, world: &Arc<World>) {
        struct Stale {
            wave: i32,
            uuid: Uuid,
            health: f32,
        }
        let mut stale = Vec::new();
        for (&wave, raiders) in &self.group_raiders {
            for &uuid in raiders {
                match world.get_entity_by_uuid(uuid) {
                    None => stale.push(Stale {
                        wave,
                        uuid,
                        health: 0.0,
                    }),
                    Some(entity) => {
                        let pos = entity.get_entity().pos.load();
                        let block_pos = BlockPos::floored_v(pos);
                        let too_far = (self.center.squared_distance(&block_pos) as f64)
                            >= RAID_REMOVAL_THRESHOLD_SQR;
                        if !entity.get_entity().is_alive() || too_far {
                            let health = entity
                                .get_living_entity()
                                .map_or(0.0, |living| living.health.load());
                            stale.push(Stale { wave, uuid, health });
                        }
                    }
                }
            }
        }

        for s in stale {
            if let Some(raiders) = self.group_raiders.get_mut(&s.wave)
                && raiders.remove(&s.uuid)
            {
                self.total_health -= s.health;
            }
            if self.group_leaders.get(&s.wave) == Some(&s.uuid) {
                self.group_leaders.remove(&s.wave);
            }
            if let Some(entity) = world.get_entity_by_uuid(s.uuid)
                && let Some(living) = entity.get_living_entity()
                && living
                    .raid_membership
                    .load()
                    .is_some_and(|membership| membership.raid_id == self.id)
            {
                living.raid_membership.store(None);
            }
        }
    }

    /// Raid.java:396-407 (Hero of the Village reward on victory).
    async fn apply_victory_rewards(&self, world: &Arc<World>) {
        for uuid in self.heroes_of_the_village.clone() {
            let Some(entity) = world.get_entity_by_uuid(uuid) else {
                continue;
            };
            let Some(living) = entity.get_living_entity() else {
                continue;
            };
            living
                .add_effect(Effect {
                    effect_type: &StatusEffect::HERO_OF_THE_VILLAGE,
                    duration: HERO_OF_THE_VILLAGE_DURATION,
                    amplifier: (self.omen_level - 1).max(0) as u8,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                })
                .await;
        }
    }

    /// Raid.java:269-431.
    async fn tick(&mut self, world: &Arc<World>) {
        if self.is_stopped() {
            return;
        }

        if self.status == RaidStatus::Ongoing {
            if world.level_info.load().difficulty == Difficulty::Peaceful {
                self.stop();
                return;
            }

            self.ticks_active += 1;
            if self.ticks_active >= RAID_TIMEOUT_TICKS {
                self.stop();
                return;
            }

            let raiders_alive = self.total_raiders_alive();
            if raiders_alive == 0 && self.has_more_waves() {
                if self.cooldown_ticks <= 0 {
                    if self.cooldown_ticks == 0 && self.groups_spawned > 0 {
                        self.cooldown_ticks = DEFAULT_PRE_RAID_TICKS;
                        return;
                    }
                } else {
                    self.cooldown_ticks -= 1;
                }
            }

            if self.ticks_active % 20 == 0 {
                self.update_players(world).await;
                self.update_raiders(world);
            }

            let mut attempts = 0;
            while self.should_spawn_group() {
                let Some(spawn_pos) = self.find_random_spawn_pos(world, 20) else {
                    attempts += 1;
                    if attempts > NUM_SPAWN_ATTEMPTS {
                        self.stop();
                        break;
                    }
                    continue;
                };
                self.started = true;
                self.spawn_group(world, spawn_pos).await;
                world.play_sound(
                    Sound::EventRaidHorn,
                    SoundCategory::Neutral,
                    &spawn_pos.to_centered_f64(),
                );
            }

            let raiders_alive = self.total_raiders_alive();
            if self.is_started() && !self.has_more_waves() && raiders_alive == 0 {
                if self.post_raid_ticks < POST_RAID_TICK_LIMIT {
                    self.post_raid_ticks += 1;
                } else {
                    self.status = RaidStatus::Victory;
                    self.apply_victory_rewards(world).await;
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
            if self.celebration_ticks >= MAX_CELEBRATION_TICKS {
                self.stop();
            }
        }
    }
}

/// Equips the ominous-banner head slot used to mark a raid wave leader and a patrol captain.
///
/// `Raid.getOminousBannerInstance` (Raid.java:656) builds a white banner carrying the eight
/// `BannerPatternLayers` from `Raid.getBannerComponentPatch` (Raid.java:633-648) plus an
/// `ITEM_NAME` override. Pumpkin's `BannerPatterns` data component
/// (`pumpkin_data::data_component_impl::BannerPatternsImpl`) is a payload-less unit struct, so
/// the pattern layers cannot be attached yet; the banner is a plain white banner until that
/// component carries data. `setDropChance(HEAD, 2.0F)` (PatrollingMonster.java:77) is likewise
/// not represented - `EntityEquipment` has no per-slot drop chance.
async fn equip_ominous_banner(mob_entity: &crate::entity::mob::MobEntity) {
    mob_entity
        .living_entity
        .entity_equipment
        .lock()
        .await
        .put(&EquipmentSlot::HEAD, ItemStack::new(1, &Item::WHITE_BANNER));
}

/// `PatrolSpawner` (PatrolSpawner.java:16-91): the world-tick spawner that seeds illager
/// patrols away from villages, one of which carries the ominous banner as its captain.
///
/// Ported field for field: the `12000 + nextInt(1200)` reschedule (line 26), the daylight gate
/// (line 27), the 1-in-5 chance (line 28), the random non-spectator player (lines 29-32), the
/// "not within 2 sections of a village" refusal (line 33), the `(24 + nextInt(24))` signed
/// offset on each horizontal axis (lines 34-36), the loaded-chunk requirement (line 38), the
/// `ceil(effectiveDifficulty) + 1` group size (line 40), leader-first with break-on-failure
/// (lines 44-47) and the `nextInt(5) - nextInt(5)` per-member walk (lines 52-53).
///
/// Scope reductions, each because the vanilla dependency has no analogue in Pumpkin:
/// - `EnvironmentAttributes.CAN_PILLAGER_PATROL_SPAWN` (line 39) - no environment-attribute
///   system exists; the biome/dimension gate is therefore not applied.
/// - The Y lookup uses `World::get_top_block` (the `WORLD_SURFACE` heightmap) rather than
///   `Heightmap.Types.MOTION_BLOCKING_NO_LEAVES` (line 43); Pumpkin tracks the latter on chunks
///   but exposes no world-level accessor for it.
/// - `PatrollingMonster.setPatrolLeader`/`findPatrolTarget`/`setPatrolling`
///   (PatrollingMonster.java:102-131) and `LongDistancePatrolGoal` are not ported - Pumpkin's
///   `PillagerEntity` carries no patrol-target state, so spawned patrols use the pillager's
///   ordinary wander and target goals instead of marching toward a distant target. The captain
///   is still marked the way vanilla marks it, by the head-slot banner
///   (PatrollingMonster.java:76).
/// - Killing a captain does not yield an ominous bottle. Vanilla routes that through the
///   `minecraft:pillager` entity loot table's `type_specific/raider {is_captain: true}`
///   predicate; `pumpkin_util::loot_table::LootCondition::EntityProperties` models only
///   `expected_type`/`is_on_fire`/`mainhand_enchantment_tag`, so the predicate cannot be
///   expressed. Drinking an ominous bottle already triggers the whole raid chain
///   (`LivingEntity::apply_effect_tick`), so this is the one missing link in the
///   captain-to-raid loop.
#[derive(Default)]
pub struct PatrolSpawner {
    next_tick: i32,
}

impl PatrolSpawner {
    /// `Level.isBrightOutside` (Level.java:384-386). A dimension with a fixed time of day is
    /// never "bright outside"; Pumpkin models that as "has no sky light".
    fn is_bright_outside(world: &Arc<World>) -> bool {
        world.dimension.has_skylight
            && world.sky_darken.load(std::sync::atomic::Ordering::Relaxed)
                < BRIGHT_OUTSIDE_SKY_DARKEN
    }

    /// `ServerLevel.hasChunksAt` over the `+/-10` block square of PatrolSpawner.java:38.
    fn has_chunks_at(world: &Arc<World>, pos: BlockPos) -> bool {
        let min_chunk_x = (pos.0.x - PATROL_REQUIRED_LOADED_DELTA) >> 4;
        let max_chunk_x = (pos.0.x + PATROL_REQUIRED_LOADED_DELTA) >> 4;
        let min_chunk_z = (pos.0.z - PATROL_REQUIRED_LOADED_DELTA) >> 4;
        let max_chunk_z = (pos.0.z + PATROL_REQUIRED_LOADED_DELTA) >> 4;
        for chunk_x in min_chunk_x..=max_chunk_x {
            for chunk_z in min_chunk_z..=max_chunk_z {
                if world
                    .level
                    .loaded_chunks
                    .get(&Vector2::new(chunk_x, chunk_z))
                    .is_none()
                {
                    return false;
                }
            }
        }
        true
    }

    /// `PatrolSpawner.spawnPatrolMember` (PatrolSpawner.java:67-91).
    async fn spawn_patrol_member(world: &Arc<World>, pos: BlockPos, is_leader: bool) -> bool {
        if !is_valid_empty_spawn_block(world.get_block_state(&pos), &EntityType::PILLAGER) {
            return false;
        }
        // PatrollingMonster.checkPatrollingMonsterSpawnRules (PatrollingMonster.java:87-95):
        // block light above 8 refuses, then the ordinary `checkMobSpawnRules` support check.
        if world
            .get_block_light_level(&pos)
            .unwrap_or(0)
            .gt(&PATROL_MAX_BLOCK_LIGHT)
        {
            return false;
        }
        if !is_spawn_position_ok(world, &pos, &EntityType::PILLAGER) {
            return false;
        }

        let spawn_pos = Vector3::new(
            f64::from(pos.0.x) + 0.5,
            f64::from(pos.0.y),
            f64::from(pos.0.z) + 0.5,
        );
        let pillager = crate::entity::r#type::from_type(
            &EntityType::PILLAGER,
            spawn_pos,
            world,
            Uuid::new_v4(),
        );
        if is_leader && let Some(mob) = pillager.get_mob() {
            equip_ominous_banner(mob.get_mob_entity()).await;
        }
        world.spawn_entity(pillager).await;
        true
    }

    /// `PatrolSpawner.tick` (PatrolSpawner.java:20-65). `spawn_enemies` mirrors
    /// `ServerChunkCache.spawnEnemies`, which `MinecraftServer` sets from
    /// `ServerLevel.isSpawningMonsters` (ServerLevel.java:1776-1778).
    async fn tick(&mut self, world: &Arc<World>) {
        let level_info = world.level_info.load();
        let game_rules = &level_info.game_rules;
        let spawn_enemies = game_rules.spawn_mobs
            && game_rules.spawn_monsters
            && level_info.difficulty != Difficulty::Peaceful;
        if !spawn_enemies || !game_rules.spawn_patrols {
            return;
        }

        self.next_tick -= 1;
        if self.next_tick > 0 {
            return;
        }
        // PatrolSpawner.java:26 accumulates onto the (already non-positive) counter rather than
        // assigning, so an overshoot is carried into the next interval.
        self.next_tick +=
            PATROL_SPAWN_INTERVAL + rand::random_range(0..PATROL_SPAWN_INTERVAL_JITTER);

        if !Self::is_bright_outside(world) {
            return;
        }
        if rand::random_range(0..5) != 0 {
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
        if world.is_close_to_village(player_pos, 2).await {
            return;
        }

        let offset_x =
            (24 + rand::random_range(0..24)) * if rand::random_range(0..2) == 0 { -1 } else { 1 };
        let offset_z =
            (24 + rand::random_range(0..24)) * if rand::random_range(0..2) == 0 { -1 } else { 1 };
        let mut spawn_pos = player_pos.add(offset_x, 0, offset_z);

        if !Self::has_chunks_at(world, spawn_pos) {
            return;
        }

        let difficulty = RegionalDifficulty::at(world, spawn_pos.to_centered_f64());
        let group_size = difficulty.effective_difficulty.ceil() as i32 + 1;

        for i in 0..group_size {
            spawn_pos = BlockPos::new(
                spawn_pos.0.x,
                world.get_top_block(Vector2::new(spawn_pos.0.x, spawn_pos.0.z)) + 1,
                spawn_pos.0.z,
            );
            if i == 0 {
                if !Self::spawn_patrol_member(world, spawn_pos, true).await {
                    break;
                }
            } else {
                Self::spawn_patrol_member(world, spawn_pos, false).await;
            }
            spawn_pos = spawn_pos.add(
                rand::random_range(0..5) - rand::random_range(0..5),
                0,
                rand::random_range(0..5) - rand::random_range(0..5),
            );
        }
    }
}

pub struct RaidManager {
    raids: HashMap<i32, Raid>,
    next_id: i32,
    /// `ServerLevel.customSpawners`' `PatrolSpawner` entry; kept here because `RaidManager`
    /// is the only raid-owned struct already ticked from `World::tick`.
    patrol_spawner: PatrolSpawner,
}

impl Default for RaidManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RaidManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            raids: HashMap::new(),
            next_id: 1,
            patrol_spawner: PatrolSpawner::default(),
        }
    }

    pub fn raid_mut(&mut self, id: i32) -> Option<&mut Raid> {
        self.raids.get_mut(&id)
    }

    /// Immutable lookup for readers that only inspect raid state (e.g.
    /// `Raid.getEnchantOdds` in `Vindicator.applyRaidBuffs`).
    #[must_use]
    pub fn raid(&self, id: i32) -> Option<&Raid> {
        self.raids.get(&id)
    }

    /// `ServerLevel.getRaidAt` plus the pre-raid activity selection used by
    /// `VillagerGoalPackages.getPreRaidPackage` (`VillagerGoalPackages.java:231-245`).
    #[must_use]
    pub fn is_pre_raid_at(&self, pos: BlockPos) -> bool {
        self.raids
            .values()
            .filter(|raid| raid.is_active())
            .filter(|raid| (raid.center.squared_distance(&pos) as f64) < VALID_RAID_RADIUS_SQR)
            .any(|raid| !raid.has_first_wave_spawned() || raid.is_between_waves())
    }

    fn find_active_raid_near(&self, pos: BlockPos) -> Option<i32> {
        self.raids
            .iter()
            .filter(|(_, raid)| raid.is_active())
            .map(|(&id, raid)| (id, raid.center.squared_distance(&pos) as f64))
            .filter(|&(_, dist_sqr)| dist_sqr < VALID_RAID_RADIUS_SQR)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// `raid.getRaidOmenLevel()` for the nearest active raid within `getRaidAt`'s radius
    /// (`ServerLevel.getRaidAt`), or `None` if there is none - used by `BadOmenMobEffect`'s
    /// "no raid, or raid not yet at max omen" gate.
    #[must_use]
    pub fn raid_omen_level_near(&self, pos: BlockPos) -> Option<i32> {
        self.find_active_raid_near(pos)
            .map(|id| self.raids[&id].raid_omen_level())
    }

    /// Raids.createOrExtendRaid (Raids.java:102-146). Called from `RaidOmenMobEffect`
    /// (`crate::entity::living::LivingEntity::apply_effect_tick`, `RAID_OMEN` branch) when a
    /// player's `RAID_OMEN` effect expires with a stored `raid_omen_position`.
    pub async fn create_or_extend_raid(
        &mut self,
        world: &Arc<World>,
        player: &Player,
        raid_position: BlockPos,
    ) {
        if player.gamemode.load() == pumpkin_util::GameMode::Spectator {
            return;
        }
        if !world.level_info.load().game_rules.raids {
            return;
        }

        let poi_positions = world
            .village_poi_positions_in_range(raid_position, 64)
            .await;
        let raid_center = if poi_positions.is_empty() {
            raid_position
        } else {
            let count = poi_positions.len() as f64;
            let (sum_x, sum_y, sum_z) = poi_positions.iter().fold((0.0, 0.0, 0.0), |acc, p| {
                (
                    acc.0 + p.0.x as f64,
                    acc.1 + p.0.y as f64,
                    acc.2 + p.0.z as f64,
                )
            });
            BlockPos::new(
                (sum_x / count).floor() as i32,
                (sum_y / count).floor() as i32,
                (sum_z / count).floor() as i32,
            )
        };

        let raid_id = self.find_active_raid_near(raid_center).unwrap_or_else(|| {
            let id = self.next_id;
            self.next_id += 1;
            let difficulty = world.level_info.load().difficulty;
            self.raids
                .insert(id, Raid::new(id, raid_center, difficulty));
            id
        });

        let should_absorb = {
            let raid = self.raids.get(&raid_id).unwrap();
            !raid.is_started() || raid.raid_omen_level() < DEFAULT_MAX_RAID_OMEN_LEVEL
        };
        if should_absorb {
            let raid = self.raids.get_mut(&raid_id).unwrap();
            raid.absorb_raid_omen(player).await;
        }
    }

    /// Raids.tick (Raids.java:79-100).
    pub async fn tick(mgr_mutex: &Mutex<Self>, world: &Arc<World>) {
        let mut mgr = mgr_mutex.lock().await;

        // `ServerLevel.tickCustomSpawners` (ServerLevel.java:483-485). Independent of the
        // `raids` game rule: patrols spawn whether or not raids are enabled.
        mgr.patrol_spawner.tick(world).await;

        if !world.level_info.load().game_rules.raids {
            for raid in mgr.raids.values_mut() {
                raid.stop();
            }
        }

        let ids: Vec<i32> = mgr.raids.keys().copied().collect();
        for id in ids {
            if let Some(raid) = mgr.raids.get_mut(&id)
                && !raid.is_stopped()
            {
                raid.tick(world).await;
            }
        }

        let stopped: Vec<i32> = mgr
            .raids
            .iter()
            .filter(|(_, raid)| raid.is_stopped())
            .map(|(&id, _)| id)
            .collect();
        for id in stopped {
            if let Some(mut raid) = mgr.raids.remove(&id) {
                raid.remove_all_bossbars(world).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Difficulty, PATROL_SPAWN_INTERVAL, PATROL_SPAWN_INTERVAL_JITTER, PatrolSpawner, Raid,
        RaiderType, num_groups_for_difficulty, potential_bonus_spawns,
    };
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
        for (omen, odds) in [
            (0, 0.0),
            (1, 0.0),
            (2, 0.1),
            (3, 0.25),
            (4, 0.5),
            (5, 0.75),
            (6, 0.0),
        ] {
            raid.omen_level = omen;
            assert!(
                (raid.enchant_odds() - odds).abs() < f32::EPSILON,
                "Bad Omen level {omen} must yield odds {odds}"
            );
        }
    }

    #[test]
    fn default_num_spawns_matches_vanilla_wave_tables() {
        let raid = Raid::new(0, BlockPos::new(0, 0, 0), Difficulty::Normal);
        assert_eq!(raid.default_num_spawns(RaiderType::Pillager, 5, false), 4);
        assert_eq!(raid.default_num_spawns(RaiderType::Vindicator, 5, false), 4);
        assert_eq!(raid.default_num_spawns(RaiderType::Evoker, 5, false), 1);
        assert_eq!(raid.default_num_spawns(RaiderType::Witch, 5, false), 0);
        assert_eq!(raid.default_num_spawns(RaiderType::Ravager, 5, false), 1);

        let hard_raid = Raid::new(0, BlockPos::new(0, 0, 0), Difficulty::Hard);
        assert_eq!(
            hard_raid.default_num_spawns(RaiderType::Vindicator, 7, false),
            5
        );
        assert_eq!(
            hard_raid.default_num_spawns(RaiderType::Ravager, 7, false),
            2
        );
    }

    #[test]
    fn bonus_wave_uses_num_groups_index() {
        let raid = Raid::new(0, BlockPos::new(0, 0, 0), Difficulty::Easy);
        assert_eq!(raid.num_groups, 3);
        assert_eq!(
            raid.default_num_spawns(RaiderType::Pillager, 1, true),
            raid.default_num_spawns(RaiderType::Pillager, raid.num_groups, false)
        );
    }

    #[test]
    fn evoker_never_gets_bonus_spawns() {
        for wave in 1..=7 {
            for difficulty in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard] {
                assert_eq!(
                    potential_bonus_spawns(RaiderType::Evoker, wave, difficulty, false),
                    0
                );
                assert_eq!(
                    potential_bonus_spawns(RaiderType::Evoker, wave, difficulty, true),
                    0
                );
            }
        }
    }

    #[test]
    fn witch_bonus_spawns_only_after_wave_two_and_not_on_wave_four() {
        assert_eq!(
            potential_bonus_spawns(RaiderType::Witch, 1, Difficulty::Normal, false),
            0
        );
        assert_eq!(
            potential_bonus_spawns(RaiderType::Witch, 2, Difficulty::Normal, false),
            0
        );
        assert_eq!(
            potential_bonus_spawns(RaiderType::Witch, 4, Difficulty::Normal, false),
            0
        );
        assert_eq!(
            potential_bonus_spawns(RaiderType::Witch, 3, Difficulty::Easy, false),
            0
        );
        for _ in 0..50 {
            assert!(potential_bonus_spawns(RaiderType::Witch, 3, Difficulty::Normal, false) <= 1);
        }
    }

    #[test]
    fn ravager_bonus_spawn_requires_bonus_wave_and_not_easy() {
        for _ in 0..50 {
            assert_eq!(
                potential_bonus_spawns(RaiderType::Ravager, 5, Difficulty::Normal, false),
                0
            );
            assert_eq!(
                potential_bonus_spawns(RaiderType::Ravager, 5, Difficulty::Easy, true),
                0
            );
            assert!(potential_bonus_spawns(RaiderType::Ravager, 5, Difficulty::Normal, true) <= 1);
        }
    }

    /// PatrolSpawner.java:24-26: the counter is decremented every tick and only rearms when it
    /// reaches zero, then accumulates rather than assigns, so an overshoot carries forward.
    #[test]
    fn patrol_spawner_reschedule_carries_overshoot() {
        let mut spawner = PatrolSpawner::default();
        assert_eq!(spawner.next_tick, 0);
        spawner.next_tick = -7;
        spawner.next_tick -= 1;
        let before = spawner.next_tick;
        spawner.next_tick += PATROL_SPAWN_INTERVAL;
        assert_eq!(spawner.next_tick, before + PATROL_SPAWN_INTERVAL);
        assert!(spawner.next_tick < PATROL_SPAWN_INTERVAL);
    }

    /// PatrolSpawner.java:26 bounds the rearmed interval to `[12000, 13200)`.
    #[test]
    fn patrol_spawn_interval_bounds_match_vanilla() {
        assert_eq!(PATROL_SPAWN_INTERVAL, 12000);
        assert_eq!(PATROL_SPAWN_INTERVAL_JITTER, 1200);
        for _ in 0..200 {
            let interval =
                PATROL_SPAWN_INTERVAL + rand::random_range(0..PATROL_SPAWN_INTERVAL_JITTER);
            assert!((12000..13200).contains(&interval));
        }
    }

    /// PatrolSpawner.java:34-35: each horizontal offset is `(24 + nextInt(24))` with a random
    /// sign, so its magnitude always lands in `[24, 48)`.
    #[test]
    fn patrol_offset_magnitude_matches_vanilla_range() {
        for _ in 0..200 {
            let offset: i32 = (24 + rand::random_range(0..24))
                * if rand::random_range(0..2) == 0 { -1 } else { 1 };
            assert!((24..48).contains(&offset.abs()));
        }
    }

    /// PatrolSpawner.java:40: `ceil(effectiveDifficulty) + 1`. `RegionalDifficulty` clamps the
    /// effective difficulty to `[2.0, 4.0]` for non-peaceful worlds, so a live patrol is 3 to 5
    /// pillagers.
    #[test]
    fn patrol_group_size_matches_vanilla_formula() {
        for (effective, expected) in [(2.0f32, 3), (2.5f32, 4), (3.0f32, 4), (4.0f32, 5)] {
            assert_eq!(effective.ceil() as i32 + 1, expected);
        }
    }
}
