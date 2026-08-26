use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

use crate::command::argument_builder::{ArgumentBuilder, argument, command};
use crate::command::argument_types::core::string::StringArgumentType;
use crate::command::argument_types::nbt::NbtCompoundArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::command::suggestion::provider::{SuggestionProvider, SuggestionProviderResult};
use crate::command::suggestion::suggestions::SuggestionsBuilder;
use crate::data::datapack::ExecuteFunctionError;

const DESCRIPTION: &str = "Runs commands found in the corresponding function files.";
const PERMISSION: &str = "minecraft:command.function";

static ERROR_UNKNOWN_FUNCTION: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENTS_FUNCTION_UNKNOWN,
    translation::java::ARGUMENTS_FUNCTION_UNKNOWN,
);

/// Port of vanilla's `ERROR_FUNCTION_INSTANTATION_FAILURE`
/// (`FunctionCommand.java:49-51`): wraps a `MacroFunction.instantiate` failure
/// (`MacroFunction.java:52-82`) raised while queueing the function
/// (`FunctionCommand.java:137-141`).
static ERROR_INSTANTIATION_FAILURE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_FUNCTION_INSTANTIATIONFAILURE,
    translation::java::COMMANDS_FUNCTION_INSTANTIATIONFAILURE,
);

struct FunctionSuggestionProvider;

impl SuggestionProvider for FunctionSuggestionProvider {
    fn suggest<'a>(
        &'a self,
        context: &'a CommandContext,
        mut builder: SuggestionsBuilder,
    ) -> SuggestionProviderResult<'a> {
        Box::pin(async move {
            let server = context.server();
            let function_names = server.datapack_manager.get_function_names().await;
            for name in function_names {
                builder = builder.suggest(name);
            }
            builder.build()
        })
    }
}

/// Runs `/function <name>` without macro arguments (vanilla
/// `FunctionCommand.java:83-88`, where the base executor passes a
/// `null` compound).
struct FunctionExecutor;

impl CommandExecutor for FunctionExecutor {
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let name_str = StringArgumentType::get(context, "name")?;
            let server = context.server();

            let executed_count = server
                .datapack_manager
                .execute_function(server, &context.source, name_str, None)
                .await
                .map_err(|error| map_error(error, name_str))?;

            send_success_feedback(context, executed_count, name_str).await;

            Ok(executed_count as i32)
        })
    }
}

/// Runs `/function <name> <compound>` with NBT compound macro arguments
/// (vanilla `CompoundTagArgument.getCompoundTag(context, "arguments")`,
/// `FunctionCommand.java:88-92`; argument type
/// `net.minecraft.commands.arguments.CompoundTagArgument`).
struct FunctionWithArgumentsExecutor;

impl CommandExecutor for FunctionWithArgumentsExecutor {
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let name_str = StringArgumentType::get(context, "name")?;
            let arguments = NbtCompoundArgumentType::get(context, "arguments")?;
            let server = context.server();

            let executed_count = server
                .datapack_manager
                .execute_function(server, &context.source, name_str, Some(arguments))
                .await
                .map_err(|error| map_error(error, name_str))?;

            send_success_feedback(context, executed_count, name_str).await;

            Ok(executed_count as i32)
        })
    }
}

async fn send_success_feedback(context: &CommandContext<'_>, executed_count: usize, name: &str) {
    if name.starts_with('#') {
        context
            .source
            .send_feedback(
                TextComponent::translate_cross(
                    translation::java::COMMANDS_FUNCTION_SUCCESS_MULTIPLE,
                    translation::java::COMMANDS_FUNCTION_SUCCESS_MULTIPLE,
                    [
                        TextComponent::text(executed_count.to_string()),
                        TextComponent::text(name.to_string()),
                    ],
                ),
                true,
            )
            .await;
    } else {
        context
            .source
            .send_feedback(
                TextComponent::translate_cross(
                    translation::java::COMMANDS_FUNCTION_SUCCESS_SINGLE,
                    translation::java::COMMANDS_FUNCTION_SUCCESS_SINGLE,
                    [
                        TextComponent::text(executed_count.to_string()),
                        TextComponent::text(name.to_string()),
                    ],
                ),
                true,
            )
            .await;
    }
}

/// Maps execution failures to their vanilla counterparts: unknown ids surface
/// as `arguments.function.unknown` and macro instantiation failures as
/// `commands.function.instantiationFailure` (`FunctionCommand.java:49-51`,
/// raised at `:139-141`).
fn map_error(error: ExecuteFunctionError, requested_name: &str) -> CommandSyntaxError {
    match error {
        ExecuteFunctionError::Unknown(_) => ERROR_UNKNOWN_FUNCTION
            .create_without_context(TextComponent::text(requested_name.to_string())),
        ExecuteFunctionError::InstantiationFailure {
            function_id,
            reason,
        } => ERROR_INSTANTIATION_FAILURE
            .create_without_context(TextComponent::text(function_id), reason),
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("function", DESCRIPTION).requires(PERMISSION).then(
            argument("name", StringArgumentType::SingleWord)
                .suggests(FunctionSuggestionProvider)
                .executes(FunctionExecutor)
                // `/function <name> <compound>` — vanilla registers the
                // compound argument under the function-name argument
                // (`FunctionCommand.java:88-92`).
                .then(
                    argument("arguments", NbtCompoundArgumentType)
                        .executes(FunctionWithArgumentsExecutor),
                ),
        ),
    );
}
