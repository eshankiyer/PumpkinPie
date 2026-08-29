//! Loom menu shell: banner/dye/pattern slots and correct shift-click routing.
//!
//! Mirrors `net/minecraft/world/inventory/LoomMenu.java` (26.2 decompile) for slot layout
//! (`INV_SLOT_START`/`END` = 4/31, `USE_ROW_SLOT_START`/`END` = 31/40, banner/dye/pattern/result
//! at 0/1/2/3) and `quickMoveStack` (LoomMenu.java:211-261).
//!
//! The result computation (`setupResultSlot`, LoomMenu.java:269-288) is not implemented because
//! Pumpkin lacks the typed banner pattern components and registry data. The result slot therefore
//! remains empty, while input restrictions and close/shift-click behavior remain vanilla-shaped.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use pumpkin_data::data_component_impl::{DyeImpl, ProvidesBannerPatternsImpl};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::screen::WindowType;
use pumpkin_data::tag::{Item as ItemTag, Taggable};
use pumpkin_protocol::java::server::play::SlotActionType;
use pumpkin_world::inventory::Inventory;
use pumpkin_world::inventory::SimpleInventory;

use crate::player::player_inventory::PlayerInventory;
use crate::screen_handler::{InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour, offer_or_drop_stack};
use crate::slot::{BoxFuture, Slot};

fn is_banner(stack: &ItemStack) -> bool {
    stack.item.has_tag(&ItemTag::MINECRAFT_BANNERS)
}

fn is_dye_item(stack: &ItemStack) -> bool {
    stack.item.has_tag(&ItemTag::MINECRAFT_LOOM_DYES)
        && stack.get_data_component::<DyeImpl>().is_some()
}

fn is_pattern_item(stack: &ItemStack) -> bool {
    stack.item.has_tag(&ItemTag::MINECRAFT_LOOM_PATTERNS)
        && stack.get_data_component::<ProvidesBannerPatternsImpl>().is_some()
}

pub struct LoomScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    pub input_inventory: Arc<SimpleInventory>,
    pub output_inventory: Arc<SimpleInventory>,
}

impl LoomScreenHandler {
    pub fn new(sync_id: u8, player_inventory: &Arc<PlayerInventory>) -> Self {
        let mut handler = Self {
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Loom)),
            input_inventory: Arc::new(SimpleInventory::new(3)),
            output_inventory: Arc::new(SimpleInventory::new(1)),
        };
        handler.add_slot(Arc::new(LoomInputSlot::new(
            handler.input_inventory.clone() as Arc<dyn Inventory>, 0, is_banner,
        )));
        handler.add_slot(Arc::new(LoomInputSlot::new(
            handler.input_inventory.clone() as Arc<dyn Inventory>, 1, is_dye_item,
        )));
        handler.add_slot(Arc::new(LoomInputSlot::new(
            handler.input_inventory.clone() as Arc<dyn Inventory>, 2, is_pattern_item,
        )));
        handler.add_slot(Arc::new(LoomResultSlot::new(
            handler.output_inventory.clone() as Arc<dyn Inventory>, 0,
        )));
        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inventory);
        handler
    }
}

impl ScreenHandler for LoomScreenHandler {
    /// Port of `LoomMenu.java:120-122`: the opening block must still be a loom and the player
    /// range follows `AbstractContainerMenu.java:93-95`.
    fn container_access(&self) -> crate::screen_handler::ContainerAccess {
        crate::screen_handler::ContainerAccess::Block(|block| {
            block.id == pumpkin_data::Block::LOOM.id
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

    /// `LoomMenu.removed` and `quickMoveStack` (`LoomMenu.java:211-261`).
    fn on_closed(&mut self, player: &dyn InventoryPlayer) {
        self.default_on_closed(player);
        for i in 0..3 {
            let stack = self.input_inventory.remove_stack(i);
            if !stack.is_empty() { offer_or_drop_stack(player, stack); }
        }
    }

    fn quick_move(&mut self, player: &dyn InventoryPlayer, slot_index: i32) -> ItemStack {
        let mut stack_left = ItemStack::EMPTY.clone();
        let Some(slot) = self.get_behaviour().slots.get(slot_index as usize).cloned() else {
            return stack_left;
        };
        if !slot.has_stack() { return stack_left; }
        let mut item = slot.get_cloned_stack();
        stack_left = item.clone();

        if slot_index == 3 {
            if !self.insert_item(&mut item, 4, 40, true) { return ItemStack::EMPTY.clone(); }
            slot.on_quick_move_crafted(item.clone(), stack_left.clone());
        } else if slot_index != 0 && slot_index != 1 && slot_index != 2 {
            let moved = if is_banner(&item) {
                self.insert_item(&mut item, 0, 1, false)
            } else if is_dye_item(&item) {
                self.insert_item(&mut item, 1, 2, false)
            } else if is_pattern_item(&item) {
                self.insert_item(&mut item, 2, 3, false)
            } else if (4..31).contains(&slot_index) {
                self.insert_item(&mut item, 31, 40, false)
            } else if (31..40).contains(&slot_index) {
                self.insert_item(&mut item, 4, 31, false)
            } else { false };
            if !moved { return ItemStack::EMPTY.clone(); }
        } else if !self.insert_item(&mut item, 4, 40, false) {
            return ItemStack::EMPTY.clone();
        }

        if item.is_empty() { slot.set_stack(ItemStack::EMPTY.clone()); } else { slot.mark_dirty(); }
        if item.item_count == stack_left.item_count { return ItemStack::EMPTY.clone(); }
        slot.on_take_item(player, &item);
        stack_left
    }
}

/// `mayPlace` for the banner/dye/pattern input slots (LoomMenu.java:64-81).
struct LoomInputSlot {
    inventory: Arc<dyn Inventory>, index: usize, id: AtomicU8,
    predicate: fn(&ItemStack) -> bool,
}

impl LoomInputSlot {
    fn new(inventory: Arc<dyn Inventory>, index: usize, predicate: fn(&ItemStack) -> bool) -> Self {
        Self { inventory, index, id: AtomicU8::new(0), predicate }
    }
}

impl Slot for LoomInputSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> { self.inventory.clone() }
    fn get_index(&self) -> usize { self.index }
    fn set_id(&self, id: usize) { self.id.store(id as u8, std::sync::atomic::Ordering::Relaxed); }
    fn can_insert(&self, stack: &ItemStack) -> bool { (self.predicate)(stack) }
    fn mark_dirty(&self) { self.inventory.mark_dirty(); }
}

/// Result slot (LoomMenu.java:82-105). Never accepts items.
struct LoomResultSlot { inventory: Arc<dyn Inventory>, index: usize, id: AtomicU8 }

impl LoomResultSlot {
    fn new(inventory: Arc<dyn Inventory>, index: usize) -> Self {
        Self { inventory, index, id: AtomicU8::new(0) }
    }
}

impl Slot for LoomResultSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> { self.inventory.clone() }
    fn get_index(&self) -> usize { self.index }
    fn set_id(&self, id: usize) { self.id.store(id as u8, std::sync::atomic::Ordering::Relaxed); }
    fn can_insert(&self, _stack: &ItemStack) -> bool { false }
    fn mark_dirty(&self) { self.inventory.mark_dirty(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_equipment_slots, entity_equipment::EntityEquipment};
    use pumpkin_data::item::Item;
    use tokio::sync::Mutex as TokioMutex;

    fn handler() -> LoomScreenHandler {
        let player_inventory = Arc::new(PlayerInventory::new(
            Arc::new(TokioMutex::new(EntityEquipment::new())),
            Arc::new(build_equipment_slots()),
        ));
        LoomScreenHandler::new(0, &player_inventory)
    }

    #[test]
    fn banner_predicate_matches_only_banners() {
        assert!(is_banner(&ItemStack::new(1, &Item::WHITE_BANNER)));
        assert!(!is_banner(&ItemStack::new(1, &Item::WHITE_DYE)));
    }

    #[test]
    fn dye_predicate_matches_only_dyes() {
        assert!(is_dye_item(&ItemStack::new(1, &Item::WHITE_DYE)));
        assert!(!is_dye_item(&ItemStack::new(1, &Item::WHITE_BANNER)));
    }

    #[test]
    fn pattern_predicate_matches_only_loom_pattern_items() {
        assert!(is_pattern_item(&ItemStack::new(1, &Item::FLOWER_BANNER_PATTERN)));
        assert!(!is_pattern_item(&ItemStack::new(1, &Item::WHITE_DYE)));
    }

    #[test]
    fn input_and_result_slots_enforce_vanilla_predicates() {
        let handler = handler();
        let slots = handler.get_behaviour().slots.clone();
        assert!(!slots[0].can_insert(&ItemStack::new(1, &Item::WHITE_DYE)));
        assert!(slots[0].can_insert(&ItemStack::new(1, &Item::WHITE_BANNER)));
        assert!(!slots[3].can_insert(&ItemStack::new(1, &Item::WHITE_BANNER)));
    }
}
