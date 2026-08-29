use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU16, Ordering};

use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::{RECIPES_STONECUTTING, StonecutterRecipe};
use pumpkin_data::screen::WindowType;
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_protocol::java::server::play::SlotActionType;
use pumpkin_world::inventory::{Inventory, SimpleInventory};

use crate::player::player_inventory::PlayerInventory;
use crate::screen_handler::{InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour};
use crate::slot::{NormalSlot, Slot};

pub struct StonecutterScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    pub input_inventory: Arc<SimpleInventory>,
    pub output_inventory: Arc<SimpleInventory>,
    pub selected_recipe: AtomicU8,
    last_input_item: AtomicU16,
}

impl StonecutterScreenHandler {
    pub fn new(sync_id: u8, player_inventory: &Arc<PlayerInventory>) -> Self {
        let input_inventory = Arc::new(SimpleInventory::new(1));
        let output_inventory = Arc::new(SimpleInventory::new(1));
        let mut handler = Self {
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Stonecutter)),
            input_inventory: input_inventory.clone(),
            output_inventory: output_inventory.clone(),
            selected_recipe: AtomicU8::new(u8::MAX),
            last_input_item: AtomicU16::new(Item::AIR.id),
        };
        handler.add_slot(Arc::new(NormalSlot::new(
            input_inventory.clone() as Arc<dyn Inventory>, 0,
        )));
        handler.add_slot(Arc::new(StonecutterOutputSlot::new(
            output_inventory as Arc<dyn Inventory>, input_inventory as Arc<dyn Inventory>, 0,
        )));
        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inventory);
        handler
    }

    /// `StonecutterMenu.slotsChanged` (`StonecutterMenu.java:127-134`): reset the selected
    /// recipe when the input item changes, but retain it when only the count changes.
    fn update_output(&self) {
        let input = self.input_inventory.get_stack(0);
        let input_item = if input.is_empty() { Item::AIR.id } else { input.item.id };
        if self.last_input_item.swap(input_item, Ordering::Relaxed) != input_item {
            self.selected_recipe.store(u8::MAX, Ordering::Relaxed);
        }
        if input.is_empty() {
            self.output_inventory.set_stack(0, ItemStack::EMPTY.clone());
            self.selected_recipe.store(u8::MAX, Ordering::Relaxed);
            return;
        }
        let recipes = Self::get_available_recipes(&input);
        let index = self.selected_recipe.load(Ordering::Relaxed);
        if index != u8::MAX && (index as usize) < recipes.len() {
            let recipe = recipes[index as usize];
            let item = Item::from_registry_key(recipe.result.id).unwrap_or(&Item::AIR);
            self.output_inventory.set_stack(0, ItemStack::new(recipe.result.count, item));
        } else {
            self.output_inventory.set_stack(0, ItemStack::EMPTY.clone());
        }
    }

    fn select_recipe(&self, id: i32) -> bool {
        if i32::from(self.selected_recipe.load(Ordering::Relaxed)) == id { return false; }
        let input = self.input_inventory.get_stack(0);
        let count = Self::get_available_recipes(&input).len();
        if id < 0 || id as usize >= count { return false; }
        self.last_input_item.store(input.item.id, Ordering::Relaxed);
        self.selected_recipe.store(id as u8, Ordering::Relaxed);
        self.update_output();
        true
    }

    fn get_available_recipes(input: &ItemStack) -> Vec<&'static StonecutterRecipe> {
        RECIPES_STONECUTTING.iter().filter(|r| r.ingredient.match_item(input.item)).collect()
    }
}

impl ScreenHandler for StonecutterScreenHandler {
    /// `StonecutterMenu.java:105-107`: the opening block must still be a stonecutter;
    /// player range follows `AbstractContainerMenu.java:93-95`.
    fn container_access(&self) -> crate::screen_handler::ContainerAccess {
        crate::screen_handler::ContainerAccess::Block(|block| {
            block.id == pumpkin_data::Block::STONECUTTER.id
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
        if slot_index == 0 || slot_index == 1 { self.update_output(); }
    }

    fn on_button_click(&mut self, _player: &dyn InventoryPlayer, id: i32) -> bool {
        let clicked = self.select_recipe(id);
        if clicked { self.send_content_updates(); }
        clicked
    }

    /// `StonecutterMenu.removed` (`StonecutterMenu.java:231-235`) returns the input to the player.
    fn on_closed(&mut self, player: &dyn InventoryPlayer) {
        self.default_on_closed(player);
        self.output_inventory.set_stack(0, ItemStack::EMPTY.clone());
        self.selected_recipe.store(u8::MAX, Ordering::Relaxed);
        self.drop_inventory(player, self.input_inventory.clone());
    }

    /// `StonecutterMenu.quickMoveStack` (`StonecutterMenu.java:198-201`).
    fn quick_move(&mut self, player: &dyn InventoryPlayer, slot_index: i32) -> ItemStack {
        let mut stack = ItemStack::EMPTY.clone();
        let Some(slot) = self.get_behaviour().slots.get(slot_index as usize).cloned() else { return stack; };
        let mut slot_stack = slot.get_cloned_stack();
        if slot_stack.is_empty() { return stack; }
        stack = slot_stack.clone();
        if slot_index < 2 {
            if !self.insert_item(&mut slot_stack, 2, 38, true) { return ItemStack::EMPTY.clone(); }
            slot.on_quick_move_crafted(slot_stack.clone(), stack.clone());
        } else {
            if Self::get_available_recipes(&slot_stack).is_empty() { return ItemStack::EMPTY.clone(); }
            if !self.insert_item(&mut slot_stack, 0, 1, false) { return ItemStack::EMPTY.clone(); }
        }
        let moved = stack.item_count - slot_stack.item_count;
        if slot_stack.is_empty() { slot.set_stack(ItemStack::EMPTY.clone()); }
        else { slot.set_stack(slot_stack); }
        if slot_index == 1 {
            let mut taken = stack.clone();
            taken.set_count(moved);
            slot.on_take_item(player, &taken);
            self.update_output();
        } else if slot_index >= 2 { self.update_output(); }
        stack
    }
}

pub struct StonecutterOutputSlot {
    pub inventory: Arc<dyn Inventory>,
    pub input_inventory: Arc<dyn Inventory>,
    pub index: usize,
    pub id: AtomicU8,
}

impl StonecutterOutputSlot {
    pub fn new(inventory: Arc<dyn Inventory>, input_inventory: Arc<dyn Inventory>, index: usize) -> Self {
        Self { inventory, input_inventory, index, id: AtomicU8::new(0) }
    }
}

impl Slot for StonecutterOutputSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> { self.inventory.clone() }
    fn get_index(&self) -> usize { self.index }
    fn set_id(&self, id: usize) { self.id.store(id as u8, Ordering::Relaxed); }
    fn on_take_item(&self, player: &dyn InventoryPlayer, stack: &ItemStack) {
        player.increment_stat(StatisticCategory::Crafted, stack.item.id as i32, stack.item_count as i32);
        self.input_inventory.remove_stack_specific(0, 1);
        self.mark_dirty();
    }
    fn can_insert(&self, _stack: &ItemStack) -> bool { false }
    fn get_stack(&self) -> ItemStack { self.inventory.get_stack(self.index) }
    fn get_cloned_stack(&self) -> ItemStack { self.inventory.get_stack(self.index) }
    fn has_stack(&self) -> bool { !self.inventory.get_stack(self.index).is_empty() }
    fn set_stack(&self, stack: ItemStack) { self.inventory.set_stack(self.index, stack); }
    fn set_stack_prev(&self, _stack: ItemStack, _previous_stack: ItemStack) {}
    fn mark_dirty(&self) { self.inventory.mark_dirty(); }
}
