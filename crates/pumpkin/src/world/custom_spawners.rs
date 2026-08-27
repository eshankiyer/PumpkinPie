use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};

use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::Taggable;
use pumpkin_data::tag::WorldgenBiome::MINECRAFT_WITHOUT_WANDERING_TRADER_SPAWNS;
use pumpkin_data::tag::WorldgenBiome::MINECRAFT_WITHOUT_ZOMBIE_SIEGES;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::mob::MobEntity;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::passive::cat::select_natural_cat_variant;
use crate::entity::passive::wandering_trader::WanderingTraderEntity;
use crate::entity::player::statistics::{CustomStatistic, StatisticCategory};
use crate::entity::r#type::from_type;
use crate::world::World;
use crate::world::natural_spawner::{is_spawn_position_ok, is_valid_empty_spawn_block};

fn entities_of_type_near(
    world: &Arc<World>,
    center: &BlockPos,
    horizontal_radius: f64,
    vertical_radius: f64,
    entity_type: &'static EntityType,
) -> usize {
    let min = Vector3::new(
        f64::from(center.0.x) - horizontal_radius,
        f64::from(center.0.y) - vertical_radius,
        f64::from(center.0.z) - horizontal_radius,
    );
    let max = Vector3::new(
        f64::from(center.0.x) + horizontal_radius,
        f64::from(center.0.y) + vertical_radius,
        f64::from(center.0.z) + horizontal_radius,
    );
    let bb = pumpkin_util::math::boundingbox::BoundingBox { min, max };
    world
        .get_entities_at_box(&bb)
        .iter()
        .filter(|e| e.get_entity().entity_type == entity_type)
        .count()
}

/// `PhantomSpawner` (`PhantomSpawner.java`): entirely absent from Pumpkin before this change, so phantoms never spawned naturally.
///
/// Ported field for field: 60-120 game-tick interval
/// (`(60 + random.nextInt(60)) * 20`), only active once `skyDarken >= 5` (or
/// the dimension has no sky light), gated per-player by `TIME_SINCE_REST >=
/// 72000` (sampled via `nextInt`) and by
/// `DifficultyInstance.isHarderThan(random.nextFloat() * 3.0F)`.
pub async fn tick_phantom_spawner(world: &Arc<World>) {
    if !world.level_info.load().game_rules.spawn_phantoms {
        return;
    }
    if world.phantom_spawn_tick.fetch_sub(1, Relaxed) - 1 > 0 {
        return;
    }
    world
        .phantom_spawn_tick
        .store((60 + rand::random_range(0..60)) * 20, Relaxed);

    let sky_darken = world.sky_darken.load(Relaxed);
    if sky_darken < 5 && world.dimension.has_skylight {
        return;
    }

    let players: Vec<_> = world.players.load().iter().cloned().collect();
    for player in players {
        if player.gamemode.load() == GameMode::Spectator {
            continue;
        }

        let player_pos = player.get_entity().block_pos.load();
        if world.dimension.has_skylight
            && (player_pos.0.y < world.sea_level || !world.can_see_sky(&player_pos))
        {
            continue;
        }

        let difficulty = RegionalDifficulty::at(world, player.position());
        if difficulty.effective_difficulty <= rand::random::<f32>() * 3.0 {
            continue;
        }

        let time_since_rest = {
            let stats = player.stats.lock().await;
            stats.get(
                StatisticCategory::Custom,
                CustomStatistic::TimeSinceRest as i32,
            )
        }
        .clamp(1, i32::MAX);

        if rand::random_range(0..time_since_rest) < 72000 {
            continue;
        }

        let spawn_pos = player_pos.add(
            rand::random_range(0..21) - 10,
            20 + rand::random_range(0..15),
            rand::random_range(0..21) - 10,
        );

        if !is_valid_empty_spawn_block(world.get_block_state(&spawn_pos), &EntityType::PHANTOM) {
            continue;
        }

        let spawn_pos_f64 = Vector3::new(
            f64::from(spawn_pos.0.x) + 0.5,
            f64::from(spawn_pos.0.y),
            f64::from(spawn_pos.0.z) + 0.5,
        );
        let group_size = 1 + rand::random_range(0..(difficulty.base_difficulty as i32 + 1));
        for _ in 0..group_size {
            let phantom = from_type(&EntityType::PHANTOM, spawn_pos_f64, world, Uuid::new_v4());
            phantom.get_entity().set_rotation(0.0, 0.0);
            world.spawn_entity(phantom).await;
        }
    }
}

/// `CatSpawner` (`CatSpawner.java`): 1200-tick interval, picks a random
/// player and a random offset in `[-32, 32]` on each axis (`8 +
/// random.nextInt(24)`, sign-randomized).
///
/// Village gate: vanilla checks `level.isCloseToVillage(spawnPos, 2)`, then
/// requires `getCountInRange(HOME, spawnPos, 48, IS_OCCUPIED) > 4` (more
/// than 4 claimed beds within a 48-block sphere) before spawning in a
/// village. See `crate::world::village_poi` for the POI registry backing
/// both checks and the `Occupancy.ANY` (not `IS_OCCUPIED`) deviation it
/// documents.
///
/// Scope reduction: vanilla's other spawn path, inside a swamp-hut structure
/// (`CATS_SPAWN_IN` structure tag), is dropped entirely - Pumpkin has no
/// structure-piece lookup by tag.
pub async fn tick_cat_spawner(world: &Arc<World>) {
    if world.cat_spawn_tick.fetch_sub(1, Relaxed) - 1 > 0 {
        return;
    }
    world.cat_spawn_tick.store(1200, Relaxed);

    let players = world.players.load();
    if players.is_empty() {
        return;
    }
    let player = players[rand::random_range(0..players.len())].clone();
    drop(players);

    let dx = (8 + rand::random_range(0..24)) * if rand::random::<bool>() { -1 } else { 1 };
    let dz = (8 + rand::random_range(0..24)) * if rand::random::<bool>() { -1 } else { 1 };
    let spawn_pos = player.get_entity().block_pos.load().add(dx, 0, dz);

    let chunk_pos = Vector2::new(spawn_pos.0.x >> 4, spawn_pos.0.z >> 4);
    if !world.active_chunks.load().contains(&chunk_pos) {
        return;
    }

    if !is_spawn_position_ok(world, &spawn_pos, &EntityType::CAT) {
        return;
    }

    if !world.is_close_to_village(spawn_pos, 2).await {
        return;
    }

    let homes_nearby = world
        .poi_count_in_range(
            crate::world::village_poi::POI_TYPE_HOME,
            spawn_pos,
            48,
            crate::world::village_poi::Occupancy::IsOccupied,
        )
        .await;
    if homes_nearby <= 4 {
        return;
    }

    let cats_nearby = entities_of_type_near(world, &spawn_pos, 48.0, 8.0, &EntityType::CAT);
    if cats_nearby >= 5 {
        return;
    }

    let time_of_day = world.get_time_of_day().await;
    let spawn_pos_f64 = Vector3::new(
        f64::from(spawn_pos.0.x) + 0.5,
        f64::from(spawn_pos.0.y),
        f64::from(spawn_pos.0.z) + 0.5,
    );
    let cat = from_type(&EntityType::CAT, spawn_pos_f64, world, Uuid::new_v4());
    cat.set_variant_name(select_natural_cat_variant(time_of_day));
    cat.get_entity().set_rotation(0.0, 0.0);
    world.spawn_entity(cat).await;
}

fn find_spawn_position_near(
    world: &Arc<World>,
    reference: &BlockPos,
    radius: i32,
) -> Option<BlockPos> {
    for _ in 0..10 {
        let x = reference.0.x + rand::random_range(0..radius * 2) - radius;
        let z = reference.0.z + rand::random_range(0..radius * 2) - radius;
        let y = world.get_top_block(Vector2::new(x, z));
        let pos = BlockPos::new(x, y, z);
        if is_spawn_position_ok(world, &pos, &EntityType::WANDERING_TRADER) {
            return Some(pos);
        }
    }
    None
}

fn has_enough_space(world: &Arc<World>, pos: &BlockPos) -> bool {
    for dx in 0..=1 {
        for dy in 0..=2 {
            for dz in 0..=1 {
                if world.get_block_state(&pos.add(dx, dy, dz)).is_solid_block() {
                    return false;
                }
            }
        }
    }
    true
}

async fn try_spawn_wandering_trader(world: &Arc<World>) -> bool {
    let players = world.players.load();
    if players.is_empty() {
        return true;
    }
    if rand::random_range(0..10) != 0 {
        return false;
    }
    let player = players[rand::random_range(0..players.len())].clone();
    drop(players);

    // No POI manager, so the "meeting point" lookup always falls through to
    // the player's own position, matching vanilla's own fallback
    // (`poiPos.orElse(playerPos)`) for the case where no meeting POI exists
    // nearby.
    let reference_pos = player.get_entity().block_pos.load();
    let Some(spawn_pos) = find_spawn_position_near(world, &reference_pos, 48) else {
        return false;
    };
    if !has_enough_space(world, &spawn_pos) {
        return false;
    }
    // This biome gate is negated: "tag absent" permits the spawn. Treating an unresolvable
    // biome as "tag absent" would therefore *allow* a wandering trader at a position whose
    // biome is unknown, so abort the attempt instead - the same choice the earlier fallback
    // fix made in `natural_spawner::can_spawn`.
    let Some(spawn_biome) = world.get_biome(&spawn_pos) else {
        return false;
    };
    if spawn_biome.has_tag(&MINECRAFT_WITHOUT_WANDERING_TRADER_SPAWNS) {
        return false;
    }

    let spawn_pos_f64 = Vector3::new(
        f64::from(spawn_pos.0.x) + 0.5,
        f64::from(spawn_pos.0.y),
        f64::from(spawn_pos.0.z) + 0.5,
    );
    let trader = from_type(
        &EntityType::WANDERING_TRADER,
        spawn_pos_f64,
        world,
        Uuid::new_v4(),
    );
    world.spawn_entity(trader.clone()).await;

    for _ in 0..2 {
        if let Some(llama_pos) =
            find_spawn_position_near(world, &trader.get_entity().block_pos.load(), 4)
        {
            let llama_pos_f64 = Vector3::new(
                f64::from(llama_pos.0.x) + 0.5,
                f64::from(llama_pos.0.y),
                f64::from(llama_pos.0.z) + 0.5,
            );
            let llama = from_type(
                &EntityType::TRADER_LLAMA,
                llama_pos_f64,
                world,
                Uuid::new_v4(),
            );
            world.spawn_entity(llama.clone()).await;
            llama.get_entity().leash_to(trader.clone()).await;
        }
    }

    // `WanderingTraderSpawner.spawn` (`WanderingTraderSpawner.java:102-104`) applies these
    // values after the trader and its llamas are created.
    if let Some(trader_entity) = trader.cast_any().downcast_ref::<WanderingTraderEntity>() {
        trader_entity.set_despawn_delay(48000);
        trader_entity.set_wander_target(Some(reference_pos));
        trader_entity.set_home_to(reference_pos, 16);
    }

    true
}

/// `WanderingTraderSpawner` (`WanderingTraderSpawner.java`).
///
/// 1200-tick outer interval, an inner 24000-tick (one day) spawn-delay
/// counter, and a spawn-chance that ramps `25 -> 50 -> 75` (capped) each day
/// until a trader actually spawns, then resets to 25.
///
/// Scope reduction: the `spawn_delay`/`spawn_chance` counters live only in memory
/// (`World::trader_spawn_delay` / `trader_spawn_chance`), not in a persisted
/// `WanderingTraderData` saved-data file, so they reset to vanilla defaults (24000 / 25) on
/// server restart.
pub async fn tick_wandering_trader_spawner(world: &Arc<World>) {
    if !world.level_info.load().game_rules.spawn_wandering_traders {
        return;
    }
    if world.trader_tick_delay.fetch_sub(1, Relaxed) - 1 > 0 {
        return;
    }
    world.trader_tick_delay.store(1200, Relaxed);

    if world.trader_spawn_delay.fetch_sub(1200, Relaxed) - 1200 > 0 {
        return;
    }
    world.trader_spawn_delay.store(24000, Relaxed);

    let chance = world.trader_spawn_chance.load(Relaxed);
    world
        .trader_spawn_chance
        .store((chance + 25).clamp(25, 75), Relaxed);

    if rand::random_range(0..100) > chance {
        return;
    }

    if try_spawn_wandering_trader(world).await {
        world.trader_spawn_chance.store(25, Relaxed);
    }
}

/// Per-level state of the village siege spawner, mirroring vanilla's private fields on
/// `VillageSiege` (`VillageSiege.java:26-32`).
///
/// Pumpkin's other custom spawners keep their counters directly on `World`; this one needs
/// seven of them, so they are grouped into a single struct that lives in one `World` field
/// instead.
pub struct VillageSiegeState {
    /// 0 = `State.SIEGE_DONE`, 1 = `State.SIEGE_TONIGHT`. Vanilla's third constant
    /// `SIEGE_CAN_ACTIVATE` (`VillageSiege.java:130`) is never read by `tick`, so it is not
    /// modelled.
    siege_tonight: AtomicBool,
    has_setup_siege: AtomicBool,
    zombies_to_spawn: AtomicI32,
    next_spawn_time: AtomicI32,
    spawn_x: AtomicI32,
    spawn_y: AtomicI32,
    spawn_z: AtomicI32,
}

impl VillageSiegeState {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            siege_tonight: AtomicBool::new(false),
            has_setup_siege: AtomicBool::new(false),
            zombies_to_spawn: AtomicI32::new(0),
            next_spawn_time: AtomicI32::new(0),
            spawn_x: AtomicI32::new(0),
            spawn_y: AtomicI32::new(0),
            spawn_z: AtomicI32::new(0),
        }
    }
}

impl Default for VillageSiegeState {
    fn default() -> Self {
        Self::new()
    }
}

/// The `roll_village_siege` time marker: tick 18000 of the overworld clock's 24000-tick day
/// (`data/minecraft/timeline/day.json`: `"minecraft:roll_village_siege": 18000`,
/// `"period_ticks": 24000`; `ClockTimeMarker.occursAt` fires when
/// `ticks == totalTicks % periodTicks`, `ClockTimeMarker.java:29-31`).
const ROLL_VILLAGE_SIEGE_TIME_OF_DAY: i64 = 18_000;

const OVERWORLD_DAY_LENGTH: i64 = 24_000;

/// `VillageSiege.tick` (`VillageSiege.java:35-67`).
///
/// Runs only while it is dark outside (`!Level.isBrightOutside`, which is `skyDarken < 4`
/// for non-fixed-time dimensions, `Level.java:385-387`) and while hostile spawning is
/// enabled; any bright/enabled-off tick tears the siege state back down (L63-66).
///
/// At the roll marker a 1-in-10 roll schedules tonight's siege (L37-40); otherwise, once per
/// two ticks, one zombie is spawned until twenty have been placed (L51-61).
///
/// Scope reduction: vanilla's `trySpawn` runs `Zombie.finalizeSpawn(..., EVENT, ...)`
/// (`VillageSiege.java:100-106`) to give siege zombies difficulty-scaled equipment and
/// jockey rolls; Pumpkin has no finalize-spawn hook and the other custom spawners here
/// (`tick_phantom_spawner`, `tick_cat_spawner`) likewise spawn bare mobs.
pub async fn tick_village_siege(world: &Arc<World>, spawn_enemies: bool) {
    let siege = &world.village_siege;
    // Vanilla `Level.isBrightOutside` (`Level.java:385-387`): fixed-time dimensions never
    // count as bright. They also contain no villages, so their sieges would die out in
    // `try_to_setup_siege` anyway; the sky-darken test below is what matters for the
    // overworld.
    let is_bright_outside =
        world.dimension.fixed_time.is_none() && world.sky_darken.load(Relaxed) < 4;

    if !is_bright_outside && spawn_enemies {
        if world.get_time_of_day().await % OVERWORLD_DAY_LENGTH == ROLL_VILLAGE_SIEGE_TIME_OF_DAY {
            let tonight = rand::random_range(0..10) == 0;
            siege.siege_tonight.store(tonight, Relaxed);
        }

        if siege.siege_tonight.load(Relaxed) {
            if !siege.has_setup_siege.load(Relaxed) {
                if !try_to_setup_siege(world).await {
                    return;
                }
                siege.has_setup_siege.store(true, Relaxed);
            }

            // L51-54: `if nextSpawnTime > 0 { nextSpawnTime-- } else { ... }`. These fields
            // are only touched from this spawner inside the serialized world tick, so a
            // plain load/store pair preserves vanilla's exact countdown shape.
            if siege.next_spawn_time.load(Relaxed) > 0 {
                siege.next_spawn_time.fetch_sub(1, Relaxed);
            } else {
                siege.next_spawn_time.store(2, Relaxed);
                if siege.zombies_to_spawn.load(Relaxed) > 0 {
                    siege.zombies_to_spawn.fetch_sub(1, Relaxed);
                    let pos = Vector3::new(
                        f64::from(siege.spawn_x.load(Relaxed)) + 0.5,
                        f64::from(siege.spawn_y.load(Relaxed)),
                        f64::from(siege.spawn_z.load(Relaxed)) + 0.5,
                    );
                    try_spawn_zombie(world, pos).await;
                } else {
                    siege.siege_tonight.store(false, Relaxed);
                }
            }
        }
    } else {
        siege.siege_tonight.store(false, Relaxed);
        siege.has_setup_siege.store(false, Relaxed);
    }
}

/// `VillageSiege.tryToSetupSiege` (`VillageSiege.java:69-94`): find a non-spectator player
/// standing inside a village whose biome does not exclude sieges, then ring it with up to
/// ten random points 32 blocks out and take the first that lands on a valid zombie spot.
///
/// Note the vanilla quirk kept here: once a qualifying player is found the method returns
/// `true` even if none of the ten points worked (L88), so a failed setup still consumes this
/// night's attempt and the state machine waits for the next dark period to retry.
async fn try_to_setup_siege(world: &Arc<World>) -> bool {
    let players = world.players.load();
    for player in players.iter() {
        if player.gamemode.load() == GameMode::Spectator {
            continue;
        }

        let center = player.get_entity().block_pos.load();
        // Vanilla `level.isVillage(center)` is `isCloseToVillage(center, 1)`
        // (`ServerLevel.java:1542-1544`).
        if !world.is_close_to_village(center, 1).await {
            continue;
        }
        if world
            .get_biome(&center)
            .is_some_and(|biome| biome.has_tag(&MINECRAFT_WITHOUT_ZOMBIE_SIEGES))
        {
            continue;
        }

        let siege = &world.village_siege;
        for _ in 0..10 {
            let angle = rand::random::<f32>() * (std::f64::consts::PI as f32 * 2.0);
            let x = center.0.x + (angle.cos() * 32.0).floor() as i32;
            let z = center.0.z + (angle.sin() * 32.0).floor() as i32;
            let y = center.0.y;
            siege.spawn_x.store(x, Relaxed);
            siege.spawn_y.store(y, Relaxed);
            siege.spawn_z.store(z, Relaxed);
            let anchor = BlockPos::new(x, y, z);
            if find_random_spawn_pos(world, anchor).await.is_some() {
                siege.next_spawn_time.store(0, Relaxed);
                siege.zombies_to_spawn.store(20, Relaxed);
                break;
            }
        }

        return true;
    }

    false
}

/// `VillageSiege.trySpawn` (`VillageSiege.java:96-111`) minus the dropped
/// `finalizeSpawn` pass (see the scope note on [`tick_village_siege`]): place one zombie at
/// the bottom-center of a valid spot with a uniformly random yaw.
async fn try_spawn_zombie(world: &Arc<World>, spawn_pos: Vector3<f64>) {
    let yaw = rand::random::<f32>() * 360.0;
    let zombie = from_type(&EntityType::ZOMBIE, spawn_pos, world, Uuid::new_v4());
    zombie.get_entity().set_rotation(yaw, 0.0);
    world.spawn_entity(zombie).await;
}

/// `VillageSiege.findRandomSpawnPos` (`VillageSiege.java:113-127`): jitter the anchor by
/// eight blocks on each horizontal axis, drop onto the world surface, and accept the first
/// position that is both inside the village and dark enough for an event-spawned zombie.
///
/// Vanilla's `Monster.checkMonsterSpawnRules(ZOMBIE, ..., EVENT, ...)` keeps the darkness
/// requirements because only `TRIAL_SPAWNER` ignores them
/// (`EntitySpawnReason.java:28-30`), and its trailing `checkMobSpawnRules` block-placement
/// predicate is covered by `is_spawn_position_ok`.
async fn find_random_spawn_pos(world: &Arc<World>, pos: BlockPos) -> Option<Vector3<f64>> {
    let is_thundering = world.weather.lock().await.thundering;
    for _ in 0..10 {
        let x = pos.0.x + rand::random_range(0..16) - 8;
        let z = pos.0.z + rand::random_range(0..16) - 8;
        // Vanilla `level.getHeight(Heightmap.Types.WORLD_SURFACE, x, z)`; same heightmap
        // read the wandering-trader path uses via `get_top_block`.
        let y = world.get_top_block(Vector2::new(x, z));
        let offset = BlockPos::new(x, y, z);
        if world.is_close_to_village(offset, 1).await
            && MobEntity::check_monster_spawn_rules(world, &offset, is_thundering)
            && is_spawn_position_ok(world, &offset, &EntityType::ZOMBIE)
        {
            return Some(Vector3::new(
                f64::from(x) + 0.5,
                f64::from(y),
                f64::from(z) + 0.5,
            ));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn spawn_chance_ramps_and_caps() {
        let mut chance = 25;
        for _ in 0..10 {
            chance = (chance + 25i32).clamp(25, 75);
        }
        assert_eq!(chance, 75);
    }

    #[test]
    fn phantom_next_tick_interval_matches_vanilla_bounds() {
        for roll in 0..60 {
            let next = (60 + roll) * 20;
            assert!((1200..=2380).contains(&next));
        }
    }
}
