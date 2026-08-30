#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_set_test_block(&self, player: &Arc<Player>, packet: &SSetTestBlock<'_>) {
        if !player.has_client_loaded() {
            return;
        }
        // `ServerGamePacketListenerImpl.handleSetTestBlock` gates the packet with
        // `Player.canUseGameMasterBlocks` (`ServerGamePacketListenerImpl.java:881-885`).
        if !player.can_use_game_master_blocks() {
            return;
        }
        player.update_last_action_time();
        debug!(
            "Set test block at {:?}: mode={:?}, message={}",
            packet.position, packet.mode, packet.message
        );
    }
}
