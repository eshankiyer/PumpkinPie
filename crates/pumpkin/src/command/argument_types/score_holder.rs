use crate::command::argument_types::argument_type::{ArgumentType, JavaClientArgumentType};
use crate::command::argument_types::entity::ENTITY_SELECTOR_PERMISSION;
use crate::command::argument_types::entity_selector::EntitySelector;
use crate::command::argument_types::entity_selector::parser::{
    EntitySelectorParser, EntitySelectorParserSuggestions,
};
use crate::command::context::command_context::CommandContext;
use crate::command::context::command_source::CommandSource;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::{CommandErrorType, READER_EXPECTED_SYMBOL};
use crate::command::string_reader::StringReader;
use crate::command::suggestion::suggestions::{Suggestions, SuggestionsBuilder};
use crate::world::scoreboard::entity_scoreboard_name;
use pumpkin_data::translation;
use pumpkin_protocol::java::client::play::ArgumentType as JavaArgumentType;
use pumpkin_protocol::java::client::play::SuggestionProviders;
use pumpkin_util::text::TextComponent;
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

const NO_RESULTS_ERROR_TYPE: CommandErrorType<0> = CommandErrorType::new(
    translation::java::ARGUMENT_SCOREHOLDER_EMPTY,
    translation::java::ARGUMENT_SCOREHOLDER_EMPTY,
);

/// A parsed score holder. Vanilla `ScoreHolderArgument` accepts ordinary names,
/// fake holders beginning with `#`, UUIDs, `*`, and entity selectors
/// (`ScoreHolderArgument.java:92-161`).
pub enum ScoreHolderArgument {
    Name(String),
    Selector(Box<EntitySelector>),
    Wildcard,
}

/// Parses score holders for commands such as `/scoreboard players`
/// (`ScoreboardCommand.java:164-367`).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct ScoreHolderArgumentType {
    multiple: bool,
}

impl ScoreHolderArgumentType {
    #[must_use]
    pub const fn score_holder() -> Self {
        Self { multiple: false }
    }

    #[must_use]
    pub const fn score_holders() -> Self {
        Self { multiple: true }
    }

    /// `ScoreHolderArgument.parse` (`ScoreHolderArgument.java:92-123`)
    fn parse_with_allow_selectors(
        self,
        reader: &mut StringReader,
        allow_selectors: bool,
    ) -> Result<ScoreHolderArgument, CommandSyntaxError> {
        if reader.peek() == Some('@') {
            let selector =
                EntitySelectorParser::new(reader, allow_selectors).parse_and_consume()?;
            if !self.multiple && selector.max_selected > 1 {
                return Err(
                    crate::command::argument_types::entity::NOT_SINGLE_ENTITY_ERROR_TYPE
                        .create(reader),
                );
            }
            Ok(ScoreHolderArgument::Selector(Box::new(selector)))
        } else {
            let start = reader.cursor();
            reader.read_until_space();
            let text = reader.string()[start..reader.cursor()].to_string();
            if text.is_empty() {
                return Err(
                    READER_EXPECTED_SYMBOL.create(reader, TextComponent::text("score holder"))
                );
            }
            if text == "*" {
                Ok(ScoreHolderArgument::Wildcard)
            } else {
                Ok(ScoreHolderArgument::Name(text))
            }
        }
    }

    /// `ScoreHolderArgument.parse` result lambdas (`ScoreHolderArgument.java:109-159`)
    async fn resolve(
        argument: &ScoreHolderArgument,
        source: &CommandSource,
        wildcard: Option<Vec<String>>,
    ) -> Result<Vec<String>, CommandSyntaxError> {
        match argument {
            ScoreHolderArgument::Name(name) => {
                if name.starts_with('#') {
                    return Ok(vec![name.clone()]);
                }
                if let Ok(uuid) = name.parse::<Uuid>() {
                    // Vanilla preserves every matching entity when a UUID occurs in multiple
                    // server levels (`ScoreHolderArgument.java:125-153`).
                    let mut matches = Vec::new();
                    for world in source.server().worlds.load().iter() {
                        if let Some(player) = world.get_player_by_uuid(uuid) {
                            matches.push(player.gameprofile.name.clone());
                        } else if let Some(entity) = world.get_entity_by_uuid(uuid) {
                            matches.push(entity_scoreboard_name(entity.as_ref()));
                        }
                    }
                    if !matches.is_empty() {
                        return Ok(matches);
                    }
                    return Ok(vec![name.clone()]);
                }
                Ok(vec![source.server().get_player_by_name(name).map_or_else(
                    || name.clone(),
                    |player| player.gameprofile.name.clone(),
                )])
            }
            ScoreHolderArgument::Selector(selector) => {
                let entities = selector.find_entities(source).await?;
                if entities.is_empty() {
                    Err(
                        crate::command::argument_types::entity::NO_ENTITIES_ERROR_TYPE
                            .create_without_context(),
                    )
                } else {
                    Ok(entities
                        .iter()
                        .map(|entity| entity_scoreboard_name(entity.as_ref()))
                        .collect())
                }
            }
            ScoreHolderArgument::Wildcard => match wildcard {
                Some(names) if !names.is_empty() => Ok(names),
                _ => Err(NO_RESULTS_ERROR_TYPE.create_without_context()),
            },
        }
    }

    /// Returns all resolved holders, using the vanilla empty wildcard supplier.
    pub async fn get_names(
        context: &CommandContext<'_>,
        name: &str,
    ) -> Result<Vec<String>, CommandSyntaxError> {
        let argument = context.get_argument::<ScoreHolderArgument>(name)?;
        Self::resolve(argument, &context.source, None).await
    }

    /// Returns all resolved holders, expanding `*` to tracked scoreboard holders
    /// (`ScoreHolderArgument.java:61-73`; `ScoreboardCommand.java:187-358`).
    pub async fn get_names_with_default_wildcard(
        context: &CommandContext<'_>,
        name: &str,
    ) -> Result<Vec<String>, CommandSyntaxError> {
        let argument = context.get_argument::<ScoreHolderArgument>(name)?;
        let wildcard = match argument {
            ScoreHolderArgument::Wildcard => Some(
                context
                    .world()
                    .scoreboard
                    .lock()
                    .await
                    .get_tracked_players()
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            ),
            _ => None,
        };
        Self::resolve(argument, &context.source, wildcard).await
    }

    /// Returns one resolved holder from a single-holder argument.
    pub async fn get_name(
        context: &CommandContext<'_>,
        name: &str,
    ) -> Result<String, CommandSyntaxError> {
        Self::get_names(context, name)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| NO_RESULTS_ERROR_TYPE.create_without_context())
    }
}

impl ArgumentType for ScoreHolderArgumentType {
    type Item = ScoreHolderArgument;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        self.parse_with_allow_selectors(reader, true)
    }

    fn parse_with_source<'a>(
        &'a self,
        reader: &'a mut StringReader,
        source: &'a CommandSource,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Item, CommandSyntaxError>> + Send + 'a>> {
        Box::pin(async move {
            self.parse_with_allow_selectors(
                reader,
                source.has_permission(ENTITY_SELECTOR_PERMISSION).await,
            )
        })
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaArgumentType::ScoreHolder {
            flags: if self.multiple {
                JavaArgumentType::SCORE_HOLDER_FLAG_ALLOW_MULTIPLE
            } else {
                0
            },
        }
    }

    fn list_suggestions<'a>(
        &'a self,
        context: &'a CommandContext,
        builder: SuggestionsBuilder,
    ) -> Pin<Box<dyn Future<Output = Suggestions> + Send + 'a>> {
        EntitySelectorParserSuggestions::list_suggestions(context, builder)
    }

    /// `ScoreHolderArgument.SUGGEST_SCORE_HOLDERS` (`ScoreboardCommand.java:170-171`)
    fn override_suggestion_providers(&self) -> Option<SuggestionProviders> {
        Some(SuggestionProviders::AskServer)
    }

    fn examples(&self) -> Vec<String> {
        vec![
            "Player".to_string(),
            "0123".to_string(),
            "*".to_string(),
            "@e".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{ScoreHolderArgument, ScoreHolderArgumentType};
    use crate::command::argument_types::argument_type::ArgumentType;
    use crate::command::string_reader::StringReader;

    #[test]
    fn parses_literal_and_wildcard_holders() {
        // Vanilla parses names, including `*` and `#` holders, without resolving them
        // (`ScoreHolderArgument.java:102-123`).
        for (input, expected) in [("Player", false), ("#team", false), ("*", true)] {
            let mut reader = StringReader::new(input);
            let value = ScoreHolderArgumentType::score_holders()
                .parse(&mut reader)
                .expect("score holder should parse");
            assert_eq!(matches!(value, ScoreHolderArgument::Wildcard), expected);
            assert_eq!(reader.remaining_length(), 0);
        }
    }

    #[test]
    fn single_holder_rejects_unbounded_selector() {
        // Vanilla rejects a selector with more than one possible result for `scoreHolder()`
        // (`ScoreHolderArgument.java:92-99`).
        let mut reader = StringReader::new("@e obj");
        let Err(error) = ScoreHolderArgumentType::score_holder().parse(&mut reader) else {
            panic!("unbounded selector should be rejected");
        };
        assert_eq!(error.context.map(|context| context.cursor), Some(2));
    }
}
