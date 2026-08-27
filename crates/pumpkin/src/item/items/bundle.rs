use std::pin::Pin;

use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};
use pumpkin_data::data_component_impl::BundleContentsImpl;
use pumpkin_data::item::Item;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag;
use pumpkin_util::Hand;

pub struct BundleItem;

impl ItemMetadata for BundleItem {
    fn ids() -> Box<[u16]> {
        tag::Item::MINECRAFT_BUNDLES.1.into()
    }
}

impl ItemBehaviour for BundleItem {
    fn normal_use<'a>(
        &'a self,
        _item: &'a Item,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let held_item = player.inventory.held_item().await;
            let hand = if !held_item.is_empty() && Self::ids().contains(&held_item.item.id) {
                Hand::Right
            } else {
                let off_hand_item = player.inventory.off_hand_item().await;
                if off_hand_item.is_empty() || !Self::ids().contains(&off_hand_item.item.id) {
                    return;
                }
                Hand::Left
            };
            let stack = player.inventory.get_stack_in_hand(hand).await;
            // Vanilla `BundleItem.use` starts the 200-tick active-use cycle
            // (`BundleItem.java:140-143,230-233`).
            player
                .living_entity
                .set_active_hand(hand, stack, Self::USE_DURATION)
                .await;
        })
    }

    fn on_use_tick<'a>(
        &'a self,
        _stack: &'a pumpkin_data::item_stack::ItemStack,
        player: &'a Player,
        remaining_use_ticks: i32,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let use_duration = Self::USE_DURATION;
            if remaining_use_ticks != use_duration
                && (remaining_use_ticks >= use_duration - 10 || remaining_use_ticks % 2 != 0)
            {
                return;
            }

            let Some(hand) = *player.living_entity.active_hand.lock().await else {
                return;
            };
            let mut bundle = player.inventory.get_stack_in_hand(hand).await;
            let Some(contents) = bundle.get_data_component_mut::<BundleContentsImpl>() else {
                return;
            };
            let Some(extracted_stack) = contents.try_extract() else {
                return;
            };
            let slot = match hand {
                Hand::Right => player.inventory.get_selected_slot() as usize,
                Hand::Left => 40, // OFF_HAND_SLOT
            };
            let position = player.position();
            player.world().play_sound(
                Sound::ItemBundleRemoveOne,
                pumpkin_data::sound::SoundCategory::Players,
                &position,
            );
            player.drop_item(extracted_stack).await;
            player.sync_hand_slot(slot, bundle).await;
        })
    }

    fn get_use_duration(&self) -> i32 {
        Self::USE_DURATION
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl BundleItem {
    const USE_DURATION: i32 = 200;
}
