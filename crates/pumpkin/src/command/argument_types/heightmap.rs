use pumpkin_data::translation;
use pumpkin_util::text::TextComponent;
use pumpkin_world::chunk::ChunkHeightmapType;
use std::pin::Pin;

use crate::command::{
    argument_types::argument_type::{ArgumentType, JavaClientArgumentType},
    context::command_context::CommandContext,
    errors::{command_syntax_error::CommandSyntaxError, error_types::CommandErrorType},
    string_reader::StringReader,
    suggestion::suggestions::{Suggestions, SuggestionsBuilder},
};

pub const INVALID_ERROR_TYPE: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENT_ENUM_INVALID,
    translation::java::ARGUMENT_ENUM_INVALID,
);

/// The heightmap types kept after worldgen, in vanilla declaration order.
///
/// Mirrors vanilla `HeightmapTypeArgument.keptTypes`
/// (`net/minecraft/commands/arguments/HeightmapTypeArgument.java:16-18`), which
/// filters `Heightmap.Types.values()` by `keepAfterWorldgen()`
/// (`net/minecraft/world/level/levelgen/Heightmap.java:180-182`): every type whose
/// usage is not `WORLDGEN`, i.e. `WORLD_SURFACE`, `OCEAN_FLOOR`,
/// `MOTION_BLOCKING` and `MOTION_BLOCKING_NO_LEAVES`
/// (`net/minecraft/world/level/levelgen/Heightmap.java:144-155`).
pub const KEPT_TYPES: [ChunkHeightmapType; 4] = [
    ChunkHeightmapType::WorldSurface,
    ChunkHeightmapType::OceanFloor,
    ChunkHeightmapType::MotionBlocking,
    ChunkHeightmapType::MotionBlockingNoLeaves,
];

/// Returns the lower-cased serialization name of a heightmap type.
///
/// Vanilla serializes each type with its upper-case key
/// (`net/minecraft/world/level/levelgen/Heightmap.java:188-191`) and lower-cases it
/// through the argument's codec and `convertId`
/// (`net/minecraft/commands/arguments/HeightmapTypeArgument.java:12-14,32-35`).
#[must_use]
pub const fn heightmap_name(heightmap_type: ChunkHeightmapType) -> &'static str {
    match heightmap_type {
        ChunkHeightmapType::WorldSurface => "world_surface",
        ChunkHeightmapType::OceanFloor => "ocean_floor",
        ChunkHeightmapType::MotionBlocking => "motion_blocking",
        ChunkHeightmapType::MotionBlockingNoLeaves => "motion_blocking_no_leaves",
    }
}

/// Parses a lower-cased heightmap type name back into a [`ChunkHeightmapType`].
#[must_use]
pub fn heightmap_from_name(name: &str) -> Option<ChunkHeightmapType> {
    KEPT_TYPES
        .iter()
        .copied()
        .find(|heightmap_type| heightmap_name(*heightmap_type).eq_ignore_ascii_case(name))
}

/// An argument type for one of the heightmap types that are kept after worldgen.
///
/// Ported from vanilla `HeightmapTypeArgument`
/// (`net/minecraft/commands/arguments/HeightmapTypeArgument.java:11-36`).
pub struct HeightmapTypeArgumentType;

impl ArgumentType for HeightmapTypeArgumentType {
    type Item = ChunkHeightmapType;

    /// Vanilla `StringRepresentableArgument.parse`
    /// (`net/minecraft/commands/arguments/StringRepresentableArgument.java:39-43`)
    /// reads an unquoted string and resolves it through the lower-case codec.
    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        let string = reader.read_unquoted_string();
        heightmap_from_name(&string)
            .ok_or_else(|| INVALID_ERROR_TYPE.create(reader, TextComponent::text(string)))
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::Heightmap
    }

    /// Suggests all kept type names, lower-cased, mirroring vanilla
    /// `StringRepresentableArgument.listSuggestions`
    /// (`net/minecraft/commands/arguments/StringRepresentableArgument.java:45-54`).
    fn list_suggestions<'a>(
        &'a self,
        _context: &'a CommandContext,
        mut builder: SuggestionsBuilder,
    ) -> Pin<Box<dyn Future<Output = Suggestions> + Send + 'a>> {
        Box::pin(async move {
            for heightmap_type in KEPT_TYPES {
                builder = builder.filter_and_suggest_one(heightmap_name(heightmap_type));
            }
            builder.build()
        })
    }

    /// Vanilla limits examples to the first two kept names, lower-cased
    /// (`net/minecraft/commands/arguments/StringRepresentableArgument.java:56-61`).
    fn examples(&self) -> Vec<String> {
        vec!["world_surface".to_string(), "ocean_floor".to_string()]
    }
}

impl_copy_get!(HeightmapTypeArgumentType, ChunkHeightmapType);

#[cfg(test)]
mod test {
    use super::{HeightmapTypeArgumentType, heightmap_from_name};
    use crate::command::{
        argument_types::argument_type::ArgumentType, string_reader::StringReader,
    };
    use pumpkin_world::chunk::ChunkHeightmapType;

    #[test]
    fn parse_heightmap_types() {
        assert_eq!(
            heightmap_from_name("world_surface"),
            Some(ChunkHeightmapType::WorldSurface)
        );
        assert_eq!(
            heightmap_from_name("ocean_floor"),
            Some(ChunkHeightmapType::OceanFloor)
        );
        assert_eq!(
            heightmap_from_name("motion_blocking"),
            Some(ChunkHeightmapType::MotionBlocking)
        );
        assert_eq!(
            heightmap_from_name("motion_blocking_no_leaves"),
            Some(ChunkHeightmapType::MotionBlockingNoLeaves)
        );
        // Vanilla lower-cases ids (`HeightmapTypeArgument.java:12-14,32-35`).
        assert_eq!(
            heightmap_from_name("MOTION_BLOCKING"),
            Some(ChunkHeightmapType::MotionBlocking)
        );
        assert_eq!(heightmap_from_name("world_surface_wg"), None);
    }

    #[test]
    fn parse_argument() {
        let mut reader = StringReader::new("motion_blocking");
        assert_eq!(
            ArgumentType::parse(&HeightmapTypeArgumentType, &mut reader),
            Ok(ChunkHeightmapType::MotionBlocking)
        );

        let mut reader = StringReader::new("not_a_heightmap");
        assert!(ArgumentType::parse(&HeightmapTypeArgumentType, &mut reader).is_err());
    }
}
