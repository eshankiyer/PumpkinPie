use pumpkin_data::{Block, BlockId};
use pumpkin_util::biome::Weather;

/// Kind of precipitation falling at a position, mirroring vanilla's
/// `Biome.Precipitation` (`NONE`, `RAIN`, `SNOW`): whether the biome has
/// precipitation at all, and if so, whether it falls as rain or snow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precipitation {
    None,
    Rain,
    Snow,
}

/// Below this temperature, precipitation falls as snow instead of rain and
/// water is cold enough to freeze. Matches the threshold already used by the
/// worldgen `FreezeTopLayerFeature`
/// (`pumpkin-world/src/generation/feature/features/freeze_top_layer.rs`).
const FREEZE_AND_SNOW_TEMPERATURE: f32 = 0.15;

/// Classifies the precipitation falling at a world position from its biome
/// `weather`, mirroring vanilla's `Biome#getPrecipitationAt`. Biomes with no
/// precipitation (deserts, badlands, ...) always report `Precipitation::None`.
/// Otherwise the position-adjusted temperature (`Weather::compute_temperature`)
/// decides rain versus snow against `FREEZE_AND_SNOW_TEMPERATURE`.
///
/// This is F1: `World::is_raining_at` and every other runtime weather check
/// that belongs in this module (freezing water, accumulating snow, filling
/// cauldrons) is gated by the `Precipitation` value this function returns.
#[must_use]
#[expect(
    clippy::todo,
    unused_variables,
    reason = "scaffold stub, filled by spec 23"
)]
pub fn precipitation_kind(weather: &Weather, x: f64, y: i32, z: f64, sea_level: i32) -> Precipitation {
    todo!("STUB: see tests::precipitation_kind_none_when_biome_has_no_precipitation")
}

/// Vanilla's runtime water-to-ice freezing: a full water block directly below
/// a `Precipitation::Snow` position turns to `Block::ICE`. `precipitation_kind`
/// already applied `FREEZE_AND_SNOW_TEMPERATURE` to produce `kind`; this
/// function only decides what happens to `below_block` once that
/// classification is known.
#[must_use]
#[expect(
    clippy::todo,
    unused_variables,
    reason = "scaffold stub, filled by spec 24"
)]
#[allow(dead_code, reason = "not wired into the chunk tick loop by this scaffold")]
pub fn freezes_to_ice(kind: Precipitation, below_block: &Block) -> bool {
    todo!("STUB: see tests::freezes_to_ice_water_when_snowing")
}

/// Vanilla's runtime snow-layer accumulation: whether a `Precipitation::Snow`
/// position may gain a fresh snow layer this tick. `top_block` is the block
/// currently occupying the target position (must be air) and `below_block` is
/// what the snow layer would rest on (must not be air, and must not carry the
/// `MINECRAFT_CANNOT_SUPPORT_SNOW_LAYER` tag, e.g. `Block::ICE`).
#[must_use]
#[expect(
    clippy::todo,
    unused_variables,
    reason = "scaffold stub, filled by spec 25"
)]
#[allow(dead_code, reason = "not wired into the chunk tick loop by this scaffold")]
pub fn snow_should_accumulate(kind: Precipitation, top_block: &Block, below_block: &Block) -> bool {
    todo!("STUB: see tests::snow_accumulates_on_open_ground_when_snowing")
}

/// Vanilla's runtime cauldron rain fill-up: an empty `BlockId::CAULDRON`
/// becomes a level-1 `BlockId::WATER_CAULDRON`, and an existing water
/// cauldron below the max level (3) gains one level. Returns the new
/// `(BlockId, level)` to set, or `None` if nothing should change: `kind` is
/// not `Precipitation::Rain`, `cauldron` is not a cauldron that collects rain
/// (lava and powder snow cauldrons are unaffected), or the water cauldron is
/// already full.
#[must_use]
#[expect(
    clippy::todo,
    unused_variables,
    reason = "scaffold stub, filled by spec 26"
)]
#[allow(dead_code, reason = "not wired into the chunk tick loop by this scaffold")]
pub fn cauldron_fill_from_rain(kind: Precipitation, cauldron: BlockId, level: u8) -> Option<(BlockId, u8)> {
    todo!("STUB: see tests::empty_cauldron_becomes_level_one_water_cauldron_in_rain")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::chunk::Biome;

    #[test]
    fn precipitation_kind_none_when_biome_has_no_precipitation() {
        assert_eq!(
            precipitation_kind(&Biome::DESERT.weather, 0.0, 63, 0.0, 63),
            Precipitation::None
        );
    }

    #[test]
    fn precipitation_kind_rain_when_warm_and_precipitating() {
        assert_eq!(
            precipitation_kind(&Biome::PLAINS.weather, 0.0, 63, 0.0, 63),
            Precipitation::Rain
        );
    }

    #[test]
    fn precipitation_kind_snow_when_cold_and_precipitating() {
        assert_eq!(
            precipitation_kind(&Biome::SNOWY_PLAINS.weather, 0.0, 63, 0.0, 63),
            Precipitation::Snow
        );
    }

    #[test]
    fn freezes_to_ice_water_when_snowing() {
        assert!(freezes_to_ice(Precipitation::Snow, &Block::WATER));
    }

    #[test]
    fn freezes_to_ice_ignores_rain() {
        assert!(!freezes_to_ice(Precipitation::Rain, &Block::WATER));
    }

    #[test]
    fn freezes_to_ice_ignores_non_water_blocks() {
        assert!(!freezes_to_ice(Precipitation::Snow, &Block::LAVA));
    }

    #[test]
    fn snow_accumulates_on_open_ground_when_snowing() {
        assert!(snow_should_accumulate(
            Precipitation::Snow,
            &Block::AIR,
            &Block::DIRT
        ));
    }

    #[test]
    fn snow_does_not_accumulate_when_raining() {
        assert!(!snow_should_accumulate(
            Precipitation::Rain,
            &Block::AIR,
            &Block::DIRT
        ));
    }

    #[test]
    fn snow_does_not_accumulate_when_top_is_occupied() {
        assert!(!snow_should_accumulate(
            Precipitation::Snow,
            &Block::STONE,
            &Block::DIRT
        ));
    }

    #[test]
    fn snow_does_not_accumulate_over_ice() {
        assert!(!snow_should_accumulate(
            Precipitation::Snow,
            &Block::AIR,
            &Block::ICE
        ));
    }

    #[test]
    fn empty_cauldron_becomes_level_one_water_cauldron_in_rain() {
        assert_eq!(
            cauldron_fill_from_rain(Precipitation::Rain, BlockId::CAULDRON, 0),
            Some((BlockId::WATER_CAULDRON, 1))
        );
    }

    #[test]
    fn water_cauldron_gains_a_level_in_rain() {
        assert_eq!(
            cauldron_fill_from_rain(Precipitation::Rain, BlockId::WATER_CAULDRON, 1),
            Some((BlockId::WATER_CAULDRON, 2))
        );
    }

    #[test]
    fn full_water_cauldron_does_not_overfill() {
        assert_eq!(
            cauldron_fill_from_rain(Precipitation::Rain, BlockId::WATER_CAULDRON, 3),
            None
        );
    }

    #[test]
    fn lava_cauldron_is_unaffected_by_rain() {
        assert_eq!(
            cauldron_fill_from_rain(Precipitation::Rain, BlockId::LAVA_CAULDRON, 3),
            None
        );
    }

    #[test]
    fn cauldron_unaffected_when_not_raining() {
        assert_eq!(
            cauldron_fill_from_rain(Precipitation::Snow, BlockId::CAULDRON, 0),
            None
        );
    }
}
