#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]

use pumpkin_data::BlockStateId;
use pumpkin_data::dimension::Dimension;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::world_seed::Seed;
use pumpkin_world::chunk_system::{Chunk, StagedChunkEnum, generate_single_chunk};
use pumpkin_world::generation::get_world_gen;
use pumpkin_world::world::WorldPortalExt;
use std::collections::HashMap;

struct BlockRegistry;
impl WorldPortalExt for BlockRegistry {
    fn can_place_at(
        &self,
        _block: &pumpkin_data::Block,
        _state: &pumpkin_data::BlockState,
        _block_accessor: &dyn pumpkin_world::world::BlockAccessor,
        _block_pos: &pumpkin_util::math::position::BlockPos,
    ) -> bool {
        true
    }
    fn mirror(
        &self,
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
        mirror: pumpkin_data::Mirror,
    ) -> &'static pumpkin_data::BlockState {
        block.mirror(state_id, mirror)
    }
    fn rotate(
        &self,
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
        rotation: pumpkin_data::Rotation,
    ) -> &'static pumpkin_data::BlockState {
        block.rotate(state_id, rotation)
    }
    fn spawn_mobs_for_chunk_generation(
        &self,
        _cache: &mut dyn pumpkin_world::generation::proto_chunk::GenerationCache,
        _biome: &'static pumpkin_data::chunk::Biome,
        _chunk_x: i32,
        _chunk_z: i32,
    ) {
    }
}

#[test]
#[ignore = "census instrument, run explicitly"]
fn overworld_block_census() {
    let n: i32 = std::env::var("CENSUS_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let seed = Seed(
        std::env::var("CENSUS_SEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(42),
    );
    let dimension = Dimension::OVERWORLD;
    let world_gen = get_world_gen(seed, dimension.clone(), false, Vec::new(), String::new());
    let registry = BlockRegistry;

    let mut totals: HashMap<&'static str, u64> = HashMap::new();
    let mut y_hist: HashMap<&'static str, HashMap<i32, u64>> = HashMap::new();
    let chunks = n * n;

    let stage = match std::env::var("CENSUS_STAGE").as_deref() {
        Ok("carvers") => StagedChunkEnum::Carvers,
        Ok("surface") => StagedChunkEnum::Surface,
        Ok("noise") => StagedChunkEnum::Noise,
        _ => StagedChunkEnum::Features,
    };
    let stride: i32 = std::env::var("CENSUS_STRIDE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    for cx in 0..n {
        for cz in 0..n {
            let chunk = generate_single_chunk(
                &dimension,
                0,
                &world_gen,
                &registry,
                cx * stride,
                cz * stride,
                stage,
            );
            let proto = match &chunk {
                Chunk::Proto(p) => &**p,
                Chunk::Level(_) => panic!("expected proto"),
            };
            for lx in 0..16 {
                for lz in 0..16 {
                    for y in -64..320 {
                        let id = proto.get_block_state(&Vector3::new(
                            cx * stride * 16 + lx,
                            y,
                            cz * stride * 16 + lz,
                        ));
                        let block = pumpkin_data::Block::from_state_id(id);
                        if block.name == "air" || block.name == "void_air" {
                            continue;
                        }
                        *totals.entry(block.name).or_default() += 1;
                        *y_hist.entry(block.name).or_default().entry(y).or_default() += 1;
                    }
                }
            }
        }
    }

    let mut rows: Vec<_> = totals.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    println!("=== census seed={} chunks={} ===", seed.0, chunks);
    for (name, count) in rows {
        let per_chunk = *count as f64 / f64::from(chunks);
        let hist = &y_hist[name];
        let ymin = hist.keys().min().unwrap();
        let ymax = hist.keys().max().unwrap();
        println!("{name:<34} total={count:<9} per_chunk={per_chunk:<12.4} y=[{ymin},{ymax}]");
    }
}

fn env_i32(key: &str, default: i32) -> i32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn census_gen() -> (
    Dimension,
    Box<pumpkin_world::generation::generator::WorldGenerator>,
) {
    let seed = Seed(
        std::env::var("CENSUS_SEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(42),
    );
    let dimension = Dimension::OVERWORLD;
    let world_gen = get_world_gen(seed, dimension.clone(), false, Vec::new(), String::new());
    (dimension, world_gen)
}

#[derive(Default)]
struct BiomeStat {
    columns: u64,
    floor_y_sum: i64,
    floor_y_min: i32,
    floor_y_max: i32,
    submerged: u64,
    floors: HashMap<&'static str, u64>,
    covers: HashMap<&'static str, u64>,
}

/// Per-biome surface census: for every column, the topmost non-air block ("cover") and the
/// topmost non-air non-fluid block ("floor"), bucketed by the biome stored at the floor.
///
/// Runs at the `Surface` stage by default so features and structures do not mask the surface
/// rule's own output. A categorical zero here (a desert with no sand, a badlands with no
/// terracotta, a snowy biome with no snow) is the signal.
#[test]
#[ignore = "census instrument, run explicitly"]
fn overworld_surface_census() {
    let (dimension, world_gen) = census_gen();
    let registry = BlockRegistry;
    let n = env_i32("CENSUS_N", 8);
    let stride = env_i32("CENSUS_STRIDE", 13);
    let stage = match std::env::var("CENSUS_STAGE").as_deref() {
        Ok("features") => StagedChunkEnum::Features,
        Ok("carvers") => StagedChunkEnum::Carvers,
        Ok("noise") => StagedChunkEnum::Noise,
        _ => StagedChunkEnum::Surface,
    };

    let mut stats: HashMap<&'static str, BiomeStat> = HashMap::new();
    let mut columns: u64 = 0;

    for cx in 0..n {
        for cz in 0..n {
            let chunk = generate_single_chunk(
                &dimension,
                0,
                &world_gen,
                &registry,
                cx * stride,
                cz * stride,
                stage,
            );
            let proto = match &chunk {
                Chunk::Proto(p) => &**p,
                Chunk::Level(_) => panic!("expected proto"),
            };
            for lx in 0..16 {
                for lz in 0..16 {
                    let wx = cx * stride * 16 + lx;
                    let wz = cz * stride * 16 + lz;
                    let mut cover: Option<&'static str> = None;
                    let mut floor: Option<(&'static str, i32)> = None;
                    for y in (-64..320).rev() {
                        let id = proto.get_block_state(&Vector3::new(wx, y, wz));
                        let block = pumpkin_data::Block::from_state_id(id);
                        if block.name == "air"
                            || block.name == "void_air"
                            || block.name == "cave_air"
                        {
                            continue;
                        }
                        if cover.is_none() {
                            cover = Some(block.name);
                        }
                        if block.name == "water" || block.name == "lava" || block.name == "ice" {
                            continue;
                        }
                        floor = Some((block.name, y));
                        break;
                    }
                    let Some((floor_name, floor_y)) = floor else {
                        continue;
                    };
                    columns += 1;
                    let biome = proto.get_biome(wx, floor_y, wz).registry_id;
                    let entry = stats.entry(biome).or_insert_with(|| BiomeStat {
                        floor_y_min: i32::MAX,
                        floor_y_max: i32::MIN,
                        ..BiomeStat::default()
                    });
                    entry.columns += 1;
                    entry.floor_y_sum += i64::from(floor_y);
                    entry.floor_y_min = entry.floor_y_min.min(floor_y);
                    entry.floor_y_max = entry.floor_y_max.max(floor_y);
                    *entry.floors.entry(floor_name).or_default() += 1;
                    if let Some(cover_name) = cover {
                        *entry.covers.entry(cover_name).or_default() += 1;
                        if cover_name != floor_name {
                            entry.submerged += 1;
                        }
                    }
                }
            }
        }
    }

    print_surface_stats(&stats, columns, n * n);
}

fn print_surface_stats(stats: &HashMap<&'static str, BiomeStat>, columns: u64, chunks: i32) {
    let mut rows: Vec<_> = stats.iter().collect();
    rows.sort_by(|a, b| b.1.columns.cmp(&a.1.columns).then_with(|| a.0.cmp(b.0)));
    println!("=== surface census columns={columns} chunks={chunks} ===");
    for (biome, stat) in rows {
        let share = stat.columns as f64 * 100.0 / columns as f64;
        let mean_y = stat.floor_y_sum as f64 / stat.columns as f64;
        let mut floors: Vec<_> = stat.floors.iter().collect();
        floors.sort_by(|a, b| b.1.cmp(a.1));
        let top: Vec<String> = floors
            .iter()
            .take(4)
            .map(|(name, count)| {
                format!(
                    "{name}={:.0}%",
                    **count as f64 * 100.0 / stat.columns as f64
                )
            })
            .collect();
        let mut covers: Vec<_> = stat.covers.iter().collect();
        covers.sort_by(|a, b| b.1.cmp(a.1));
        let cover_top = covers.first().map_or("-", |(name, _)| *name);
        println!(
            "{biome:<40} share={share:<7.3} n={:<7} y[mean={mean_y:<7.1} min={:<5} max={:<5}] cover={cover_top:<14} floors: {}",
            stat.columns,
            stat.floor_y_min,
            stat.floor_y_max,
            top.join(" ")
        );
    }
}

/// Carve-volume census: generates each chunk twice, at `Noise` and at `Carvers`, and counts
/// the blocks the carvers removed, bucketed by y. The canyon carver's own configured y range
/// is 10..=67 (`Carvers.java:73`), so a zero or an order-of-magnitude anomaly in that band is
/// the canyon signal; the mass below y=10 is the two cave carvers.
#[test]
#[ignore = "census instrument, run explicitly"]
fn overworld_carve_census() {
    let (dimension, world_gen) = census_gen();
    let registry = BlockRegistry;
    let n = env_i32("CENSUS_N", 6);
    let stride = env_i32("CENSUS_STRIDE", 13);

    let mut removed_by_y: HashMap<i32, u64> = HashMap::new();
    let mut removed_to: HashMap<&'static str, u64> = HashMap::new();
    let mut total: u64 = 0;
    let mut band: u64 = 0;
    let chunks = n * n;

    for cx in 0..n {
        for cz in 0..n {
            let (gx, gz) = (cx * stride, cz * stride);
            let before = generate_single_chunk(
                &dimension,
                0,
                &world_gen,
                &registry,
                gx,
                gz,
                StagedChunkEnum::Surface,
            );
            let after = generate_single_chunk(
                &dimension,
                0,
                &world_gen,
                &registry,
                gx,
                gz,
                StagedChunkEnum::Carvers,
            );
            let (Chunk::Proto(before), Chunk::Proto(after)) = (&before, &after) else {
                panic!("expected proto");
            };
            for lx in 0..16 {
                for lz in 0..16 {
                    for y in -64..320 {
                        let pos = Vector3::new(gx * 16 + lx, y, gz * 16 + lz);
                        let old = pumpkin_data::Block::from_state_id(before.get_block_state(&pos));
                        let new = pumpkin_data::Block::from_state_id(after.get_block_state(&pos));
                        if old.id == new.id {
                            continue;
                        }
                        if old.name == "air" || old.name == "void_air" {
                            continue;
                        }
                        total += 1;
                        *removed_by_y.entry(y).or_default() += 1;
                        *removed_to.entry(new.name).or_default() += 1;
                        if (10..=67).contains(&y) {
                            band += 1;
                        }
                    }
                }
            }
        }
    }

    println!("=== carve census chunks={chunks} ===");
    println!(
        "removed_total={total} per_chunk={:.2} canyon_band_y10_67={band} per_chunk={:.2}",
        total as f64 / f64::from(chunks),
        band as f64 / f64::from(chunks)
    );
    let mut to: Vec<_> = removed_to.iter().collect();
    to.sort_by(|a, b| b.1.cmp(a.1));
    for (name, count) in to {
        println!("  replaced_with {name:<16} {count}");
    }
    for bucket in -8..40 {
        let lo = bucket * 8;
        let sum: u64 = (lo..lo + 8)
            .filter_map(|y| removed_by_y.get(&y))
            .sum::<u64>();
        if sum > 0 {
            println!(
                "  y[{lo:>4}..{:>4}) {sum:<9} per_chunk={:.2}",
                lo + 8,
                sum as f64 / f64::from(chunks)
            );
        }
    }
}

/// Biome-placement census: counts every quart-resolution biome cell over a scattered area at
/// the `Biomes` stage, across the full y range so the underground-only biomes
/// (`lush_caves`, `dripstone_caves`, `deep_dark`) are visible too. The signal is a
/// categorical zero: a biome in `OverworldBiomeBuilder`'s parameter list that never appears.
#[test]
#[ignore = "census instrument, run explicitly"]
fn overworld_biome_census() {
    let (dimension, world_gen) = census_gen();
    let registry = BlockRegistry;
    let n = env_i32("CENSUS_N", 16);
    let stride = env_i32("CENSUS_STRIDE", 97);

    let mut counts: HashMap<&'static str, u64> = HashMap::new();
    let mut total: u64 = 0;

    for cx in 0..n {
        for cz in 0..n {
            let chunk = generate_single_chunk(
                &dimension,
                0,
                &world_gen,
                &registry,
                cx * stride,
                cz * stride,
                StagedChunkEnum::Biomes,
            );
            let proto = match &chunk {
                Chunk::Proto(p) => &**p,
                Chunk::Level(_) => panic!("expected proto"),
            };
            for qx in 0..4 {
                for qz in 0..4 {
                    for qy in 0..96 {
                        let wx = cx * stride * 16 + qx * 4;
                        let wz = cz * stride * 16 + qz * 4;
                        let wy = -64 + qy * 4;
                        let biome = proto.get_biome(wx, wy, wz).registry_id;
                        *counts.entry(biome).or_default() += 1;
                        total += 1;
                    }
                }
            }
        }
    }

    let mut rows: Vec<_> = counts.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    println!(
        "=== biome census cells={total} chunks={} distinct={} ===",
        n * n,
        rows.len()
    );
    for (biome, count) in rows {
        println!(
            "{biome:<34} {count:<9} share={:.4}%",
            *count as f64 * 100.0 / total as f64
        );
    }
}
