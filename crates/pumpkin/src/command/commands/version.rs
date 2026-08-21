use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_world::CURRENT_MC_VERSION as CURRENT_MC_VERSION_NAME;
use pumpkin_world::world_info::MAXIMUM_SUPPORTED_WORLD_DATA_VERSION;

use crate::command::argument_builder::{ArgumentBuilder, command};
use crate::command::context::command_context::CommandContext;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};

const NAME: &str = "version";

const DESCRIPTION: &str = "Prints the version of the game this server implements.";
const PERMISSION: &str = "minecraft:command.version";

/// Values taken from the 26.2 server jar's `version.json`, which is what
/// `SharedConstants.getCurrentVersion()` is built from.
const SERIES_ID: &str = "main";
const BUILD_TIME: &str = "2026-06-16T12:01:27+00:00";
const RESOURCE_PACK_FORMAT: &str = "88.0";
const DATA_PACK_FORMAT: &str = "107.1";
const STABLE: bool = true;

struct Executor;

impl CommandExecutor for Executor {
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let source = &context.source;

            // VersionCommand.java:20-25 sends the header, then every line of
            // `dumpVersion`, as system messages rather than command feedback.
            source
                .send_message(TextComponent::translate(
                    translation::java::COMMANDS_VERSION_HEADER,
                    [],
                ))
                .await;

            let protocol = CURRENT_MC_VERSION.protocol_version();

            // VersionCommand.java:29-38.
            for line in [
                TextComponent::translate(
                    translation::java::COMMANDS_VERSION_ID,
                    [TextComponent::text(CURRENT_MC_VERSION_NAME)],
                ),
                TextComponent::translate(
                    translation::java::COMMANDS_VERSION_NAME,
                    [TextComponent::text(CURRENT_MC_VERSION_NAME)],
                ),
                TextComponent::translate(
                    translation::java::COMMANDS_VERSION_DATA,
                    [TextComponent::text(
                        MAXIMUM_SUPPORTED_WORLD_DATA_VERSION.to_string(),
                    )],
                ),
                TextComponent::translate(
                    translation::java::COMMANDS_VERSION_SERIES,
                    [TextComponent::text(SERIES_ID)],
                ),
                TextComponent::translate(
                    translation::java::COMMANDS_VERSION_PROTOCOL,
                    [
                        TextComponent::text(protocol.to_string()),
                        TextComponent::text(format!("0x{protocol:x}")),
                    ],
                ),
                TextComponent::translate(
                    translation::java::COMMANDS_VERSION_BUILD_TIME,
                    [TextComponent::text(BUILD_TIME)],
                ),
                TextComponent::translate(
                    translation::java::COMMANDS_VERSION_PACK_RESOURCE,
                    [TextComponent::text(RESOURCE_PACK_FORMAT)],
                ),
                TextComponent::translate(
                    translation::java::COMMANDS_VERSION_PACK_DATA,
                    [TextComponent::text(DATA_PACK_FORMAT)],
                ),
                TextComponent::translate(
                    if STABLE {
                        translation::java::COMMANDS_VERSION_STABLE_YES
                    } else {
                        translation::java::COMMANDS_VERSION_STABLE_NO
                    },
                    [],
                ),
            ] {
                source.send_message(line).await;
            }

            Ok(1)
        })
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    // VersionCommand.java:19: a dedicated server passes `checkPermissions = true`
    // (Commands.java:238), so the requirement is LEVEL_GAMEMASTERS.
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command(NAME, DESCRIPTION)
            .requires(PERMISSION)
            .executes(Executor),
    );
}
