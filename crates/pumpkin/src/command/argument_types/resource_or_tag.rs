use std::collections::BTreeSet;
use std::pin::Pin;

use pumpkin_data::structures::StructureSet;
use pumpkin_data::tag::{self, RegistryKey};
use pumpkin_data::translation;
use pumpkin_util::identifier::Identifier;
use pumpkin_util::text::TextComponent;
use pumpkin_world::poi::POI_TYPE_NETHER_PORTAL;

use crate::command::argument_types::FromStringReader;
use crate::command::argument_types::argument_type::{ArgumentType, JavaClientArgumentType};
use crate::command::argument_types::resource_key::BIOME_REGISTRY;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::string_reader::StringReader;
use crate::command::suggestion::suggestions::{Suggestions, SuggestionsBuilder};

pub static STRUCTURE_REGISTRY: Identifier = Identifier::vanilla_static("worldgen/structure");
pub static POI_REGISTRY: Identifier = Identifier::vanilla_static("point_of_interest_type");

pub static ERROR_UNKNOWN_RESOURCE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_RESOURCE_NOT_FOUND,
    translation::java::ARGUMENT_RESOURCE_NOT_FOUND,
);

pub static ERROR_UNKNOWN_TAG: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_RESOURCE_TAG_NOT_FOUND,
    translation::java::ARGUMENT_RESOURCE_TAG_NOT_FOUND,
);

/// A parsed reference to either a single registry entry or a `#`-prefixed tag
/// of entries, as produced by [`ResourceOrTagKeyArgument`] and
/// [`ResourceOrTagArgument`].
#[derive(Debug, Clone)]
pub enum ResourceOrTag {
    Resource(Identifier),
    Tag(Identifier),
}

impl ResourceOrTag {
    /// The user-facing form of this reference (vanilla's `asPrintable`):
    /// `namespace:path` for entries, `#namespace:path` for tags.
    #[must_use]
    pub fn printable(&self) -> String {
        match self {
            Self::Resource(id) => id.to_string(),
            Self::Tag(id) => format!("#{id}"),
        }
    }

    fn from_string_reader(reader: &mut StringReader) -> Result<Self, CommandSyntaxError> {
        if reader.peek() == Some('#') {
            // Vanilla restores the cursor when tag parsing fails
            // (ResourceOrTagKeyArgument.java:51-66), so a caller can report the
            // syntax error against the complete argument rather than after '#'.
            let cursor = reader.cursor();
            reader.skip();
            match Identifier::from_reader(reader) {
                Ok(identifier) => Ok(Self::Tag(identifier)),
                Err(error) => {
                    reader.set_cursor(cursor);
                    Err(error)
                }
            }
        } else {
            Ok(Self::Resource(Identifier::from_reader(reader)?))
        }
    }
}

/// Registry-aware suggestions shared by both argument types: the known entry
/// ids plus, when tag data exists for the registry, its `#`-prefixed tags.
fn suggest_for_registry(registry: &Identifier, builder: SuggestionsBuilder) -> Suggestions {
    let tag_names = |key: RegistryKey| {
        tag::get_latest_map(key)
            .into_iter()
            .flat_map(|map| map.keys().map(|tag| format!("#{tag}")))
    };

    if *registry == STRUCTURE_REGISTRY {
        // The generator models vanilla's structure sets, so those are the
        // locatable names. There is no structure tag data to offer.
        builder
            .filter_and_suggest_iter(
                StructureSet::NAMES
                    .iter()
                    .map(|name| format!("minecraft:{name}")),
            )
            .build()
    } else if *registry == *BIOME_REGISTRY {
        let biomes = pumpkin_data::biome::Biome::ALL
            .iter()
            .map(|biome| format!("minecraft:{}", biome.registry_id));
        builder
            .filter_and_suggest_iter(biomes.chain(tag_names(RegistryKey::WorldgenBiome)))
            .build()
    } else if *registry == POI_REGISTRY {
        // There is no generated POI type registry (yet), so offer the types
        // known from tag data plus the ones the server actually creates.
        let mut names: BTreeSet<String> = tag::get_latest_map(RegistryKey::PointOfInterestType)
            .into_iter()
            .flat_map(|map| map.values().flat_map(|tag| tag.0.iter()))
            .map(|name| format!("minecraft:{name}"))
            .collect();
        names.insert(POI_TYPE_NETHER_PORTAL.to_string());
        builder
            .filter_and_suggest_iter(
                names
                    .into_iter()
                    .chain(tag_names(RegistryKey::PointOfInterestType)),
            )
            .build()
    } else {
        Suggestions::empty()
    }
}

/// An argument type that parses an entry id or `#`-tag of a registry.
///
/// The value is not validated against the registry, like vanilla's
/// `ResourceOrTagKeyArgument`; resolution (and the matching error) is left to
/// the command.
pub struct ResourceOrTagKeyArgument(pub Identifier);

impl ArgumentType for ResourceOrTagKeyArgument {
    type Item = ResourceOrTag;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        ResourceOrTag::from_string_reader(reader)
    }

    fn list_suggestions<'a>(
        &'a self,
        _context: &'a CommandContext,
        builder: SuggestionsBuilder,
    ) -> Pin<Box<dyn Future<Output = Suggestions> + Send + 'a>> {
        Box::pin(async move { suggest_for_registry(&self.0, builder) })
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::ResourceOrTagKey {
            identifier: self.0.clone(),
        }
    }

    fn examples(&self) -> Vec<String> {
        // Vanilla exposes these five examples for both resource/tag argument
        // types (ResourceOrTagKeyArgument.java:28-29).
        examples!(
            "foo",
            "foo:bar",
            "012",
            "#skeletons",
            "#minecraft:skeletons"
        )
    }
}

/// An argument type that parses and validates an entry id or `#`-tag of a
/// registry.
///
/// Validation happens at parse time where registry data is available, like
/// vanilla's `ResourceOrTagArgument`: it currently covers biome ids and the
/// tags of every registry with generated tag data; ids of registries without
/// generated entry data (such as POI types) are accepted as-is.
pub struct ResourceOrTagArgument(pub Identifier);

impl ArgumentType for ResourceOrTagArgument {
    type Item = ResourceOrTag;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        let value = ResourceOrTag::from_string_reader(reader)?;

        match &value {
            ResourceOrTag::Resource(id) => {
                if self.0 == *BIOME_REGISTRY
                    && !(id.is_vanilla()
                        && pumpkin_data::biome::Biome::from_name(id.path()).is_some())
                {
                    return Err(ERROR_UNKNOWN_RESOURCE.create_without_context(
                        TextComponent::text(id.to_string()),
                        TextComponent::text(self.0.to_string()),
                    ));
                }
            }
            ResourceOrTag::Tag(id) => {
                let registry_key = if self.0 == *BIOME_REGISTRY {
                    Some(RegistryKey::WorldgenBiome)
                } else if self.0 == POI_REGISTRY {
                    Some(RegistryKey::PointOfInterestType)
                } else {
                    None
                };

                if let Some(key) = registry_key
                    && tag::get_tag_values(key, &id.to_string()).is_none()
                {
                    return Err(ERROR_UNKNOWN_TAG.create_without_context(
                        TextComponent::text(id.to_string()),
                        TextComponent::text(self.0.to_string()),
                    ));
                }
            }
        }

        Ok(value)
    }

    fn list_suggestions<'a>(
        &'a self,
        _context: &'a CommandContext,
        builder: SuggestionsBuilder,
    ) -> Pin<Box<dyn Future<Output = Suggestions> + Send + 'a>> {
        Box::pin(async move { suggest_for_registry(&self.0, builder) })
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::ResourceOrTag {
            identifier: self.0.clone(),
        }
    }

    fn examples(&self) -> Vec<String> {
        // Vanilla exposes these five examples for both resource/tag argument
        // types (ResourceOrTagArgument.java:32-33).
        examples!(
            "foo",
            "foo:bar",
            "012",
            "#skeletons",
            "#minecraft:skeletons"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{ResourceOrTag, ResourceOrTagKeyArgument};
    use crate::command::argument_types::argument_type::ArgumentType;
    use crate::command::string_reader::StringReader;
    use pumpkin_util::identifier::Identifier;

    #[test]
    fn invalid_tag_resets_reader_cursor() {
        // The cursor reset is required by ResourceOrTagKeyArgument.parse
        // (ResourceOrTagKeyArgument.java:51-66).
        // A bare `#` is NOT an error: `Identifier.parse("")` succeeds because
        // `isValidPath("")` is a zero-length loop (`Identifier.java:230-238`). A second
        // colon puts an invalid character in the path, which is what actually throws.
        let argument = ResourceOrTagKeyArgument(Identifier::vanilla_static("test"));
        let mut reader = StringReader::new("#foo:bar:baz");

        assert!(argument.parse(&mut reader).is_err());
        assert_eq!(reader.cursor(), 0);
    }

    #[test]
    fn examples_match_vanilla() {
        // ResourceOrTagKeyArgument.getExamples returns the five entries below
        // (ResourceOrTagKeyArgument.java:28-29, 74-76).
        let argument = ResourceOrTagKeyArgument(Identifier::vanilla_static("test"));

        assert_eq!(
            argument.examples(),
            vec![
                "foo".to_string(),
                "foo:bar".to_string(),
                "012".to_string(),
                "#skeletons".to_string(),
                "#minecraft:skeletons".to_string(),
            ]
        );
    }

    #[test]
    fn printable_resource_and_tag_forms_match_vanilla() {
        // Result.asPrintable uses the identifier and '#'+identifier forms
        // (ResourceOrTagKeyArgument.java:114-161).
        assert_eq!(
            ResourceOrTag::Resource(Identifier::vanilla_static("stone")).printable(),
            "minecraft:stone"
        );
        assert_eq!(
            ResourceOrTag::Tag(Identifier::vanilla_static("skeletons")).printable(),
            "#minecraft:skeletons"
        );
    }
}
