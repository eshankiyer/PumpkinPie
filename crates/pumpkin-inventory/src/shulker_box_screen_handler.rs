//! Shulker box screen handler.
//!
//! Port of `ShulkerBoxMenu.java`. A shulker box looks like a single chest but is
//! not one: it uses `MenuType.SHULKER_BOX` (`ShulkerBoxMenu.java:18`) and its 27
//! slots are `ShulkerBoxSlot`s, which refuse any item that cannot be nested
//! inside a container item (`ShulkerBoxSlot.java:11-14`). That is what stops a
//! shulker box from being placed inside another shulker box.

use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

use pumpkin_data::{
    Block,
    item_stack::ItemStack,
    screen::WindowType,
    tag::{self, Taggable},
};
use pumpkin_world::inventory::Inventory;

use crate::{
    player::player_inventory::PlayerInventory,
    screen_handler::{
        InventoryPlayer, ItemStackFuture, ScreenHandler, ScreenHandlerBehaviour,
        ScreenHandlerFuture,
    },
    slot::{BoxFuture, Slot},
};

/// Number of slots inside a shulker box (`ShulkerBoxMenu.java:10`).
pub const SHULKER_BOX_SIZE: usize = 27;

/// Vanilla `Item.canFitInsideContainerItems` (`Item.java:364`, overridden by
/// `BlockItem.java:193-196` as `!(this.getBlock() instanceof ShulkerBoxBlock)`).
///
/// Items with no corresponding block always fit; block items only fail for the
/// shulker box blocks, which are exactly the `minecraft:shulker_boxes` tag.
#[must_use]
pub fn can_fit_inside_container_items(stack: &ItemStack) -> bool {
    Block::from_item_id(stack.get_item().id)
        .is_none_or(|block| !block.has_tag(&tag::Block::MINECRAFT_SHULKER_BOXES))
}

/// A slot inside a shulker box (`ShulkerBoxSlot.java`).
pub struct ShulkerBoxSlot {
    inventory: Arc<dyn Inventory>,
    index: usize,
    id: AtomicU8,
}

impl ShulkerBoxSlot {
    #[must_use]
    pub fn new(inventory: Arc<dyn Inventory>, index: usize) -> Self {
        Self {
            inventory,
            index,
            id: AtomicU8::new(0),
        }
    }
}

impl Slot for ShulkerBoxSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> {
        self.inventory.clone()
    }

    fn get_index(&self) -> usize {
        self.index
    }

    fn set_id(&self, id: usize) {
        self.id.store(id as u8, Ordering::Relaxed);
    }

    fn mark_dirty(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.inventory.mark_dirty();
        })
    }

    /// `ShulkerBoxSlot.java:11-14`: only items that fit inside container items.
    fn can_insert<'a>(&'a self, stack: &'a ItemStack) -> BoxFuture<'a, bool> {
        Box::pin(async move { can_fit_inside_container_items(stack) })
    }
}

/// Screen handler for a shulker box block (`ShulkerBoxMenu.java`).
pub struct ShulkerBoxScreenHandler {
    /// The shulker box's inventory.
    pub inventory: Arc<dyn Inventory>,
    behaviour: ScreenHandlerBehaviour,
}

impl ShulkerBoxScreenHandler {
    /// Creates a shulker box screen handler.
    ///
    /// `ShulkerBoxMenu.java:17-32`: 27 shulker slots in a 3x9 grid, then the
    /// standard player inventory slots.
    pub async fn new(
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        inventory: Arc<dyn Inventory>,
    ) -> Self {
        let mut handler = Self {
            inventory: inventory.clone(),
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(WindowType::ShulkerBox)),
        };

        inventory.on_open().await;

        for index in 0..SHULKER_BOX_SIZE {
            handler.add_slot(Arc::new(ShulkerBoxSlot::new(inventory.clone(), index)));
        }

        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inventory);

        handler
    }
}

impl ScreenHandler for ShulkerBoxScreenHandler {
    /// Port of `ShulkerBoxMenu.java:35-37`, which delegates to
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
            self.inventory.on_close().await;
        })
    }

    /// `ShulkerBoxMenu.java:39-62`. Note it has no `onTake`/count-equality tail,
    /// unlike `DispenserMenu.quickMoveStack`.
    fn quick_move<'a>(
        &'a mut self,
        _player: &'a dyn InventoryPlayer,
        slot_index: i32,
    ) -> ItemStackFuture<'a> {
        Box::pin(async move {
            let mut stack_left = ItemStack::EMPTY.clone();
            let slot = self.get_behaviour().slots[slot_index as usize].clone();
            let container_size = SHULKER_BOX_SIZE as i32;

            if slot.has_stack().await {
                let mut slot_stack = slot.get_stack().await;
                stack_left = slot_stack.clone();

                if slot_index < container_size {
                    if !self
                        .insert_item(
                            &mut slot_stack,
                            container_size,
                            self.get_behaviour().slots.len() as i32,
                            true,
                        )
                        .await
                    {
                        return ItemStack::EMPTY.clone();
                    }
                } else if !self
                    .insert_item(&mut slot_stack, 0, container_size, false)
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

            stack_left
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use pumpkin_data::item::Item;
    use pumpkin_world::inventory::SimpleInventory;
    use tokio::sync::Mutex;

    use crate::entity_equipment::EntityEquipment;

    use super::*;

    async fn handler() -> (ShulkerBoxScreenHandler, Arc<SimpleInventory>) {
        let player_inventory = Arc::new(PlayerInventory::new(
            Arc::new(Mutex::new(EntityEquipment::new())),
            Arc::new(HashMap::new()),
        ));
        let inventory = Arc::new(SimpleInventory::new(SHULKER_BOX_SIZE));
        let handler = ShulkerBoxScreenHandler::new(0, &player_inventory, inventory.clone()).await;
        (handler, inventory)
    }

    #[test]
    fn shulker_boxes_cannot_fit_inside_container_items() {
        assert!(!can_fit_inside_container_items(&ItemStack::new(
            1,
            &Item::SHULKER_BOX
        )));
        assert!(!can_fit_inside_container_items(&ItemStack::new(
            1,
            &Item::RED_SHULKER_BOX
        )));
    }

    #[test]
    fn ordinary_items_fit_inside_container_items() {
        assert!(can_fit_inside_container_items(&ItemStack::new(
            1,
            &Item::STONE
        )));
        assert!(can_fit_inside_container_items(&ItemStack::new(
            1,
            &Item::DIAMOND
        )));
        assert!(can_fit_inside_container_items(&ItemStack::new(
            1,
            &Item::CHEST
        )));
    }

    #[tokio::test]
    async fn shulker_slots_reject_nested_shulker_boxes() {
        let (handler, _) = handler().await;
        for index in 0..SHULKER_BOX_SIZE {
            let slot = handler.get_behaviour().slots[index].clone();
            assert!(
                !slot
                    .can_insert(&ItemStack::new(1, &Item::SHULKER_BOX))
                    .await
            );
            assert!(slot.can_insert(&ItemStack::new(1, &Item::STONE)).await);
        }
    }

    #[tokio::test]
    async fn player_slots_still_accept_shulker_boxes() {
        let (handler, _) = handler().await;
        let player_slot = handler.get_behaviour().slots[SHULKER_BOX_SIZE].clone();
        assert!(
            player_slot
                .can_insert(&ItemStack::new(1, &Item::SHULKER_BOX))
                .await
        );
    }

    #[tokio::test]
    async fn layout_is_27_container_slots_plus_36_player_slots() {
        let (handler, _) = handler().await;
        assert_eq!(handler.get_behaviour().slots.len(), SHULKER_BOX_SIZE + 36);
        assert_eq!(handler.window_type(), Some(WindowType::ShulkerBox));
    }

    #[tokio::test]
    async fn quick_move_from_container_reaches_player_inventory() {
        let (mut handler, inventory) = handler().await;
        inventory
            .set_stack(0, ItemStack::new(6, &Item::STONE))
            .await;

        // `quick_move` ignores the player argument for this handler.
        let player_inventory = Arc::new(PlayerInventory::new(
            Arc::new(Mutex::new(EntityEquipment::new())),
            Arc::new(HashMap::new()),
        ));
        let player = crate::shulker_box_screen_handler::tests::TestPlayer {
            inventory: player_inventory,
        };
        handler.quick_move(&player, 0).await;

        assert!(inventory.get_stack(0).await.is_empty());
        let last = handler.get_behaviour().slots.len() - 1;
        assert_eq!(
            handler.get_behaviour().slots[last]
                .get_stack()
                .await
                .item_count,
            6
        );
    }

    #[tokio::test]
    async fn quick_move_from_player_reaches_container() {
        let (mut handler, inventory) = handler().await;
        let player_slot = SHULKER_BOX_SIZE as i32;
        handler.get_behaviour().slots[player_slot as usize]
            .set_stack(ItemStack::new(3, &Item::STONE))
            .await;

        let player_inventory = Arc::new(PlayerInventory::new(
            Arc::new(Mutex::new(EntityEquipment::new())),
            Arc::new(HashMap::new()),
        ));
        let player = crate::shulker_box_screen_handler::tests::TestPlayer {
            inventory: player_inventory,
        };
        handler.quick_move(&player, player_slot).await;

        assert_eq!(inventory.get_stack(0).await.item_count, 3);
    }

    pub struct TestPlayer {
        pub inventory: Arc<PlayerInventory>,
    }

    impl InventoryPlayer for TestPlayer {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn drop_item(
            &self,
            _item: ItemStack,
            _retain_ownership: bool,
        ) -> crate::screen_handler::PlayerFuture<'_, ()> {
            Box::pin(async {})
        }
        fn get_inventory(&self) -> Arc<PlayerInventory> {
            self.inventory.clone()
        }
        fn play_sound(
            &self,
            _sound: pumpkin_data::sound::Sound,
        ) -> crate::screen_handler::PlayerFuture<'_, ()> {
            Box::pin(async {})
        }
        fn has_infinite_materials(&self) -> bool {
            false
        }
        fn is_creative(&self) -> bool {
            false
        }
        fn experience_level(&self) -> i32 {
            0
        }
        fn add_experience_levels(
            &self,
            _levels: i32,
        ) -> crate::screen_handler::PlayerFuture<'_, ()> {
            Box::pin(async {})
        }
        fn enchantment_seed(&self) -> i32 {
            0
        }
        fn set_enchantment_seed(&self, _seed: i32) -> crate::screen_handler::PlayerFuture<'_, ()> {
            Box::pin(async {})
        }
        fn enqueue_inventory_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetContainerContent,
            _window_type: Option<WindowType>,
        ) -> crate::screen_handler::PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_slot_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetContainerSlot,
            _window_type: Option<WindowType>,
            _total_slots: usize,
        ) -> crate::screen_handler::PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_cursor_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetCursorItem,
        ) -> crate::screen_handler::PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_property_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetContainerProperty,
        ) -> crate::screen_handler::PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_slot_set_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetPlayerInventory,
        ) -> crate::screen_handler::PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_set_held_item_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetSelectedSlot,
        ) -> crate::screen_handler::PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_equipment_change<'a>(
            &'a self,
            _slot: &'a pumpkin_data::data_component_impl::EquipmentSlot,
            _stack: &'a ItemStack,
        ) -> crate::screen_handler::PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn award_experience(&self, _amount: i32) -> crate::screen_handler::PlayerFuture<'_, ()> {
            Box::pin(async {})
        }
        fn increment_stat(
            &self,
            _category: pumpkin_data::statistic::StatisticCategory,
            _stat_id: i32,
            _amount: i32,
        ) -> crate::screen_handler::PlayerFuture<'_, ()> {
            Box::pin(async {})
        }
    }
}
