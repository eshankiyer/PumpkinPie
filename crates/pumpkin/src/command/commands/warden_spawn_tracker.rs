use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::core::integer::IntegerArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::entity::EntityBase;
use crate::entity::mob::warden::warden_spawn_tracker;

const DESCRIPTION: &str = "Controls the Warden spawn warning tracker.";
const PERMISSION: &str = "minecraft:command.warden_spawn_tracker";
const ARG_WARNING_LEVEL: &str = "warning_level";

/// Implements `WardenSpawnTrackerCommand.setWarningLevel`
/// (`WardenSpawnTrackerCommand.java:34-43`).
struct SetExecutor;

impl CommandExecutor for SetExecutor {
    /// Sets the source player's warning level and reports the command result.
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let player = context.source.player_or_err()?;
            let warning_level = *context.get_argument::<i32>(ARG_WARNING_LEVEL)?;
            warden_spawn_tracker::set_warning_level_of(
                &player.world(),
                player.gameprofile.id,
                warning_level,
            )
            .await;

            context
                .source
                .send_feedback(
                    TextComponent::text(format!(
                        "Set Warden spawn warning level for {} to {warning_level}",
                        player.get_display_name().await.get_text()
                    )),
                    true,
                )
                .await;
            Ok(1)
        })
    }
}

/// Implements `WardenSpawnTrackerCommand.resetTracker`
/// (`WardenSpawnTrackerCommand.java:47-58`).
struct ClearExecutor;

impl CommandExecutor for ClearExecutor {
    /// Resets the source player's tracker and reports the command result.
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let player = context.source.player_or_err()?;
            warden_spawn_tracker::reset_tracker_of(&player.world(), player.gameprofile.id).await;

            context
                .source
                .send_feedback(
                    TextComponent::text(format!(
                        "Cleared Warden spawn warning tracker for {}",
                        player.get_display_name().await.get_text()
                    )),
                    true,
                )
                .await;
            Ok(1)
        })
    }
}

/// Registers `/warden_spawn_tracker`, matching
/// `WardenSpawnTrackerCommand.register` (`WardenSpawnTrackerCommand.java:15-32`).
pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("warden_spawn_tracker", DESCRIPTION)
            .requires(PERMISSION)
            .then(literal("clear").executes(ClearExecutor))
            .then(
                literal("set").then(
                    argument(
                        ARG_WARNING_LEVEL,
                        IntegerArgumentType::new(0, warden_spawn_tracker::MAX_WARNING_LEVEL),
                    )
                    .executes(SetExecutor),
                ),
            ),
    );
}
