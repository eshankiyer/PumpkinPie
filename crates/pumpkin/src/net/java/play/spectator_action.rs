#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_protocol::java::{client::play::CSetCamera, server::play::SSpectatorAction};
use pumpkin_util::GameMode;

impl JavaClient {
    /// Vanilla `ServerGamePacketListenerImpl.handleSpectatorAction`: attach a spectator's camera
    /// to an entity it can actually see. Unlike `handle_teleport_to_entity`, this does not move
    /// the player to the target.
    pub async fn handle_spectator_action(&self, player: &Arc<Player>, packet: SSpectatorAction) {
        if !player.has_client_loaded() || player.gamemode.load() != GameMode::Spectator {
            return;
        }
        player.update_last_action_time();

        let Some(entity_id) = packet.entity_id else {
            return;
        };

        let world = player.world();
        let Some(target) = world.get_entity_or_part(entity_id.0) else {
            return;
        };

        let target_pos = target.get_entity().pos.load();
        let max_range = player.entity_interaction_range() + 3.0;
        if !world
            .worldborder
            .lock()
            .await
            .contains_block(target_pos.x.floor() as i32, target_pos.z.floor() as i32)
            || target
                .get_entity()
                .bounding_box
                .load()
                .squared_magnitude(player.eye_position())
                >= max_range * max_range
            || !target.is_pickable()
        {
            return;
        }

        player.camera_target_id.store(Some(entity_id.0));
        player.send_client_packet(&CSetCamera::new(entity_id)).await;
    }
}
