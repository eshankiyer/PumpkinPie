use pumpkin_data::{BlockState, BlockStateId, Rotation};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::{
    HeightMap,
    math::{position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};

use super::weighted_random_selector::weighted_index;
use crate::generation::proto_chunk::GenerationCache;
use crate::generation::structure::template::{BlockPlacer, get_template, place_template};
use crate::world::WorldPortalExt;

pub struct TemplateEntry {
    /// `TemplateFeatureConfiguration.TemplateEntry.template` (`TemplateFeatureConfiguration.java:16-19`).
    pub id: &'static str,
    /// `TemplateFeatureConfiguration.TemplateEntry.rotations`, defaulting to every
    /// `Rotation` value when the JSON omits the field
    /// (`TemplateFeatureConfiguration.java:20-23`).
    pub rotations: Vec<Rotation>,
    pub weight: i32,
}

/// `minecraft:template`.
///
/// Vanilla `TemplateFeature.place` (`TemplateFeature.java:22-35`) draws a weighted
/// template entry, then a rotation from that entry, then places the structure template
/// centred on the origin.
pub struct TemplateFeature {
    pub templates: Vec<TemplateEntry>,
}

/// Bridges the generic worldgen cache onto the structure-template placer, which is only
/// implemented for a concrete `ProtoChunk`.
struct CachePlacer<'a, T: GenerationCache>(&'a mut T);

impl<T: GenerationCache> BlockPlacer for CachePlacer<'_, T> {
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId {
        GenerationCache::get_block_state(self.0, pos)
    }

    fn set_block_state(&mut self, pos: &Vector3<i32>, state: &BlockState) {
        GenerationCache::set_block_state(self.0, pos, state);
    }

    fn add_block_entity(&mut self, nbt: NbtCompound) {
        let pos = Vector3::new(
            nbt.get_int("x").unwrap_or(0),
            nbt.get_int("y").unwrap_or(0),
            nbt.get_int("z").unwrap_or(0),
        );
        GenerationCache::add_block_entity(self.0, &pos, nbt);
    }

    fn get_top_y(&self, heightmap: &HeightMap, x: i32, z: i32) -> i32 {
        GenerationCache::get_top_y(self.0, heightmap, x, z)
    }
}

/// The un-rotated half-extent offsets vanilla applies before placing.
///
/// `TemplateFeature.getRotatedOffset` (`TemplateFeature.java:37-39`) rotates the negative
/// unit vector of each horizontal axis and scales it by half that axis' template size, so
/// the placed template ends up centred on the feature origin. `Axis.X.getNegative()` is
/// `WEST` (`-1, 0, 0`) and `Axis.Z.getNegative()` is `NORTH` (`0, 0, -1`).
#[must_use]
pub const fn centering_offset(rotation: Rotation, size: Vector3<i32>) -> (i32, i32) {
    let (x_dx, x_dz) = rotation.rotate_offset(-(size.x / 2), 0);
    let (z_dx, z_dz) = rotation.rotate_offset(0, -(size.z / 2));
    (x_dx + z_dx, x_dz + z_dz)
}

/// Reconciles the two block-rotation conventions.
///
/// Vanilla rotates template blocks about the zero pivot that
/// `StructureTemplate.calculateRelativePosition` supplies
/// (`StructureTemplate.java:248-250`), so `StructureTemplate.transform`
/// (`StructureTemplate.java:539-564`) sends a clockwise-90 block `(x, z)` to `(-z, x)` and the
/// rotated footprint runs negative. This codebase's `Rotation::transform_pos`
/// (`pumpkin-data/src/block_rotation.rs:52-58`) instead sends it to `(size.z - 1 - z, x)`,
/// keeping the footprint in the positive quadrant. The two differ by a whole-footprint
/// shift, corrected here exactly as `jigsaw.rs:561-569` already does for pool elements.
#[must_use]
pub const fn quadrant_correction(rotation: Rotation, size: Vector3<i32>) -> (i32, i32) {
    let (corner_x, corner_z) = rotation.rotate_offset(size.x - 1, size.z - 1);
    (
        if corner_x < 0 { corner_x } else { 0 },
        if corner_z < 0 { corner_z } else { 0 },
    )
}

/// Total XZ shift from the feature origin to the template's placement origin.
#[must_use]
pub const fn template_offset(rotation: Rotation, size: Vector3<i32>) -> (i32, i32) {
    let (center_x, center_z) = centering_offset(rotation, size);
    let (correct_x, correct_z) = quadrant_correction(rotation, size);
    (center_x + correct_x, center_z + correct_z)
}

impl TemplateFeature {
    #[expect(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        _block_registry: &dyn WorldPortalExt,
        _min_y: i8,
        _height: u16,
        _feature_name: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let total: i32 = self.templates.iter().map(|entry| entry.weight).sum();
        if total <= 0 {
            return false;
        }
        // `templates().getRandomOrThrow(random)` (`TemplateFeature.java:26`,
        // `WeightedList.java:88-91`).
        let selection = random.next_bounded_i32(total);
        let weights: Vec<i32> = self.templates.iter().map(|entry| entry.weight).collect();
        let Some(index) = weighted_index(&weights, selection) else {
            return false;
        };
        let entry = &self.templates[index];

        // `Util.getRandom(templateEntry.rotations(), random)` (`TemplateFeature.java:27`).
        if entry.rotations.is_empty() {
            return false;
        }
        let rotation = entry.rotations[random
            .next_bounded_i32(entry.rotations.len() as i32)
            .unsigned_abs() as usize];

        let Some(template) = get_template(entry.id) else {
            tracing::warn!(
                "template feature references missing template '{}'",
                entry.id
            );
            return false;
        };

        let (dx, dz) = template_offset(rotation, template.size);
        let origin = Vector3::new(pos.0.x + dx, pos.0.y, pos.0.z + dz);

        let mut placer = CachePlacer(chunk);
        // Vanilla places with the default `StructurePlaceSettings`, which keeps neither
        // `knownShape` nor an air filter set, so air is written and existing liquids are
        // preserved around waterloggable blocks (`TemplateFeature.java:33-34`).
        place_template(
            &mut placer,
            &template,
            origin,
            (0, 0),
            rotation,
            false,
            true,
            &[],
            None,
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{centering_offset, template_offset};
    use pumpkin_data::Rotation;
    use pumpkin_util::math::vector3::Vector3;

    /// With no rotation the template is shifted west and north by half its footprint.
    #[test]
    fn centering_offset_unrotated_is_negative_half_size() {
        let size = Vector3::new(9, 5, 7);
        assert_eq!(centering_offset(Rotation::None, size), (-4, -3));
    }

    /// `Rotation.CLOCKWISE_90` sends WEST to NORTH and NORTH to EAST, so the X half-extent
    /// lands on -Z and the Z half-extent on +X.
    #[test]
    fn centering_offset_clockwise_90() {
        let size = Vector3::new(9, 5, 7);
        assert_eq!(centering_offset(Rotation::Clockwise90, size), (3, -4));
    }

    #[test]
    fn centering_offset_rotate_180() {
        let size = Vector3::new(9, 5, 7);
        assert_eq!(centering_offset(Rotation::Rotate180, size), (4, 3));
    }

    /// `Rotation.COUNTERCLOCKWISE_90` sends WEST to SOUTH and NORTH to WEST.
    #[test]
    fn centering_offset_counter_clockwise_90() {
        let size = Vector3::new(9, 5, 7);
        assert_eq!(
            centering_offset(Rotation::CounterClockwise90, size),
            (-3, 4)
        );
    }

    /// Odd and even extents both truncate toward zero, matching Java integer division.
    #[test]
    fn centering_offset_truncates_toward_zero() {
        assert_eq!(
            centering_offset(Rotation::None, Vector3::new(1, 1, 1)),
            (0, 0)
        );
        assert_eq!(
            centering_offset(Rotation::None, Vector3::new(8, 1, 8)),
            (-4, -4)
        );
    }

    /// A 7x7 footprint rotated clockwise 90 must still start 3 blocks west and 3 blocks
    /// north of the origin: vanilla's rotated block range is `x in [-6, 0]` shifted by the
    /// `+3` centering offset, i.e. a minimum corner at `origin - 3`. Without the quadrant
    /// correction this codebase's positive-quadrant transform would put it at `origin + 3`.
    #[test]
    fn template_offset_keeps_rotated_footprint_centred() {
        let size = Vector3::new(7, 4, 7);
        for rotation in Rotation::values() {
            assert_eq!(
                template_offset(rotation, size),
                (-3, -3),
                "rotation {rotation:?} must centre a square footprint identically"
            );
        }
    }

    /// A non-square footprint swaps extents under the quarter turns.
    #[test]
    fn template_offset_handles_non_square_footprints() {
        let size = Vector3::new(9, 5, 7);
        assert_eq!(template_offset(Rotation::None, size), (-4, -3));
        // Clockwise 90 puts the rotated 7-wide Z extent on X and the 9-long X extent on Z.
        assert_eq!(template_offset(Rotation::Clockwise90, size), (-3, -4));
        assert_eq!(template_offset(Rotation::Rotate180, size), (-4, -3));
        assert_eq!(
            template_offset(Rotation::CounterClockwise90, size),
            (-3, -4)
        );
    }
}
