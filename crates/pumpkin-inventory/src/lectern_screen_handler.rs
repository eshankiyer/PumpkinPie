use std::any::Any;
use std::sync::Arc;

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::screen::WindowType;
use pumpkin_world::block::entities::PropertyDelegate;
use pumpkin_world::inventory::Inventory;

use crate::screen_handler::{InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour, ScreenProperty, offer_or_drop_stack};
use crate::slot::NormalSlot;

/// Callbacks into the lectern block for page and book state changes.
pub trait LecternController: Send + Sync {
    fn current_page(&self) -> i32;
    fn set_page(&self, page: i32);
    fn on_book_taken(&self);
}

struct PageDelegate(Arc<dyn LecternController>);

impl PropertyDelegate for PageDelegate {
    fn get_property(&self, index: i32) -> i32 {
        if index == 0 { self.0.current_page() } else { 0 }
    }
    fn set_property(&self, _index: i32, _value: i32) {}
    fn get_properties_size(&self) -> i32 { 1 }
}

/// Vanilla `LecternScreenHandler`: one book slot and current page property 0.
pub struct LecternScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    inventory: Arc<dyn Inventory>,
    controller: Arc<dyn LecternController>,
}

impl LecternScreenHandler {
    const PREVIOUS_PAGE_BUTTON_ID: i32 = 1;
    const NEXT_PAGE_BUTTON_ID: i32 = 2;
    const TAKE_BOOK_BUTTON_ID: i32 = 3;
    const JUMP_TO_PAGE_OFFSET: i32 = 100;

    pub fn new(sync_id: u8, inventory: Arc<dyn Inventory>, controller: Arc<dyn LecternController>) -> Self {
        let mut handler = Self {
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Lectern)),
            inventory: inventory.clone(),
            controller: controller.clone(),
        };
        handler.add_slot(Arc::new(NormalSlot::new(inventory, 0)));
        handler.add_property(ScreenProperty::new(Arc::new(PageDelegate(controller)), 0));
        handler
    }

    /// `LecternMenu.getBook` (LecternMenu.java:90-92).
    pub fn get_book(&self) -> ItemStack { self.inventory.get_stack(0) }
}

impl ScreenHandler for LecternScreenHandler {
    /// `LecternMenu.java:86-88` delegates to `Container.java:94-101`; block identity is closed
    /// by the world container check and range follows `Player.java:2014-2016`.
    fn container_access(&self) -> crate::screen_handler::ContainerAccess {
        crate::screen_handler::ContainerAccess::RangeOnly
    }
    fn as_any(&self) -> &dyn Any { self }
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
    fn get_behaviour(&self) -> &ScreenHandlerBehaviour { &self.behaviour }
    fn get_behaviour_mut(&mut self) -> &mut ScreenHandlerBehaviour { &mut self.behaviour }

    /// `LecternMenu.clickMenuButton` and `setData` (`LecternMenu.java:40-56,79-83`), including
    /// the immediate broadcast behavior of `LecternMenu.java:79-83`.
    fn on_button_click(&mut self, player: &dyn InventoryPlayer, id: i32) -> bool {
        match id {
            Self::PREVIOUS_PAGE_BUTTON_ID => {
                self.controller.set_page(self.controller.current_page() - 1);
                self.send_content_updates();
                true
            }
            Self::NEXT_PAGE_BUTTON_ID => {
                self.controller.set_page(self.controller.current_page() + 1);
                self.send_content_updates();
                true
            }
            Self::TAKE_BOOK_BUTTON_ID => {
                let stack = self.inventory.remove_stack(0);
                if stack.is_empty() { return false; }
                self.inventory.mark_dirty();
                self.controller.on_book_taken();
                offer_or_drop_stack(player, stack);
                self.send_content_updates();
                true
            }
            _ if id >= Self::JUMP_TO_PAGE_OFFSET => {
                self.controller.set_page(id - Self::JUMP_TO_PAGE_OFFSET);
                self.send_content_updates();
                true
            }
            _ => false,
        }
    }

    fn quick_move(&mut self, _player: &dyn InventoryPlayer, _slot_index: i32) -> ItemStack {
        ItemStack::EMPTY.clone()
    }
}
