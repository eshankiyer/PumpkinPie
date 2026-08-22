#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Empirical check that `minecraft:sulfur_caves` can be produced by the overworld
//! multi-noise biome search.
//!
//! Vanilla registers it in `OverworldBiomeBuilder.addUndergroundBiomes`
//! (`OverworldBiomeBuilder.java:880-888`), but `assets/multi_noise_biome_tree.json` was
//! extracted from a pre-26.2 version and has no leaf for it, so the search could never
//! return it. The leaf is now patched back in by `tools/pumpkin-codegen/src/biome.rs`.
//!
//! The fixture is the same vanilla-derived census the in-crate wide-area test uses:
//! 724271 biome cells (biome coords -50..=50 on x/z, -20..=50 on y, seed 0), of which 872
//! are `sulfur_caves` in vanilla.

use std::collections::{BTreeMap, HashMap};

use pumpkin_data::chunk::Biome;
use pumpkin_data::dimension::Dimension;
use pumpkin_util::read_data_from_file;
use pumpkin_util::world_seed::Seed;
use pumpkin_world::biome::{BiomeSupplier, MultiNoiseBiomeSupplier};
use pumpkin_world::generation::generator::{GeneratorInit, VanillaGenerator};
use pumpkin_world::generation::noise::router::multi_noise_sampler::{
    MultiNoiseSampler, MultiNoiseSamplerBuilderOptions,
};

const SULFUR_CAVES_ID: u8 = 53;

/// The one biome column where the search hits an exact distance tie; see the second test.
const KNOWN_TIE_COLUMN: (i32, i32) = (-49, -24);

/// Vanilla biome ids for one chunk's cells, keyed by biome coordinate.
type ChunkExpectations = HashMap<(i32, i32, i32), u8>;

#[test]
fn sulfur_caves_is_reachable_and_matches_vanilla() {
    assert_eq!(
        Biome::from_id(SULFUR_CAVES_ID).map(|b| b.registry_id),
        Some("sulfur_caves"),
        "registry id 53 is no longer sulfur_caves"
    );

    let expected_data: Vec<(i32, i32, i32, u8)> =
        read_data_from_file!("../../../assets/tests/multi_noise_biome_source_test.json");

    let generator = VanillaGenerator::new(Seed(0), Dimension::OVERWORLD);

    let mut by_chunk: BTreeMap<(i32, i32), ChunkExpectations> = BTreeMap::new();
    for entry in expected_data {
        by_chunk
            .entry((entry.0.div_euclid(4), entry.2.div_euclid(4)))
            .or_default()
            .insert((entry.0, entry.1, entry.2), entry.3);
    }

    let mut expected_sulfur = 0usize;
    let mut matched_sulfur = 0usize;
    let mut produced_sulfur = 0usize;
    let mut wrong_where_sulfur_expected: Vec<String> = Vec::new();
    let mut sulfur_where_not_expected: Vec<String> = Vec::new();

    for ((chunk_x, chunk_z), entries) in &by_chunk {
        let options = MultiNoiseSamplerBuilderOptions::new(chunk_x * 4, chunk_z * 4, 4);
        let mut sampler = MultiNoiseSampler::generate(&generator.base_router.multi_noise, &options);

        // Same iteration order as `ProtoChunk::populate_biomes`: the search keeps a
        // thread-local "last result node" shortcut, so order is load bearing.
        for section in -6i32..=13 {
            for x in 0..4 {
                for y in 0..4 {
                    for z in 0..4 {
                        let biome_x = chunk_x * 4 + x;
                        let biome_y = section * 4 + y;
                        let biome_z = chunk_z * 4 + z;
                        let Some(&expected) = entries.get(&(biome_x, biome_y, biome_z)) else {
                            continue;
                        };
                        let actual = MultiNoiseBiomeSupplier::OVERWORLD.biome(
                            biome_x,
                            biome_y,
                            biome_z,
                            &mut sampler,
                        );
                        if actual.id == SULFUR_CAVES_ID {
                            produced_sulfur += 1;
                        }
                        if expected == SULFUR_CAVES_ID {
                            expected_sulfur += 1;
                            if actual.id == SULFUR_CAVES_ID {
                                matched_sulfur += 1;
                            } else if wrong_where_sulfur_expected.len() < 10 {
                                wrong_where_sulfur_expected.push(format!(
                                    "biome({biome_x},{biome_y},{biome_z}) got {}",
                                    actual.registry_id
                                ));
                            }
                        } else if actual.id == SULFUR_CAVES_ID
                            && sulfur_where_not_expected.len() < 10
                        {
                            sulfur_where_not_expected.push(format!(
                                "biome({biome_x},{biome_y},{biome_z}) expected {:?}",
                                Biome::from_id(expected).map(|b| b.registry_id)
                            ));
                        }
                    }
                }
            }
        }
    }

    assert_eq!(expected_sulfur, 872, "fixture coverage changed");
    assert!(
        wrong_where_sulfur_expected.is_empty(),
        "cells where vanilla generates sulfur_caves and we do not: {matched_sulfur}/{expected_sulfur} matched, e.g. {wrong_where_sulfur_expected:#?}"
    );
    assert!(
        sulfur_where_not_expected.is_empty(),
        "cells where we generate sulfur_caves and vanilla does not: {sulfur_where_not_expected:#?}"
    );
    assert_eq!(produced_sulfur, expected_sulfur);
    assert!(produced_sulfur > 0);
}

/// Whole-fixture parity: with the `sulfur_caves` leaf restored and the distance metric
/// squared per dimension (vanilla `Climate.RTree.Node.distance`, `Climate.java:424-431`),
/// the only remaining disagreements with the 724271 vanilla-derived cells are the 71 in
/// the single pre-existing biome column (x=-49, z=-24) where the noise point is exactly
/// equidistant from a `forest` leaf and a `birch_forest` leaf at every y, so the winner
/// depends on leaf order in the tree rather than on the metric. That count is unchanged by
/// the `sulfur_caves` fix.
#[test]
fn overworld_multi_noise_matches_vanilla_fixture_exactly() {
    let expected_data: Vec<(i32, i32, i32, u8)> =
        read_data_from_file!("../../../assets/tests/multi_noise_biome_source_test.json");

    let generator = VanillaGenerator::new(Seed(0), Dimension::OVERWORLD);

    let mut by_chunk: BTreeMap<(i32, i32), ChunkExpectations> = BTreeMap::new();
    for entry in expected_data {
        by_chunk
            .entry((entry.0.div_euclid(4), entry.2.div_euclid(4)))
            .or_default()
            .insert((entry.0, entry.1, entry.2), entry.3);
    }

    let mut total = 0usize;
    let mut mismatches = 0usize;
    let mut samples: Vec<String> = Vec::new();

    for ((chunk_x, chunk_z), entries) in &by_chunk {
        let options = MultiNoiseSamplerBuilderOptions::new(chunk_x * 4, chunk_z * 4, 4);
        let mut sampler = MultiNoiseSampler::generate(&generator.base_router.multi_noise, &options);

        for section in -6i32..=13 {
            for x in 0..4 {
                for y in 0..4 {
                    for z in 0..4 {
                        let biome_x = chunk_x * 4 + x;
                        let biome_y = section * 4 + y;
                        let biome_z = chunk_z * 4 + z;
                        let Some(&expected) = entries.get(&(biome_x, biome_y, biome_z)) else {
                            continue;
                        };
                        total += 1;
                        let actual = MultiNoiseBiomeSupplier::OVERWORLD.biome(
                            biome_x,
                            biome_y,
                            biome_z,
                            &mut sampler,
                        );
                        if actual.id != expected {
                            mismatches += 1;
                            if (biome_x, biome_z) != KNOWN_TIE_COLUMN && samples.len() < 10 {
                                samples.push(format!(
                                    "biome({biome_x},{biome_y},{biome_z}) expected {:?} got {}",
                                    Biome::from_id(expected).map(|b| b.registry_id),
                                    actual.registry_id
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    assert_eq!(total, 724271, "fixture coverage changed");
    assert!(
        samples.is_empty(),
        "mismatches vs vanilla outside the known tie column: {samples:#?}"
    );
    assert_eq!(mismatches, 71, "tie-break column coverage changed");
}
