use md5::compute;
use pumpkin_protocol::java::client::play::{CAddResourcePack, CRemoveResourcePack};
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use uuid::Uuid;

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::core::string::StringArgumentType;
use crate::command::argument_types::uuid::UuidArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};

const DESCRIPTION: &str = "Pushes or removes a server resource pack.";
const PERMISSION: &str = "minecraft:command.serverpack";
const ARG_URL: &str = "url";
const ARG_UUID: &str = "uuid";
const ARG_HASH: &str = "hash";

/// Implements `ServerPackCommand.push` (`ServerPackCommand.java:47-61`).
struct PushExecutor;

impl CommandExecutor for PushExecutor {
    /// Implements the packet construction and broadcast in
    /// `ServerPackCommand.push` (`ServerPackCommand.java:47-61`).
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let url = StringArgumentType::get(context, ARG_URL)?;
            let uuid = context
                .get_argument::<Uuid>(ARG_UUID)
                .copied()
                .unwrap_or_else(|_| java_name_uuid_from_bytes(url.as_bytes()));
            let hash = StringArgumentType::get(context, ARG_HASH).unwrap_or_default();
            let packet = CAddResourcePack::new(&uuid, url, hash, false, None);

            for player in context.server().get_all_players() {
                player.send_client_packet(&packet).await;
            }

            Ok(0)
        })
    }
}

/// Implements `ServerPackCommand.pop` (`ServerPackCommand.java:63-68`).
struct PopExecutor;

impl CommandExecutor for PopExecutor {
    /// Implements the removal packet broadcast in
    /// `ServerPackCommand.pop` (`ServerPackCommand.java:63-68`).
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let uuid = *context.get_argument::<Uuid>(ARG_UUID)?;
            let packet = CRemoveResourcePack::new(Some(&uuid));

            for player in context.server().get_all_players() {
                player.send_client_packet(&packet).await;
            }

            Ok(0)
        })
    }
}

/// Matches Java `UUID.nameUUIDFromBytes` used by `ServerPackCommand.push`
/// (`ServerPackCommand.java:51-52`) without introducing a namespace prefix.
fn java_name_uuid_from_bytes(bytes: &[u8]) -> Uuid {
    let mut digest = compute(bytes).0;
    digest[6] = (digest[6] & 0x0f) | 0x30;
    digest[8] = (digest[8] & 0x3f) | 0x80;
    Uuid::from_bytes(digest)
}

/// Registers `/serverpack`, matching `ServerPackCommand.register`
/// (`ServerPackCommand.java:16-45`).
pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let push = literal("push").then(
        argument(ARG_URL, StringArgumentType::SingleWord)
            .executes(PushExecutor)
            .then(
                argument(ARG_UUID, UuidArgumentType)
                    .executes(PushExecutor)
                    .then(
                        argument(ARG_HASH, StringArgumentType::SingleWord).executes(PushExecutor),
                    ),
            ),
    );

    dispatcher.register(
        command("serverpack", DESCRIPTION)
            .requires(PERMISSION)
            .then(push)
            .then(literal("pop").then(argument(ARG_UUID, UuidArgumentType).executes(PopExecutor))),
    );
}

#[cfg(test)]
mod tests {
    use super::java_name_uuid_from_bytes;

    #[test]
    fn generated_pack_uuid_uses_java_name_uuid_rules() {
        assert_eq!(
            java_name_uuid_from_bytes(b"https://example.invalid/pack.zip").to_string(),
            "f2dfd86d-3bee-3650-9f80-4317b330dccc"
        );
    }
}
