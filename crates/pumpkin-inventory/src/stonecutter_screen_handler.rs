use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU16, Ordering};

use crate::player::player_inventory::PlayerInventory;
use crate::screen_handler::{
    InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour, ScreenProperty,
};
use crate::slot::{NormalSlot, Slot};

use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::{RECIPES_STONECUTTING, StonecutterRecipe};
use pumpkin_data::screen::WindowType;
use pumpkin_data::sound::Sound;
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_protocol::java::server::play::SlotActionType;
use pumpkin_world::block::entities::PropertyDelegate;
use pumpkin_world::inventory::Inventory;
use pumpkin_world::inventory::SimpleInventory;

pub struct StonecutterScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    pub input_inventory: Arc<SimpleInventory>,
    pub output_inventory: Arc<SimpleInventory>,
    pub selected_recipe: Arc<AtomicU8>,
    last_input_item: AtomicU16,
}

struct SelectedRecipeDelegate(Arc<AtomicU8>);

impl PropertyDelegate for SelectedRecipeDelegate {
    fn get_property(&self, index: i32) -> i32 {
        if index != 0 {
            return 0;
        }

        let selected_recipe = self.0.load(Ordering::Relaxed);
        if selected_recipe == u8::MAX {
            -1
        } else {
            i32::from(selected_recipe)
        }
    }

    fn set_property(&self, index: i32, value: i32) {
        if index == 0 {
            self.0.store(
                if value < 0 { u8::MAX } else { value as u8 },
                Ordering::Relaxed,
            );
        }
    }

    fn get_properties_size(&self) -> i32 {
        1
    }
}

impl StonecutterScreenHandler {
    pub fn new(sync_id: u8, player_inventory: &Arc<PlayerInventory>) -> Self {
        let behaviour = ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Stonecutter));
        let input_inventory = Arc::new(SimpleInventory::new(1));
        let output_inventory = Arc::new(SimpleInventory::new(1));
        let selected_recipe = Arc::new(AtomicU8::new(u8::MAX));

        let mut handler = Self {
            behaviour,
            input_inventory: input_inventory.clone(),
            output_inventory: output_inventory.clone(),
            selected_recipe: selected_recipe.clone(),
            last_input_item: AtomicU16::new(Item::AIR.id),
        };

        handler.add_slot(Arc::new(NormalSlot::new(
            input_inventory.clone() as Arc<dyn Inventory>,
            0,
        )));
        handler.add_slot(Arc::new(StonecutterOutputSlot::new(
            output_inventory as Arc<dyn Inventory>,
            input_inventory as Arc<dyn Inventory>,
            0,
        )));

        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();

        handler.add_player_slots(&player_inventory);

        // `StonecutterMenu` registers selectedRecipeIndex as a DataSlot
        // (`StonecutterMenu.java:28,85`), whose -1 sentinel is returned by
        // `getSelectedRecipeIndex` (`StonecutterMenu.java:88-90`).
        handler.add_property(ScreenProperty::new(
            Arc::new(SelectedRecipeDelegate(selected_recipe)),
            0,
        ));

        handler
    }

    fn update_output(&self) {
        let input_lock = self.input_inventory.get_stack(0);

        // `StonecutterMenu.slotsChanged` resets the selected recipe when the input
        // item changes (StonecutterMenu.java:127-134), but retains it when only the
        // count changes. Track the item id so every input mutation path, including
        // quick-move from the player inventory, gets the same reset.
        let input_item = if input_lock.is_empty() {
            Item::AIR.id
        } else {
            input_lock.item.id
        };
        if self.last_input_item.swap(input_item, Ordering::Relaxed) != input_item
            && self.selected_recipe.load(Ordering::Relaxed) != u8::MAX
        {
            self.selected_recipe.store(u8::MAX, Ordering::Relaxed);
        }

        if input_lock.is_empty() {
            self.output_inventory.set_stack(0, ItemStack::EMPTY.clone());
            self.selected_recipe.store(u8::MAX, Ordering::Relaxed);
            return;
        }

        let available_recipes = Self::get_available_recipes(&input_lock);
        let recipe_index = self.selected_recipe.load(Ordering::Relaxed);

        if recipe_index != u8::MAX && (recipe_index as usize) < available_recipes.len() {
            let recipe = available_recipes[recipe_index as usize];
            let item = Item::from_registry_key(recipe.result.id).unwrap_or(&Item::AIR);
            let result = ItemStack::new(recipe.result.count, item);
            self.output_inventory.set_stack(0, result);
        } else {
            self.output_inventory.set_stack(0, ItemStack::EMPTY.clone());
        }
    }

    fn select_recipe(&self, id: i32) -> bool {
        if i32::from(self.selected_recipe.load(Ordering::Relaxed)) == id {
            return false;
        }

        let input_stack = self.input_inventory.get_stack(0);
        let recipe_count = Self::get_available_recipes(&input_stack).len();

        if !(0..recipe_count).contains(&(id as usize)) {
            return false;
        }

        self.last_input_item
            .store(input_stack.item.id, Ordering::Relaxed);
        self.selected_recipe.store(id as u8, Ordering::Relaxed);
        self.update_output();
        true
    }

    fn get_available_recipes(input: &ItemStack) -> Vec<&'static StonecutterRecipe> {
        let item = input.item;
        RECIPES_STONECUTTING
            .iter()
            .filter(|r| r.ingredient.match_item(item))
            .collect()
    }
}

impl ScreenHandler for StonecutterScreenHandler {
    /// Port of `StonecutterMenu.java:105-107`: the block at the opening position must still be
    /// `Blocks.STONECUTTER` and the player must still be within
    /// `blockInteractionRange() + 4.0` (`AbstractContainerMenu.java:93-95`).
    fn container_access(&self) -> crate::screen_handler::ContainerAccess {
        crate::screen_handler::ContainerAccess::Block(|block| {
            block.id == pumpkin_data::Block::STONECUTTER.id
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

    /// `StonecutterMenu.removed` (StonecutterMenu.java:231-235) discards the result slot and
    /// then `clearContainer`s the menu's own input container, so the ingredient goes back to
    /// the player (or is dropped). Without this override only the cursor stack was returned
    /// and the item sitting in the input slot was destroyed on close.
    fn on_closed(&mut self, player: &dyn InventoryPlayer) {
        self.default_on_closed(player);
        self.output_inventory.set_stack(0, ItemStack::EMPTY.clone());
        self.selected_recipe.store(u8::MAX, Ordering::Relaxed);
        let input: Arc<dyn Inventory> = self.input_inventory.clone();
        self.drop_inventory(player, input);
    }

    fn on_slot_click(
        &mut self,
        slot_index: i32,
        button: i32,
        action_type: SlotActionType,
        player: &dyn InventoryPlayer,
    ) {
        self.internal_on_slot_click(slot_index, button, action_type, player);
        if slot_index == 0 || slot_index == 1 {
            self.update_output();
        }
    }

    fn on_button_click(&mut self, _player: &dyn InventoryPlayer, id: i32) -> bool {
        let clicked = self.select_recipe(id);
        if clicked {
            self.send_content_updates();
        }
        clicked
    }

    fn quick_move(&mut self, player: &dyn InventoryPlayer, slot_index: i32) -> ItemStack {
        let mut stack = ItemStack::EMPTY.clone();
        let slot = self.get_behaviour().slots.get(slot_index as usize).cloned();

        if let Some(slot) = slot {
            let mut slot_stack = slot.get_cloned_stack();
            if !slot_stack.is_empty() {
                stack = slot_stack.clone();
                if slot_index < 2 {
                    // From Stonecutter to Player
                    if !self.insert_item(&mut slot_stack, 2, 38, true) {
                        return ItemStack::EMPTY.clone();
                    }
                    slot.on_quick_move_crafted(slot_stack.clone(), stack.clone());
                } else {
                    // From Player to Stonecutter
                    // `StonecutterMenu.quickMoveStack` only routes items accepted
                    // by the stonecutter recipe manager to the input slot
                    // (StonecutterMenu.java:198-201).
                    if Self::get_available_recipes(&slot_stack).is_empty() {
                        return ItemStack::EMPTY.clone();
                    }
                    if !self.insert_item(&mut slot_stack, 0, 1, false) {
                        return ItemStack::EMPTY.clone();
                    }
                }

                let moved_count = stack.item_count - slot_stack.item_count;
                if slot_stack.is_empty() {
                    slot.set_stack(ItemStack::EMPTY.clone());
                } else {
                    slot.set_stack(slot_stack);
                }

                if slot_index == 1 {
                    let mut taken_stack = stack.clone();
                    taken_stack.set_count(moved_count);
                    slot.on_take_item(player, &taken_stack);
                    self.update_output();
                } else if slot_index >= 2 {
                    self.update_output();
                }
            }
        }
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
    pub fn new(
        inventory: Arc<dyn Inventory>,
        input_inventory: Arc<dyn Inventory>,
        index: usize,
    ) -> Self {
        Self {
            inventory,
            input_inventory,
            index,
            id: AtomicU8::new(0),
        }
    }
}

impl Slot for StonecutterOutputSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> {
        self.inventory.clone()
    }

    fn get_index(&self) -> usize {
        self.index
    }

    fn set_id(&self, id: usize) {
        self.id.store(id as u8, Ordering::Relaxed);
    }

    fn on_take_item(&self, player: &dyn InventoryPlayer, stack: &ItemStack) {
        player.increment_stat(
            StatisticCategory::Crafted,
            stack.item.id as i32,
            stack.item_count as i32,
        );
        self.input_inventory.remove_stack_specific(0, 1);
        self.mark_dirty();
        player.play_sound(Sound::UiStonecutterTakeResult);
    }

    fn can_insert(&self, _stack: &ItemStack) -> bool {
        false
    }

    fn get_stack(&self) -> ItemStack {
        self.inventory.get_stack(self.index)
    }

    fn get_cloned_stack(&self) -> ItemStack {
        self.inventory.get_stack(self.index)
    }

    fn has_stack(&self) -> bool {
        !self.inventory.get_stack(self.index).is_empty()
    }

    fn set_stack(&self, stack: ItemStack) {
        self.inventory.set_stack(self.index, stack);
    }

    fn set_stack_prev(&self, _stack: ItemStack, _previous_stack: ItemStack) {
        // Do nothing
    }

    fn mark_dirty(&self) {
        self.inventory.mark_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_equipment_slots, entity_equipment::EntityEquipment};
    use std::sync::Mutex;

    fn handler() -> StonecutterScreenHandler {
        let player_inventory = Arc::new(PlayerInventory::new(
            Arc::new(Mutex::new(EntityEquipment::new())),
            Arc::new(build_equipment_slots()),
        ));
        StonecutterScreenHandler::new(0, &player_inventory)
    }

    #[tokio::test]
    fn selecting_a_valid_recipe_refreshes_the_output() {
        let handler = handler();
        let input = ItemStack::new(1, &Item::STONE);
        let recipes = StonecutterScreenHandler::get_available_recipes(&input);
        assert!(!recipes.is_empty());
        handler.input_inventory.set_stack(0, input);

        assert!(handler.select_recipe(0));
        assert_eq!(handler.selected_recipe.load(Ordering::Relaxed), 0);

        let output = handler.output_inventory.get_stack(0);
        let expected = Item::from_registry_key(recipes[0].result.id)
            .expect("stonecutter recipe result must be a registered item");
        assert_eq!(output.item.id, expected.id);
        assert_eq!(output.item_count, recipes[0].result.count);
        // The selected recipe is a synced DataSlot in `StonecutterMenu`
        // (`StonecutterMenu.java:28,85,88-90`).
        assert_eq!(handler.get_behaviour().properties[0].get(), 0);
    }

    #[tokio::test]
    fn invalid_recipe_selection_preserves_the_current_output() {
        let handler = handler();
        handler
            .input_inventory
            .set_stack(0, ItemStack::new(1, &Item::STONE));
        assert!(handler.select_recipe(0));

        let output_before = handler.output_inventory.remove_stack(0);
        handler.output_inventory.set_stack(0, output_before.clone());

        assert!(!handler.select_recipe(i32::MAX));
        assert_eq!(handler.selected_recipe.load(Ordering::Relaxed), 0);
        let output_after = handler.output_inventory.remove_stack(0);
        assert_eq!(output_after.item.id, output_before.item.id);
        assert_eq!(output_after.item_count, output_before.item_count);
    }

    #[tokio::test]
    fn output_refresh_clears_selection_when_input_is_depleted() {
        let handler = handler();
        handler
            .input_inventory
            .set_stack(0, ItemStack::new(1, &Item::STONE));
        assert!(handler.select_recipe(0));

        handler
            .input_inventory
            .set_stack(0, ItemStack::EMPTY.clone());
        handler.update_output();

        assert_eq!(handler.selected_recipe.load(Ordering::Relaxed), u8::MAX);
        // Clearing the input resets the selected DataSlot to vanilla's -1 sentinel
        // (`StonecutterMenu.java:136-143`).
        assert_eq!(handler.get_behaviour().properties[0].get(), -1);
        let output = handler.output_inventory.get_stack(0);
        assert!(output.is_empty());
    }

    #[tokio::test]
    fn changing_input_item_clears_the_selected_recipe() {
        let handler = handler();
        handler
            .input_inventory
            .set_stack(0, ItemStack::new(1, &Item::STONE));
        assert!(handler.select_recipe(0));

        handler
            .input_inventory
            .set_stack(0, ItemStack::new(1, &Item::COBBLESTONE));
        handler.update_output();

        assert_eq!(handler.selected_recipe.load(Ordering::Relaxed), u8::MAX);
        assert!(handler.output_inventory.get_stack(0).is_empty());
    }
}
