use std::any::Any;
use std::sync::Arc;

use pumpkin_data::data_component_impl::MapIdImpl;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::screen::WindowType;
use pumpkin_protocol::java::server::play::SlotActionType;
use pumpkin_world::inventory::{Inventory, SimpleInventory};

use crate::player::player_inventory::PlayerInventory;
use crate::screen_handler::{InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour};
use crate::slot::PredicateSlot;

/// `CartographyTableMenu.INV_SLOT_START` (`CartographyTableMenu.java:21`).
const INV_SLOT_START: i32 = 3;
/// `CartographyTableMenu.INV_SLOT_END` (`CartographyTableMenu.java:22`).
const INV_SLOT_END: i32 = 30;
/// `CartographyTableMenu.USE_ROW_SLOT_START` (`CartographyTableMenu.java:23`).
const USE_ROW_SLOT_START: i32 = 30;
/// `CartographyTableMenu.USE_ROW_SLOT_END` (`CartographyTableMenu.java:24`).
const USE_ROW_SLOT_END: i32 = 39;

/// `CartographyTableMenu.java:49-54`: the map slot accepts a stack carrying `MAP_ID`.
fn may_place_map(stack: &ItemStack) -> bool {
    stack.get_data_component::<MapIdImpl>().is_some()
}

/// `CartographyTableMenu.java:55-60`: paper, another map, or a glass pane.
fn may_place_additional(stack: &ItemStack) -> bool {
    stack.item == &Item::PAPER || stack.item == &Item::MAP || stack.item == &Item::GLASS_PANE
}

/// `CartographyTableMenu.java:61-65`: the result slot never accepts a placed item.
const fn may_place_result(_stack: &ItemStack) -> bool { false }

pub struct CartographyTableScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    pub input_inventory: Arc<SimpleInventory>,
    pub output_inventory: Arc<SimpleInventory>,
}

impl CartographyTableScreenHandler {
    pub fn new(sync_id: u8, player_inventory: &Arc<PlayerInventory>) -> Self {
        let mut handler = Self {
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(WindowType::CartographyTable)),
            input_inventory: Arc::new(SimpleInventory::new(2)),
            output_inventory: Arc::new(SimpleInventory::new(1)),
        };
        handler.add_slot(Arc::new(PredicateSlot::new(
            handler.input_inventory.clone() as Arc<dyn Inventory>, 0, may_place_map,
        )));
        handler.add_slot(Arc::new(PredicateSlot::new(
            handler.input_inventory.clone() as Arc<dyn Inventory>, 1, may_place_additional,
        )));
        handler.add_slot(Arc::new(PredicateSlot::new(
            handler.output_inventory.clone() as Arc<dyn Inventory>, 0, may_place_result,
        )));
        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inventory);
        handler
    }
}

impl ScreenHandler for CartographyTableScreenHandler {
    /// `CartographyTableMenu.java:93-95`: the opening block must still be a cartography table;
    /// the range check is `AbstractContainerMenu.java:93-95`.
    fn container_access(&self) -> crate::screen_handler::ContainerAccess {
        crate::screen_handler::ContainerAccess::Block(|block| {
            block.id == pumpkin_data::Block::CARTOGRAPHY_TABLE.id
        })
    }

    fn get_behaviour(&self) -> &ScreenHandlerBehaviour { &self.behaviour }
    fn get_behaviour_mut(&mut self) -> &mut ScreenHandlerBehaviour { &mut self.behaviour }
    fn as_any(&self) -> &dyn Any { self }
    fn as_any_mut(&mut self) -> &mut dyn Any { self }

    fn on_slot_click(
        &mut self, slot_index: i32, button: i32, action_type: SlotActionType,
        player: &dyn InventoryPlayer,
    ) {
        self.internal_on_slot_click(slot_index, button, action_type, player);
    }

    /// `CartographyTableMenu.removed` (`CartographyTableMenu.java:198-203`).
    fn on_closed(&mut self, player: &dyn InventoryPlayer) {
        self.default_on_closed(player);
        self.drop_inventory(player, self.input_inventory.clone());
    }

    /// `CartographyTableMenu.quickMoveStack` (`CartographyTableMenu.java:149-196`). Result
    /// computation (`CartographyTableMenu.java:97-141`, including
    /// `CartographyTableMenu.java:97-141`) remains unavailable because Pumpkin's
    /// map post-processing component is only a marker, so the result is empty in practice.
    // Slot construction mirrors `CartographyTableMenu.java:21`, `CartographyTableMenu.java:22`,
    // `CartographyTableMenu.java:23`, `CartographyTableMenu.java:24`, `CartographyTableMenu.java:49-54`,
    // `CartographyTableMenu.java:55-60`, and `CartographyTableMenu.java:61-65`.
    fn quick_move(&mut self, _player: &dyn InventoryPlayer, slot_index: i32) -> ItemStack {
        let mut stack = ItemStack::EMPTY.clone();
        let Some(slot) = self.get_behaviour().slots.get(slot_index as usize).cloned() else {
            return stack;
        };
        let mut slot_stack = slot.get_cloned_stack();
        if slot_stack.is_empty() { return stack; }
        stack = slot_stack.clone();
        let total_slots = self.get_behaviour().slots.len() as i32;

        if slot_index == 2 {
            if !self.insert_item(&mut slot_stack, INV_SLOT_START, total_slots, true) {
                return ItemStack::EMPTY.clone();
            }
        } else if slot_index != 0 && slot_index != 1 {
            if slot_stack.get_data_component::<MapIdImpl>().is_some() {
                if !self.insert_item(&mut slot_stack, 0, 1, false) { return ItemStack::EMPTY.clone(); }
            } else if may_place_additional(&slot_stack) {
                if !self.insert_item(&mut slot_stack, 1, 2, false) { return ItemStack::EMPTY.clone(); }
            } else if (INV_SLOT_START..INV_SLOT_END).contains(&slot_index) {
                if !self.insert_item(&mut slot_stack, USE_ROW_SLOT_START, USE_ROW_SLOT_END, false) {
                    return ItemStack::EMPTY.clone();
                }
            } else if (USE_ROW_SLOT_START..USE_ROW_SLOT_END).contains(&slot_index)
                && !self.insert_item(&mut slot_stack, INV_SLOT_START, INV_SLOT_END, false)
            {
                return ItemStack::EMPTY.clone();
            }
        } else if !self.insert_item(&mut slot_stack, INV_SLOT_START, total_slots, false) {
            return ItemStack::EMPTY.clone();
        }

        if slot_stack.is_empty() { slot.set_stack(ItemStack::EMPTY.clone()); }
        else { slot.set_stack(slot_stack); }
        stack
    }
}
