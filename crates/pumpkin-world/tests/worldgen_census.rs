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
