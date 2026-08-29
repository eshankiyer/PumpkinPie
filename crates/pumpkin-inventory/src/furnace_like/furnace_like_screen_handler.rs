//! Furnace-like screen handler.
//!
//! This module implements the screen handler for furnace-like blocks:
//! - Furnace
//! - Smoker
//! - Blast Furnace
//!
//! All three share the same 3-slot layout:
//! - Slot 0: Input (item to smelt/cook)
//! - Slot 1: Fuel (coal, charcoal, etc.)
//! - Slot 2: Output (smelted result)
//!
//! The screen handler tracks 4 properties:
//! - Property 0: Fire icon animation (fuel burn time remaining)
//! - Property 1: Maximum fuel burn time
//! - Property 2: Progress arrow (cooking/smelt time)
//! - Property 3: Maximum progress (typically 200 ticks for furnace)

use std::{any::Any, sync::Arc};

use pumpkin_data::{fuels::is_fuel, item_stack::ItemStack, recipes::{CookingRecipeKind, get_cooking_recipe_with_ingredient}, screen::WindowType};
use pumpkin_world::{
    block::entities::{ExperienceContainer, PropertyDelegate},
    inventory::Inventory,
};

use crate::{
    player::player_inventory::PlayerInventory,
    screen_handler::{
        InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour, ScreenHandlerListener,
        ScreenProperty,
    },
};
use tracing::debug;

use super::furnace_like_slot::{FurnaceLikeSlot, FurnaceLikeSlotType, FurnaceOutputSlot};


/// Vanilla `AbstractFurnaceMenu.canSmelt` (`AbstractFurnaceMenu.java:134-136`) checks the
/// station's `RecipePropertySet`; those sets come from `RecipeManager.RECIPE_PROPERTY_SETS`
/// (`RecipeManager.java:41-56`) and `finalizeRecipeLoading` (`RecipeManager.java:88-120`).
/// The direct table check keeps furnace, blast-furnace and smoker types separate
/// (`RecipeManager.java:48-53`).
/// The menu constructors select their station recipe set at `AbstractFurnaceMenu.java:40-53`.
fn can_smelt(item: &pumpkin_data::item::Item, window_type: Option<WindowType>) -> bool {
    let kind = match window_type {
        Some(WindowType::BlastFurnace) => CookingRecipeKind::Blasting,
        Some(WindowType::Smoker) => CookingRecipeKind::Smoking,
        _ => CookingRecipeKind::Smelting,
    };
    get_cooking_recipe_with_ingredient(item, kind).is_some()
}
/// Screen handler for furnace-like containers.
///
/// Handles the UI for furnaces, smokers, and blast furnaces.
/// These all share the same slot layout and quick-move behavior.
pub struct FurnaceLikeScreenHandler {
    /// The furnace's inventory (3 slots: 0 input, 1 fuel, 2 output).
    pub inventory: Arc<dyn Inventory>,
    /// Container that tracks accumulated smelting experience.
    ///
    /// Experience is awarded to the player when they take items from the output slot.
    experience_container: Arc<dyn ExperienceContainer>,
    /// Core screen handler behavior (slots, sync ID, properties, listeners).
    behaviour: ScreenHandlerBehaviour,
}

impl FurnaceLikeScreenHandler {
    /// Creates a new furnace-like screen handler.
    ///
    /// # Arguments
    /// - `sync_id` - The sync ID for client-server matching
    /// - `player_inventory` - The player's inventory
    /// - `inventory` - The furnace's inventory (3 slots)
    /// - `property_delegate` - Delegate for accessing furnace properties
    /// - `experience_container` - Container that tracks smelting experience
    /// - `window_type` - The window type (Furnace, Smoker, or `BlastFurnace`)
    pub fn new(
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        inventory: Arc<dyn Inventory>,
        property_delegate: &Arc<dyn PropertyDelegate>,
        experience_container: Arc<dyn ExperienceContainer>,
        window_type: WindowType,
    ) -> Self {
        struct FurnaceLikeScreenListener;
        impl ScreenHandlerListener for FurnaceLikeScreenListener {
            fn on_property_update(
                &self,
                screen_handler: &ScreenHandlerBehaviour,
                property: u8,
                value: i32,
            ) {
                if let Some(sync_handler) = screen_handler.sync_handler.as_ref() {
                    sync_handler.update_property(screen_handler, i32::from(property), value);
                }
            }
        }
        let mut handler = Self {
            inventory,
            experience_container,
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(window_type)),
        };

        // 0: Fire icon (fuel left) counting from fuel burn time down to 0 (in-game ticks)
        // 1: Maximum fuel burn time fuel burn time or 0 (in-game ticks)
        // 2: Progress arrow counting from 0 to maximum progress (in-game ticks)
        // 3: Maximum progress always 200 on the vanilla server
        for i in 0..4 {
            handler.add_property(ScreenProperty::new(property_delegate.clone(), i));
        }

        handler.add_listener(Arc::new(FurnaceLikeScreenListener));
        handler.add_inventory_slots();
        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inventory);

        handler
    }

    /// Adds the 3 furnace inventory slots.
    ///
    /// - Slot 0: Input (top)
    /// - Slot 1: Fuel (bottom)
    /// - Slot 2: Output
    fn add_inventory_slots(&mut self) {
        self.add_slot(Arc::new(FurnaceLikeSlot::new(
            self.inventory.clone(),
            FurnaceLikeSlotType::Top,
        )));
        self.add_slot(Arc::new(FurnaceLikeSlot::new(
            self.inventory.clone(),
            FurnaceLikeSlotType::Bottom,
        )));
        // Output slot awards experience when items are taken
        self.add_slot(Arc::new(FurnaceOutputSlot::new(
            self.inventory.clone(),
            self.experience_container.clone(),
        )));
    }
}

impl ScreenHandler for FurnaceLikeScreenHandler {
    /// `AbstractFurnaceMenu.java:82-84` delegates to `Container.stillValidBlockEntity`
    /// (`Container.java:94-101`) with player range from
    /// `Player.java:2014-2016`.
    fn container_access(&self) -> crate::screen_handler::ContainerAccess {
        crate::screen_handler::ContainerAccess::RangeOnly
    }
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_behaviour(&self) -> &ScreenHandlerBehaviour {
        &self.behaviour
    }

    fn get_behaviour_mut(&mut self) -> &mut ScreenHandlerBehaviour {
        &mut self.behaviour
    }

    fn on_closed(&mut self, player: &dyn InventoryPlayer) {
        self.default_on_closed(player);
        // TODO: self.inventory.on_closed(player);
    }

    /// Quick move logic for furnace-like containers.
    ///
    /// `AbstractFurnaceMenu.quickMoveStack` (`AbstractFurnaceMenu.java:97-121`):
    /// station-specific smeltable items go to input before fuel is considered, and
    /// other player items swap between main inventory and hotbar.
    fn quick_move(&mut self, player: &dyn InventoryPlayer, slot_index: i32) -> ItemStack {
        const INPUT_SLOT: i32 = 0;
        const FUEL_SLOT: i32 = 1;
        const OUTPUT_SLOT: i32 = 2;
        const MAIN_START: i32 = 3;
        const MAIN_END: i32 = 30;
        const HOTBAR_START: i32 = 30;
        const HOTBAR_END: i32 = 39;

        debug!("FurnaceLikeScreenHandler::quick_move slot_index={slot_index}");
        let mut stack_left = ItemStack::EMPTY.clone();
        let slot = self.get_behaviour().slots[slot_index as usize].clone();
        if !slot.has_stack() { return stack_left; }
        let mut stack = slot.get_stack();
        stack_left = stack.clone();

        let success = if slot_index < 3 {
            self.insert_item(&mut stack, MAIN_START, self.get_behaviour().slots.len() as i32, true)
        } else if can_smelt(stack.item, self.get_behaviour().window_type) {
            self.insert_item(&mut stack, INPUT_SLOT, INPUT_SLOT + 1, false)
        } else if is_fuel(stack.item.id) {
            self.insert_item(&mut stack, FUEL_SLOT, FUEL_SLOT + 1, false)
        } else if (MAIN_START..MAIN_END).contains(&slot_index) {
            self.insert_item(&mut stack, HOTBAR_START, HOTBAR_END, false)
        } else if (HOTBAR_START..HOTBAR_END).contains(&slot_index) {
            self.insert_item(&mut stack, MAIN_START, MAIN_END, false)
        } else { false };

        if !success { return ItemStack::EMPTY.clone(); }
        if stack.is_empty() { slot.set_stack(ItemStack::EMPTY.clone()); } else { slot.set_stack(stack); }
        if slot_index == OUTPUT_SLOT { slot.on_take_item(player, &stack_left); }
        stack_left
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::item::Item;
    #[test]
    fn can_smelt_filters_station_recipe_sets() {
        assert!(can_smelt(&Item::IRON_ORE, Some(WindowType::BlastFurnace)));
        assert!(!can_smelt(&Item::IRON_ORE, Some(WindowType::Smoker)));
        assert!(can_smelt(&Item::BEEF, Some(WindowType::Smoker)));
        assert!(!can_smelt(&Item::BEEF, Some(WindowType::BlastFurnace)));
    }
}
