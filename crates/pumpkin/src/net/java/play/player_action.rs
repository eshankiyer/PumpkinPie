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
                    if player.gamemode.load() == GameMode::Spectator {
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }
                    let position = player_action.position;
                    let entity = &player.get_entity();
                    let world = entity.world.load_full();

                    // ServerPlayerGameMode rejects positions above the level's
                    // maximum build height before starting a destroy action.
                    if position.0.y > world.get_top_y() {
                        self.sync_block_state_to_client(&world, position).await;
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    if player
                        .is_under_spawn_protection(server, &world, &position)
                        .await
                    {
                        let message = TextComponent::translate_cross(
                            translation::java::BUILD_SPAWN_PROTECTION,
                            translation::java::BUILD_SPAWN_PROTECTION,
                            vec![TextComponent::text(format!("[{position}]"))],
                        );
                        // `ServerPlayer.sendOverlayMessage` uses the overlay chat position
                        // (`ServerPlayer.java:1798-1805`).
                        player.send_overlay_message(&message).await;
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    // ServerLevel.mayInteract also applies the world border.
                    let inside_world_border = {
                        let border = world.worldborder.lock().await;
                        border.contains_block(position.0.x, position.0.z)
                    };
                    if !inside_world_border {
                        self.sync_block_state_to_client(&world, position).await;
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    let (block, state) = world.get_block_and_state(&position);

                    // Vanilla checks Player.blockActionRestricted at the start of a
                    // destroy action and defers the held item's destruction rules to
                    // ServerPlayerGameMode.destroyBlock.
                    if !player.can_start_block_break(block, state).await {
                        self.sync_block_state_to_client(&world, position).await;
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
                    // Vanilla starts an air target with progress 1.0, dispatches
                    // BlockState.attack for every other block, and only then decides
                    // between an instant break and a tracked destroy target.
                    let speed = if state.is_air() {
                        1.0f32
                    } else {
                        server
                            .block_registry
                            .attack(&world, block, state, &position, player)
                            .await;
                        block::calc_block_breaking(player, state, block).await
                    };

                    if !state.is_air() && speed >= 1.0 {
                        // Instant break
                        player.finish_block_break(server, &world, position).await;
                        self.sync_block_state_to_client(&world, position).await;
                    } else {
                        // Vanilla tracks an air target as an active destroy target so a
                        // subsequent STOP for another position cannot complete it.
                        let old_position = *player.mining_pos.lock().await;
                        let already_mining = player.mining.load(Ordering::Relaxed);
                        if already_mining && old_position != position {
                            self.send_packet(&CBlockUpdate::new(
                                old_position,
                                VarInt(i32::from(world.get_block_state(&old_position).id.as_u16())),
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
                    let entity = &player.get_entity();
                    let world = entity.world.load_full();
                    // ServerPlayerGameMode rejects any block action above the level's
                    // maximum build height, including ABORT_DESTROY_BLOCK.
                    if player_action.position.0.y > world.get_top_y() {
                        self.sync_block_state_to_client(&world, player_action.position)
                            .await;
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }
                    let active_position = *player.mining_pos.lock().await;
                    player.mining.store(false, Ordering::Relaxed);
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

                    // Block break & play sound
                    let entity = &player.get_entity();
                    let world = entity.world.load_full();

                    // ServerPlayerGameMode rejects any block action above the level's
                    // maximum build height, including STOP_DESTROY_BLOCK.
                    if location.0.y > world.get_top_y() {
                        self.sync_block_state_to_client(&world, location).await;
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    let (block, state) = world.get_block_and_state(&location);
                    if player.gamemode.load() == GameMode::Spectator {
                        player.mining.store(false, Ordering::Relaxed);
                        player.delayed_mining.store(false, Ordering::Relaxed);
                        self.sync_block_state_to_client(&world, location).await;
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    let mining_position = *player.mining_pos.lock().await;
                    let delayed_position = *player.delayed_mining_pos.lock().await;
                    let delayed_target = player.delayed_mining.load(Ordering::Relaxed)
                        && delayed_position == location;
                    let same_mining_target = (player.mining.load(Ordering::Relaxed)
                        && mining_position == location)
                        || delayed_target;
                    // Vanilla only evaluates STOP_DESTROY_BLOCK for the target
                    // currently held by ServerPlayerGameMode.
                    if !same_mining_target {
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }
                    // ServerPlayerGameMode leaves the active target untouched
                    // when STOP arrives after that target became air.
                    if state.is_air() {
                        self.update_sequence(player, player_action.sequence.0);
                        return;
                    }

                    let destroy_start = if delayed_target {
                        player.delayed_mining_start_time.load(Ordering::Relaxed)
                    } else {
                        player.start_mining_time.load(Ordering::Relaxed)
                    };
                    let elapsed_ticks = player
                        .tick_counter
                        .load(Ordering::Relaxed)
                        .saturating_sub(destroy_start);
                    let speed = block::calc_block_breaking(player, state, block).await;
                    // ServerPlayerGameMode only destroys a non-instant block after the
                    // STOP packet has accumulated vanilla's 0.7 progress threshold.
                    // A client cannot bypass this by sending START and STOP back to back.
                    if player.gamemode.load() != GameMode::Creative
                        && speed * ((elapsed_ticks + 1).max(0) as f32) < 0.7
                    {
                        if !delayed_target {
                            *player.delayed_mining_pos.lock().await = location;
                            player
                                .delayed_mining_start_time
                                .store(destroy_start, Ordering::Relaxed);
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
