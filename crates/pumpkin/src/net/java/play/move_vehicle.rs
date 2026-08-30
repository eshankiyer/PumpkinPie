#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub async fn handle_move_vehicle(&self, player: &Arc<Player>, packet: SMoveVehicle) {
        let entity = player.get_entity();
        let pos = Vector3::new(packet.x, packet.y, packet.z);
        let vehicle = entity.vehicle.lock().await;
        if let Some(vehicle) = vehicle.as_ref() {
            let vehicle_entity = vehicle.get_entity();
            let old_pos = vehicle_entity.pos.load();
            vehicle_entity.set_pos(pos);
            vehicle_entity.set_rotation(packet.yaw, packet.pitch);
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
        }
        drop(vehicle);
        entity.set_pos(pos);
        chunker::update_position(player).await;
    }
}
