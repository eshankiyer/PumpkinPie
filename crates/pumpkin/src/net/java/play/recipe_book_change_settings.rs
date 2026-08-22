#[allow(clippy::wildcard_imports)]
use super::*;

use crate::data::recipe_book::RecipeBookType;

impl JavaClient {
    /// `ServerGamePacketListenerImpl.handleRecipeBookChangeSettingsPacket`, which
    /// applies the tab's open/filter pair with `RecipeBook.setBookSetting`
    /// (`RecipeBook.java:32-35`).
    pub async fn handle_recipe_book_change_settings(
        &self,
        server: &Arc<Server>,
        player: &Arc<Player>,
        packet: SRecipeBookChangeSettings,
    ) {
        let mut event = crate::plugin::api::events::player::player_recipe_book_settings_change::PlayerRecipeBookSettingsChangeEvent::new(
            player.clone(),
            format!("{:?}", packet.book_type),
            packet.is_open,
            packet.is_filtering,
        );
        server.plugin_manager.fire(server, &mut event).await;

        if let Some(book_type) = RecipeBookType::from_id(packet.book_type.0) {
            player
                .set_recipe_book_setting(book_type, packet.is_open, packet.is_filtering)
                .await;
        }
    }
}
