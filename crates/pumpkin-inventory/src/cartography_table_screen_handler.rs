use std::any::Any;
use std::sync::Arc;

use crate::player::player_inventory::PlayerInventory;
use crate::screen_handler::{
    InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour, ScreenHandlerFuture,
};
use crate::slot::PredicateSlot;

use pumpkin_data::data_component_impl::MapIdImpl;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::screen::WindowType;
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

/// `CartographyTableMenu.java:61-65`: the result slot never accepts a placed item.
const fn may_place_result(_stack: &ItemStack) -> bool {
    false
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
            input_inventory as Arc<dyn Inventory>,
            1,
            may_place_additional,
        )));
        handler.add_slot(Arc::new(PredicateSlot::new(
            output_inventory as Arc<dyn Inventory>,
            0,
            may_place_result,
        )));

        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inventory);

        handler
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
            self.internal_on_slot_click(slot_index, button, action_type, player)
                .await;
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
    /// `slotsChanged`/`setupResultSlot` (`CartographyTableMenu.java:97-141`) -- the crafting
    /// logic that actually populates the result slot from the map + paper/glass-pane/map
    /// combination -- is not implemented: it needs `MapPostProcessing` to carry a real
    /// Scale/Lock payload (`MapPostProcessingImpl` here is a zero-field marker,
    /// `pumpkin-data/src/data_component_impl/utility.rs:64-68`) and a consumer that applies it
    /// when the map item is next held (no such consumer exists). So the result slot is always
    /// empty in practice; the routing below still matches vanilla for the two input slots.
    fn quick_move<'a>(
        &'a mut self,
        _player: &'a dyn InventoryPlayer,
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
            stack
        })
    }
}
