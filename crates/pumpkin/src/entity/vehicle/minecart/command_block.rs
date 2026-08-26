use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::{GameMode, PermissionLvl};

use crate::{
    block::entities::command_block::CommandBlockEntity,
    command::CommandSender,
    entity::{Entity, player::Player},
    world::World,
};

/// Port of vanilla `MinecartCommandBlock`
/// (net/minecraft/world/entity/vehicle/minecart/MinecartCommandBlock.java:27-141):
/// a minecart carrying an anonymous `BaseCommandBlock` field
/// (MinecartCommandBlock.java:32) that runs its command when the cart crosses a
/// powered activator rail (MinecartCommandBlock.java:81-86).
///
/// Like vanilla's `MinecartCommandBase extends BaseCommandBlock`, the carried
/// command state reuses [`CommandBlockEntity`] as its storage; the dispatcher's
/// `@` source, permissions and output routing all come from that shared type.
pub(super) struct CommandBlockMinecart {
    /// `MinecartCommandBlock.commandBlock` (MinecartCommandBlock.java:32). Its
    /// `position` tracks the cart's current block position so relative
    /// coordinates resolve like vanilla's `createCommandSourceStack`
    /// (MinecartCommandBlock.java:122-134), which reads the live cart position.
    pub(super) command_block: Arc<CommandBlockEntity>,
    /// Vanilla compares `tickCount - lastActivated` against
    /// `ACTIVATION_DELAY` = 4 (MinecartCommandBlock.java:33, 82); this counter
    /// plays the role of `Entity.tickCount`, incremented once per cart tick.
    tick_count: AtomicI32,
    /// `MinecartCommandBlock.lastActivated` (MinecartCommandBlock.java:34),
    /// stored as a `tickCount` value after each activation
    /// (MinecartCommandBlock.java:84).
    last_activated: AtomicI32,
}

impl CommandBlockMinecart {
    /// `ACTIVATION_DELAY` (MinecartCommandBlock.java:33).
    const ACTIVATION_DELAY: i32 = 4;

    pub(super) fn new() -> Self {
        Self {
            // Fresh `BaseCommandBlock` defaults (BaseCommandBlock.java:26-31):
            // empty command, trackOutput = true.
            command_block: Arc::new(CommandBlockEntity::new(
                BlockPos(pumpkin_util::math::vector3::Vector3::new(0, 0, 0)),
                true,
                false,
            )),
            tick_count: AtomicI32::new(0),
            last_activated: AtomicI32::new(0),
        }
    }

    /// Advances the local `tickCount` mirror once per cart tick.
    pub(super) fn tick(&self) {
        self.tick_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Port of `MinecartCommandBlock.activateMinecart`
    /// (MinecartCommandBlock.java:81-86): run the carried command when the cart
    /// crosses a powered activator rail, but at most once every
    /// `ACTIVATION_DELAY` ticks. The caller only invokes this while the rail is
    /// powered, matching vanilla's `state` argument
    /// (NewMinecartBehavior.java:249-251).
    pub(super) async fn activate(&self, world: &Arc<World>, entity: &Entity) {
        let tick_count = self.tick_count.load(Ordering::Relaxed);
        if tick_count.saturating_sub(self.last_activated.load(Ordering::Relaxed))
            < Self::ACTIVATION_DELAY
        {
            return;
        }
        self.perform_command(world, entity).await;
        self.last_activated.store(tick_count, Ordering::Relaxed);
    }

    /// Port of `BaseCommandBlock.performCommand`
    /// (net/minecraft/world/level/BaseCommandBlock.java:89-130) as reached
    /// through the minecart's `performCommand` call site.
    async fn perform_command(&self, world: &Arc<World>, entity: &Entity) {
        // `level.isCommandBlockEnabled()` gates on the COMMAND_BLOCKS_WORK
        // gamerule (BaseCommandBlock.java:101).
        let command_blocks_work = { world.level_info.load().game_rules.command_blocks_work };

        let command = self.command_block.command.lock().await;
        if !command_blocks_work || command.is_empty() {
            // Vanilla zeroes the success count before dispatching
            // (BaseCommandBlock.java:100).
            self.command_block.success_count.store(0, Ordering::Relaxed);
            return;
        }

        // "Searge" easter egg (BaseCommandBlock.java:94-98).
        if command.eq_ignore_ascii_case("Searge") {
            *self.command_block.last_output.lock().await = "#itzlipofutzli".to_string();
            self.command_block.success_count.store(1, Ordering::Relaxed);
            self.sync_metadata(entity).await;
            return;
        }

        self.command_block.success_count.store(0, Ordering::Relaxed);

        // `MinecartCommandBase.createCommandSourceStack`
        // (MinecartCommandBlock.java:122-134) executes at the cart's live
        // position; keep the shared entity's position in sync first.
        self.command_block.position.store(entity.block_pos.load());

        let Some(server) = world.server.upgrade() else {
            return;
        };
        let source = CommandSender::CommandBlock(self.command_block.clone(), world.clone())
            .into_source(&server)
            .await;
        server
            .command_dispatcher
            .load()
            .handle_command(&source, &command)
            .await;
        drop(command);

        // `MinecartCommandBase.onUpdated` (MinecartCommandBlock.java:116-119):
        // push the (possibly changed) command/output back into synced data so
        // clients editing the cart see fresh state.
        self.sync_metadata(entity).await;
    }

    /// Sends `DATA_ID_COMMAND_NAME`/`DATA_ID_LAST_OUTPUT`
    /// (MinecartCommandBlock.java:28-29, defined at :51-55) to tracking
    /// clients; the vanilla client builds its edit screen purely from these.
    pub(super) async fn sync_metadata(&self, entity: &Entity) {
        let command = self.command_block.command.lock().await.clone();
        let last_output = self.command_block.last_output.lock().await.clone();
        entity.send_meta_data(
            &[
                Metadata::new(
                    pumpkin_data::tracked_data::command_block_minecart::DATA_ID_COMMAND_NAME,
                    command,
                ),
                Metadata::new(
                    pumpkin_data::tracked_data::command_block_minecart::DATA_ID_LAST_OUTPUT,
                    last_output,
                ),
            ],
            None,
        );
    }

    /// Port of `MinecartCommandBlock.readAdditionalSaveData`
    /// (MinecartCommandBlock.java:58-63) -> `BaseCommandBlock.load`
    /// (BaseCommandBlock.java:61-78). Only the keys modelled by
    /// [`CommandBlockEntity`] are read; unknown ones are ignored, as in vanilla.
    pub(super) async fn read_nbt(&self, nbt: &NbtCompound) {
        *self.command_block.command.lock().await =
            nbt.get_string("Command").unwrap_or("").to_string();
        *self.command_block.last_output.lock().await =
            nbt.get_string("LastOutput").unwrap_or("").to_string();
        self.command_block.success_count.store(
            nbt.get_int("SuccessCount").unwrap_or(0) as u32,
            Ordering::Relaxed,
        );
        self.command_block.track_output.store(
            nbt.get_bool("TrackOutput").unwrap_or(true),
            Ordering::Relaxed,
        );
    }

    /// Port of `MinecartCommandBlock.addAdditionalSaveData`
    /// (MinecartCommandBlock.java:66-69) -> `BaseCommandBlock.save`
    /// (BaseCommandBlock.java:46-59). `LastOutput` is written only when output
    /// tracking is enabled (BaseCommandBlock.java:51-53).
    pub(super) async fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_string(
            "Command",
            self.command_block.command.lock().await.to_string(),
        );
        nbt.put_int(
            "SuccessCount",
            self.command_block.success_count.load(Ordering::Relaxed) as i32,
        );
        let track_output = self.command_block.track_output.load(Ordering::Relaxed);
        nbt.put_bool("TrackOutput", track_output);
        if track_output {
            nbt.put_string(
                "LastOutput",
                self.command_block.last_output.lock().await.to_string(),
            );
        }
        nbt.put_bool("UpdateLastExecution", false);
    }

    /// Port of `MinecartCommandBlock.interact`
    /// (MinecartCommandBlock.java:89-99): only game-master players may open the
    /// cart's command screen; everyone else passes through. The vanilla client
    /// opens its editor locally from synced data, so the server side is just
    /// the permission gate plus SUCCESS.
    pub(super) fn interact(player: &Player) -> bool {
        // `Player.canUseGameMasterBlocks` (Player.java:1863-1865): creative
        // mode plus permission level 2.
        player.gamemode.load() == GameMode::Creative
            && player.permission_lvl.load() >= PermissionLvl::Two
    }
}
