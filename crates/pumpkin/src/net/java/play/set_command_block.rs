#[allow(clippy::wildcard_imports)]
use super::*;
use crossbeam::atomic::AtomicCell;

/// Vanilla `CommandBlockEntity.setAutomatic` (`CommandBlockEntity.java:102-108`) and
/// `onModeSwitch` (`:110-115`), both invoked from
/// `ServerGamePacketListenerImpl.handleSetCommandBlock` (`:648-650`): newly enabling
/// "always active" starts the clock unless this is a chain block, and switching into
/// repeating mode while powered or automatic starts it immediately.
fn schedule_command_block_clock(
    player: &Arc<Player>,
    pos: pumpkin_util::math::position::BlockPos,
    previous_block: &'static Block,
    new_block: &Block,
    auto: bool,
    previous_auto: bool,
    powered: bool,
) {
    let schedule = |block: &Block| {
        player.world().schedule_block_tick(
            block,
            pos,
            1,
            pumpkin_world::tick::TickPriority::Normal,
        );
    };

    if !previous_auto && auto && !powered && new_block.id != Block::CHAIN_COMMAND_BLOCK.id {
        schedule(new_block);
    }

    if previous_block.id != new_block.id
        && new_block.id == Block::REPEATING_COMMAND_BLOCK.id
        && (auto || powered)
    {
        schedule(new_block);
    }
}

impl JavaClient {
    pub async fn handle_set_command_block(
        &self,
        player: &Arc<Player>,
        command: SSetCommandBlock<'_>,
    ) {
        if !player.is_creative() {
            return;
        }
        if player.permission_lvl.load() < PermissionLvl::Two {
            return;
        }
        let pos = command.pos;
        let block_entity = player.world().get_block_entity(&pos);
        if let Some(block_entity) = block_entity {
            if block_entity.resource_location() != CommandBlockEntity::ID {
                warn!("Client tried to change Command block but not Command block entity found");
                return;
            }

            let Ok(command_block_mode) = CommandBlockMode::try_from(command.mode) else {
                self.kick(TextComponent::text("Invalid Command block mode"))
                    .await;
                return;
            };

            let block = player.world().get_block(&pos);
            let old_state_id = player.world().get_block_state_id(&pos);
            let mut props = CommandBlockLikeProperties::from_state_id(old_state_id, block);

            let block_type = match command_block_mode {
                CommandBlockMode::Chain => Block::CHAIN_COMMAND_BLOCK,
                CommandBlockMode::Repeating => Block::REPEATING_COMMAND_BLOCK,
                CommandBlockMode::Impulse => Block::COMMAND_BLOCK,
            };

            let Some(old_command_block) =
                block_entity.as_any().downcast_ref::<CommandBlockEntity>()
            else {
                return;
            };

            props.conditional = command.is_conditional();

            let new_state_id = props.to_state_id(&block_type);
            player
                .world()
                .set_block_state(
                    &command.pos,
                    new_state_id,
                    BlockFlags::SKIP_BLOCK_ADDED_CALLBACK,
                )
                .await;

            let mut cmd = command.command;
            if cmd.starts_with('/') {
                cmd = &cmd[1..];
            }

            let command_block = Arc::new(CommandBlockEntity {
                position: AtomicCell::new(pos),
                powered: old_command_block.powered.load(Ordering::SeqCst).into(),
                condition_met: old_command_block
                    .condition_met
                    .load(Ordering::SeqCst)
                    .into(),
                auto: command.is_automatic().into(),
                dirty: old_command_block.dirty.load(Ordering::SeqCst).into(),
                command: Mutex::new(cmd.to_string()),
                last_output: old_command_block.last_output.lock().await.clone().into(),
                track_output: command.track_output().into(),
                success_count: AtomicU32::new(0),
                // Preserve the command block's BaseCommandBlock name while replacing its
                // block-state-backed entity (`CommandBlockEntity.java:69-84`).
                custom_name: std::sync::Mutex::new(
                    old_command_block
                        .custom_name
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone(),
                ),
            });
            player.world().add_block_entity(command_block.clone());
            command_block.mark_condition_met(&player.world());
            command_block.on_updated(&player.world());

            player
                .send_system_message(&TextComponent::text(format!(
                    "Command set: {}",
                    command.command
                )))
                .await;

            schedule_command_block_clock(
                player,
                pos,
                block,
                &block_type,
                command.is_automatic(),
                old_command_block.auto.load(Ordering::SeqCst),
                old_command_block.powered.load(Ordering::SeqCst),
            );
        }
    }
}
