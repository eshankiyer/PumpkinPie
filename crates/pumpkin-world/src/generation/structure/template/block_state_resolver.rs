//! Block state resolution from template palette entries.
//!
//! This module handles converting NBT palette entries (block name + properties)
//! to the runtime block state IDs used by the world, with support for rotation
//! and mirroring transformations.

use pumpkin_data::{Block, BlockState, Mirror, Rotation};
use tracing::warn;

use super::PaletteEntry;

/// Resolves template palette entries to block state IDs.
///
/// This resolver handles:
/// - Looking up blocks by name
/// - Applying block state properties
/// - Rotating/mirroring the state, by delegating to the shared
///   [`Rotation::apply_to_props`] / [`Mirror::apply_to_props`] transforms so that
///   templates get the same treatment vanilla's per-block `rotate`/`mirror`
///   overrides give (`StructureTemplate.Palette`: `state.mirror(m).rotate(r)`).
pub struct BlockStateResolver;

impl BlockStateResolver {
    /// Resolves a palette entry to a block state, applying rotation and mirror transforms.
    ///
    /// Returns the resolved `BlockState` or `None` if the block is not found.
    #[must_use]
    pub fn resolve(
        entry: &PaletteEntry,
        rotation: Rotation,
        mirror: Mirror,
    ) -> Option<&'static BlockState> {
        // Strip minecraft: prefix if present
        let block_name = entry.name.strip_prefix("minecraft:").unwrap_or(&entry.name);

        // Find the block
        let block = Block::from_name(&entry.name).or_else(|| Block::from_registry_key(block_name));

        let Some(block) = block else {
            warn!("Unknown block in template: {}", entry.name);
            return None;
        };

        // Blocks with no state properties have nothing to transform.
        let Some(default_props) = block.properties(block.default_state.id) else {
            return Some(block.default_state);
        };

        // Untransformed state, straight from the palette entry.
        let base_props = if entry.properties.is_empty() {
            default_props
        } else {
            let props_slice = entry
                .properties
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>();
            block.from_properties(&props_slice)
        };

        if rotation == Rotation::None && mirror == Mirror::None {
            return Some(BlockState::from_id(base_props.to_state_id(block)));
        }

        // `to_props` hands back the block's own `'static` property strings, which is
        // exactly what the shared transforms want; no interning or leaking needed.
        let mut props = base_props.to_props();
        // Vanilla mirrors first and rotates second; `Mirror::apply_to_props` reads the
        // pre-mirror facing to decide the stair shape remap, so the order matters.
        mirror.apply_to_props(block.name, &mut props);
        rotation.apply_to_props(block.name, &mut props);

        let state_id = block.from_properties(&props).to_state_id(block);
        Some(BlockState::from_id(state_id))
    }

    /// Resolves a palette entry without any transformation.
    #[must_use]
    pub fn resolve_simple(entry: &PaletteEntry) -> Option<&'static BlockState> {
        Self::resolve(entry, Rotation::None, Mirror::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, properties: &[(&str, &str)]) -> PaletteEntry {
        PaletteEntry::with_properties(
            name.to_string(),
            properties
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        )
    }

    fn expect_state(name: &str, properties: &[(&str, &str)]) -> &'static BlockState {
        BlockStateResolver::resolve_simple(&entry(name, properties)).expect("known block")
    }

    #[test]
    fn resolve_simple_block() {
        let entry = PaletteEntry::new("minecraft:stone".to_string());
        let state = BlockStateResolver::resolve_simple(&entry);
        assert!(state.is_some());
    }

    #[test]
    fn resolve_with_properties() {
        let state = expect_state(
            "minecraft:oak_stairs",
            &[
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "straight"),
                ("waterlogged", "false"),
            ],
        );
        assert_eq!(state.id.to_block_id(), Block::OAK_STAIRS.id);
    }

    #[test]
    fn unknown_block_returns_none() {
        let entry = PaletteEntry::new("minecraft:nonexistent_block".to_string());
        let state = BlockStateResolver::resolve_simple(&entry);
        assert!(state.is_none());
    }

    // HorizontalDirectionalBlock.java:21-23
    #[test]
    fn rotation_transforms_facing() {
        let source = entry(
            "minecraft:furnace",
            &[("facing", "north"), ("lit", "false")],
        );
        for (rotation, want) in [
            (Rotation::None, "north"),
            (Rotation::Clockwise90, "east"),
            (Rotation::Rotate180, "south"),
            (Rotation::CounterClockwise90, "west"),
        ] {
            let got = BlockStateResolver::resolve(&source, rotation, Mirror::None).unwrap();
            assert_eq!(
                got.id,
                expect_state("minecraft:furnace", &[("facing", want), ("lit", "false")]).id,
                "furnace under {rotation:?}"
            );
        }
    }

    // StairBlock.java:169-171: rotation moves the facing and leaves the shape alone.
    #[test]
    fn stairs_rotate_facing_and_keep_shape() {
        let source = entry(
            "minecraft:oak_stairs",
            &[
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "inner_left"),
                ("waterlogged", "false"),
            ],
        );
        for (rotation, want) in [
            (Rotation::None, "north"),
            (Rotation::Clockwise90, "east"),
            (Rotation::Rotate180, "south"),
            (Rotation::CounterClockwise90, "west"),
        ] {
            let got = BlockStateResolver::resolve(&source, rotation, Mirror::None).unwrap();
            assert_eq!(
                got.id,
                expect_state(
                    "minecraft:oak_stairs",
                    &[
                        ("facing", want),
                        ("half", "bottom"),
                        ("shape", "inner_left"),
                        ("waterlogged", "false"),
                    ],
                )
                .id,
                "oak stairs under {rotation:?}"
            );
        }
    }

    // BaseRailBlock.java:146-228: the hand-rolled resolver never touched `shape`.
    #[test]
    fn rail_shape_rotates() {
        let source = entry("minecraft:rail", &[("shape", "north_east")]);
        for (rotation, want) in [
            (Rotation::None, "north_east"),
            (Rotation::Clockwise90, "south_east"),
            (Rotation::Rotate180, "south_west"),
            (Rotation::CounterClockwise90, "north_west"),
        ] {
            let got = BlockStateResolver::resolve(&source, rotation, Mirror::None).unwrap();
            assert_eq!(
                got.id,
                expect_state("minecraft:rail", &[("shape", want)]).id,
                "rail under {rotation:?}"
            );
        }
    }

    // DoorBlock.java:257-259
    #[test]
    fn door_hinge_mirrors() {
        let source = entry(
            "minecraft:oak_door",
            &[
                ("facing", "east"),
                ("half", "lower"),
                ("hinge", "left"),
                ("open", "false"),
                ("powered", "false"),
            ],
        );
        let got = BlockStateResolver::resolve(&source, Rotation::None, Mirror::LeftRight).unwrap();
        assert_eq!(
            got.id,
            expect_state(
                "minecraft:oak_door",
                &[
                    ("facing", "east"),
                    ("half", "lower"),
                    ("hinge", "right"),
                    ("open", "false"),
                    ("powered", "false"),
                ],
            )
            .id
        );
    }

    // JigsawBlock.java:41-43
    #[test]
    fn jigsaw_orientation_rotates() {
        let source = entry("minecraft:jigsaw", &[("orientation", "north_up")]);
        let got =
            BlockStateResolver::resolve(&source, Rotation::Clockwise90, Mirror::None).unwrap();
        assert_eq!(
            got.id,
            expect_state("minecraft:jigsaw", &[("orientation", "east_up")]).id
        );
    }

    // FireBlock.java:30 declares no rotate override, so the sides stay put.
    #[test]
    fn fire_sides_are_exempt() {
        let source = entry(
            "minecraft:fire",
            &[
                ("north", "true"),
                ("east", "false"),
                ("south", "false"),
                ("west", "false"),
                ("up", "false"),
                ("age", "0"),
            ],
        );
        let got =
            BlockStateResolver::resolve(&source, Rotation::Clockwise90, Mirror::None).unwrap();
        assert_eq!(
            got.id,
            BlockStateResolver::resolve_simple(&source).unwrap().id
        );
    }

    // A palette entry that lists no properties still describes a real state, and
    // vanilla rotates that state like any other.
    #[test]
    fn default_state_is_still_transformed() {
        let source = PaletteEntry::new("minecraft:rail".to_string());
        let got =
            BlockStateResolver::resolve(&source, Rotation::Clockwise90, Mirror::None).unwrap();
        assert_eq!(
            got.id,
            expect_state("minecraft:rail", &[("shape", "east_west")]).id
        );
    }
}
