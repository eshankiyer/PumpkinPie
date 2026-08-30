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
use crate::command::errors::error_types::{CommandErrorType, DISPATCHER_PARSE_EXCEPTION};
use crate::command::node::attached::AttachedNode;
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

static ERROR_INVALID_RESOURCE_TYPE: CommandErrorType<3> = CommandErrorType::new(
    translation::java::ARGUMENT_RESOURCE_INVALID_TYPE,
    translation::java::ARGUMENT_RESOURCE_INVALID_TYPE,
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
            reader.skip();
            Ok(Self::Tag(Identifier::from_reader(reader)?))
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
        examples!("foo", "foo:bar", "#foo")
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

impl ResourceOrTagArgument {
    /// `ResourceOrTagArgument.getResourceOrTag` (`ResourceOrTagArgument.java:52-64`).
    /// Validate the registry carried by the command node before returning its parsed value.
    pub fn get_resource_or_tag<'a>(
        context: &'a CommandContext,
        name: &str,
        registry: &Identifier,
    ) -> Result<&'a ResourceOrTag, CommandSyntaxError> {
        let value = context.get_argument::<ResourceOrTag>(name)?;
        let argument = context
            .tree
            .iter()
            .find_map(|node| {
                if let AttachedNode::Argument(argument) = node
                    && argument.meta.name == name
                {
                    Some(argument)
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                DISPATCHER_PARSE_EXCEPTION.create_without_context(TextComponent::text(format!(
                    "Could not find argument with name '{name}'"
                )))
            })?;
        let argument_type = argument
            .meta
            .argument_type
            .as_any()
            .downcast_ref::<Self>()
            .ok_or_else(|| {
                DISPATCHER_PARSE_EXCEPTION.create_without_context(TextComponent::text(format!(
                    "argument with name '{name}' isn't a ResourceOrTagArgument"
                )))
            })?;

        if argument_type.0 == *registry {
            Ok(value)
        } else {
            Err(ERROR_INVALID_RESOURCE_TYPE.create_without_context(
                TextComponent::text(value.printable()),
                TextComponent::text(argument_type.0.to_string()),
                TextComponent::text(registry.to_string()),
            ))
        }
    }
}

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
        examples!("foo", "foo:bar", "#foo")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_preserves_resource_and_tag_prefixes() {
        // Vanilla `ResourceOrTagArgument` prints these forms (`ResourceOrTagArgument.java:148-155, 177-184`).
        let argument = ResourceOrTagKeyArgument(BIOME_REGISTRY.clone());

        let mut resource_reader = StringReader::new("minecraft:plains");
        assert_eq!(
            argument
                .parse(&mut resource_reader)
                .expect("resource should parse")
                .printable(),
            "minecraft:plains"
        );

        let mut tag_reader = StringReader::new("#minecraft:is_overworld");
        assert_eq!(
            argument
                .parse(&mut tag_reader)
                .expect("tag should parse")
                .printable(),
            "#minecraft:is_overworld"
        );
    }
}
