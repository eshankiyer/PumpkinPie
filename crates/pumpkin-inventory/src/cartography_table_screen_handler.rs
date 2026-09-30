use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::player::player_inventory::PlayerInventory;
use crate::screen_handler::{
    InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour, ScreenHandlerFuture,
};
use crate::slot::{BoxFuture, PredicateSlot, Slot};

use pumpkin_data::data_component_impl::{
    DataComponentImpl, MapIdImpl, MapPostProcessing, MapPostProcessingImpl,
};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::screen::WindowType;
use pumpkin_data::sound::Sound;
use pumpkin_protocol::java::server::play::SlotActionType;
use pumpkin_world::inventory::Inventory;
use pumpkin_world::inventory::SimpleInventory;

/// `CartographyTableMenu.INV_SLOT_START` (`CartographyTableMenu.java:21`).
const INV_SLOT_START: i32 = 3;
/// `CartographyTableMenu.INV_SLOT_END` (`CartographyTableMenu.java:22`).
const INV_SLOT_END: i32 = 30;
/// `CartographyTableMenu.USE_ROW_SLOT_START` (`CartographyTableMenu.java:23`).
const USE_ROW_SLOT_START: i32 = 30;
/// `CartographyTableMenu.USE_ROW_SLOT_END` (`CartographyTableMenu.java:24`).
const USE_ROW_SLOT_END: i32 = 39;

/// `CartographyTableMenu.java:49-54`: the map slot only accepts an item stack carrying a
/// `DataComponents.MAP_ID` component.
fn may_place_map(stack: &ItemStack) -> bool {
    stack.get_data_component::<MapIdImpl>().is_some()
}

/// `CartographyTableMenu.java:55-60`: the "additional" slot accepts paper (scale up), another
/// map (copy), or a glass pane (lock).
fn may_place_additional(stack: &ItemStack) -> bool {
    stack.item == &Item::PAPER || stack.item == &Item::MAP || stack.item == &Item::GLASS_PANE
}

pub struct CartographyTableScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    pub input_inventory: Arc<SimpleInventory>,
    pub output_inventory: Arc<SimpleInventory>,
}

impl CartographyTableScreenHandler {
    pub fn new(sync_id: u8, player_inventory: &Arc<PlayerInventory>) -> Self {
        let behaviour = ScreenHandlerBehaviour::new(sync_id, Some(WindowType::CartographyTable));
        let input_inventory = Arc::new(SimpleInventory::new(2));
        let output_inventory = Arc::new(SimpleInventory::new(1));

        let mut handler = Self {
            behaviour,
            input_inventory: input_inventory.clone(),
            output_inventory: output_inventory.clone(),
        };

        handler.add_slot(Arc::new(PredicateSlot::new(
            input_inventory.clone() as Arc<dyn Inventory>,
            0,
            may_place_map,
        )));
        handler.add_slot(Arc::new(PredicateSlot::new(
            input_inventory.clone() as Arc<dyn Inventory>,
            1,
            may_place_additional,
        )));
        handler.add_slot(Arc::new(CartographyResultSlot::new(
            output_inventory as Arc<dyn Inventory>,
            input_inventory,
        )));

        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inventory);

        handler
    }

    /// `CartographyTableMenu.slotsChanged` (`CartographyTableMenu.java:97-109`) and
    /// `setupResultSlot` (`CartographyTableMenu.java:111-139`). A stale result is cleared when an
    /// input is gone; with both inputs present the result is only recomputed when the saved map
    /// data can be found (`mapData != null`), otherwise the existing result is left untouched.
    async fn update_result(&self, player: &dyn InventoryPlayer) {
        let map = self.input_inventory.get_stack(0).await;
        let additional = self.input_inventory.get_stack(1).await;
        let current = self.output_inventory.get_stack(0).await;
        if map.is_empty() || additional.is_empty() {
            if !current.is_empty() {
                self.output_inventory
                    .set_stack(0, ItemStack::EMPTY.clone())
                    .await;
            }
            return;
        }
        let Some(map_id) = map.get_data_component::<MapIdImpl>().map(|id| id.id) else {
            return;
        };
        let Some((scale, locked)) = player.map_scale_and_lock(map_id).await else {
            return;
        };
        let result = craft_result(&map, &additional, scale, locked)
            .unwrap_or_else(|| ItemStack::EMPTY.clone());
        self.output_inventory.set_stack(0, result).await;
    }
}

/// The recipe selection of `CartographyTableMenu.setupResultSlot`
/// (`CartographyTableMenu.java:116-133`) once the map's saved data is known. `None` is the
/// `removeItemNoUpdate(2)` branch (`:125-129`).
fn craft_result(map: &ItemStack, additional: &ItemStack, scale: i8, locked: bool) -> Option<ItemStack> {
    let marked = |processing| {
        let mut result = map.copy_with_count(1);
        result.patch.push((
            pumpkin_data::data_component::DataComponent::MapPostProcessing,
            Some(MapPostProcessingImpl { processing }.to_dyn()),
        ));
        result
    };
    if additional.item == &Item::PAPER && !locked && scale < 4 {
        Some(marked(MapPostProcessing::Scale))
    } else if additional.item == &Item::GLASS_PANE && !locked {
        Some(marked(MapPostProcessing::Lock))
    } else if additional.item == &Item::MAP {
        Some(map.copy_with_count(2))
    } else {
        None
    }
}

/// Result slot for `CartographyTableMenu`. It mirrors the vanilla result-slot `onTake` callback
/// (`CartographyTableMenu.java:61-80`) so normal clicks and shift-clicks consume both inputs.
struct CartographyResultSlot {
    output_inventory: Arc<dyn Inventory>,
    input_inventory: Arc<SimpleInventory>,
    id: AtomicU8,
}

impl CartographyResultSlot {
    fn new(output_inventory: Arc<dyn Inventory>, input_inventory: Arc<SimpleInventory>) -> Self {
        Self {
            output_inventory,
            input_inventory,
            id: AtomicU8::new(0),
        }
    }
}

impl Slot for CartographyResultSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> {
        self.output_inventory.clone()
    }

    fn get_index(&self) -> usize {
        0
    }

    fn set_id(&self, id: usize) {
        self.id.store(id as u8, Ordering::Relaxed);
    }

    fn can_insert<'a>(&'a self, _stack: &'a ItemStack) -> BoxFuture<'a, bool> {
        // `CartographyTableMenu`'s result slot rejects every placed stack
        // (`CartographyTableMenu.java:61-65`).
        Box::pin(async { false })
    }

    fn mark_dirty(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.output_inventory.mark_dirty();
        })
    }

    fn on_take_item<'a>(
        &'a self,
        player: &'a dyn InventoryPlayer,
        _stack: &'a ItemStack,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            // `CartographyTableMenu` removes one stack from both inputs before playing the
            // take-result sound (`CartographyTableMenu.java:67-79`).
            self.input_inventory.remove_stack_specific(0, 1).await;
            self.input_inventory.remove_stack_specific(1, 1).await;
            player.play_sound(Sound::UiCartographyTableTakeResult).await;
            self.mark_dirty().await;
        })
    }
}

impl ScreenHandler for CartographyTableScreenHandler {
    /// Port of `CartographyTableMenu.java:93-95`: the block at the opening position must still be
    /// `Blocks.CARTOGRAPHY_TABLE` and the player must still be within
    /// `blockInteractionRange() + 4.0` (`AbstractContainerMenu.java:93-95`).
    fn container_access(&self) -> crate::screen_handler::ContainerAccess {
        crate::screen_handler::ContainerAccess::Block(|block| {
            block.id == pumpkin_data::Block::CARTOGRAPHY_TABLE.id
        })
    }

    fn get_behaviour(&self) -> &ScreenHandlerBehaviour {
        &self.behaviour
    }

    fn get_behaviour_mut(&mut self) -> &mut ScreenHandlerBehaviour {
        &mut self.behaviour
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn on_slot_click<'a>(
        &'a mut self,
        slot_index: i32,
        button: i32,
        action_type: SlotActionType,
        player: &'a dyn InventoryPlayer,
    ) -> ScreenHandlerFuture<'a, ()> {
        Box::pin(async move {
            if slot_index == 2
                && matches!(
                    action_type,
                    SlotActionType::Pickup | SlotActionType::QuickMove
                )
            {
                let mut result = self.output_inventory.get_stack(0).await;
                player.process_item_stack_after_crafting(&mut result).await;
                self.output_inventory.set_stack(0, result).await;
            }
            if action_type == SlotActionType::PickupAll && button == 0 {
                // `canTakeItemForPickAll` excludes slots backed by the result container
                // (`CartographyTableMenu.java:143-146`). The shared handler has no per-slot
                // pickup-all predicate, so exclude this result while it performs the scan.
                self.output_inventory.remove_stack(0).await;
                self.internal_on_slot_click(slot_index, button, action_type, player)
                    .await;
                self.update_result(player).await;
                return;
            }
            self.internal_on_slot_click(slot_index, button, action_type, player)
                .await;
            // Vanilla invokes `slotsChanged` from both input containers' `setChanged`
            // callbacks (`CartographyTableMenu.java:27-39, 97-109`). The existing screen
            // handler has no inventory listener, so refresh after every live click instead.
            self.update_result(player).await;
        })
    }

    /// `CartographyTableMenu.removed` (`CartographyTableMenu.java:198-203`): the input slots
    /// (map + paper/glass pane/map) are dropped back to the player on close, not left in the
    /// table. The result slot is a `ResultContainer` that is never itself serialized, matching
    /// `resultContainer.removeItemNoUpdate(2)`.
    fn on_closed<'a>(&'a mut self, player: &'a dyn InventoryPlayer) -> ScreenHandlerFuture<'a, ()> {
        Box::pin(async move {
            self.default_on_closed(player).await;
            let input_inventory: Arc<dyn Inventory> = self.input_inventory.clone();
            self.drop_inventory(player, input_inventory).await;
        })
    }

    /// `CartographyTableMenu.quickMoveStack` (`CartographyTableMenu.java:149-196`).
    ///
    /// The result branch invokes the result-slot callback after moving the stack, matching
    /// `CartographyTableMenu.java:155-162` and its input consumption at `:67-80`. The result is
    /// processed before the move through `ItemStack.onCraftedBy` (`ItemStack.java:722-725`)
    /// and `Item.onCraftedBy` (`Item.java:292-297`).
    fn quick_move<'a>(
        &'a mut self,
        player: &'a dyn InventoryPlayer,
        slot_index: i32,
    ) -> ScreenHandlerFuture<'a, ItemStack> {
        Box::pin(async move {
            let mut stack = ItemStack::EMPTY.clone();
            let slot = self.get_behaviour().slots.get(slot_index as usize).cloned();
            let total_slots = self.get_behaviour().slots.len() as i32;

            if let Some(slot) = slot {
                let mut slot_stack = slot.get_cloned_stack().await;
                if !slot_stack.is_empty() {
                    stack = slot_stack.clone();

                    if slot_index == 2 {
                        // Result slot: move to the full player inventory.
                        if !self
                            .insert_item(&mut slot_stack, INV_SLOT_START, total_slots, true)
                            .await
                        {
                            return ItemStack::EMPTY.clone();
                        }
                        // `CartographyTableMenu`'s result-slot `onTake` consumes one item from
                        // each input (`CartographyTableMenu.java:67-80`).
                        slot.on_take_item(player, &stack).await;
                    } else if slot_index != 0 && slot_index != 1 {
                        if slot_stack.get_data_component::<MapIdImpl>().is_some() {
                            if !self.insert_item(&mut slot_stack, 0, 1, false).await {
                                return ItemStack::EMPTY.clone();
                            }
                        } else if !may_place_additional(&slot_stack) {
                            if (INV_SLOT_START..INV_SLOT_END).contains(&slot_index) {
                                if !self
                                    .insert_item(
                                        &mut slot_stack,
                                        USE_ROW_SLOT_START,
                                        USE_ROW_SLOT_END,
                                        false,
                                    )
                                    .await
                                {
                                    return ItemStack::EMPTY.clone();
                                }
                            } else if (USE_ROW_SLOT_START..USE_ROW_SLOT_END).contains(&slot_index)
                                && !self
                                    .insert_item(
                                        &mut slot_stack,
                                        INV_SLOT_START,
                                        INV_SLOT_END,
                                        false,
                                    )
                                    .await
                            {
                                return ItemStack::EMPTY.clone();
                            }
                        } else if !self.insert_item(&mut slot_stack, 1, 2, false).await {
                            return ItemStack::EMPTY.clone();
                        }
                    } else if !self
                        .insert_item(&mut slot_stack, INV_SLOT_START, total_slots, false)
                        .await
                    {
                        return ItemStack::EMPTY.clone();
                    }

                    if slot_stack.is_empty() {
                        slot.set_stack(ItemStack::EMPTY.clone()).await;
                    } else {
                        slot.set_stack(slot_stack).await;
                    }
                }
            }
            self.update_result(player).await;
            stack
        })
    }
}

#[cfg(test)]
mod tests {
    use super::craft_result;
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{
        DataComponentImpl, MapIdImpl, MapPostProcessing, MapPostProcessingImpl,
    };
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    fn map_stack() -> ItemStack {
        let mut map = ItemStack::new(1, &Item::FILLED_MAP);
        map.patch
            .push((DataComponent::MapId, Some(MapIdImpl { id: 7 }.to_dyn())));
        map
    }

    /// `CartographyTableMenu.setupResultSlot` copies a map with count two when the second input
    /// is another map (`CartographyTableMenu.java:125-133`).
    #[test]
    fn map_copy_yields_two() {
        let result = craft_result(&map_stack(), &ItemStack::new(1, &Item::MAP), 0, false)
            .expect("copy result");
        assert_eq!(result.item.id, Item::FILLED_MAP.id);
        assert_eq!(result.item_count, 2);
        assert_eq!(
            result.get_data_component::<MapIdImpl>().map(|id| id.id),
            Some(7)
        );
    }

    /// `CartographyTableMenu.setupResultSlot` (`CartographyTableMenu.java:116-123`) marks
    /// paper and glass-pane results for `MapItem.onCraftedPostProcess`.
    #[test]
    fn map_transform_carries_post_processing() {
        let result = craft_result(&map_stack(), &ItemStack::new(1, &Item::PAPER), 0, false)
            .expect("scale result");
        assert_eq!(
            result
                .get_data_component::<MapPostProcessingImpl>()
                .map(|value| value.processing),
            Some(MapPostProcessing::Scale)
        );
        let result = craft_result(&map_stack(), &ItemStack::new(1, &Item::GLASS_PANE), 0, false)
            .expect("lock result");
        assert_eq!(
            result
                .get_data_component::<MapPostProcessingImpl>()
                .map(|value| value.processing),
            Some(MapPostProcessing::Lock)
        );
    }

    /// Paper needs `!locked && scale < 4`, glass pane needs `!locked`
    /// (`CartographyTableMenu.java:116,120`); failing either clears the result (`:125-129`).
    #[test]
    fn saved_data_gates_paper_and_pane() {
        let paper = ItemStack::new(1, &Item::PAPER);
        let pane = ItemStack::new(1, &Item::GLASS_PANE);
        assert!(craft_result(&map_stack(), &paper, 4, false).is_none());
        assert!(craft_result(&map_stack(), &paper, 3, false).is_some());
        assert!(craft_result(&map_stack(), &paper, 0, true).is_none());
        assert!(craft_result(&map_stack(), &pane, 0, true).is_none());
        assert!(craft_result(&map_stack(), &pane, 4, false).is_some());
    }
}
