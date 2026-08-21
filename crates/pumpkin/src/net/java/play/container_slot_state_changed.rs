#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_inventory::crafter_screen_handler::CrafterScreenHandler;
use pumpkin_protocol::java::server::play::SContainerSlotStateChanged;
use pumpkin_util::GameMode;

use crate::block::entities::crafter::CrafterBlockEntity;

impl JavaClient {
    /// `ServerGamePacketListenerImpl.handleContainerSlotStateChanged`
    /// (`ServerGamePacketListenerImpl.java:1040-1047`): non-spectators may toggle a crafter
    /// input slot on the menu they actually have open. Vanilla routes the toggle to
    /// `CrafterBlockEntity.setSlotState` (`CrafterBlockEntity.java:76-81`), not to
    /// `CrafterMenu.setSlotState`, so the `slotCanBeDisabled` gate (the slot must be empty)
    /// applies.
    pub async fn handle_container_slot_state_changed(
        &self,
        player: &Player,
        packet: &SContainerSlotStateChanged,
    ) {
        debug!(
            "Player {} container {} slot {} state changed to {}",
            player.gameprofile.name, packet.container_id.0, packet.slot_id.0, packet.new_state
        );

        if player.gamemode.load() == GameMode::Spectator {
            return;
        }

        let screen_handler = player.current_screen_handler.lock().await.clone();
        let screen_handler = screen_handler.lock().await;

        if i32::from(screen_handler.sync_id()) != packet.container_id.0 {
            return;
        }

        let Some(crafter_handler) = screen_handler
            .as_any()
            .downcast_ref::<CrafterScreenHandler>()
        else {
            return;
        };

        let Ok(slot) = usize::try_from(packet.slot_id.0) else {
            return;
        };

        if let Some(crafter) = crafter_handler
            .inventory
            .as_any()
            .downcast_ref::<CrafterBlockEntity>()
        {
            crafter.set_slot_state(slot, packet.new_state).await;
        }
    }
}
