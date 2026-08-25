use pumpkin_util::PermissionLvl;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

use crate::command::argument_builder::{ArgumentBuilder, argument, command};
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::error_types::LiteralCommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::entity::ai::pathfinder::Navigator;

const DESCRIPTION: &str = "Finds a mob path to a target position.";
const PERMISSION: &str = "minecraft:command.debugpath";
const ARG_TARGET: &str = "to";

static ERROR_NOT_MOB: LiteralCommandErrorType = LiteralCommandErrorType::new("Source is not a mob");
static ERROR_NO_PATH: LiteralCommandErrorType = LiteralCommandErrorType::new("Path not found");
static ERROR_NOT_COMPLETE: LiteralCommandErrorType =
    LiteralCommandErrorType::new("Target not reached");

struct DebugPathExecutor;

impl CommandExecutor for DebugPathExecutor {
    /// Implements `DebugPathCommand.fillBlocks` (`DebugPathCommand.java:29-45`).
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            // DebugPathCommand.java:29-45 creates a ground path navigation for the source mob,
            // rejects missing or incomplete paths, and reports success only for a reachable path.
            let mob = context
                .source
                .entity
                .as_ref()
                .and_then(|entity| entity.get_mob())
                .ok_or_else(|| ERROR_NOT_MOB.create_without_context())?;
            let target = BlockPosArgumentType::get_loaded_block_pos(context, ARG_TARGET)?;
            let destination = Vector3::new(
                f64::from(target.0.x),
                f64::from(target.0.y),
                f64::from(target.0.z),
            );

            // Vanilla constructs a GroundPathNavigation here, independent of the mob's
            // currently configured navigation implementation.
            let mut navigator = Navigator::default();
            let path = navigator
                .compute_path(&mob.get_mob_entity().living_entity, destination)
                .await
                .ok_or_else(|| ERROR_NO_PATH.create_without_context())?;
            if !path.can_reach() {
                return Err(ERROR_NOT_COMPLETE.create_without_context());
            }

            context
                .source
                .send_feedback(TextComponent::text("Made path"), true)
                .await;
            Ok(1)
        })
    }
}

/// Registers `/debugpath`, matching `DebugPathCommand.java:21-27`.
pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("debugpath", DESCRIPTION)
            .requires(PERMISSION)
            .then(argument(ARG_TARGET, BlockPosArgumentType).executes(DebugPathExecutor)),
    );
}
