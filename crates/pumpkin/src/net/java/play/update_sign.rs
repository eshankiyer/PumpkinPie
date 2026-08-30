#[allow(clippy::wildcard_imports)]
use super::*;

use crate::block::blocks::signs::as_sign_text_access;

impl JavaClient {
    pub async fn handle_sign_update(&self, player: &Player, sign_data: SUpdateSign<'_>) {
        let world = player.get_entity().world.load_full();
        let Some(block_entity) = world.get_block_entity(&sign_data.location) else {
            return;
        };
        // Vanilla's edit path accepts any `SignBlockEntity`, hanging signs included
        // (SignBlockEntity.java:131-134); route both Pumpkin structs through
        // `SignTextAccess`.
        let Some(sign_entity) = as_sign_text_access(block_entity.as_any()) else {
            return;
        };
        if sign_entity.sign_is_waxed() {
            return;
        }

        // Vanilla `SignBlockEntity.updateSignText` only accepts the edit from the player
        // holding the edit lock (`playerWhoMayEdit`); anyone else's packet is dropped with a
        // warning. Without this check any player could rewrite any unwaxed sign's text.
        let mut currently_editing = sign_entity.editing_player().lock().await;
        if *currently_editing != Some(player.gameprofile.id) {
            tracing::warn!(
                "Player {} just tried to change non-editable sign",
                player.gameprofile.name
            );
            return;
        }

        let lines = vec![
            sign_data.line_1.to_string(),
            sign_data.line_2.to_string(),
            sign_data.line_3.to_string(),
            sign_data.line_4.to_string(),
        ];

        if let Some(player_arc) = world.get_player_by_uuid(player.gameprofile.id) {
            let mut event = crate::plugin::api::events::block::sign_change::SignChangeEvent::new(
                player_arc,
                sign_data.location,
                lines.clone(),
            );
            if let Some(server) = world.server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
            if event.cancelled {
                return;
            }
        }

        // Vanilla updateSignText delegates line replacement to updateText, then clears the
        // editor and sends the block update (`SignBlockEntity.java:130-143`).
        sign_entity.update_text(
            sign_data.is_front_text,
            [
                sign_data.line_1.into(),
                sign_data.line_2.into(),
                sign_data.line_3.into(),
                sign_data.line_4.into(),
            ],
        );
        *currently_editing = None;
        drop(currently_editing);
        world.update_block_entity(&block_entity);
    }
}
