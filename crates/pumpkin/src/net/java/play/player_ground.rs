#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_player_ground(&self, player: &Player, ground: &SSetPlayerGround) {
        player
            .living_entity
            .entity
            .on_ground
            .store(ground.on_ground, Ordering::Relaxed);
        // `handleMovePlayer` applies the packet's horizontal-collision bit with the client
        // movement (`ServerGamePacketListenerImpl.java:1165-1167`).
        player
            .living_entity
            .entity
            .horizontal_collision
            .store(ground.horizontal_collision, Ordering::Relaxed);
    }
}
