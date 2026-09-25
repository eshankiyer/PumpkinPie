#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_bundle_item_selected(&self, player: &Arc<Player>, packet: &SBundleItemSelected) {
        if !player.has_client_loaded() {
            return;
        }
        player.update_last_action_time();

        let selected_item_index = packet.selected_item_index.0;
        if selected_item_index < 0 && selected_item_index != -1 {
            self.try_kick(&TextComponent::text("Invalid selected item index"));
            return;
        }

        let current_handler = player
            .current_screen_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let handler = current_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(slot) = handler
            .get_behaviour()
            .slots
            .get(usize::try_from(packet.slot_id.0).unwrap_or(usize::MAX))
            .cloned()
        else {
            return;
        };
        let mut stack = slot.get_stack();
        if let Some(contents) =
            stack.get_data_component_mut::<pumpkin_data::data_component_impl::BundleContentsImpl>()
        {
            contents.toggle_selected_item(selected_item_index);
            slot.set_stack(stack);
            slot.mark_dirty();
        }
    }
}
