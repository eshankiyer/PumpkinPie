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

use std::{any::Any, pin::Pin, sync::Arc};

use pumpkin_data::{
    fuels::is_fuel,
    item_stack::ItemStack,
    recipes::{CookingRecipeKind, get_cooking_recipe_with_ingredient},
    screen::WindowType,
};
use pumpkin_world::{
    block::entities::{ExperienceContainer, PropertyDelegate},
    inventory::Inventory,
};

use crate::{
    player::player_inventory::PlayerInventory,
    screen_handler::{
        InventoryPlayer, ItemStackFuture, ScreenHandler, ScreenHandlerBehaviour,
        ScreenHandlerFuture, ScreenHandlerListener, ScreenProperty,
    },
};
use tracing::debug;

use super::furnace_like_slot::{FurnaceLikeSlot, FurnaceLikeSlotType, FurnaceOutputSlot};

/// Vanilla `AbstractFurnaceMenu.canSmelt` (`AbstractFurnaceMenu.java:134-136`): tests the
/// item against the station's accepted-input `RecipePropertySet`. Those sets are built by
/// `RecipeManager.RECIPE_PROPERTY_SETS` (`RecipeManager.java:41-56`) as "every input
/// ingredient of at least one recipe of this station's cooking type" and collected in
/// `finalizeRecipeLoading` (`RecipeManager.java:88-120`); here that membership test is
/// computed directly from the static cooking recipe table. Each station filters its own
/// type: a blast furnace only accepts blastable ores, a smoker only smokeable food, a
/// furnace anything smeltable (`FurnaceMenu`/`BlastFurnaceMenu`/`SmokerMenu` pass their
/// respective `RecipePropertySet` key, `AbstractFurnaceMenu.java:40-53`).
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
    pub async fn new(
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        inventory: Arc<dyn Inventory>,
        property_delegate: Arc<dyn PropertyDelegate>,
        experience_container: Arc<dyn ExperienceContainer>,
        window_type: WindowType,
    ) -> Self {
        struct FurnaceLikeScreenListener;
        impl ScreenHandlerListener for FurnaceLikeScreenListener {
            fn on_property_update<'a>(
                &'a self,
                screen_handler: &'a ScreenHandlerBehaviour,
                property: u8,
                value: i32,
            ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
                Box::pin(async move {
                    if let Some(sync_handler) = screen_handler.sync_handler.as_ref() {
                        sync_handler
                            .update_property(screen_handler, i32::from(property), value)
                            .await;
                    }
                })
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

        handler
            .add_listener(Arc::new(FurnaceLikeScreenListener))
            .await;
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
    /// Port of `AbstractFurnaceMenu.java:82-84`, which delegates to
    /// `Container.stillValidBlockEntity` (`Container.java:94-101`): same block entity at
    /// the opening position, and the player within `blockInteractionRange() + 4.0`
    /// (`Player.java:2014-2016`). Pumpkin cannot compare block-entity identity here, so
    /// only the range half is enforced; a destroyed block entity is handled by
    /// `World::close_container_screens_at`.
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

    fn on_closed<'a>(&'a mut self, player: &'a dyn InventoryPlayer) -> ScreenHandlerFuture<'a, ()> {
        Box::pin(async move {
            self.default_on_closed(player).await;
            // TODO: self.inventory.on_closed(player).await;
        })
    }

    /// Quick move logic for furnace-like containers.
    ///
    /// - From furnace slots (0-2): Move to player inventory
    /// - Smeltable items: Move to input slot (0)
    /// - Fuel items: Move to fuel slot (1)
    /// - Other items: Swap between main inventory (3..30) and hotbar (30..39)
    fn quick_move<'a>(
        &'a mut self,
        player: &'a dyn InventoryPlayer,
        slot_index: i32,
    ) -> ItemStackFuture<'a> {
        Box::pin(async move {
            const INPUT_SLOT_RANGE: std::ops::Range<i32> = 0..1;
            const FUEL_SLOT_RANGE: std::ops::Range<i32> = 1..2;
            const MAIN_INV_SLOT_RANGE: std::ops::Range<i32> = 3..30;
            const HOTBAR_SLOT_RANGE: std::ops::Range<i32> = 30..39;
            const OUTPUT_SLOT: i32 = 2;

            debug!("FurnaceLikeScreenHandler::quick_move slot_index={slot_index}");

            let mut stack_left = ItemStack::EMPTY.clone();

            let slot = self.get_behaviour().slots[slot_index as usize].clone();

            if !slot.has_stack().await {
                return stack_left;
            }

            let mut stack = slot.get_stack().await;
            stack_left = stack.clone();

            // Routing order mirrors `AbstractFurnaceMenu.quickMoveStack`
            // (`AbstractFurnaceMenu.java:97-121`): smeltable items go to the input
            // slot before fuel is considered, and leftovers shuffle between the
            // player's main inventory and hotbar instead of entering the furnace.
            let success = if slot_index < 3 {
                // If clicked slot is one of the Furnace slots (0, 1, 2):
                // Try to move to player inventory (slots 3 onwards, starting from the end)
                self.insert_item(&mut stack, 3, self.get_behaviour().slots.len() as i32, true)
                    .await
            } else if can_smelt(stack.item, self.get_behaviour().window_type) {
                self.insert_item(
                    &mut stack,
                    INPUT_SLOT_RANGE.start,
                    INPUT_SLOT_RANGE.end,
                    false,
                )
                .await
            } else if is_fuel(stack.item.id) {
                self.insert_item(
                    &mut stack,
                    FUEL_SLOT_RANGE.start,
                    FUEL_SLOT_RANGE.end,
                    false,
                )
                .await
            } else if MAIN_INV_SLOT_RANGE.contains(&slot_index) {
                self.insert_item(
                    &mut stack,
                    HOTBAR_SLOT_RANGE.start,
                    HOTBAR_SLOT_RANGE.end,
                    false,
                )
                .await
            } else if HOTBAR_SLOT_RANGE.contains(&slot_index)
                && !self
                    .insert_item(
                        &mut stack,
                        MAIN_INV_SLOT_RANGE.start,
                        MAIN_INV_SLOT_RANGE.end,
                        false,
                    )
                    .await
            {
                return ItemStack::EMPTY.clone();
            } else {
                false
            };

            if !success {
                return ItemStack::EMPTY.clone();
            }

            if stack.is_empty() {
                slot.set_stack(ItemStack::EMPTY.clone()).await;
            } else {
                slot.set_stack(stack).await;
            }

            // Award XP when taking from output slot (slot 2)
            if slot_index == OUTPUT_SLOT {
                debug!("quick_move: taking from output slot, calling on_take_item");
                slot.on_take_item(player, &stack_left).await;
            }

            stack_left
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::item::Item;

    /// `RecipePropertySet.FURNACE_INPUT`/`BLAST_FURNACE_INPUT`/`SMOKER_INPUT`
    /// (`RecipeManager.java:48-53`): each station accepts exactly the ingredients of its
    /// own cooking recipes - an ore is blastable but not smokeable, food is smokeable but
    /// not blastable, and a non-cooking item is accepted by no furnace-like station.
    #[test]
    fn can_smelt_filters_per_station_cooking_type() {
        assert!(can_smelt(&Item::IRON_ORE, Some(WindowType::Furnace)));
        assert!(can_smelt(&Item::IRON_ORE, Some(WindowType::BlastFurnace)));
        assert!(!can_smelt(&Item::IRON_ORE, Some(WindowType::Smoker)));

        assert!(can_smelt(&Item::BEEF, Some(WindowType::Furnace)));
        assert!(can_smelt(&Item::BEEF, Some(WindowType::Smoker)));
        assert!(!can_smelt(&Item::BEEF, Some(WindowType::BlastFurnace)));

        assert!(!can_smelt(&Item::DIAMOND_SWORD, Some(WindowType::Furnace)));
        assert!(!can_smelt(
            &Item::DIAMOND_SWORD,
            Some(WindowType::BlastFurnace)
        ));
    }
}
