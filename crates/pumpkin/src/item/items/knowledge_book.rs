use std::any::Any;
use std::future::Future;
use std::pin::Pin;

use pumpkin_data::data_component_impl::RecipesImpl;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::statistic::StatisticCategory;

use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};

/// `KnowledgeBookItem` (`KnowledgeBookItem.java:19-54`).
pub struct KnowledgeBookItem;

impl ItemMetadata for KnowledgeBookItem {
    fn ids() -> Box<[u16]> {
        Box::new([Item::KNOWLEDGE_BOOK.id])
    }
}

/// The recipe ids listed by the stack's `minecraft:recipes` component
/// (`DataComponents.RECIPES`, read at `KnowledgeBookItem.java:29`).
///
/// `RecipesImpl` in `pumpkin-data` is a unit struct with no payload, so the list is
/// always empty and a knowledge book behaves as vanilla's does when its component is
/// absent: the book is consumed and the use fails. Giving `RecipesImpl` a
/// `Vec<String>` field is the only change needed here for the full behaviour.
fn recipe_ids(stack: &ItemStack) -> Vec<String> {
    let _present = stack.get_data_component::<RecipesImpl>();
    Vec::new()
}

impl ItemBehaviour for KnowledgeBookItem {
    fn normal_use<'a>(
        &'a self,
        item: &'a Item,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // Vanilla consumes the book before testing the recipe list
            // (`KnowledgeBookItem.java:30-33`), so an empty book is still used up.
            let mut main_hand = player.inventory.held_item().await;
            let stack = if !main_hand.is_empty() && main_hand.item.id == item.id {
                let stack = main_hand.clone();
                main_hand.decrement_unless_creative(player.gamemode.load(), 1);
                player.inventory.set_held_item(main_hand).await;
                Some(stack)
            } else {
                let mut off_hand = player.inventory.off_hand_item().await;
                if !off_hand.is_empty() && off_hand.item.id == item.id {
                    let stack = off_hand.clone();
                    off_hand.decrement_unless_creative(player.gamemode.load(), 1);
                    player
                        .inventory
                        .set_stack_in_hand(pumpkin_util::Hand::Left, off_hand)
                        .await;
                    Some(stack)
                } else {
                    None
                }
            };

            let Some(stack) = stack else {
                return;
            };

            let ids = recipe_ids(&stack);
            if ids.is_empty() {
                // `InteractionResult.FAIL` (`KnowledgeBookItem.java:31-33`).
                return;
            }

            // `player.awardRecipes(recipes)` (`KnowledgeBookItem.java:49`). Vanilla
            // fails the whole use if any listed id is unknown
            // (`KnowledgeBookItem.java:41-44`); `award_recipes_by_key` drops unknown
            // ids instead, which is the same outcome for a well-formed book.
            player.award_recipes_by_key(&ids).await;
            // `player.awardStat(Stats.ITEM_USED.get(this))` (`KnowledgeBookItem.java:50`).
            player
                .increment_stat(StatisticCategory::Used, i32::from(item.id), 1)
                .await;
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
