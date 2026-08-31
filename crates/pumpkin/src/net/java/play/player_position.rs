#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    const fn clamp_horizontal(pos: f64) -> f64 {
        pos.clamp(-3.0E7, 3.0E7)
    }

    const fn clamp_vertical(pos: f64) -> f64 {
        pos.clamp(-2.0E7, 2.0E7)
    }

    /// Vanilla `Player.maybeBackOffFromEdge` (`Player.java:880-940`) trims a sneaking player's
    /// horizontal movement in 0.05-block increments while the inset footprint would otherwise
    /// fall farther than `STEP_HEIGHT`.
    fn maybe_back_off_from_edge(
        player: &Player,
        world: &World,
        position: Vector3<f64>,
        last_position: Vector3<f64>,
        flying: bool,
    ) -> Vector3<f64> {
        let entity = player.get_entity();
        let delta = position - last_position;
        let max_down_step = player
            .living_entity
            .get_attribute_value(&pumpkin_data::attributes::Attributes::STEP_HEIGHT);
        let can_fall_at_least = |delta_x: f64, delta_z: f64, min_height: f64| {
            let bounding_box = entity.bounding_box.load();
            world.is_space_empty(BoundingBox::new(
                Vector3::new(
                    bounding_box.min.x + 1.0E-7 + delta_x,
                    bounding_box.min.y - min_height - 1.0E-7,
                    bounding_box.min.z + 1.0E-7 + delta_z,
                ),
                Vector3::new(
                    bounding_box.max.x - 1.0E-7 + delta_x,
                    bounding_box.min.y,
                    bounding_box.max.z - 1.0E-7 + delta_z,
                ),
            ))
        };

        let fall_distance = player.living_entity.fall_distance.load();
        let above_ground = entity.on_ground.load(Ordering::Relaxed)
            || (f64::from(fall_distance) < max_down_step
                && !can_fall_at_least(0.0, 0.0, max_down_step - f64::from(fall_distance)));
        if flying || delta.y > 0.0 || !entity.sneaking.load(Ordering::Relaxed) || !above_ground {
            return position;
        }

        let can_fall =
            |delta_x: f64, delta_z: f64| can_fall_at_least(delta_x, delta_z, max_down_step);
        let mut delta_x = delta.x;
        let mut delta_z = delta.z;
        let step_x = delta_x.signum() * 0.05;
        let step_z = delta_z.signum() * 0.05;

        while delta_x != 0.0 && can_fall(delta_x, 0.0) {
            if delta_x.abs() <= 0.05 {
                delta_x = 0.0;
                break;
            }
            delta_x -= step_x;
        }
        while delta_z != 0.0 && can_fall(0.0, delta_z) {
            if delta_z.abs() <= 0.05 {
                delta_z = 0.0;
                break;
            }
            delta_z -= step_z;
        }
        while delta_x != 0.0 && delta_z != 0.0 && can_fall(delta_x, delta_z) {
            if delta_x.abs() <= 0.05 {
                delta_x = 0.0;
            } else {
                delta_x -= step_x;
            }
            if delta_z.abs() <= 0.05 {
                delta_z = 0.0;
            } else {
                delta_z -= step_z;
            }
        }

        last_position + Vector3::new(delta_x, delta.y, delta_z)
    }

    /// Returns whether syncing the position was needed
    fn sync_position(
        player: &Arc<Player>,
        world: &World,
        pos: Vector3<f64>,
        last_pos: Vector3<f64>,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    ) -> bool {
        let delta = Vector3::new(pos.x - last_pos.x, pos.y - last_pos.y, pos.z - last_pos.z);
        let entity_id = player.entity_id();

        // Teleport when more than 8 blocks (-8..=7.999755859375)
        if delta.length_squared() < 64.0 {
            return false;
        }
        // Sync position with all other players.
        world.broadcast_packet_except(
            &[player.gameprofile.id],
            &CEntityPositionSync::new(
                entity_id.into(),
                pos,
                Vector3::new(0.0, 0.0, 0.0),
                yaw,
                pitch,
                on_ground,
            ),
        );
        true
    }

    #[expect(clippy::too_many_lines)]
    pub async fn handle_position(
        &self,
        player: &Arc<Player>,
        server: &Arc<Server>,
        packet: SPlayerPosition,
    ) {
        if !player.has_client_loaded() {
            return;
        }
        if player.get_entity().has_vehicle().await {
            return;
        }
        // Ignore movement packets while awaiting a teleport confirmation (vanilla behavior)
        if player.awaiting_teleport.lock().await.is_some() {
            return;
        }
        // y = feet Y
        let position = packet.position;
        if !has_finite_position(position) {
            self.kick(TextComponent::translate_cross(
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_PLAYER_MOVEMENT,
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_PLAYER_MOVEMENT,
                [],
            ))
            .await;
            return;
        }
        let position = Vector3::new(
            Self::clamp_horizontal(position.x),
            Self::clamp_vertical(position.y),
            Self::clamp_horizontal(position.z),
        );
        let entity = player.get_entity();
        let last_pos = entity.pos.load();
        let flying = player.abilities.lock().await.flying;
        let position =
            Self::maybe_back_off_from_edge(player, &player.world(), position, last_pos, flying);
        let (player_movement_check, elytra_movement_check) = {
            let level_info = player.world().level_info.load();
            (
                level_info.game_rules.player_movement_check,
                level_info.game_rules.elytra_movement_check,
            )
        };
        if movement_requires_correction(
            last_pos,
            position,
            entity.velocity.load(),
            MovementCheckContext {
                fall_flying: entity.is_fall_flying(),
                ticks_run_normally: server.tick_rate_manager.runs_normally(),
                player_movement_check,
                elytra_movement_check,
                sleeping: player.sleeping_since.load().is_some(),
                // `handleMovePlayer` exempts impulse grace time from the moved-wrongly check
                // (`ServerGamePacketListenerImpl.java:1140-1145`).
                post_impulse_grace_time: player.living_entity.is_in_post_impulse_grace_time(),
            },
        ) {
            self.force_tp(player, last_pos).await;
            return;
        }

        send_cancellable! {{
            server;
            PlayerMoveEvent {
                player: player.clone(),
                from: player.get_entity().pos.load(),
                to: position,
                cancelled: false,
            };

            'after: {
                let pos = event.to;
                let entity = &player.get_entity();
                let last_pos = entity.pos.load();
                player.get_entity().set_pos(pos);

                let distance = last_pos.squared_distance_to_vec(&pos).sqrt();
                let cm = (distance * 100.0) as i32;
                if cm > 0 {
                    let stat = player.get_movement_statistic().await;
                    player
                        .increment_stat(StatisticCategory::Custom, stat as i32, cm)
                        .await;
                }

                let height_difference = pos.y - last_pos.y;
                if entity.on_ground.load(Ordering::Relaxed) && packet.collision & FLAG_ON_GROUND == 0 && height_difference > 0.0 {
                    player.jump().await;
                }

                let new_on_ground = packet.collision & FLAG_ON_GROUND != 0;
                entity.on_ground.store(new_on_ground, Ordering::Relaxed);
                // `handleMovePlayer` applies the packet's horizontal-collision bit with the
                // client movement (`ServerGamePacketListenerImpl.java:1165-1167`).
                entity.horizontal_collision.store(
                    packet.collision & FLAG_HORIZONTAL_COLLISION != 0,
                    Ordering::Relaxed,
                );
                if new_on_ground && entity.is_fall_flying() {
                    entity.set_fall_flying(false).await;
                }
                // `handleMovePlayer` resets impulse context after a ground or liquid landing
                // (`ServerGamePacketListenerImpl.java:1178-1184`).
                if packet.collision & FLAG_ON_GROUND != 0
                    || player.living_entity.has_landed_in_liquid()
                {
                    player.living_entity.try_reset_current_impulse_context();
                }
                let world = &player.world();

                // TODO: Warn when player moves to quickly
                if !Self::sync_position(player, world, pos, last_pos, entity.yaw.load(), entity.pitch.load(), packet.collision & FLAG_ON_GROUND != 0) {
                    // Send the new position to all other players.
                    world.broadcast_packet_except_editioned_sync(
                        &[player.gameprofile.id],
                        &CUpdateEntityPos::new(
                            player.entity_id().into(),
                            Vector3::new(
                                pos.x.mul_add(4096.0, -(last_pos.x * 4096.0)) as i16,
                                pos.y.mul_add(4096.0, -(last_pos.y * 4096.0)) as i16,
                                pos.z.mul_add(4096.0, -(last_pos.z * 4096.0)) as i16,
                            ),
                            packet.collision & FLAG_ON_GROUND != 0,
                        ),
                        &CMovePlayer::new(
                            VarULong(player.entity_id() as u64),
                            Vector3::new(pos.x as f32, pos.y as f32 + player.get_entity().entity_type.eye_height, pos.z as f32),
                            entity.pitch.load(),
                            entity.yaw.load(),
                            entity.yaw.load(),
                            CMovePlayer::MODE_NORMAL,
                            (packet.collision & FLAG_ON_GROUND) != 0,
                            VarULong(0),
                            0,
                            0,
                            VarULong(0),
                        ),
                    );
                }

                // `ServerGamePacketListenerImpl` invokes `doCheckFallDamage` after accepting
                // movement (`ServerGamePacketListenerImpl.java:1165-1167`).
                entity
                    .do_check_fall_damage(
                        player.clone(),
                        pos.x - last_pos.x,
                        height_difference,
                        pos.z - last_pos.z,
                        packet.collision & FLAG_ON_GROUND != 0,
                    )
                    .await;
                chunker::update_position(player).await;
                let delta = Vector3::new(
                    pos.x - last_pos.x,
                    pos.y - last_pos.y,
                    pos.z - last_pos.z,
                );
                // `handleMovePlayer` records accepted client movement until the next tick-end
                // packet (`ServerGamePacketListenerImpl.java:1165-1168`).
                self.handle_player_known_movement(player, delta);
                // Only update idle timeout if there's actual movement (vanilla threshold)
                if delta.length_squared() > 1.0E-5 {
                    player.update_last_action_time();
                }
                player.progress_motion(delta).await;
            }

            'cancelled: {
                self.force_tp(player, player.get_entity().pos.load()).await;
            }
        }}
    }

    #[expect(clippy::too_many_lines)]
    pub async fn handle_position_rotation(
        &self,
        player: &Arc<Player>,
        server: &Arc<Server>,
        packet: SPlayerPositionRotation,
    ) {
        if !player.has_client_loaded() {
            return;
        }
        if player.get_entity().has_vehicle().await {
            return;
        }
        // Ignore movement packets while awaiting a teleport confirmation (vanilla behavior)
        if player.awaiting_teleport.lock().await.is_some() {
            return;
        }
        // y = feet Y
        let position = packet.position;
        if !has_finite_position(position) || !packet.yaw.is_finite() || !packet.pitch.is_finite() {
            self.kick(TextComponent::translate_cross(
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_PLAYER_MOVEMENT,
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_PLAYER_MOVEMENT,
                [],
            ))
            .await;
            return;
        }

        let position = Vector3::new(
            Self::clamp_horizontal(position.x),
            Self::clamp_vertical(position.y),
            Self::clamp_horizontal(position.z),
        );
        let entity = player.get_entity();
        let last_pos = entity.pos.load();
        let flying = player.abilities.lock().await.flying;
        let position =
            Self::maybe_back_off_from_edge(player, &player.world(), position, last_pos, flying);
        let (player_movement_check, elytra_movement_check) = {
            let level_info = player.world().level_info.load();
            (
                level_info.game_rules.player_movement_check,
                level_info.game_rules.elytra_movement_check,
            )
        };
        if movement_requires_correction(
            last_pos,
            position,
            entity.velocity.load(),
            MovementCheckContext {
                fall_flying: entity.is_fall_flying(),
                ticks_run_normally: server.tick_rate_manager.runs_normally(),
                player_movement_check,
                elytra_movement_check,
                sleeping: player.sleeping_since.load().is_some(),
                // `handleMovePlayer` exempts impulse grace time from the moved-wrongly check
                // (`ServerGamePacketListenerImpl.java:1140-1145`).
                post_impulse_grace_time: player.living_entity.is_in_post_impulse_grace_time(),
            },
        ) {
            self.force_tp(player, last_pos).await;
            return;
        }

        send_cancellable! {{
            server;
            PlayerMoveEvent::new(
                player.clone(),
                player.get_entity().pos.load(),
                position,
            );

            'after: {
                let pos = event.to;
                let entity = &player.get_entity();
                let last_pos = entity.pos.load();
                player.get_entity().set_pos(pos);

                let distance = last_pos.squared_distance_to_vec(&pos).sqrt();
                let cm = (distance * 100.0) as i32;
                if cm > 0 {
                    let stat = player.get_movement_statistic().await;
                    player
                        .increment_stat(StatisticCategory::Custom, stat as i32, cm)
                        .await;
                }

                let height_difference = pos.y - last_pos.y;
                if entity.on_ground.load(Ordering::Relaxed)
                    && (packet.collision & FLAG_ON_GROUND) != 0
                    && height_difference > 0.0
                {
                    player.jump().await;
                }
                entity
                    .on_ground
                    .store((packet.collision & FLAG_ON_GROUND) != 0, Ordering::Relaxed);
                // `handleMovePlayer` applies the packet's horizontal-collision bit with the
                // client movement (`ServerGamePacketListenerImpl.java:1165-1167`).
                entity.horizontal_collision.store(
                    packet.collision & FLAG_HORIZONTAL_COLLISION != 0,
                    Ordering::Relaxed,
                );

                // `handleMovePlayer` resets impulse context after a ground or liquid landing
                // (`ServerGamePacketListenerImpl.java:1178-1184`).
                if packet.collision & FLAG_ON_GROUND != 0
                    || player.living_entity.has_landed_in_liquid()
                {
                    player.living_entity.try_reset_current_impulse_context();
                }

                entity.set_rotation(wrap_degrees(packet.yaw) % 360.0, wrap_degrees(packet.pitch));
                // `Entity.turn` notifies the vehicle after passenger rotation changes
                // (`Entity.java:490-501`).
                entity.notify_vehicle_of_turn().await;

                let entity_id = entity.entity_id;

                let yaw = (entity.yaw.load() * 256.0 / 360.0).rem_euclid(256.0);
                let pitch = (entity.pitch.load() * 256.0 / 360.0).rem_euclid(256.0);
                // let head_yaw = (entity.head_yaw * 256.0 / 360.0).floor();
                let world = entity.world.load_full();

                // TODO: Warn when player moves to quickly
                if !Self::
                    sync_position(player, &world, pos, last_pos, yaw, pitch, (packet.collision & FLAG_ON_GROUND) != 0)
                {
                    // Send the new position to all other players.
                    world.broadcast_packet_except_editioned_sync(
                        &[player.gameprofile.id],
                        &CUpdateEntityPosRot::new(
                            entity_id.into(),
                            Vector3::new(
                                pos.x.mul_add(4096.0, -(last_pos.x * 4096.0)) as i16,
                                pos.y.mul_add(4096.0, -(last_pos.y * 4096.0)) as i16,
                                pos.z.mul_add(4096.0, -(last_pos.z * 4096.0)) as i16,
                            ),
                            yaw as u8,
                            pitch as u8,
                            (packet.collision & FLAG_ON_GROUND) != 0,
                        ),
                        &CMovePlayer::new(
                            VarULong(entity_id as u64),
                            Vector3::new(pos.x as f32, pos.y as f32 + player.get_entity().entity_type.eye_height, pos.z as f32),
                            entity.pitch.load(),
                            entity.yaw.load(),
                            entity.yaw.load(),
                            CMovePlayer::MODE_NORMAL,
                            (packet.collision & FLAG_ON_GROUND) != 0,
                            VarULong(0),
                            0,
                            0,
                            VarULong(0),
                        ),
                    );
                }

                world
                    .broadcast_packet_except(
                        &[player.gameprofile.id],
                        &CHeadRot::new(entity_id.into(), yaw as u8),
                    )
                   ;
                // `ServerGamePacketListenerImpl` invokes `doCheckFallDamage` after accepting
                // movement (`ServerGamePacketListenerImpl.java:1165-1167`).
                entity
                    .do_check_fall_damage(
                        player.clone(),
                        pos.x - last_pos.x,
                        height_difference,
                        pos.z - last_pos.z,
                        (packet.collision & FLAG_ON_GROUND) != 0,
                    )
                    .await;
                chunker::update_position(player).await;
                let delta = Vector3::new(
                    pos.x - last_pos.x,
                    pos.y - last_pos.y,
                    pos.z - last_pos.z,
                );
                // `handleMovePlayer` records accepted client movement until the next tick-end
                // packet (`ServerGamePacketListenerImpl.java:1165-1168`).
                self.handle_player_known_movement(player, delta);
                // Only update idle timeout if there's actual movement (vanilla threshold)
                if delta.length_squared() > 1.0E-5 {
                    player.update_last_action_time();
                }
                player.progress_motion(delta).await;
            }

            'cancelled: {
                self.force_tp(player, position).await;
            }
        }}
    }

    pub async fn force_tp(&self, player: &Arc<Player>, position: Vector3<f64>) {
        let teleport_id = player.teleport_id_count.fetch_add(1, Ordering::Relaxed) + 1;
        *player.awaiting_teleport.lock().await = Some((teleport_id.into(), position));
        self.enqueue_client_packet(&CPlayerPosition::new(
            teleport_id.into(),
            player.get_entity().pos.load(),
            Vector3::new(0.0, 0.0, 0.0),
            player.get_entity().yaw.load(),
            player.get_entity().pitch.load(),
            Vec::new(),
        ))
        .await;
    }
}
