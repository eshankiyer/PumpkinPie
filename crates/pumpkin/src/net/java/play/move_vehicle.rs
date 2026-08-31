#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub async fn handle_move_vehicle(&self, player: &Arc<Player>, packet: SMoveVehicle) {
        // `handleMoveVehicle` disconnects invalid coordinates or rotations before movement
        // handling (`ServerGamePacketListenerImpl.java:430-432,443-447`).
        if !packet.x.is_finite()
            || !packet.y.is_finite()
            || !packet.z.is_finite()
            || !packet.yaw.is_finite()
            || !packet.pitch.is_finite()
        {
            self.kick(TextComponent::translate_cross(
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_VEHICLE_MOVEMENT,
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_VEHICLE_MOVEMENT,
                [],
            ))
            .await;
            return;
        }
        // Vanilla only applies vehicle movement after the client-load and controlling-vehicle
        // gates (`ServerGamePacketListenerImpl.java:447-450`).
        if !player.has_client_loaded() {
            return;
        }
        let entity = player.get_entity();
        let pos = Vector3::new(packet.x, packet.y, packet.z);
        let vehicle = entity.vehicle.lock().await.clone();
        if let Some(vehicle) = vehicle {
            let is_controlling_passenger = vehicle
                .get_entity()
                .passengers
                .lock()
                .await
                .first()
                .is_some_and(|passenger| passenger.get_entity().entity_id == player.entity_id());
            if !is_controlling_passenger {
                return;
            }
            let vehicle_entity = vehicle.get_entity();
            let old_pos = vehicle_entity.pos.load();
            vehicle_entity.set_pos(pos);
            vehicle_entity.set_rotation(packet.yaw, packet.pitch);
            // `handleMoveVehicle` records the vehicle delta as known player movement
            // (`ServerGamePacketListenerImpl.java:502-507`).
            self.handle_player_known_movement(
                player,
                Vector3::new(pos.x - old_pos.x, pos.y - old_pos.y, pos.z - old_pos.z),
            );
            // Vehicle movement calls `doCheckFallDamage` after applying the client delta
            // (`ServerGamePacketListenerImpl.java:507-508`).
            vehicle_entity
                .do_check_fall_damage(
                    vehicle.clone(),
                    pos.x - old_pos.x,
                    pos.y - old_pos.y,
                    pos.z - old_pos.z,
                    packet.on_ground,
                )
                .await;

            drop(vehicle);
            entity.set_pos(pos);
            chunker::update_position(player).await;
        }
    }
}
