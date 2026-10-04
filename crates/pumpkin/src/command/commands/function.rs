use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

use crate::command::argument_builder::{ArgumentBuilder, argument, command};
use crate::command::argument_types::function::FunctionArgumentType;
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

static ERROR_UNKNOWN_TAG: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENTS_FUNCTION_TAG_UNKNOWN,
    translation::java::ARGUMENTS_FUNCTION_TAG_UNKNOWN,
);

/// Port of vanilla's `ERROR_NO_FUNCTIONS` (`FunctionCommand.java:45-47`),
/// raised for a function tag without any functions (`:244-246`).
static ERROR_NO_FUNCTIONS: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_FUNCTION_SCHEDULED_NO_FUNCTIONS,
    translation::java::COMMANDS_FUNCTION_SCHEDULED_NO_FUNCTIONS,
);

/// Port of vanilla's `ERROR_FUNCTION_INSTANTATION_FAILURE`
/// (`FunctionCommand.java:49-51`): wraps a `MacroFunction.instantiate` failure
/// (`MacroFunction.java:52-82`) raised while queueing the function
/// (`FunctionCommand.java:137-141`).
static ERROR_INSTANTIATION_FAILURE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_FUNCTION_INSTANTIATIONFAILURE,
    translation::java::COMMANDS_FUNCTION_INSTANTIATIONFAILURE,
);

// Vanilla supplies the same function-name suggestions to `/function` and the execute-function
// argument (`ExecuteCommand.java:652-656`, `FunctionCommand.java:160-164`).
pub(super) struct FunctionSuggestionProvider;

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
        Box::pin(async move { run_guarded(context, None).await })
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
            let arguments = NbtCompoundArgumentType::get(context, "arguments")?;
            run_guarded(context, Some(arguments)).await
        })
    }
}

/// Port of `FunctionCustomExecutor.runGuarded` (`FunctionCommand.java:235-265`):
/// resolves the functions, fails on an empty collection, announces the scheduled
/// functions to the sender and only then runs them.
async fn run_guarded(
    context: &CommandContext<'_>,
    arguments: Option<&NbtCompound>,
) -> Result<i32, CommandSyntaxError> {
    let name = FunctionArgumentType::get(context, "name")?.printable();
    let server = context.server();

    let function_ids = server
        .datapack_manager
        .resolve_function_ids(&name)
        .await
        .map_err(map_error)?;
    if function_ids.is_empty() {
        let tag_id = name.strip_prefix('#').unwrap_or(&name).to_string();
        return Err(ERROR_NO_FUNCTIONS.create_without_context(TextComponent::text(tag_id)));
    }

    let message = if let [function_id] = function_ids.as_slice() {
        TextComponent::translate_cross(
            translation::java::COMMANDS_FUNCTION_SCHEDULED_SINGLE,
            translation::java::COMMANDS_FUNCTION_SCHEDULED_SINGLE,
            [TextComponent::text(function_id.clone())],
        )
    } else {
        // `ComponentUtils.formatList` with the gray ", " separator
        // (`ComponentUtils.java:19,116-138`).
        TextComponent::translate_cross(
            translation::java::COMMANDS_FUNCTION_SCHEDULED_MULTIPLE,
            translation::java::COMMANDS_FUNCTION_SCHEDULED_MULTIPLE,
            [TextComponent::join_with_comma(
                function_ids.iter().cloned().map(TextComponent::text).collect(),
            )],
        )
    };
    context.source.send_feedback(message, true).await;

    let executed_count = server
        .datapack_manager
        .execute_function(server, &context.source, &name, arguments)
        .await
        .map_err(map_error)?;

    Ok(executed_count as i32)
}

/// Maps execution failures to their vanilla counterparts: unknown ids surface
/// as `arguments.function.unknown` / `arguments.function.tag.unknown`
/// (`FunctionArgument.java:76-86`) and macro instantiation failures as
/// `commands.function.instantiationFailure` (`FunctionCommand.java:49-51`,
/// raised at `:139-141`).
fn map_error(error: ExecuteFunctionError) -> CommandSyntaxError {
    match error {
        ExecuteFunctionError::Unknown(function_id) => {
            ERROR_UNKNOWN_FUNCTION.create_without_context(TextComponent::text(function_id))
        }
        ExecuteFunctionError::UnknownTag(tag_id) => {
            ERROR_UNKNOWN_TAG.create_without_context(TextComponent::text(tag_id))
        }
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
            argument("name", FunctionArgumentType)
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
