use std::cmp::min;

use pumpkin_util::{
    HeightMap,
    math::{position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::proto_chunk::GenerationCache;

use pumpkin_data::{Block, tag};

use super::can_replace;

pub struct LargeDripstoneFeature;

// Configuration values from
// `assets/datapacks/26_2/data/minecraft/worldgen/configured_feature/large_dripstone.json`
// (the only configured feature of this type in 26.2), mirroring the fields of vanilla
// `LargeDripstoneConfiguration.java:11-21`. Hard-coded following the same convention as
// `HugeRedMushroomFeature`/`HugeBrownMushroomFeature`, whose single datapack usages are
// also folded into constants.

/// Codec default when the JSON omits the field
/// (`LargeDripstoneConfiguration.java:15`).
const FLOOR_TO_CEILING_SEARCH_RANGE: i32 = 30;
/// Bounds of the `column_radius` clamped int provider; generation time only consumes
/// `minInclusive`/`maxInclusive` (`LargeDripstoneFeature.java:52-53`). JSON clamps
/// uniform(3,19) to [3,16].
const COLUMN_RADIUS_MIN: i32 = 3;
const COLUMN_RADIUS_MAX: i32 = 16;
/// `height_scale` uniform float provider [0.4, 2.0) from the JSON.
const HEIGHT_SCALE_MIN: f32 = 0.4;
const HEIGHT_SCALE_MAX: f32 = 2.0;
/// `max_column_radius_to_cave_height_ratio` from the JSON.
const MAX_COLUMN_RADIUS_TO_CAVE_HEIGHT_RATIO: f32 = 0.33;
/// `stalactite_bluntness` uniform float provider [0.3, 0.9) from the JSON.
const STALACTITE_BLUNTNESS_MIN: f32 = 0.3;
const STALACTITE_BLUNTNESS_MAX: f32 = 0.9;
/// `stalagmite_bluntness` uniform float provider [0.4, 1.0) from the JSON.
const STALAGMITE_BLUNTNESS_MIN: f32 = 0.4;
const STALAGMITE_BLUNTNESS_MAX: f32 = 1.0;
/// `wind_speed` uniform float provider [0.0, 0.3) from the JSON.
const WIND_SPEED_MAX: f32 = 0.3;
/// `min_radius_for_wind` from the JSON.
const MIN_RADIUS_FOR_WIND: i32 = 4;
/// `min_bluntness_for_wind` from the JSON.
const MIN_BLUNTNESS_FOR_WIND: f64 = 0.6;

impl LargeDripstoneFeature {
    #[allow(clippy::unused_self)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        // Vanilla `LargeDripstoneFeature.place` (`LargeDripstoneFeature.java:27-82`).
        if !is_empty_or_water(chunk, &pos) {
            return false;
        }

        // `Column.scan` for a `Column.Range` (`Column.java:64-98`,
        // `LargeDripstoneFeature.java:36-45`): the column must terminate on a valid edge
        // (dripstone base / replaceable / lava) on both ends.
        let Some((floor, ceiling)) = scan_column_range(chunk, &pos, FLOOR_TO_CEILING_SEARCH_RANGE)
        else {
            return false;
        };
        let column_height = ceiling - floor - 1;
        if column_height < 4 {
            // `LargeDripstoneFeature.java:47-49`.
            return false;
        }

        // Radius selection (`LargeDripstoneFeature.java:51-53`).
        let max_column_radius_based_on_column_height =
            (column_height as f32 * MAX_COLUMN_RADIUS_TO_CAVE_HEIGHT_RATIO) as i32;
        let max_column_radius =
            max_column_radius_based_on_column_height.clamp(COLUMN_RADIUS_MIN, COLUMN_RADIUS_MAX);
        let radius = random.next_inbetween_i32(COLUMN_RADIUS_MIN, max_column_radius);

        // Stalactite grows down from the ceiling, stalagmite grows up from the floor
        // (`makeDripstone`, `LargeDripstoneFeature.java:54-59`, `84-93`).
        let mut stalactite = LargeDripstone {
            root: BlockPos::new(pos.0.x, ceiling - 1, pos.0.z),
            pointing_up: false,
            radius,
            bluntness: random.next_inbetween_f32(STALACTITE_BLUNTNESS_MIN, STALACTITE_BLUNTNESS_MAX)
                as f64,
            scale: random.next_inbetween_f32(HEIGHT_SCALE_MIN, HEIGHT_SCALE_MAX) as f64,
        };
        let mut stalagmite = LargeDripstone {
            root: BlockPos::new(pos.0.x, floor + 1, pos.0.z),
            pointing_up: true,
            radius,
            bluntness: random.next_inbetween_f32(STALAGMITE_BLUNTNESS_MIN, STALAGMITE_BLUNTNESS_MAX)
                as f64,
            scale: random.next_inbetween_f32(HEIGHT_SCALE_MIN, HEIGHT_SCALE_MAX) as f64,
        };

        // Wind applies only when both speleothems are thick and blunt enough
        // (`LargeDripstoneFeature.java:60-65`, `195-197`); max horizontal offset is
        // 16 - radius.
        let wind = if stalactite.is_suitable_for_wind() && stalagmite.is_suitable_for_wind() {
            WindOffsetter::new(pos.0.y, random, 16 - radius)
        } else {
            WindOffsetter::no_wind()
        };

        // Each speleothem places only if its base ended up embedded in stone
        // (`LargeDripstoneFeature.java:67-75`).
        if stalactite
            .move_back_until_base_is_inside_stone_and_shrink_radius_if_necessary(chunk, &wind)
        {
            stalactite.place_blocks(chunk, random, &wind);
        }

        if stalagmite
            .move_back_until_base_is_inside_stone_and_shrink_radius_if_necessary(chunk, &wind)
        {
            stalagmite.place_blocks(chunk, random, &wind);
        }

        true
    }
}

struct LargeDripstone {
    root: BlockPos,
    pointing_up: bool,
    radius: i32,
    bluntness: f64,
    scale: f64,
}

impl LargeDripstone {
    /// `LargeDripstoneFeature.java:122-124`.
    fn get_height(&self) -> i32 {
        self.get_height_at_radius(0.0)
    }

    /// `LargeDripstoneFeature.java:158-160`, backed by
    /// `SpeleothemUtils.getSpeleothemHeight` (`SpeleothemUtils.java:17-30`).
    fn get_height_at_radius(&self, check_radius: f32) -> i32 {
        get_speleothem_height(check_radius, self.radius, self.scale, self.bluntness) as i32
    }

    /// `moveBackUntilBaseIsInsideStoneAndShrinkRadiusIfNecessary`
    /// (`LargeDripstoneFeature.java:134-156`): walks the root towards the stone until the
    /// disc at the root is mostly embedded, halving the radius every 10 failed tries.
    fn move_back_until_base_is_inside_stone_and_shrink_radius_if_necessary<T: GenerationCache>(
        &mut self,
        chunk: &T,
        wind: &WindOffsetter,
    ) -> bool {
        while self.radius > 1 {
            let mut new_root = self.root;
            let max_tries = min(10, self.get_height());

            for _ in 0..max_tries {
                if GenerationCache::get_block_state(chunk, &new_root.0).to_block_id()
                    == Block::LAVA.id
                {
                    return false;
                }

                if is_circle_mostly_embedded_in_stone(chunk, wind.offset(new_root), self.radius) {
                    self.root = new_root;
                    return true;
                }

                new_root = if self.pointing_up {
                    new_root.down()
                } else {
                    new_root.up()
                };
            }

            self.radius /= 2;
        }

        false
    }

    /// `placeBlocks` (`LargeDripstoneFeature.java:162-193`): fills the cone of dripstone
    /// blocks, jittering heights and stopping at overworld stone once surfaced.
    fn place_blocks<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        wind: &WindOffsetter,
    ) {
        for dx in -self.radius..=self.radius {
            for dz in -self.radius..=self.radius {
                let current_radius = ((dx * dx + dz * dz) as f32).sqrt();
                if current_radius <= self.radius as f32 {
                    let mut height = self.get_height_at_radius(current_radius);
                    if height > 0 {
                        if random.next_f32() < 0.2 {
                            height = (height as f32 * random.next_inbetween_f32(0.8, 1.0)) as i32;
                        }

                        let mut pos = self.root.offset(Vector3::new(dx, 0, dz));
                        let mut has_been_out_of_stone = false;
                        // Only upward-growing columns stop at the world surface
                        // (`LargeDripstoneFeature.java:175`).
                        let max_y = if self.pointing_up {
                            chunk.get_top_y(&HeightMap::WorldSurfaceWg, pos.0.x, pos.0.z)
                        } else {
                            i32::MAX
                        };

                        let mut i = 0;
                        while i < height && pos.0.y < max_y {
                            let wind_adjusted_pos = wind.offset(pos);
                            let wind_block_id =
                                GenerationCache::get_block_state(chunk, &wind_adjusted_pos.0)
                                    .to_block_id();
                            if is_empty_or_water_or_lava_id(wind_block_id) {
                                has_been_out_of_stone = true;
                                chunk.set_block_state(
                                    &wind_adjusted_pos.0,
                                    Block::DRIPSTONE_BLOCK.default_state,
                                );
                            } else if has_been_out_of_stone
                                && wind_block_id.has_tag(tag::Block::MINECRAFT_BASE_STONE_OVERWORLD)
                            {
                                break;
                            }

                            pos = if self.pointing_up {
                                pos.up()
                            } else {
                                pos.down()
                            };
                            i += 1;
                        }
                    }
                }
            }
        }
    }

    /// `isSuitableForWind` (`LargeDripstoneFeature.java:195-197`) against the fixed
    /// `min_radius_for_wind`/`min_bluntness_for_wind` config.
    fn is_suitable_for_wind(&self) -> bool {
        self.radius >= MIN_RADIUS_FOR_WIND && self.bluntness >= MIN_BLUNTNESS_FOR_WIND
    }
}

struct WindOffsetter {
    origin_y: i32,
    wind_speed: Option<Vector3<f64>>,
    max_offset: i32,
}

impl WindOffsetter {
    /// `WindOffsetter` ctor (`LargeDripstoneFeature.java:205-211`): a random horizontal
    /// drift whose magnitude comes from the `wind_speed` provider (uniform [0.0, 0.3)).
    fn new(origin_y: i32, random: &mut RandomGenerator, max_offset: i32) -> Self {
        let speed = random.next_inbetween_f32(0.0, WIND_SPEED_MAX) as f64;
        let direction = random.next_inbetween_f32(0.0, std::f32::consts::PI) as f64;
        Self {
            origin_y,
            wind_speed: Some(Vector3::new(
                direction.cos() * speed,
                0.0,
                direction.sin() * speed,
            )),
            max_offset,
        }
    }

    /// `WindOffsetter.noWind` (`LargeDripstoneFeature.java:213-221`).
    const fn no_wind() -> Self {
        Self {
            origin_y: 0,
            wind_speed: None,
            max_offset: 0,
        }
    }

    /// `offset` (`LargeDripstoneFeature.java:223-233`): displaces each block horizontally
    /// proportionally to its vertical distance from the feature origin.
    fn offset(&self, pos: BlockPos) -> BlockPos {
        let Some(wind_speed) = self.wind_speed else {
            return pos;
        };

        let dy = self.origin_y - pos.0.y;
        let total_wind_adjust =
            Vector3::new(wind_speed.x * dy as f64, 0.0, wind_speed.z * dy as f64);
        let dx = (total_wind_adjust.x.floor() as i32).clamp(-self.max_offset, self.max_offset);
        let dz = (total_wind_adjust.z.floor() as i32).clamp(-self.max_offset, self.max_offset);
        pos.offset(Vector3::new(dx, 0, dz))
    }
}

/// Port of `Column.scan` restricted to the `Column.Range` outcome needed here
/// (`Column.java:64-98`): scans up/down while blocks stay empty-or-water, and both scans
/// must end on a valid edge (dripstone base / replaceable / lava). Returns
/// `(floor, ceiling)`; `None` covers the `Line`/`Ray` outcomes and the non-positive
/// height that vanilla rejects in the `Range` constructor (`Column.java:134-136`).
fn scan_column_range<T: GenerationCache>(
    chunk: &T,
    origin: &BlockPos,
    search_range: i32,
) -> Option<(i32, i32)> {
    let inside_column = |pos: &BlockPos| -> bool { is_empty_or_water(chunk, pos) };
    let valid_edge = |pos: &BlockPos| -> bool {
        let id = GenerationCache::get_block_state(chunk, &pos.0).to_block_id();
        can_replace(id) || id == Block::LAVA.id
    };

    if !inside_column(origin) {
        return None;
    }

    let scan_direction = |start_y: i32, up: bool| -> Option<i32> {
        // `Column.scanDirection` (`Column.java:82-98`): advance while inside the column
        // and under the search budget, then require a valid edge at the stop position.
        let step = if up { 1 } else { -1 };
        let mut y = start_y;
        let mut i = 1;
        while i < search_range && inside_column(&BlockPos::new(origin.0.x, y, origin.0.z)) {
            y += step;
            i += 1;
        }
        valid_edge(&BlockPos::new(origin.0.x, y, origin.0.z)).then_some(y)
    };

    let ceiling = scan_direction(origin.0.y, true)?;
    let floor = scan_direction(origin.0.y, false)?;

    (ceiling > floor).then_some((floor, ceiling))
}

/// `SpeleothemUtils.isEmptyOrWater(BlockState)` (`SpeleothemUtils.java:126-128`) applied
/// through the generation cache.
fn is_empty_or_water<T: GenerationCache>(chunk: &T, pos: &BlockPos) -> bool {
    chunk.is_air(&pos.0) || is_empty_or_water_id(get_block_id(chunk, pos))
}

/// `SpeleothemUtils.isEmptyOrWaterOrLava(LevelAccessor, BlockPos)`
/// (`SpeleothemUtils.java:55-57`, predicate at `:134-136`) applied through the
/// generation cache.
fn is_empty_or_water_or_lava<T: GenerationCache>(chunk: &T, pos: &BlockPos) -> bool {
    is_empty_or_water_or_lava_id(get_block_id(chunk, pos))
}

fn get_block_id<T: GenerationCache>(chunk: &T, pos: &BlockPos) -> pumpkin_data::BlockId {
    GenerationCache::get_block_state(chunk, &pos.0).to_block_id()
}

/// `SpeleothemUtils.isEmptyOrWater(BlockState)` (`SpeleothemUtils.java:126-128`).
fn is_empty_or_water_id(id: pumpkin_data::BlockId) -> bool {
    id == Block::WATER.id || id == Block::AIR.id
}

/// `SpeleothemUtils.isEmptyOrWaterOrLava(BlockState)` (`SpeleothemUtils.java:134-136`).
fn is_empty_or_water_or_lava_id(id: pumpkin_data::BlockId) -> bool {
    id == Block::WATER.id || id == Block::LAVA.id || id == Block::AIR.id
}

/// `SpeleothemUtils.isCircleMostlyEmbeddedInStone` (`SpeleothemUtils.java:32-49`): the
/// centre and every sampled point on the circle of the given XZ radius must be
/// non-empty.
fn is_circle_mostly_embedded_in_stone<T: GenerationCache>(
    chunk: &T,
    center: BlockPos,
    xz_radius: i32,
) -> bool {
    if is_empty_or_water_or_lava(chunk, &center) {
        return false;
    }

    let arc_length = 6.0f32;
    let angle_increment = arc_length / xz_radius as f32;

    let mut angle = 0.0f32;
    while angle < std::f32::consts::PI * 2.0 {
        let dx = (angle.cos() * xz_radius as f32) as i32;
        let dz = (angle.sin() * xz_radius as f32) as i32;
        if is_empty_or_water_or_lava(chunk, &center.offset(Vector3::new(dx, 0, dz))) {
            return false;
        }
        angle += angle_increment;
    }

    true
}

/// `SpeleothemUtils.getSpeleothemHeight` (`SpeleothemUtils.java:17-30`): analytic cone
/// profile mapping an XZ distance from the axis to a column height.
fn get_speleothem_height(
    xz_distance_from_center: f32,
    speleothem_radius: i32,
    scale: f64,
    bluntness: f64,
) -> f64 {
    let xz_distance_from_center = f64::from(xz_distance_from_center).max(bluntness);

    let r = xz_distance_from_center / f64::from(speleothem_radius) * 0.384;
    let part1 = 0.75 * r.powf(1.333_333_4);
    let part2 = r.powf(0.666_666_7);
    let part3 = 0.333_333_34 * r.ln();
    let height_relative_to_max_radius = scale * (part1 - part2 - part3);
    height_relative_to_max_radius.max(0.0) / 0.384 * f64::from(speleothem_radius)
}
