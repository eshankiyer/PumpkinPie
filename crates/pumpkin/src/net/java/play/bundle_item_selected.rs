#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub async fn handle_bundle_item_selected(
        &self,
        player: &Arc<Player>,
        packet: SBundleItemSelected,
    ) {
        if !player.has_client_loaded() {
            return;
        }
        player.update_last_action_time();

        let selected_item_index = packet.selected_item_index.0;
        if selected_item_index < 0 && selected_item_index != -1 {
            self.kick(TextComponent::text("Invalid selected item index"))
                .await;
            return;
        }

        let current_handler = player.current_screen_handler.lock().await.clone();
        let handler = current_handler.lock().await;
        let Some(slot) = handler
            .get_behaviour()
            .slots
            .get(usize::try_from(packet.slot_id.0).unwrap_or(usize::MAX))
            .cloned()
        else {
            return;
        };
        let mut stack = slot.get_stack().await;
        if let Some(contents) =
            stack.get_data_component_mut::<pumpkin_data::data_component_impl::BundleContentsImpl>()
        {
            contents.toggle_selected_item(selected_item_index);
            slot.set_stack(stack).await;
            slot.mark_dirty().await;
        }
    }
}
