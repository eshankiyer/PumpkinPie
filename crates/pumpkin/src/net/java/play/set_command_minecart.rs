#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_protocol::java::server::play::SSetCommandMinecart;

impl JavaClient {
    pub fn handle_set_command_minecart(&self, player: &Player, packet: &SSetCommandMinecart<'_>) {
        // `ServerGamePacketListenerImpl.handleSetCommandMinecart` gates the packet with
        // `Player.canUseGameMasterBlocks` (`ServerGamePacketListenerImpl.java:667-670`).
        if !player.can_use_game_master_blocks() {
            return;
        }

        let world = player.world();
        if let Some(entity) = world.get_entity_by_id(packet.entity_id.0) {
            debug!(
                "Player {} updated command minecart {} command to: {}",
                player.gameprofile.name,
                entity.get_entity().entity_id,
                packet.command
            );
        }
    }
}
