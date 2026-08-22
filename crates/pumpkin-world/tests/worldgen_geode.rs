//! Empirical check that geodes generate.
//!
//! `GeodeConfiguration.java:37-39` makes `distribution_points` and `point_offset`
//! `optionalFieldOf` with `UniformInt` defaults, and the datapack JSON omits both. The codegen
//! used to map an absent field to `IntProvider::Constant(0)`, which made every geode produce
//! zero distribution points and therefore no blocks at all.
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

const GEODE_BLOCKS: [&str; 5] = [
    "amethyst_block",
    "budding_amethyst",
    "calcite",
    "smooth_basalt",
    "amethyst_cluster",
];

/// Counts geode marker blocks over an `n` by `n` chunk square starting at `origin`.
fn count_geode_blocks(seed: u64, origin: (i32, i32), n: i32) -> [u64; 5] {
    let dimension = Dimension::OVERWORLD;
    let world_gen = get_world_gen(
        Seed(seed),
        dimension.clone(),
        false,
        Vec::new(),
        String::new(),
    );
    let registry = BlockRegistry;
    let mut counts = [0u64; 5];

    for cx in origin.0..origin.0 + n {
        for cz in origin.1..origin.1 + n {
            let chunk = generate_single_chunk(
                &dimension,
                0,
                &world_gen,
                &registry,
                cx,
                cz,
                StagedChunkEnum::Features,
            );
            let Chunk::Proto(proto) = &chunk else {
                panic!("expected proto chunk");
            };
            for lx in 0..16 {
                for lz in 0..16 {
                    // Geodes are placed between y = -58 and y = 30.
                    for y in -64..48 {
                        let id =
                            proto.get_block_state(&Vector3::new(cx * 16 + lx, y, cz * 16 + lz));
                        let name = pumpkin_data::Block::from_state_id(id).name;
                        if let Some(i) = GEODE_BLOCKS.iter().position(|b| *b == name) {
                            counts[i] += 1;
                        }
                    }
                }
            }
        }
    }
    counts
}

/// Deterministic regression guard: this seed and chunk window contains a geode.
///
/// Before the codegen fix this window contained zero amethyst, calcite and smooth basalt.
#[test]
fn amethyst_geode_generates() {
    let counts = count_geode_blocks(42, (0, 0), 6);
    for (name, count) in GEODE_BLOCKS.iter().zip(counts) {
        println!("{name:<20} {count}");
    }
    assert!(
        counts[0] > 0 && counts[2] > 0 && counts[3] > 0,
        "no geode generated: {GEODE_BLOCKS:?} = {counts:?}"
    );
}

/// Wider survey, run explicitly: `GEODE_N=12 cargo test -p pumpkin-world -- --ignored`.
#[test]
#[ignore = "survey instrument, run explicitly"]
fn amethyst_geode_survey() {
    let n: i32 = std::env::var("GEODE_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    let seed: u64 = std::env::var("GEODE_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(42);
    let counts = count_geode_blocks(seed, (0, 0), n);
    println!("=== geode survey seed={seed} chunks={} ===", n * n);
    for (name, count) in GEODE_BLOCKS.iter().zip(counts) {
        println!("{name:<20} {count}");
    }
}
