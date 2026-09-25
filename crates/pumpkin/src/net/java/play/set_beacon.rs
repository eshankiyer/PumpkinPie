#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_protocol::java::server::play::SSetBeacon;

impl JavaClient {
    pub fn handle_set_beacon(&self, player: &Arc<Player>, packet: &SSetBeacon) {
        // Vanilla ignores stale menus but disconnects when updateEffects fails.
        if !player.on_set_beacon(packet) {
            warn!(
                "Player {} tried to set invalid beacon effects",
                player.gameprofile.name
            );
            self.try_kick(&TextComponent::translate(
                "multiplayer.disconnect.generic",
                &[],
            ));
        }
    }
}
