#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_test_instance_block_action(
        &self,
        player: &Arc<Player>,
        packet: &STestInstanceBlockAction<'_>,
    ) {
        if !player.has_client_loaded() {
            return;
        }
        // `ServerGamePacketListenerImpl.handleTestInstanceBlockAction` gates the packet with
        // `Player.canUseGameMasterBlocks` (`ServerGamePacketListenerImpl.java:894-897`).
        if !player.can_use_game_master_blocks() {
            return;
        }
        player.update_last_action_time();
        debug!(
            "Test instance block action at {:?}: action={:?}",
            packet.pos, packet.action
        );
    }
}
