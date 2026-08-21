#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    #[expect(clippy::too_many_lines)]
    pub async fn handle_player_action(
        &self,
        player: &Arc<Player>,
        player_action: SPlayerAction,
        server: &Server,
    ) {
        if !player.has_client_loaded() {
            return;
        }
        player.update_last_action_time();
        match Status::try_from(player_action.status.0) {
            Ok(status) => match status {
                Status::StartedDigging => {
                    if !player.can_interact_with_block_at(&player_action.position, 1.0) {
                        warn!(
                            "Player {0} tried to interact with block out of reach at {1}",
                            player.gameprofile.name, player_action.position
                        );
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }
                    let position = player_action.position;
                    let entity = &player.get_entity();
                    let world = entity.world.load_full();
                    let (block, state) = world.get_block_and_state(&position);

                    // Vanilla rejects restricted block actions before calling
                    // BlockState.attack or firing any block-use side effects.
                    if !player.can_break_block(server, block).await {
                        self.enqueue_client_packet(&CBlockUpdate::new(
                            position,
                            VarInt(i32::from(state.id.as_u16())),
                        ))
                        .await;
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    if let Some(server_arc) = world.server.upgrade() {
                        let mut event =
                            crate::plugin::api::events::block::block_damage::BlockDamageEvent::new(
                                player.clone(),
                                block,
                                position,
                                false,
                            );
                        server_arc
                            .plugin_manager
                            .fire(&server_arc, &mut event)
                            .await;
                        if event.cancelled {
                            self.update_sequence(player, player_action.sequence.0);
                            return;
                        }
                    }

                    if block == &pumpkin_data::Block::NOTE_BLOCK {
                        let props =
                            pumpkin_data::block_properties::NoteBlockLikeProperties::from_state_id(
                                state.id, block,
                            );
                        crate::block::blocks::note::NoteBlock::play_note(
                            &props,
                            &world,
                            &position,
                            crate::world::game_event::GameEventContext::of_entity(
                                player.clone() as Arc<dyn EntityBase>
                            ),
                        )
                        .await;
                        player
                            .increment_stat(
                                StatisticCategory::Custom,
                                CustomStatistic::PlayNoteblock as i32,
                                1,
                            )
                            .await;
                    }

                    if player.gamemode.load() == GameMode::Creative {
                        player.finish_block_break(server, &world, position).await;
                        self.sync_block_state_to_client(&world, position).await;
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }
                    player.start_mining_time.store(
                        player.tick_counter.load(Ordering::Relaxed),
                        Ordering::Relaxed,
                    );
                    if !state.is_air() {
                        let speed = block::calc_block_breaking(player, state, block).await;
                        // Instant break
                        if speed >= 1.0 {
                            player.finish_block_break(server, &world, position).await;
                            self.sync_block_state_to_client(&world, position).await;
                        } else {
                            let old_position = *player.mining_pos.lock().await;
                            let already_mining = player.mining.load(Ordering::Relaxed);
                            if already_mining && old_position != position {
                                self.send_packet(&CBlockUpdate::new(
                                    old_position,
                                    VarInt(i32::from(
                                        world.get_block_state(&old_position).id.as_u16(),
                                    )),
                                ))
                                .await;
                                world
                                    .set_block_breaking(
                                        entity,
                                        old_position,
                                        BlockBreakingProgress::Stop,
                                    )
                                    .await;
                            }
                            player.mining.store(true, Ordering::Relaxed);
                            *player.mining_pos.lock().await = position;
                            let progress = (speed * 10.0) as i32;
                            player
                                .current_block_breaking_speed
                                .store(speed.to_bits(), Ordering::Relaxed);
                            world
                                .set_block_breaking(
                                    entity,
                                    position,
                                    BlockBreakingProgress::Start {
                                        stage: progress,
                                        speed,
                                    },
                                )
                                .await;
                            player
                                .current_block_destroy_stage
                                .store(progress, Ordering::Relaxed);
                        }
                    }
                    self.update_sequence(player, player_action.sequence.0);
                }
                Status::CancelledDigging => {
                    if !player.can_interact_with_block_at(&player_action.position, 1.0) {
                        warn!(
                            "Player {0} tried to interact with block out of reach at {1}",
                            player.gameprofile.name, player_action.position
                        );
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }
                    player.mining.store(false, Ordering::Relaxed);
                    let entity = &player.get_entity();
                    let world = entity.world.load_full();
                    let active_position = *player.mining_pos.lock().await;
                    if active_position != player_action.position {
                        world
                            .set_block_breaking(
                                entity,
                                active_position,
                                BlockBreakingProgress::Stop,
                            )
                            .await;
                    }
                    world
                        .set_block_breaking(
                            entity,
                            player_action.position,
                            BlockBreakingProgress::Stop,
                        )
                        .await;
                    self.update_sequence(player, player_action.sequence.0);
                }
                Status::FinishedDigging => {
                    let location = player_action.position;
                    if !player.can_interact_with_block_at(&location, 1.0) {
                        warn!(
                            "Player {0} tried to interact with block out of reach at {1}",
                            player.gameprofile.name, player_action.position
                        );
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    // Vanilla only accepts STOP_DESTROY_BLOCK for the block currently being
                    // mined. Without this check a client can start one block and finish any
                    // other reachable block immediately.
                    if !player.mining.load(Ordering::Relaxed)
                        || *player.mining_pos.lock().await != location
                    {
                        let world = player.world();
                        self.sync_block_state_to_client(&world, location).await;
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    // Block break & play sound
                    let entity = &player.get_entity();
                    let world = entity.world.load_full();

                    let (block, state) = world.get_block_and_state(&location);
                    if state.is_air() {
                        player.mining.store(false, Ordering::Relaxed);
                        self.sync_block_state_to_client(&world, location).await;
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    let elapsed_ticks = player.tick_counter.load(Ordering::Relaxed)
                        - player.start_mining_time.load(Ordering::Relaxed);
                    let speed = block::calc_block_breaking(player, state, block).await;
                    if speed * ((elapsed_ticks + 1).max(0) as f32) < 0.7 {
                        if !player.delayed_mining.load(Ordering::Relaxed) {
                            *player.delayed_mining_pos.lock().await = location;
                            player.delayed_mining_start_time.store(
                                player.start_mining_time.load(Ordering::Relaxed),
                                Ordering::Relaxed,
                            );
                            player.delayed_mining.store(true, Ordering::Relaxed);
                        }
                        player.mining.store(false, Ordering::Relaxed);
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    player.mining.store(false, Ordering::Relaxed);
                    world
                        .set_block_breaking(entity, location, BlockBreakingProgress::Stop)
                        .await;
                    if !player.finish_block_break(server, &world, location).await {
                        self.sync_block_state_to_client(&world, location).await;
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    self.sync_block_state_to_client(&world, location).await;

                    self.update_sequence(player, player_action.sequence.0);
                }
                Status::DropItem => {
                    player.drop_held_item(false).await;
                }
                Status::DropItemStack => {
                    player.drop_held_item(true).await;
                }
                Status::ReleaseItemInUse => {
                    let item_in_use = player.living_entity.item_in_use.lock().await.clone();
                    if let Some(stack) = item_in_use {
                        server.item_registry.on_stopped_using(&stack, player).await;
                    }

                    player.living_entity.clear_active_hand().await;
                }
                Status::SwapItem => {
                    player.swap_item().await;
                }
                Status::SpearJab => {
                    debug!("todo");
                }
            },
            Err(_) => self.kick(TextComponent::text("Invalid status")).await,
        }
    }

    pub fn update_sequence(&self, _player: &Player, sequence: i32) {
        if sequence < 0 {
            error!("Expected packet sequence >= 0");
        }
        self.packet_sequence.store(
            self.packet_sequence.load(Ordering::Relaxed).max(sequence),
            Ordering::Relaxed,
        );
    }

    async fn sync_block_state_to_client(&self, world: &World, position: BlockPos) {
        let synced_state_id = world.get_block_state_id(&position);
        self.send_packet(&CBlockUpdate::new(
            position,
            VarInt(i32::from(synced_state_id.as_u16())),
        ))
        .await;
    }
}
