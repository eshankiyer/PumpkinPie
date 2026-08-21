use std::future::Future;
use std::pin::Pin;

use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::identifier::Identifier;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::core::double::DoubleArgumentType;
use crate::command::argument_types::identifier::IdentifierArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::command::suggestion::provider::SuggestionProvider;
use crate::command::suggestion::suggestions::{Suggestions, SuggestionsBuilder};
use crate::world::stopwatch::{Stopwatch, Stopwatches};

const NAME: &str = "stopwatch";

const DESCRIPTION: &str = "Creates, queries, restarts and removes named stopwatches.";
const PERMISSION: &str = "minecraft:command.stopwatch";

const ARG_ID: &str = "id";
const ARG_SCALE: &str = "scale";

/// StopwatchCommand.java:19-21.
static ERROR_ALREADY_EXISTS: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_STOPWATCH_ALREADY_EXISTS,
    "A stopwatch with the ID %s already exists",
);

/// StopwatchCommand.java:22-24.
static ERROR_DOES_NOT_EXIST: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_STOPWATCH_DOES_NOT_EXIST,
    "No stopwatch with the ID %s exists",
);

/// StopwatchCommand.java:25-27.
struct StopwatchSuggestionProvider;

impl SuggestionProvider for StopwatchSuggestionProvider {
    fn suggest(
        &self,
        context: &CommandContext,
        mut builder: SuggestionsBuilder,
    ) -> Pin<Box<dyn Future<Output = Suggestions> + Send>> {
        let server = context.source.server.clone();

        Box::pin(async move {
            if let Some(server) = server {
                let ids = server.stopwatches.lock().await.ids();
                for id in ids {
                    builder = builder.suggest(id.to_string());
                }
            }
            builder.build()
        })
    }
}

fn id_arg(id: &Identifier) -> TextComponent {
    TextComponent::text(id.to_string())
}

struct CreateExecutor;

impl CommandExecutor for CreateExecutor {
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            // StopwatchCommand.java:68-77.
            let id = context.get_argument::<Identifier>(ARG_ID)?.clone();
            let server = context.server();

            let now = Stopwatches::current_time();
            if !server
                .stopwatches
                .lock()
                .await
                .add(id.clone(), Stopwatch::new(now))
            {
                return Err(ERROR_ALREADY_EXISTS.create_without_context(id_arg(&id)));
            }

            context
                .source
                .send_feedback(
                    TextComponent::translate(
                        translation::java::COMMANDS_STOPWATCH_CREATE_SUCCESS,
                        [id_arg(&id)],
                    ),
                    true,
                )
                .await;

            Ok(1)
        })
    }
}

struct QueryExecutor;

impl QueryExecutor {
    async fn run(context: &CommandContext<'_>, scale: f64) -> Result<i32, CommandSyntaxError> {
        // StopwatchCommand.java:79-91.
        let id = context.get_argument::<Identifier>(ARG_ID)?.clone();
        let server = context.server();

        let now = Stopwatches::current_time();
        let elapsed_seconds = {
            let stopwatches = server.stopwatches.lock().await;
            let stopwatch = stopwatches
                .get(&id)
                .ok_or_else(|| ERROR_DOES_NOT_EXIST.create_without_context(id_arg(&id)))?;
            stopwatch.elapsed_seconds(now)
        };

        context
            .source
            .send_feedback(
                TextComponent::translate(
                    translation::java::COMMANDS_STOPWATCH_QUERY,
                    [
                        id_arg(&id),
                        TextComponent::text(elapsed_seconds.to_string()),
                    ],
                ),
                true,
            )
            .await;

        Ok((elapsed_seconds * scale) as i32)
    }
}

impl CommandExecutor for QueryExecutor {
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            // StopwatchCommand.java:46: the scale-less branch queries with a scale of 1.0.
            let scale = context
                .get_argument::<f64>(ARG_SCALE)
                .copied()
                .unwrap_or(1.0);
            Self::run(context, scale).await
        })
    }
}

struct RestartExecutor;

impl CommandExecutor for RestartExecutor {
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            // StopwatchCommand.java:93-102: restart replaces the stopwatch outright,
            // so the accumulated time is dropped rather than added to.
            let id = context.get_argument::<Identifier>(ARG_ID)?.clone();
            let server = context.server();

            let now = Stopwatches::current_time();
            if !server
                .stopwatches
                .lock()
                .await
                .update(&id, |_| Stopwatch::new(now))
            {
                return Err(ERROR_DOES_NOT_EXIST.create_without_context(id_arg(&id)));
            }

            context
                .source
                .send_feedback(
                    TextComponent::translate(
                        translation::java::COMMANDS_STOPWATCH_RESTART_SUCCESS,
                        [id_arg(&id)],
                    ),
                    true,
                )
                .await;

            Ok(1)
        })
    }
}

struct RemoveExecutor;

impl CommandExecutor for RemoveExecutor {
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            // StopwatchCommand.java:104-113.
            let id = context.get_argument::<Identifier>(ARG_ID)?.clone();
            let server = context.server();

            if !server.stopwatches.lock().await.remove(&id) {
                return Err(ERROR_DOES_NOT_EXIST.create_without_context(id_arg(&id)));
            }

            context
                .source
                .send_feedback(
                    TextComponent::translate(
                        translation::java::COMMANDS_STOPWATCH_REMOVE_SUCCESS,
                        [id_arg(&id)],
                    ),
                    true,
                )
                .await;

            Ok(1)
        })
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    // StopwatchCommand.java:32: LEVEL_GAMEMASTERS.
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command(NAME, DESCRIPTION)
            .requires(PERMISSION)
            .then(
                literal("create")
                    .then(argument(ARG_ID, IdentifierArgumentType).executes(CreateExecutor)),
            )
            .then(
                literal("query").then(
                    argument(ARG_ID, IdentifierArgumentType)
                        .suggests(StopwatchSuggestionProvider)
                        .then(
                            argument(ARG_SCALE, DoubleArgumentType::any()).executes(QueryExecutor),
                        )
                        .executes(QueryExecutor),
                ),
            )
            .then(
                literal("restart").then(
                    argument(ARG_ID, IdentifierArgumentType)
                        .suggests(StopwatchSuggestionProvider)
                        .executes(RestartExecutor),
                ),
            )
            .then(
                literal("remove").then(
                    argument(ARG_ID, IdentifierArgumentType)
                        .suggests(StopwatchSuggestionProvider)
                        .executes(RemoveExecutor),
                ),
            ),
    );
}
