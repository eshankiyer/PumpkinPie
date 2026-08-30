#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub async fn handle_player_command(
        &self,
        player: &Arc<Player>,
        command: SPlayerCommand,
        server: &Arc<Server>,
    ) {
        if command.entity_id != player.entity_id().into() {
            return;
        }
        if !player.has_client_loaded() {
            return;
        }
        player.update_last_action_time();

        let entity = &player.get_entity();
        let jump_boost = command.jump_boost.0;
        match command.action {
            Action::StartSprinting => {
                if !entity.is_sprinting() {
                    send_cancellable! {{
                        server;
                        PlayerToggleSprintEvent::new(player.clone(), true);
                        'after: {
                            player.get_entity().set_sprinting(event.is_sprinting).await;
                        }
                    }}
                }
            }
            Action::StopSprinting => {
                if entity.is_sprinting() {
                    send_cancellable! {{
                        server;
                        PlayerToggleSprintEvent::new(player.clone(), false);
                        'after: {
                            player.get_entity().set_sprinting(event.is_sprinting).await;
                        }
                    }}
                }
            }
            Action::LeaveBed => player.wake_up().await,

            Action::StartHorseJump => {
                // `ServerGamePacketListenerImpl.handlePlayerCommand` checks `canJump`, then
                // calls `handleStartJump` (`ServerGamePacketListenerImpl.java:1721-1727`).
                let vehicle = entity.vehicle.lock().await.clone();
                if let Some(vehicle) = vehicle
                    && let Some(mob) = vehicle.get_mob()
                    && jump_boost > 0
                    && mob.can_jump().await
                {
                    // `AbstractHorse.onPlayerJump` stores the charge for `tickRidden`
                    // (`AbstractHorse.java:720-738,878-895`); the shared Mob hooks expose that
                    // server-side path to the packet handler.
                    mob.on_player_jump(jump_boost);
                    mob.handle_start_jump(jump_boost);
                }
            }
            Action::StopHorseJump => {
                // `ServerGamePacketListenerImpl.handlePlayerCommand` forwards stop-jump to the
                // controlled vehicle (`ServerGamePacketListenerImpl.java:1729-1732`).
                let vehicle = entity.vehicle.lock().await.clone();
                if let Some(vehicle) = vehicle
                    && let Some(mob) = vehicle.get_mob()
                {
                    mob.handle_stop_jump();
                }
            }
            Action::OpenVehicleInventory => {
                debug!("todo");
            }
            Action::StartFlyingElytra => {
                // Vanilla `ServerGamePacketListenerImpl.handlePlayerCommand` START_FALL_FLYING
                // (`ServerGamePacketListenerImpl.java:1736-1739`): `Player.tryToStartFallFlying`
                // starts the glide only when not already gliding, a valid glider passes
                // `canGlide`, and the player is not in water; otherwise `stopFallFlying`
                // resyncs the shared flag so the client ends its glide animation.
                let living = &player.living_entity;
                let caller: Arc<dyn EntityBase> = player.clone();
                let try_start = !living.entity.is_fall_flying()
                    && living.can_glide(&caller).await
                    && !living
                        .entity
                        .was_touching_water
                        .load(std::sync::atomic::Ordering::SeqCst);
                if try_start {
                    let mut event = crate::plugin::api::events::entity::entity_toggle_glide::EntityToggleGlideEvent::new(
                        living.entity.entity_id,
                        true,
                    );
                    server.plugin_manager.fire(server, &mut event).await;
                    if event.cancelled || !event.is_gliding {
                        living.entity.stop_fall_flying();
                    } else {
                        living.entity.set_fall_flying(true).await;
                    }
                } else {
                    living.entity.stop_fall_flying();
                }
            }
            // <= 1.21.5
            Action::StartSneaking | Action::StopSneaking => {
                self.handle_player_input(
                    player,
                    SPlayerInput {
                        input: SPlayerInput::SNEAK,
                    },
                    server,
                )
                .await;
            }
        }
    }
}
