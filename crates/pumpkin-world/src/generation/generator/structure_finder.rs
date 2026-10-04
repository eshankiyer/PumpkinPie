use pumpkin_data::structures::{
    RandomSpreadStructurePlacement, StructurePlacement, StructurePlacementType, StructureSet,
};
use pumpkin_util::math::{floor_div, position::BlockPos, vector2::Vector2};

use crate::generation::structure::placement::{
    GlobalStructureCache, apply_additional_chunk_restrictions, get_locate_pos,
    get_structure_chunk_in_region,
};

use super::WorldGenerator;

/// Block-level position of a found structure plus squared distance from the
/// search origin, used internally to track the running nearest candidate.
#[derive(Debug, Clone)]
pub struct FoundStructure {
    pub pos: BlockPos,
    pub distance_sq: f64,
}

impl FoundStructure {
    /// Wraps a locate position with its squared distance to `origin`, as vanilla's
    /// `BlockPos.distSqr` (all three axes, low corner to low corner).
    fn new(origin: BlockPos, pos: BlockPos) -> Self {
        let dx = f64::from(pos.0.x) - f64::from(origin.0.x);
        let dy = f64::from(pos.0.y) - f64::from(origin.0.y);
        let dz = f64::from(pos.0.z) - f64::from(origin.0.z);
        Self {
            pos,
            distance_sq: dx * dx + dy * dy + dz * dz,
        }
    }
}

/// Finds the block position of the nearest structure whose placement is listed
/// in `placements`, within `max_search_radius` chunk-region rings.
///
/// Mirrors the two-pass logic in vanilla's
/// `ChunkGenerator.findNearestMapStructure`:
///
/// 1. **Concentric-rings** placements (strongholds) are resolved in one pass
///    from the pre-computed [`GlobalStructureCache`].
/// 2. **Random-spread** placements are searched ring-by-ring outward. Every
///    placement is evaluated at a radius, and the search stops after the first
///    radius where any of them produced a result.
///
/// The best candidate from both passes is returned, as the placement's
/// locate position (chunk min corner plus `locate_offset`).
pub fn find_nearest_structure(
    origin: BlockPos,
    placements: &[&StructurePlacement],
    max_search_radius: i32,
    world_seed: i64,
    global_cache: &GlobalStructureCache,
) -> Option<BlockPos> {
    if placements.is_empty() {
        return None;
    }

    let mut nearest: Option<FoundStructure> = None;

    // ── Pass 1: Concentric-rings (strongholds) ──────────────────────────────
    for p in placements {
        if let StructurePlacementType::ConcentricRings(_) = &p.placement_type
            && let Some(found) = find_nearest_concentric(origin, p, global_cache)
            && nearest
                .as_ref()
                .is_none_or(|n| found.distance_sq < n.distance_sq)
        {
            nearest = Some(found);
        }
    }

    let random_spread: Vec<(&StructurePlacement, &RandomSpreadStructurePlacement)> = placements
        .iter()
        .filter_map(|p| {
            if let StructurePlacementType::RandomSpread(r) = &p.placement_type {
                Some((*p, r))
            } else {
                None
            }
        })
        .collect();

    if !random_spread.is_empty() {
        let chunk_origin_x = origin.0.x >> 4;
        let chunk_origin_z = origin.0.z >> 4;

        for radius in 0..=max_search_radius {
            let mut found_something = false;
            for (placement, spread) in &random_spread {
                if let Some(found) = find_first_random_spread_at_radius(
                    origin,
                    chunk_origin_x,
                    chunk_origin_z,
                    radius,
                    world_seed,
                    placement,
                    spread,
                ) {
                    found_something = true;
                    if nearest
                        .as_ref()
                        .is_none_or(|n| found.distance_sq < n.distance_sq)
                    {
                        nearest = Some(found);
                    }
                }
            }
            if found_something {
                break;
            }
        }
    }

    nearest.map(|f| f.pos)
}

/// Finds the first candidate, in vanilla ring order, that produces one of `target_structures`.
///
/// Returns its locate position. Explorer maps use this instead of pointing at a
/// placement-only candidate whose biome may reject the requested structure.
#[must_use]
#[expect(clippy::too_many_lines)]
pub fn find_nearest_structure_start(
    origin: BlockPos,
    structure_set: &StructureSet,
    target_structures: &[pumpkin_data::structures::StructureKeys],
    max_search_radius: i32,
    generator: &WorldGenerator,
) -> Option<BlockPos> {
    use crate::{
        ProtoChunk,
        biome::{BiomeSupplier, MultiNoiseBiomeSupplier},
        generation::{
            biome_coords,
            noise::router::{
                multi_noise_sampler::{MultiNoiseSampler, MultiNoiseSamplerBuilderOptions},
                surface_height_sampler::{
                    SurfaceHeightEstimateSampler, SurfaceHeightSamplerBuilderOptions,
                },
            },
            positions::chunk_pos::{start_block_x, start_block_z},
            structure::{
                lazily_generate_structure,
                placement::should_generate_structure,
                structures::{StructureGeneratorContext, create_chunk_random},
            },
        },
    };
    use pumpkin_data::structures::Structure;

    let WorldGenerator::Noise(noise_generator) = generator else {
        return None;
    };
    let StructurePlacementType::RandomSpread(placement) = &structure_set.placement.placement_type
    else {
        return None;
    };

    let chunk_origin_x = origin.0.x >> 4;
    let chunk_origin_z = origin.0.z >> 4;
    let region_origin_x = floor_div(chunk_origin_x, placement.spacing);
    let region_origin_z = floor_div(chunk_origin_z, placement.spacing);
    let world_seed = noise_generator.random_config.seed as i64;
    let global_cache = &noise_generator.global_structure_cache;

    for radius in 0..=max_search_radius {
        for region_x_offset in -radius..=radius {
            for region_z_offset in -radius..=radius {
                if region_x_offset.abs() != radius && region_z_offset.abs() != radius {
                    continue;
                }
                let (chunk_x, chunk_z) = get_structure_chunk_in_region(
                    placement,
                    world_seed,
                    region_origin_x + region_x_offset,
                    region_origin_z + region_z_offset,
                    structure_set.placement.salt,
                );
                let placement_chunk = ProtoChunk::new(chunk_x, chunk_z, generator);
                if !should_generate_structure(
                    &structure_set.placement,
                    &noise_generator.structure_calculator,
                    chunk_x,
                    chunk_z,
                    global_cache,
                    &placement_chunk,
                    &[],
                ) {
                    continue;
                }

                for &key in target_structures {
                    let start =
                        global_cache.get_or_compute_structure_start(key, chunk_x, chunk_z, || {
                            let start_x = start_block_x(chunk_x);
                            let start_z = start_block_z(chunk_z);
                            let settings = noise_generator.settings;
                            let mut height_sampler = SurfaceHeightEstimateSampler::generate(
                                &noise_generator.base_router.surface_estimator,
                                &SurfaceHeightSamplerBuilderOptions::new(
                                    biome_coords::from_block(start_x),
                                    biome_coords::from_block(start_z),
                                    4,
                                    settings.shape.min_y as i32,
                                    settings.shape.height as i32,
                                    (settings.shape.height
                                        / settings.shape.vertical_cell_block_count() as u16)
                                        as usize,
                                ),
                            );
                            let mut biome_sampler = MultiNoiseSampler::generate(
                                &noise_generator.base_router.multi_noise,
                                &MultiNoiseSamplerBuilderOptions::new(0, 0, 0),
                            );
                            let biome_supplier: &dyn BiomeSupplier =
                                &MultiNoiseBiomeSupplier::OVERWORLD;
                            let context = StructureGeneratorContext {
                                seed: world_seed,
                                chunk_x,
                                chunk_z,
                                random: create_chunk_random(world_seed, chunk_x, chunk_z),
                                sea_level: settings.sea_level,
                                min_y: noise_generator.dimension.min_y,
                                height_sampler: Some(&mut height_sampler),
                                structure_key: Some(key),
                            };
                            lazily_generate_structure(
                                &key,
                                Structure::get(&key),
                                context,
                                biome_supplier,
                                &mut biome_sampler,
                            )
                        });
                    // Vanilla `getStructureGeneratingAt` returns the first hit's
                    // `getLocatePos(start.getChunkPos())`, not the nearest of the ring.
                    if start.is_some() {
                        return Some(get_locate_pos(
                            &structure_set.placement,
                            Vector2::new(chunk_x, chunk_z),
                        ));
                    }
                }
            }
        }
    }
    None
}

fn find_nearest_concentric(
    origin: BlockPos,
    placement: &StructurePlacement,
    global_cache: &GlobalStructureCache,
) -> Option<FoundStructure> {
    let strongholds = global_cache.get_stronghold_chunks();

    let ox = f64::from(origin.0.x);
    let oz = f64::from(origin.0.z);

    // Vanilla ranks ring chunks by distance from their centre (the constant Y=32
    // term does not change the order); the earliest of equally near chunks wins.
    let mut closest: Option<((i32, i32), f64)> = None;
    for &(cx, cz) in strongholds {
        let dx = f64::from((cx << 4) + 8) - ox;
        let dz = f64::from((cz << 4) + 8) - oz;
        let dist_sq = dx * dx + dz * dz;
        if closest.is_none_or(|(_, best)| dist_sq < best) {
            closest = Some(((cx, cz), dist_sq));
        }
    }

    closest.map(|((cx, cz), _)| {
        FoundStructure::new(origin, get_locate_pos(placement, Vector2::new(cx, cz)))
    })
}

/// Port of vanilla's random-spread `getNearestGeneratedStructure`: walks the edge
/// cells of the ring at `radius` (x outer, z inner) and returns the first potential
/// structure chunk that passes the frequency check, as its locate position.
fn find_first_random_spread_at_radius(
    origin: BlockPos,
    chunk_origin_x: i32,
    chunk_origin_z: i32,
    radius: i32,
    world_seed: i64,
    placement: &StructurePlacement,
    spread: &RandomSpreadStructurePlacement,
) -> Option<FoundStructure> {
    let spacing = spread.spacing;

    for rx_off in -radius..=radius {
        for rz_off in -radius..=radius {
            if rx_off.abs() != radius && rz_off.abs() != radius {
                continue;
            }

            let rx = floor_div(chunk_origin_x, spacing) + rx_off;
            let rz = floor_div(chunk_origin_z, spacing) + rz_off;

            let (struct_cx, struct_cz) =
                get_structure_chunk_in_region(spread, world_seed, rx, rz, placement.salt);

            // StructureCheck.checkStart rejects chunks failing the frequency reducer.
            if !apply_additional_chunk_restrictions(placement, world_seed, struct_cx, struct_cz) {
                continue;
            }

            return Some(FoundStructure::new(
                origin,
                get_locate_pos(placement, Vector2::new(struct_cx, struct_cz)),
            ));
        }
    }

    None
}
