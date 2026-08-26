#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub async fn handle_pick_item_from_block(
        &self,
        player: &Arc<Player>,
        pick_item: SPickItemFromBlock,
    ) {
        if !player.can_interact_with_block_at(&pick_item.pos, 1.0) {
            return;
        }

        let world = player.world();
        let block = world.get_block(&pick_item.pos);

        // Vanilla `Block#getCloneItemStack`: a block may override the item a player receives when
        // middle-clicking it (e.g. attached stems give their seed). Otherwise fall back to the
        // block's registered item; an `item_id` of 0 means the block cannot be picked.
        let Some(stack) = world
            .block_registry
            .get_clone_item_stack(block, &world, &pick_item.pos)
            .or_else(|| Item::from_id(block.item_id).map(|item| ItemStack::new(1, item)))
        else {
            return;
        };
        if stack.is_empty() {
            return;
        }

        let slot_with_stack = player.inventory().get_slot_with_stack(&stack).await;

        if slot_with_stack != -1 {
            if PlayerInventory::is_valid_hotbar_index(slot_with_stack as usize) {
                player.inventory.set_selected_slot(slot_with_stack as u8);
            } else {
                player
                    .inventory
                    .swap_slot_with_hotbar(slot_with_stack as usize)
                    .await;
            }
        } else if player.gamemode.load() == GameMode::Creative {
            player.inventory.swap_stack_with_hotbar(stack).await;
        }

        player
            .send_client_packet(&CSetSelectedSlot::new(
                player.inventory.get_selected_slot() as i8
            ))
            .await;
        player
            .player_screen_handler
            .lock()
            .await
            .send_content_updates()
            .await;
    }

    pub async fn handle_pick_item_from_entity(
        &self,
        player: &Arc<Player>,
        pick_item: SPickItemFromEntity,
    ) {
        use pumpkin_data::entity::{entity_from_egg, spawn_egg_ids};

        let world = player.world();
        let Some(target) = world.get_entity_by_id(pick_item.id.0) else {
            return;
        };

        let p_eye = player.get_entity().get_eye_pos();
        let t_eye = target.get_eye_pos();
        let dx = p_eye.x - t_eye.x;
        let dy = p_eye.y - t_eye.y;
        let dz = p_eye.z - t_eye.z;
        if dx * dx + dy * dy + dz * dz > 64.0 {
            return;
        }

        let target_type_id = target.get_entity().entity_type.id;
        let mut found_egg: Option<u16> = None;
        for &egg_id in &spawn_egg_ids() {
            if let Some(et) = entity_from_egg(egg_id)
                && et.id == target_type_id
            {
                found_egg = Some(egg_id);
                break;
            }
        }

        if let Some(item) = found_egg.and_then(Item::from_id) {
            let stack = ItemStack::new(1, item);

            let slot_with_stack = player.inventory().get_slot_with_stack(&stack).await;

            if slot_with_stack != -1 {
                if PlayerInventory::is_valid_hotbar_index(slot_with_stack as usize) {
                    player.inventory.set_selected_slot(slot_with_stack as u8);
                } else {
                    player
                        .inventory
                        .swap_slot_with_hotbar(slot_with_stack as usize)
                        .await;
                }
            } else if player.gamemode.load() == GameMode::Creative {
                player.inventory.swap_stack_with_hotbar(stack).await;
            }

            player
                .send_client_packet(&CSetSelectedSlot::new(
                    player.inventory.get_selected_slot() as i8
                ))
                .await;
            player
                .player_screen_handler
                .lock()
                .await
                .send_content_updates()
                .await;
        }
    }
}
