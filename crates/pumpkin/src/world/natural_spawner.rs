use crate::entity::EntityBase;
use crate::entity::passive::tropical_fish::TropicalFishEntity;
use crate::entity::r#type::{
    check_spawn_obstruction, check_spawn_obstruction_state, check_spawn_rules, from_type,
};
use crate::world::World;
use arc_swap::ArcSwap;
use pumpkin_data::biome::Spawner;
use pumpkin_data::chunk::Biome;
use pumpkin_data::entity::{EntityType, MobCategory, SpawnLocation};
use pumpkin_data::tag::Block::MINECRAFT_FIRE;
use pumpkin_data::tag::Block::MINECRAFT_PREVENT_MOB_SPAWNING_INSIDE;
use pumpkin_data::tag::Fluid::{MINECRAFT_LAVA, MINECRAFT_WATER};
use pumpkin_data::tag::Taggable;
use pumpkin_data::tag::WorldgenBiome::MINECRAFT_REDUCE_WATER_AMBIENT_SPAWNS;
use pumpkin_data::{Block, BlockDirection, BlockState};
use pumpkin_util::GameMode;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::get_section_cord;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::xoroshiro128::Xoroshiro;
use pumpkin_util::random::{RandomImpl, get_seed};
use pumpkin_world::chunk::{ChunkData, ChunkHeightmapType};
use pumpkin_world::generation::proto_chunk::GenerationCache;
use rand::seq::IndexedRandom;
use rand::{RngExt, rng};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

const MAGIC_NUMBER: i32 = 17 * 17;

fn initialize_schooling_spawn(
    entity: &Arc<dyn EntityBase>,
    leader: &mut Option<Arc<dyn EntityBase>>,
    group_size: i32,
    record_tracker: bool,
) -> bool {
    let Some(mob) = entity.get_mob() else {
        return false;
    };
    if mob.get_mob_entity().max_school_size() == 0 {
        return false;
    }

    if let Some(leader_entity) = leader.clone() {
        if let (Some(fish), Some(leader_fish)) = (
            entity.cast_any().downcast_ref::<TropicalFishEntity>(),
            leader_entity
                .cast_any()
                .downcast_ref::<TropicalFishEntity>(),
        ) {
            fish.copy_spawn_group_state(leader_fish, record_tracker);
        }
        mob.get_mob_entity()
            .start_schooling_following(leader_entity);
    } else {
        if record_tracker && let Some(fish) = entity.cast_any().downcast_ref::<TropicalFishEntity>()
        {
            fish.record_spawn_group_state();
        }
        if !mob.is_max_group_size_reached(group_size) {
            *leader = Some(entity.clone());
        }
    }

    mob.is_max_group_size_reached(group_size)
}

/// Matches the base `NaturalSpawner.createState` persistence predicate.
///
/// Persistent mobs, passengers, and leashed mobs are deliberately omitted from
/// natural-spawn cap accounting. The vehicle and leash locks are only held for
/// this short read, so this remains synchronous like the surrounding counter
/// updates while matching the base state used by `Mob.checkDespawn`.
fn counts_for_spawn_caps(entity: &dyn EntityBase) -> bool {
    let base_entity = entity.get_entity();
    if base_entity.entity_type.category == &MobCategory::MISC {
        return false;
    }

    let Some(mob) = entity.get_mob() else {
        return true;
    };

    if mob.is_persistence_required() || mob.requires_custom_persistence_cached() {
        return false;
    }

    true
}

use dashmap::{DashMap, DashSet};
use std::sync::atomic::{AtomicI32, Ordering::Relaxed};

pub struct MobCounts([AtomicI32; 8]);

impl Default for MobCounts {
    fn default() -> Self {
        Self(std::array::from_fn(|_| AtomicI32::new(0)))
    }
}

impl fmt::Debug for MobCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|a| a.load(Relaxed)))
            .finish()
    }
}

impl Clone for MobCounts {
    fn clone(&self) -> Self {
        Self(std::array::from_fn(|i| {
            AtomicI32::new(self.0[i].load(Relaxed))
        }))
    }
}

impl MobCounts {
    #[inline]
    pub fn add(&self, category: &'static MobCategory) {
        self.0[category.id].fetch_add(1, Relaxed);
    }

    #[inline]
    pub fn remove(&self, category: &'static MobCategory) {
        self.0[category.id].fetch_sub(1, Relaxed);
    }
    #[inline]
    pub fn can_spawn(&self, category: &'static MobCategory) -> bool {
        self.0[category.id].load(Relaxed) < category.max
    }
}

pub struct LocalMobCapCalculator {
    player_mob_counts: DashMap<i32, MobCounts>,
    players_near_chunk: DashMap<Vector2<i32>, Vec<i32>>,
}

impl Clone for LocalMobCapCalculator {
    fn clone(&self) -> Self {
        let player_mob_counts = DashMap::new();
        for r in &self.player_mob_counts {
            player_mob_counts.insert(*r.key(), r.value().clone());
        }
        let players_near_chunk = DashMap::new();
        for r in &self.players_near_chunk {
            players_near_chunk.insert(*r.key(), r.value().clone());
        }
        Self {
            player_mob_counts,
            players_near_chunk,
        }
    }
}

impl Default for LocalMobCapCalculator {
    fn default() -> Self {
        Self {
            player_mob_counts: DashMap::new(),
            players_near_chunk: DashMap::new(),
        }
    }
}

impl fmt::Debug for LocalMobCapCalculator {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("LocalMobCapCalculator")
            .field("world", &"<skipped>")
            .finish()
    }
}

impl LocalMobCapCalculator {
    const fn calc_distance(chunk_pos: Vector2<i32>, player_pos: &Vector3<f64>) -> f64 {
        let dx = ((chunk_pos.x << 4) + 8) as f64 - player_pos.x;
        let dy = ((chunk_pos.y << 4) + 8) as f64 - player_pos.z;
        dx * dx + dy * dy
    }

    fn get_players_near(&self, world: &World, chunk_pos: Vector2<i32>) -> Vec<i32> {
        if let Some(players) = self.players_near_chunk.get(&chunk_pos) {
            return players.value().clone();
        }

        let mut players = Vec::new();
        for player in world.players.load().iter() {
            if player.gamemode.load() == GameMode::Spectator {
                continue;
            }
            if Self::calc_distance(chunk_pos, &player.position()) < 16384. {
                players.push(player.entity_id());
            }
        }
        self.players_near_chunk.insert(chunk_pos, players.clone());
        players
    }

    pub fn add_mob(&self, chunk_pos: Vector2<i32>, world: &World, category: &'static MobCategory) {
        let players = self.get_players_near(world, chunk_pos);
        for player in players {
            self.player_mob_counts
                .entry(player)
                .or_default()
                .add(category);
        }
    }

    pub fn remove_mob(
        &self,
        chunk_pos: Vector2<i32>,
        world: &World,
        category: &'static MobCategory,
    ) {
        let players = self.get_players_near(world, chunk_pos);
        for player in players {
            if let Some(count) = self.player_mob_counts.get(&player) {
                count.remove(category);
            }
        }
    }

    pub fn can_spawn(
        &self,
        category: &'static MobCategory,
        world: &World,
        chunk_pos: Vector2<i32>,
    ) -> bool {
        let players = self.get_players_near(world, chunk_pos);
        for player in players {
            if let Some(count) = self.player_mob_counts.get(&player) {
                if count.can_spawn(category) {
                    return true;
                }
            } else {
                return true;
            }
        }
        false
    }
}

#[derive(Debug, Clone)]
struct PointCharge(Vector3<f64>, f64);

impl PointCharge {
    fn get_potential_change(&self, pos: &BlockPos) -> f64 {
        let dst = self.0.sub(&pos.to_f64()).length();
        self.1 / dst
    }
}

#[derive(Default, Debug)]
struct PotentialCalculator(std::sync::Mutex<Vec<PointCharge>>);

impl Clone for PotentialCalculator {
    fn clone(&self) -> Self {
        Self(std::sync::Mutex::new(self.0.lock().unwrap().clone()))
    }
}

impl PotentialCalculator {
    pub fn add_charge(&self, pos: &BlockPos, charge: f64) {
        if charge != 0. {
            self.0
                .lock()
                .unwrap()
                .push(PointCharge(pos.to_f64(), charge));
        }
    }

    pub fn remove_charge(&self, pos: &BlockPos, charge: f64) {
        if charge != 0. {
            let mut charges = self.0.lock().unwrap();
            let pos_f64 = pos.to_f64();
            if let Some(idx) = charges.iter().position(|c| c.0 == pos_f64 && c.1 == charge) {
                charges.swap_remove(idx);
            }
        }
    }
    pub fn get_potential_energy_change(&self, pos: &BlockPos, charge: f64) -> f64 {
        if charge == 0. {
            return 0.;
        }
        let mut sum: f64 = 0.;
        let charges = self.0.lock().unwrap();
        for i in charges.iter() {
            sum += i.get_potential_change(pos);
        }
        sum * charge
    }
}

use crossbeam::atomic::AtomicCell;

pub struct SpawnState {
    spawnable_chunk_count: i32,
    pub mob_category_counts: MobCounts,
    spawn_potential: PotentialCalculator,
    local_mob_cap_calculator: LocalMobCapCalculator,
    counted_entities: DashSet<Uuid>,
    // unmodifiable_mob_category_counts: MobCounts, seems only for debug
    last_checked: AtomicCell<Option<(BlockPos, &'static EntityType, f64)>>,
}

impl Clone for SpawnState {
    fn clone(&self) -> Self {
        Self {
            spawnable_chunk_count: self.spawnable_chunk_count,
            mob_category_counts: self.mob_category_counts.clone(),
            spawn_potential: self.spawn_potential.clone(),
            local_mob_cap_calculator: self.local_mob_cap_calculator.clone(),
            counted_entities: self.counted_entities.clone(),
            last_checked: AtomicCell::new(self.last_checked.load()),
        }
    }
}

impl fmt::Debug for SpawnState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SpawnState")
            .field("spawnable_chunk_count", &self.spawnable_chunk_count)
            .field("mob_category_counts", &self.mob_category_counts)
            .field("spawn_potential", &self.spawn_potential)
            .field("local_mob_cap_calculator", &self.local_mob_cap_calculator)
            .field("counted_entities", &self.counted_entities)
            .field("last_checked", &self.last_checked)
            .finish()
    }
}

impl SpawnState {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            spawnable_chunk_count: 0,
            mob_category_counts: MobCounts::default(),
            spawn_potential: PotentialCalculator::default(),
            local_mob_cap_calculator: LocalMobCapCalculator::default(),
            counted_entities: DashSet::new(),
            last_checked: AtomicCell::new(None),
        }
    }

    pub const fn set_spawnable_chunk_count(&mut self, count: i32) {
        self.spawnable_chunk_count = count;
    }

    pub fn add_entity(&self, world: &World, entity: &dyn EntityBase) {
        if !counts_for_spawn_caps(entity) {
            return;
        }
        let base_entity = entity.get_entity();
        if !world
            .active_chunks
            .load()
            .contains(&base_entity.chunk_pos.load())
        {
            return;
        }
        if !self.counted_entities.insert(base_entity.entity_uuid) {
            return;
        }
        let entity_type = base_entity.entity_type;
        let entity_pos = base_entity.block_pos.load();
        let biome = base_entity.current_biome.load();
        if let Some(cost) = biome.spawn_costs.get(entity_type.resource_name) {
            self.spawn_potential.add_charge(&entity_pos, cost.charge);
        }
        if entity_type.mob {
            self.local_mob_cap_calculator.add_mob(
                base_entity.chunk_pos.load(),
                world,
                entity_type.category,
            );
        }
        self.mob_category_counts.add(entity_type.category);
    }

    pub fn remove_entity(&self, world: &World, entity: &dyn EntityBase) {
        let base_entity = entity.get_entity();
        if self
            .counted_entities
            .remove(&base_entity.entity_uuid)
            .is_none()
        {
            return;
        }
        let entity_type = base_entity.entity_type;
        let entity_pos = base_entity.block_pos.load();
        let biome = base_entity.current_biome.load();
        if let Some(cost) = biome.spawn_costs.get(entity_type.resource_name) {
            self.spawn_potential.remove_charge(&entity_pos, cost.charge);
        }
        if entity_type.mob {
            self.local_mob_cap_calculator.remove_mob(
                base_entity.chunk_pos.load(),
                world,
                entity_type.category,
            );
        }
        self.mob_category_counts.remove(entity_type.category);
    }

    pub fn new(
        chunk_count: i32,
        entities: &ArcSwap<Vec<Arc<dyn EntityBase>>>,
        world: &Arc<World>,
    ) -> Self {
        let potential = PotentialCalculator::default();
        let local_mob_cap = LocalMobCapCalculator::default();
        let counter = MobCounts::default();
        let counted_entities = DashSet::new();
        let active_chunks = world.active_chunks.load();
        for entity in entities.load().iter() {
            if !counts_for_spawn_caps(entity.as_ref()) {
                continue;
            }
            let entity = entity.get_entity();
            let entity_type = entity.entity_type;
            let chunk_pos = entity.chunk_pos.load();
            if !active_chunks.contains(&chunk_pos) {
                continue;
            }
            counted_entities.insert(entity.entity_uuid);
            let entity_pos = entity.block_pos.load();
            let biome = entity.current_biome.load();
            if let Some(cost) = biome.spawn_costs.get(entity_type.resource_name) {
                potential.add_charge(&entity_pos, cost.charge);
            }
            if entity_type.mob {
                local_mob_cap.add_mob(chunk_pos, world, entity_type.category);
            }
            counter.add(entity_type.category);
        }
        Self {
            spawnable_chunk_count: chunk_count,
            mob_category_counts: counter,
            spawn_potential: potential,
            local_mob_cap_calculator: local_mob_cap,
            counted_entities,
            last_checked: AtomicCell::new(None),
        }
    }
    #[inline]
    pub fn can_spawn_for_category_global(&self, category: &'static MobCategory) -> bool {
        self.mob_category_counts.0[category.id].load(Relaxed)
            < category.max * self.spawnable_chunk_count / MAGIC_NUMBER
    }
    pub fn can_spawn_for_category_local(
        &self,
        world: &Arc<World>,
        category: &'static MobCategory,
        chunk_pos: Vector2<i32>,
    ) -> bool {
        self.local_mob_cap_calculator
            .can_spawn(category, world, chunk_pos)
    }
    pub fn can_spawn(
        &self,
        entity_type: &'static EntityType,
        pos: &BlockPos,
        world: &Arc<World>,
    ) -> bool {
        // TODO get biome
        // No resolvable biome means no spawn table applies here, so refuse the spawn
        // rather than fall back to some other biome's rules.
        let Some(biome) = world.level.get_rough_biome(pos) else {
            return false;
        };
        biome
            .spawn_costs
            .get(entity_type.resource_name)
            .map_or_else(
                || {
                    self.last_checked.store(Some((*pos, entity_type, 0.)));
                    true
                },
                |cost| {
                    self.last_checked
                        .store(Some((*pos, entity_type, cost.charge)));
                    self.spawn_potential
                        .get_potential_energy_change(pos, cost.charge)
                        <= cost.energy_budget
                },
            )
    }
    pub fn after_spawn(
        &self,
        entity_type: &'static EntityType,
        pos: &BlockPos,
        entity_uuid: Uuid,
        world: &Arc<World>,
    ) {
        let charge = if let Some((l_pos, l_type, l_charge)) = self.last_checked.load()
            && l_pos.eq(pos)
            && l_type == entity_type
        {
            Some(l_charge)
        } else {
            None
        };

        let charge = charge.unwrap_or_else(|| {
            // TODO get biome
            // Charge 0.0 is the same value this path already yields when the biome has
            // no spawn cost entry for this type, so it is not an invented biome; it is
            // the "no cost known" case. The mob-cap accounting below still runs.
            world
                .level
                .get_rough_biome(pos)
                .and_then(|biome| biome.spawn_costs.get(entity_type.resource_name))
                .map_or(0., |cost| cost.charge)
        });

        if !self.counted_entities.insert(entity_uuid) {
            return;
        }
        self.spawn_potential.add_charge(pos, charge);
        self.mob_category_counts.add(entity_type.category);
        self.local_mob_cap_calculator.add_mob(
            Vector2::<i32>::new(get_section_cord(pos.0.x), get_section_cord(pos.0.z)),
            world,
            entity_type.category,
        );
    }
}

#[must_use]
pub fn get_filtered_spawning_categories(
    state: &SpawnState,
    spawn_friendlies: bool,
    spawn_enemies: bool,
    spawn_passives: bool,
) -> Vec<&'static MobCategory> {
    let mut ret = Vec::with_capacity(MobCategory::SPAWNING_CATEGORIES.len());
    for category in MobCategory::SPAWNING_CATEGORIES {
        let is_type_allowed = if category.is_friendly {
            spawn_friendlies
        } else {
            spawn_enemies
        };

        if !is_type_allowed {
            continue;
        }

        if category.is_persistent && !spawn_passives {
            continue;
        }

        if state.can_spawn_for_category_global(category) {
            ret.push(category);
        }
    }
    ret
}

pub fn spawn_for_chunk(
    world: &Arc<World>,
    chunk_pos: Vector2<i32>,
    chunk: &Arc<ChunkData>,
    spawn_state: &SpawnState,
    spawn_list: &Vec<&'static MobCategory>,
    is_thundering: bool,
) -> Vec<Arc<dyn EntityBase>> {
    // debug!("spawn for chunk {:?}", chunk_pos);
    let mut entities = Vec::new();
    for category in spawn_list {
        if spawn_state.can_spawn_for_category_local(world, category, chunk_pos) {
            let random_pos = get_random_pos_within(world.min_y, &chunk_pos, chunk);
            if random_pos.0.y > world.min_y {
                entities.extend(spawn_category_for_position(
                    category,
                    world,
                    random_pos,
                    &chunk_pos,
                    spawn_state,
                    is_thundering,
                ));
            }
        }
    }
    entities
}
pub fn get_random_pos_within(
    min_y: i32,
    chunk_pos: &Vector2<i32>,
    chunk: &Arc<ChunkData>,
) -> BlockPos {
    let mut rng = Xoroshiro::from_seed(get_seed());

    let x = (chunk_pos.x << 4) + rng.next_bounded_i32(16);
    let z = (chunk_pos.y << 4) + rng.next_bounded_i32(16);
    let temp_y = chunk.heightmap.lock().unwrap().get(
        ChunkHeightmapType::WorldSurface,
        x,
        z,
        chunk.section.min_y,
    ) + 1;
    let y = rng.next_inbetween_i32(min_y, temp_y);
    BlockPos::new(x, y, z)
}

pub fn spawn_mobs_for_chunk_generation(
    world: &Arc<World>,
    cache: &mut dyn GenerationCache,
    biome: &'static Biome,
    chunk_x: i32,
    chunk_z: i32,
) {
    if !world.level_info.load().game_rules.spawn_mobs {
        return;
    }

    let mob_settings = &biome.spawners;
    let creatures = &mob_settings.creature;

    if creatures.is_empty() {
        return;
    }

    let xo = chunk_x << 4;
    let zo = chunk_z << 4;

    while rand::random::<f32>() < biome.creature_spawn_probability {
        let Ok(spawner_data) = creatures.choose_weighted(&mut rand::rng(), |s| s.weight) else {
            continue;
        };

        let count = spawner_data.min_count
            + rand::random_range(0..(1 + spawner_data.max_count - spawner_data.min_count).max(1));
        let entity_type = EntityType::from_name(
            spawner_data
                .r#type
                .strip_prefix("minecraft:")
                .unwrap_or(spawner_data.r#type),
        )
        .unwrap();

        let mut x = xo + rand::random_range(0..16);
        let mut z = zo + rand::random_range(0..16);
        let start_x = x;
        let start_z = z;
        let mut schooling_leader: Option<Arc<dyn EntityBase>> = None;
        let mut group_size = 0;

        for _ in 0..count {
            let mut success = false;

            // Try 4 times to find a valid spot in the immediate area
            for _ in 0..4 {
                if success {
                    break;
                }

                let pos = get_top_non_colliding_pos(world, cache, entity_type, x, z);
                let width = f64::from(entity_type.dimension[0]);
                let spawn_x = f64::from(x).clamp(f64::from(xo) + width, f64::from(xo + 16) - width);
                let spawn_z = f64::from(z).clamp(f64::from(zo) + width, f64::from(zo + 16) - width);
                let spawn_rule_pos =
                    BlockPos::new(spawn_x.floor() as i32, pos.0.y, spawn_z.floor() as i32);

                if entity_type.summonable
                    && is_spawn_position_ok_cache(cache, &pos, entity_type)
                    // Vanilla checks the exact clamped spawn AABB before calling the
                    // random CHUNK_GENERATION predicate. Keep this order so rejected
                    // candidates do not consume the ocelot predicate's random draw.
                    && is_space_empty_cache(
                        cache,
                        world,
                        spawn_x,
                        f64::from(pos.0.y),
                        spawn_z,
                        entity_type,
                    )
                    && check_spawn_rules(entity_type, world, &spawn_rule_pos, false)
                    && check_spawn_obstruction_state(
                        spawn_rule_pos.0.y,
                        world.sea_level,
                        GenerationCache::get_block_state(cache, &spawn_rule_pos.down().0).to_state(),
                        contains_any_liquid_cache(
                            cache,
                            spawn_x,
                            f64::from(pos.0.y),
                            spawn_z,
                            entity_type,
                        ),
                        false,
                        entity_type,
                    )
                {
                    let spawn_pos_f64 = Vector3::new(spawn_x, f64::from(pos.0.y), spawn_z);

                    let entity = from_type(entity_type, spawn_pos_f64, world, Uuid::new_v4());
                    entity
                        .get_entity()
                        .set_rotation(rand::random::<f32>() * 360., 0.);
                    group_size += 1;
                    initialize_schooling_spawn(&entity, &mut schooling_leader, group_size, true);
                    world.spawn_entity_non_save(&entity);
                    success = true;
                }

                // Random jitter for the next mob in the group
                x += rand::random_range(0..5) - rand::random_range(0..5);
                z += rand::random_range(0..5) - rand::random_range(0..5);

                // Keep group within the chunk bounds
                if x < xo || x >= xo + 16 || z < zo || z >= zo + 16 {
                    x = start_x;
                    z = start_z;
                }
            }
        }
    }
}

pub fn get_top_non_colliding_pos(
    world: &World,
    cache: &dyn GenerationCache,
    entity_type: &'static EntityType,
    x: i32,
    z: i32,
) -> BlockPos {
    let mut y = cache.get_top_y(&entity_type.spawn_restriction.heightmap, x, z);
    let mut pos_vec = Vector3::new(x, y, z);
    let min_y = world.min_y;

    if world.dimension.has_ceiling {
        loop {
            y -= 1;
            pos_vec.y = y;
            // Use UFCS to avoid the ambiguity error from earlier
            if GenerationCache::get_block_state(cache, &pos_vec)
                .to_state()
                .is_air()
                || y <= min_y
            {
                break;
            }
        }

        loop {
            y -= 1;
            pos_vec.y = y;
            if !GenerationCache::get_block_state(cache, &pos_vec)
                .to_state()
                .is_air()
                || y <= min_y
            {
                break;
            }
        }
    }

    let pos = BlockPos::new(x, y, z);

    adjust_spawn_position_cache(cache, pos, entity_type)
}

pub fn spawn_category_for_position(
    category: &'static MobCategory,
    world: &Arc<World>,
    pos: BlockPos,
    chunk_pos: &Vector2<i32>,
    spawn_state: &SpawnState,
    is_thundering: bool,
) -> Vec<Arc<dyn EntityBase>> {
    if world.get_block_state(&pos).is_solid_block() {
        return Vec::new();
    }

    let mut batch_buffer = vec![];
    let mut spawn_cluster_size = 0;
    let player_positions: Vec<_> = world
        .players
        .load()
        .iter()
        .filter(|player| counts_for_natural_spawning(player.gamemode.load()))
        .map(|p| p.position())
        .collect();
    // Vanilla's getNearestPlayer(..., false) returns null when only spectators are
    // online, and NaturalSpawner skips the attempt in that case. Do not let the
    // f64::MAX sentinel in get_nearest_player turn that into an eligible spawn.
    if player_positions.is_empty() {
        return batch_buffer;
    }
    let level_info = world.level_info.load();
    let spawn_position = Vector3::new(level_info.spawn_x, level_info.spawn_y, level_info.spawn_z);

    'group_loop: for _ in 0..3 {
        let mut new_x = pos.0.x;
        let mut new_z = pos.0.z;

        let mut random_group_size = (rng().random::<f32>() * 4.).ceil() as i32;
        let mut inc = 0;
        let mut current_spawner = None;
        let mut schooling_leader: Option<Arc<dyn EntityBase>> = None;
        let mut group_size = 0;

        'spawn_loop: while inc < random_group_size {
            new_x += rng().random_range(0..6) - rng().random_range(0..6);
            new_z += rng().random_range(0..6) - rng().random_range(0..6);
            let mut new_pos = BlockPos::new(new_x, pos.0.y, new_z);

            if current_spawner.is_none() {
                let Some(spawner) = get_random_spawn_mob_at(world, category, &new_pos) else {
                    break 'spawn_loop;
                };
                current_spawner = Some(spawner);
                random_group_size = rng().random_range(spawner.min_count..=spawner.max_count);
            }

            let spawner = current_spawner.unwrap();
            let entity_type =
                &EntityType::from_name(spawner.r#type.strip_prefix("minecraft:").unwrap()).unwrap();

            if !is_spawner_allowed_at(world, category, &new_pos, spawner) {
                inc += 1;
                continue;
            }

            new_pos = adjust_spawn_position(world, new_pos, entity_type);

            let spawn_pos_f64 = Vector3::new(
                f64::from(new_pos.0.x) + 0.5,
                f64::from(new_pos.0.y),
                f64::from(new_pos.0.z) + 0.5,
            );

            let player_distance = get_nearest_player(&spawn_pos_f64, &player_positions);
            if !is_right_distance_to_player_and_spawn_point(
                &new_pos,
                player_distance,
                chunk_pos,
                &spawn_position,
            ) {
                inc += 1;
                continue;
            }

            if !is_valid_spawn_position_for_type(
                world,
                &new_pos,
                category,
                entity_type,
                player_distance,
                is_thundering,
            ) {
                inc += 1;
                continue;
            }
            if !spawn_state.can_spawn(entity_type, &new_pos, world) {
                inc += 1;
                continue;
            }

            let entity = from_type(entity_type, spawn_pos_f64, world, Uuid::new_v4());
            entity
                .get_entity()
                .set_rotation(rng().random::<f32>() * 360., 0.);
            let entity_uuid = entity.get_entity().entity_uuid;

            spawn_cluster_size += 1;
            group_size += 1;
            let group_ended =
                initialize_schooling_spawn(&entity, &mut schooling_leader, group_size, false);
            batch_buffer.push(entity);
            spawn_state.after_spawn(entity_type, &new_pos, entity_uuid, world);
            if spawn_cluster_size >= entity_type.limit_per_chunk {
                break 'group_loop;
            }

            if group_ended {
                break 'spawn_loop;
            }

            inc += 1;
        }
    }
    batch_buffer
}

#[must_use]
pub fn get_nearest_player(pos: &Vector3<f64>, player_positions: &[Vector3<f64>]) -> f64 {
    let mut min_dst_sq = f64::MAX;

    for player_pos in player_positions {
        let cur_dst_sq = player_pos.squared_distance_to_vec(pos);
        if cur_dst_sq < min_dst_sq {
            min_dst_sq = cur_dst_sq;
        }
    }
    min_dst_sq
}

#[inline]
const fn counts_for_natural_spawning(gamemode: GameMode) -> bool {
    !matches!(gamemode, GameMode::Spectator)
}

#[must_use]
pub fn is_right_distance_to_player_and_spawn_point(
    pos: &BlockPos,
    distance: f64,
    chunk_pos: &Vector2<i32>,
    spawn_position: &Vector3<i32>,
) -> bool {
    if distance <= 24. * 24. {
        return false;
    }
    if pos.to_centered_f64().squared_distance_to(
        f64::from(spawn_position.x) + 0.5,
        f64::from(spawn_position.y) + 0.5,
        f64::from(spawn_position.z) + 0.5,
    ) <= 24. * 24.
    {
        return false;
    }
    #[expect(clippy::nonminimal_bool)]
    {
        chunk_pos == &Vector2::new(get_section_cord(pos.0.x), get_section_cord(pos.0.z)) || false // TODO canSpawnEntitiesInChunk(ChunkPos chunkPos)
    }
}

#[must_use]
pub fn get_random_spawn_mob_at(
    world: &Arc<World>,
    category: &'static MobCategory,
    block_pos: &BlockPos,
) -> Option<&'static Spawner> {
    // TODO Holder<Biome> holder = level.getBiome(pos);
    // Without a resolvable biome there is no spawn list to draw from; picking one
    // would spawn that biome's mobs in a dimension that never lists them.
    let biome = world.level.get_rough_biome(block_pos)?;
    if category == &MobCategory::WATER_AMBIENT
        && biome.has_tag(&MINECRAFT_REDUCE_WATER_AMBIENT_SPAWNS)
        && rng().random::<f32>() < 0.98f32
    {
        None
    } else {
        // TODO isInNetherFortressBounds(pos, level, cetagory, structureManager) then NetherFortressStructure.FORTRESS_ENEMIES
        // TODO structureManager.getAllStructuresAt(pos); ChunkGenerator::getMobsAt
        spawners_for_category(biome, category)
            .choose_weighted(&mut rng(), |s| s.weight)
            .ok()
    }
}

fn spawners_for_category(
    biome: &'static Biome,
    category: &'static MobCategory,
) -> &'static [Spawner] {
    match category.id {
        id if id == MobCategory::MONSTER.id => biome.spawners.monster,
        id if id == MobCategory::CREATURE.id => biome.spawners.creature,
        id if id == MobCategory::AMBIENT.id => biome.spawners.ambient,
        id if id == MobCategory::AXOLOTLS.id => biome.spawners.axolotls,
        id if id == MobCategory::UNDERGROUND_WATER_CREATURE.id => {
            biome.spawners.underground_water_creature
        }
        id if id == MobCategory::WATER_CREATURE.id => biome.spawners.water_creature,
        id if id == MobCategory::WATER_AMBIENT.id => biome.spawners.water_ambient,
        id if id == MobCategory::MISC.id => biome.spawners.misc,
        _ => panic!(),
    }
}

fn same_spawner(left: &Spawner, right: &Spawner) -> bool {
    left.r#type == right.r#type
        && left.min_count == right.min_count
        && left.max_count == right.max_count
        && left.weight == right.weight
}

/// Mirrors NaturalSpawner.canSpawnMobAt: the selected biome entry must still
/// be present at every jittered candidate position.
fn is_spawner_allowed_at(
    world: &Arc<World>,
    category: &'static MobCategory,
    block_pos: &BlockPos,
    expected: &Spawner,
) -> bool {
    world
        .level
        .get_rough_biome(block_pos)
        .is_some_and(|biome| is_spawner_allowed_in_biome(biome, category, expected))
}

fn is_spawner_allowed_in_biome(
    biome: &'static Biome,
    category: &'static MobCategory,
    expected: &Spawner,
) -> bool {
    spawners_for_category(biome, category)
        .iter()
        .any(|candidate| same_spawner(candidate, expected))
}

pub fn is_valid_spawn_position_for_type(
    world: &Arc<World>,
    block_pos: &BlockPos,
    category: &'static MobCategory,
    entity_type: &'static EntityType,
    distance: f64,
    is_thundering: bool,
) -> bool {
    // TODO !SpawnPlacements.checkSpawnRules(entityType, level, EntitySpawnReason.NATURAL, pos, level.random)
    if category == &MobCategory::MISC {
        return false;
    }
    if !entity_type.can_spawn_far_from_player
        && distance
            > f64::from(entity_type.category.despawn_distance)
                * f64::from(entity_type.category.despawn_distance)
    {
        return false;
    }
    if !entity_type.summonable {
        return false;
    }
    if !is_spawn_position_ok(world, block_pos, entity_type) {
        return false;
    }
    if !check_spawn_rules(entity_type, world, block_pos, is_thundering) {
        return false;
    }
    if !check_spawn_obstruction(world, block_pos, entity_type) {
        return false;
    }
    // NaturalSpawner checks the complete spawn AABB against both blocks and
    // collidable entities before creating the mob. The block-only check used
    // to allow mobs to spawn inside boats, minecarts, and other mobs.
    let spawn_box = BoundingBox::new_from_pos(
        f64::from(block_pos.0.x) + 0.5,
        f64::from(block_pos.0.y),
        f64::from(block_pos.0.z) + 0.5,
        &spawn_dimensions(entity_type),
    );
    if !world.is_space_empty(spawn_box)
        || world
            .get_all_at_box(&spawn_box.expand_all(1.0e-7))
            .iter()
            .any(|entity| !entity.is_spectator() && entity.can_be_collided_with())
    {
        return false;
    }
    true
}

/// Returns the dimensions used by vanilla `EntityType.getSpawnAABB`.
///
/// The entity registry stores base dimensions, while the Java entity type
/// applies `spawnDimensionsScale` only when checking a natural-spawn box.
/// Slimes and magma cubes use 4.0, and sulfur cubes use 2.0 in 26.2.
fn spawn_dimensions(entity_type: &'static EntityType) -> EntityDimensions {
    let scale = match entity_type.resource_name {
        "magma_cube" | "slime" => 4.0,
        "sulfur_cube" => 2.0,
        _ => 1.0,
    };

    EntityDimensions {
        width: entity_type.dimension[0] * scale,
        height: entity_type.dimension[1] * scale,
        // `getSpawnAABB` does not use eye height, but preserving it keeps this
        // value suitable for callers that carry the complete dimensions.
        eye_height: entity_type.eye_height,
    }
}

pub fn is_spawn_position_ok(
    world: &Arc<World>,
    block_pos: &BlockPos,
    entity_type: &'static EntityType,
) -> bool {
    match entity_type.spawn_restriction.location {
        SpawnLocation::InLava => world.get_fluid(block_pos).has_tag(&MINECRAFT_LAVA),
        SpawnLocation::InWater => {
            let above_state = world.get_block_state(&block_pos.up());
            can_spawn_in_water(
                world.get_fluid(block_pos).has_tag(&MINECRAFT_WATER),
                above_state,
            )
        }
        SpawnLocation::OnGround => {
            let down = world.get_block_state(&block_pos.down());
            let up = world.get_block_state(&block_pos.up());
            let cur = world.get_block_state(block_pos);
            // TODO: blockState.allowsSpawning
            let is_valid_spawn_below = is_valid_spawn_support(down, entity_type);

            if is_valid_spawn_below {
                is_valid_empty_spawn_block(cur, entity_type)
                    && is_valid_empty_spawn_block(up, entity_type)
            } else {
                false
            }
        }
        SpawnLocation::Unrestricted => true,
    }
}

#[must_use]
const fn can_spawn_in_water(fluid_is_water: bool, above_state: &'static BlockState) -> bool {
    fluid_is_water && !above_state.is_solid_block()
}

/// Cache-based version of `is_spawn_position_ok` used during world generation.
pub fn is_spawn_position_ok_cache(
    cache: &dyn GenerationCache,
    block_pos: &BlockPos,
    entity_type: &'static EntityType,
) -> bool {
    let pos_vec = block_pos.0;
    let state = GenerationCache::get_block_state(cache, &pos_vec).to_state();

    match entity_type.spawn_restriction.location {
        SpawnLocation::InLava => {
            // During generation, we check the block state's liquid property and tag
            state.is_liquid() && Block::from_state_id(state.id).has_tag(&MINECRAFT_LAVA)
        }
        SpawnLocation::InWater => {
            let above_pos = block_pos.up().0;
            let above_state = GenerationCache::get_block_state(cache, &above_pos).to_state();

            can_spawn_in_water(
                state.is_liquid() && Block::from_state_id(state.id).has_tag(&MINECRAFT_WATER),
                above_state,
            )
        }
        SpawnLocation::OnGround => {
            let down_pos = block_pos.down().0;
            let up_pos = block_pos.up().0;

            let down = GenerationCache::get_block_state(cache, &down_pos).to_state();
            let up = GenerationCache::get_block_state(cache, &up_pos).to_state();

            // Logic: solid surface below and low enough light level (if applicable in generation)
            let is_valid_spawn_below = is_valid_spawn_support(down, entity_type);

            if is_valid_spawn_below {
                is_valid_empty_spawn_block(state, entity_type)
                    && is_valid_empty_spawn_block(up, entity_type)
            } else {
                false
            }
        }
        SpawnLocation::Unrestricted => true,
    }
}

/// Cache equivalent of the natural spawner's `noCollision` check. Generation
/// runs before the staged chunk is installed in the live world, so this must
/// inspect the generation cache rather than `World`.
fn is_space_empty_cache(
    cache: &dyn GenerationCache,
    world: &World,
    x: f64,
    y: f64,
    z: f64,
    entity_type: &'static EntityType,
) -> bool {
    let bounding_box = BoundingBox::new_from_pos(x, y, z, &spawn_dimensions(entity_type));

    if world
        .get_all_at_box(&bounding_box.expand_all(1.0e-7))
        .iter()
        .any(|entity| !entity.is_spectator() && entity.can_be_collided_with())
    {
        return false;
    }

    for block_pos in BlockPos::iterate(bounding_box.min_block_pos(), bounding_box.max_block_pos()) {
        let state = GenerationCache::get_block_state(cache, &block_pos.0).to_state();
        if state
            .get_block_collision_shapes()
            .map(|shape| shape.at_pos(block_pos))
            .any(|shape| shape.intersects(&bounding_box))
        {
            return false;
        }
    }

    true
}

fn contains_any_liquid_cache(
    cache: &dyn GenerationCache,
    x: f64,
    y: f64,
    z: f64,
    entity_type: &'static EntityType,
) -> bool {
    let bounding_box = BoundingBox::new_from_pos(x, y, z, &spawn_dimensions(entity_type));

    for block_x in bounding_box.min.x.floor() as i32..bounding_box.max.x.ceil() as i32 {
        for block_y in bounding_box.min.y.floor() as i32..bounding_box.max.y.ceil() as i32 {
            for block_z in bounding_box.min.z.floor() as i32..bounding_box.max.z.ceil() as i32 {
                let block_pos = BlockPos::new(block_x, block_y, block_z);
                if !cache.get_fluid_and_fluid_state(&block_pos.0).1.is_empty {
                    return true;
                }
            }
        }
    }

    false
}

/// Cache-based version of `adjust_spawn_position` used during world generation.
pub fn adjust_spawn_position_cache(
    cache: &dyn GenerationCache,
    pos: BlockPos,
    entity_type: &'static EntityType,
) -> BlockPos {
    if matches!(
        entity_type.spawn_restriction.location,
        SpawnLocation::OnGround
    ) {
        let below = pos.down();
        let state = GenerationCache::get_block_state(cache, &below.0).to_state();

        if !state.is_full_cube() && !state.is_liquid() {
            return below;
        }
    }
    pos
}

pub fn adjust_spawn_position(
    world: &World,
    pos: BlockPos,
    entity_type: &'static EntityType,
) -> BlockPos {
    if matches!(
        entity_type.spawn_restriction.location,
        SpawnLocation::OnGround
    ) {
        let below = pos.down();
        let state = world.get_block_state(&below);
        // Approximation of isPathfindable(LAND)
        if !state.is_full_cube() && !state.is_liquid() {
            return below;
        }
    }
    pos
}

#[must_use]
pub fn is_valid_empty_spawn_block(
    state: &'static BlockState,
    entity_type: &'static EntityType,
) -> bool {
    if state.is_full_cube() {
        return false;
    }
    if state.is_signal_source() {
        return false;
    }
    if state.is_liquid() {
        return false;
    }
    if Block::from_state_id(state.id).has_tag(&MINECRAFT_PREVENT_MOB_SPAWNING_INSIDE) {
        return false;
    }
    let block = Block::from_state_id(state.id);
    if block == &Block::WITHER_ROSE
        || block == &Block::SWEET_BERRY_BUSH
        || block == &Block::CACTUS
        || block == &Block::POWDER_SNOW
    {
        return false;
    }

    entity_type.fire_immune || !block.has_tag(&MINECRAFT_FIRE)
}

/// Matches the support-block portion of vanilla `Mob.checkMobSpawnRules`.
/// Magma blocks override `Block.isValidSpawn` and only permit fire-immune
/// entities; a generic sturdy top-face check is not sufficient here.
fn is_valid_spawn_support(state: &'static BlockState, entity_type: &'static EntityType) -> bool {
    state.is_side_solid(BlockDirection::Up)
        && state.luminance < 14
        && (entity_type.fire_immune || Block::from_state_id(state.id) != &Block::MAGMA_BLOCK)
}

#[cfg(test)]
mod tests {
    use super::{
        IndexedRandom, can_spawn_in_water, counts_for_natural_spawning,
        is_right_distance_to_player_and_spawn_point, is_spawner_allowed_in_biome,
        is_valid_empty_spawn_block, is_valid_spawn_support, spawn_dimensions,
    };
    use pumpkin_data::Block;
    use pumpkin_data::biome::{Biome, Spawner};
    use pumpkin_data::entity::EntityType;
    use pumpkin_util::GameMode;
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_util::math::vector2::Vector2;
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn spectators_do_not_count_as_nearby_players_for_spawning() {
        assert!(!counts_for_natural_spawning(GameMode::Spectator));
        assert!(counts_for_natural_spawning(GameMode::Survival));
        assert!(counts_for_natural_spawning(GameMode::Adventure));
        assert!(counts_for_natural_spawning(GameMode::Creative));
    }

    #[test]
    fn magma_support_requires_fire_immunity() {
        let magma = &Block::MAGMA_BLOCK.default_state;

        assert!(!is_valid_spawn_support(magma, &EntityType::ZOMBIE));
        assert!(is_valid_spawn_support(magma, &EntityType::BLAZE));
    }

    /// Vanilla `WeightedList` picks entries proportionally to `weight` (e.g. `warm_ocean`'s
    /// `water_creature` list: nautilus weight 5, squid weight 10, dolphin weight 2 - see
    /// `assets/biome.json`). Selection must not degrade to a uniform pick across entries,
    /// which would bias squid's real ~59% share down to an even 1/3.
    #[test]
    fn spawner_selection_is_weighted_not_uniform() {
        let spawners = [
            Spawner {
                r#type: "minecraft:heavy",
                min_count: 1,
                max_count: 1,
                weight: 1000,
            },
            Spawner {
                r#type: "minecraft:light",
                min_count: 1,
                max_count: 1,
                weight: 1,
            },
        ];

        let mut heavy_picks = 0;
        for _ in 0..200 {
            if let Ok(pick) = spawners.choose_weighted(&mut rand::rng(), |s| s.weight)
                && pick.r#type == "minecraft:heavy"
            {
                heavy_picks += 1;
            }
        }

        // With a 1000:1 weight ratio the light entry should be picked essentially never;
        // a uniform `.choose()` would instead land on it roughly half the time.
        assert!(
            heavy_picks >= 190,
            "expected the heavily-weighted entry to dominate selection, got {heavy_picks}/200"
        );
    }

    /// Every `Spawner::r#type` entry in every biome's spawn tables must resolve
    /// through `EntityType::from_name`. `spawn_category_for_position` and
    /// `spawn_mobs_for_chunk_generation` both do
    /// `EntityType::from_name(spawner.r#type.strip_prefix("minecraft:").unwrap()).unwrap()`
    /// with no fallback, so an unresolvable name panics the chunk-tick task that
    /// runs natural spawning (see `World::tick`'s `chunk_tasks.spawn`, whose only
    /// handling is logging the panic) for every category attempted in that tick,
    /// not just the offending one. Vanilla's equivalent, `getMobForSpawn` in
    /// `NaturalSpawner.java`, explicitly catches the failure and returns `null`
    /// instead of propagating.
    #[test]
    fn all_biome_spawner_entries_resolve_to_known_entity_types() {
        let mut missing = Vec::new();
        for biome in Biome::ALL {
            let groups = [
                ("monster", biome.spawners.monster),
                ("ambient", biome.spawners.ambient),
                ("axolotls", biome.spawners.axolotls),
                ("creature", biome.spawners.creature),
                ("misc", biome.spawners.misc),
                (
                    "underground_water_creature",
                    biome.spawners.underground_water_creature,
                ),
                ("water_ambient", biome.spawners.water_ambient),
                ("water_creature", biome.spawners.water_creature),
            ];
            for (category, entries) in groups {
                for entry in entries {
                    let name = entry
                        .r#type
                        .strip_prefix("minecraft:")
                        .unwrap_or(entry.r#type);
                    if EntityType::from_name(name).is_none() {
                        missing.push(format!(
                            "{}/{category}: {}",
                            biome.registry_id, entry.r#type
                        ));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "biome spawn tables reference unknown entity types (would panic the \
             natural-spawn tick task): {missing:#?}"
        );
    }

    #[test]
    fn dangerous_blocks_reject_nonimmune_spawns() {
        assert!(!is_valid_empty_spawn_block(
            Block::SWEET_BERRY_BUSH.default_state,
            &EntityType::ZOMBIE,
        ));
        assert!(!is_valid_empty_spawn_block(
            Block::FIRE.default_state,
            &EntityType::ZOMBIE,
        ));
        assert!(is_valid_empty_spawn_block(
            Block::FIRE.default_state,
            &EntityType::BLAZE,
        ));
    }

    #[test]
    fn signal_sources_reject_empty_spawn_positions() {
        assert!(!is_valid_empty_spawn_block(
            Block::REDSTONE_TORCH.default_state,
            &EntityType::ZOMBIE,
        ));
        assert!(!is_valid_empty_spawn_block(
            Block::LEVER.default_state,
            &EntityType::ZOMBIE,
        ));
        assert!(!is_valid_empty_spawn_block(
            Block::OAK_BUTTON.default_state,
            &EntityType::ZOMBIE,
        ));
        assert!(!is_valid_empty_spawn_block(
            Block::REDSTONE_BLOCK.default_state,
            &EntityType::ZOMBIE,
        ));
        assert!(!is_valid_empty_spawn_block(
            Block::EXPOSED_LIGHTNING_ROD.default_state,
            &EntityType::ZOMBIE,
        ));
        assert!(!is_valid_empty_spawn_block(
            Block::WAXED_OXIDIZED_LIGHTNING_ROD.default_state,
            &EntityType::ZOMBIE,
        ));
        assert!(!Block::POWERED_RAIL.default_state.is_signal_source());
        assert!(!Block::ACTIVATOR_RAIL.default_state.is_signal_source());
        assert!(is_valid_empty_spawn_block(
            Block::AIR.default_state,
            &EntityType::ZOMBIE,
        ));
    }

    #[test]
    fn water_spawn_rejects_redstone_conductors_above() {
        assert!(!can_spawn_in_water(true, Block::STONE.default_state));
        assert!(can_spawn_in_water(true, Block::AIR.default_state));
        assert!(!can_spawn_in_water(false, Block::AIR.default_state));
    }

    #[test]
    fn jittered_spawn_candidate_must_keep_the_selected_biome_entry() {
        let glow_squid = &Biome::BAMBOO_JUNGLE.spawners.underground_water_creature[0];

        assert!(is_spawner_allowed_in_biome(
            &Biome::BAMBOO_JUNGLE,
            EntityType::GLOW_SQUID.category,
            glow_squid,
        ));
        assert!(!is_spawner_allowed_in_biome(
            &Biome::DEEP_DARK,
            EntityType::GLOW_SQUID.category,
            glow_squid,
        ));
    }

    #[test]
    fn spawn_distance_uses_world_spawn_coordinates() {
        let pos = BlockPos::new(100, 64, 100);
        let chunk = Vector2::new(6, 6);

        assert!(!is_right_distance_to_player_and_spawn_point(
            &pos,
            25. * 25.,
            &chunk,
            &Vector3::new(100, 64, 100),
        ));
        assert!(is_right_distance_to_player_and_spawn_point(
            &pos,
            25. * 25.,
            &chunk,
            &Vector3::new(0, 64, 0),
        ));
    }

    #[test]
    fn spawn_aabb_uses_vanilla_special_entity_scales() {
        assert_eq!(spawn_dimensions(&EntityType::ZOMBIE).width, 0.6);
        assert_eq!(spawn_dimensions(&EntityType::SLIME).width, 2.08);
        assert_eq!(spawn_dimensions(&EntityType::MAGMA_CUBE).height, 2.08);
        assert_eq!(spawn_dimensions(&EntityType::SULFUR_CUBE).width, 0.98);
    }
}
