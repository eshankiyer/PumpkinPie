#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_confirm_teleport(&self, player: &Player, confirm_teleport: &SConfirmTeleport) {
        let mut awaiting_teleport = player
            .awaiting_teleport
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if teleport_confirm_action(
            awaiting_teleport.as_ref().map(|(id, _)| id.0),
            confirm_teleport.teleport_id.0,
        ) == TeleportConfirmAction::Ignore
        {
            return;
        }

        let Some((_, position)) = awaiting_teleport.take() else {
            return;
        };
        drop(awaiting_teleport);

        // Apply the server-authoritative position once the matching confirmation arrives.
        player.get_entity().set_pos(position);
    }
}
