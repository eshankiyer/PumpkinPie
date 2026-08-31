use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::core::string::StringArgumentType;
use crate::command::argument_types::entity::EntityArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::command::suggestion::provider::SuggestionProvider;
use crate::command::suggestion::suggestions::{Suggestions, SuggestionsBuilder};
use crate::entity::EntityBase;
use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use std::future::Future;
use std::pin::Pin;

const DESCRIPTION: &str = "Gives or takes player recipes.";
const PERMISSION: &str = "minecraft:command.recipe";

static ERROR_RECIPE_NOT_FOUND: CommandErrorType<1> =
    CommandErrorType::new(translation::java::RECIPE_NOTFOUND, "Unknown recipe: %s");

struct RecipeSuggestionProvider;

impl SuggestionProvider for RecipeSuggestionProvider {
    fn suggest(
        &self,
        context: &CommandContext,
        mut builder: SuggestionsBuilder,
    ) -> Pin<Box<dyn Future<Output = Suggestions> + Send>> {
        let server = context.source.server.clone();

        Box::pin(async move {
            builder = builder.suggest("*");
            if let Some(server) = server {
                let recipes = server.recipe_manager.get_recipe_ids().await;
                for id in recipes {
                    builder = builder.suggest(id);
                }
            }
            builder.build()
        })
    }
}

struct RecipeGiveExecutor;

impl CommandExecutor for RecipeGiveExecutor {
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let targets = EntityArgumentType::get_players(context, "targets").await?;
            let recipe_str = StringArgumentType::get(context, "recipe")?;

            let server = context.source.server.as_ref().ok_or_else(|| {
                ERROR_RECIPE_NOT_FOUND
                    .create_without_context(TextComponent::text(recipe_str.to_string()))
            })?;

            // RecipeManager.java:175-177 enumerates the complete recipe map; use the same full
            // set for `/recipe give`, including generated vanilla recipes.
            let all_recipes = server.recipe_manager.get_recipe_ids().await;

            let is_all = recipe_str == "*";
            let matching_recipes = if is_all {
                all_recipes.clone()
            } else {
                all_recipes
                    .iter()
                    .filter(|id| {
                        id.as_str() == recipe_str
                            || id.strip_prefix("minecraft:").unwrap_or(id) == recipe_str
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            };

            if !is_all && matching_recipes.is_empty() {
                return Err(ERROR_RECIPE_NOT_FOUND
                    .create_without_context(TextComponent::text(recipe_str.to_string())));
            }

            let recipe_count = matching_recipes.len();

            for player in &targets {
                // `/recipe give` is `ServerPlayer.awardRecipes`: the unlock has to enter the
                // player's book, or it is forgotten on reconnect. `award_recipes` also sends
                // the subset add packet, so sending a full-table add here as well would
                // overwrite the per-player set that was just established.
                player
                    .award_recipes(matching_recipes.iter().map(String::as_str))
                    .await;
            }

            let recipe_count_str = recipe_count.to_string();
            if targets.len() == 1 {
                let msg = TextComponent::translate_cross(
                    translation::java::COMMANDS_RECIPE_GIVE_SUCCESS_SINGLE,
                    translation::java::COMMANDS_RECIPE_GIVE_SUCCESS_SINGLE,
                    [
                        TextComponent::text(recipe_count_str),
                        targets[0].get_display_name().await,
                    ],
                );
                context.source.send_feedback(msg, true).await;
            } else {
                let msg = TextComponent::translate_cross(
                    translation::java::COMMANDS_RECIPE_GIVE_SUCCESS_MULTIPLE,
                    translation::java::COMMANDS_RECIPE_GIVE_SUCCESS_MULTIPLE,
                    [
                        TextComponent::text(recipe_count_str),
                        TextComponent::text(targets.len().to_string()),
                    ],
                );
                context.source.send_feedback(msg, true).await;
            }

            Ok((targets.len() * recipe_count) as i32)
        })
    }
}

struct RecipeTakeExecutor;

impl CommandExecutor for RecipeTakeExecutor {
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let targets = EntityArgumentType::get_players(context, "targets").await?;
            let recipe_str = StringArgumentType::get(context, "recipe")?;

            let server = context.source.server.as_ref().ok_or_else(|| {
                ERROR_RECIPE_NOT_FOUND
                    .create_without_context(TextComponent::text(recipe_str.to_string()))
            })?;

            // RecipeManager.java:175-177 enumerates the complete recipe map; `/recipe take`
            // must be able to remove generated vanilla recipes as well as dynamic recipes.
            let all_recipes = server.recipe_manager.get_recipe_ids().await;

            let is_all = recipe_str == "*";

            let mut matched = false;
            // RecipeCommand.java:94-99 passes only the selected collection to
            // `ServerPlayer.resetRecipes`; keep a single-recipe take from removing everything.
            let matching_recipes = if is_all {
                matched = true;
                all_recipes.clone()
            } else {
                all_recipes
                    .iter()
                    .filter(|id| {
                        let is_match = id.as_str() == recipe_str
                            || id.strip_prefix("minecraft:").unwrap_or(id) == recipe_str;
                        if is_match {
                            matched = true;
                        }
                        !is_match
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            };

            if !matched {
                return Err(ERROR_RECIPE_NOT_FOUND
                    .create_without_context(TextComponent::text(recipe_str.to_string())));
            }

            let taken_count = matching_recipes.len();

            for player in &targets {
                // `reset_recipes` sends the remove packet naming exactly the display ids it
                // dropped, which is what `ServerRecipeBook.removeRecipes` does. A full-table
                // add here would re-add everything it had just removed.
                player
                    .reset_recipes(matching_recipes.iter().map(String::as_str))
                    .await;
            }

            let taken_count_str = taken_count.to_string();
            if targets.len() == 1 {
                let msg = TextComponent::translate_cross(
                    translation::java::COMMANDS_RECIPE_TAKE_SUCCESS_SINGLE,
                    translation::java::COMMANDS_RECIPE_TAKE_SUCCESS_SINGLE,
                    [
                        TextComponent::text(taken_count_str),
                        targets[0].get_display_name().await,
                    ],
                );
                context.source.send_feedback(msg, true).await;
            } else {
                let msg = TextComponent::translate_cross(
                    translation::java::COMMANDS_RECIPE_TAKE_SUCCESS_MULTIPLE,
                    translation::java::COMMANDS_RECIPE_TAKE_SUCCESS_MULTIPLE,
                    [
                        TextComponent::text(taken_count_str),
                        TextComponent::text(targets.len().to_string()),
                    ],
                );
                context.source.send_feedback(msg, true).await;
            }

            Ok((targets.len() * taken_count) as i32)
        })
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let builder = command("recipe", DESCRIPTION)
        .requires(PERMISSION)
        .then(
            literal("give").then(
                argument("targets", EntityArgumentType::Players).then(
                    argument("recipe", StringArgumentType::SingleWord)
                        .suggests(RecipeSuggestionProvider)
                        .executes(RecipeGiveExecutor),
                ),
            ),
        )
        .then(
            literal("take").then(
                argument("targets", EntityArgumentType::Players).then(
                    argument("recipe", StringArgumentType::SingleWord)
                        .suggests(RecipeSuggestionProvider)
                        .executes(RecipeTakeExecutor),
                ),
            ),
        );

    dispatcher.register(builder);
}
