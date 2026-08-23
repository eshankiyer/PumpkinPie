use std::{any::Any, pin::Pin, sync::Arc};

use pumpkin_data::{item_stack::ItemStack, screen::WindowType};
use pumpkin_world::{block::entities::PropertyDelegate, inventory::Inventory};

use crate::{
    player::player_inventory::PlayerInventory,
    screen_handler::{
        InventoryPlayer, ItemStackFuture, ScreenHandler, ScreenHandlerBehaviour,
        ScreenHandlerFuture, ScreenProperty,
    },
    slot::BeaconPaymentSlot,
};

/// `BeaconMenu.INV_SLOT_START` (`BeaconMenu.java:21`).
const INV_SLOT_START: i32 = 1;
/// `BeaconMenu.INV_SLOT_END` (`BeaconMenu.java:22`).
const INV_SLOT_END: i32 = 28;
/// `BeaconMenu.USE_ROW_SLOT_START` (`BeaconMenu.java:23`).
const USE_ROW_SLOT_START: i32 = 28;
/// `BeaconMenu.USE_ROW_SLOT_END` (`BeaconMenu.java:24`).
const USE_ROW_SLOT_END: i32 = 37;

/// Creates a beacon container screen handler.
///
/// Beacons feature a single payment slot and a specialized UI for selecting status effects.
pub async fn create_beacon_handler(
    sync_id: u8,
    player_inventory: &Arc<PlayerInventory>,
    inventory: Arc<dyn Inventory>,
    property_delegate: Arc<dyn PropertyDelegate>,
) -> BeaconScreenHandler {
    BeaconScreenHandler::new(sync_id, player_inventory, inventory, property_delegate).await
}

/// Screen handler specifically for Beacon blocks.
pub struct BeaconScreenHandler {
    /// The beacon's inventory (contains exactly 1 slot for payment).
    pub inventory: Arc<dyn Inventory>,
    /// Core screen handler behavior (slots, sync ID, listeners).
    behaviour: ScreenHandlerBehaviour,
    /// Delegate for the levels/primary/secondary properties synced to the client.
    _property_delegate: Arc<dyn PropertyDelegate>,
}

impl BeaconScreenHandler {
    /// Creates a new beacon screen handler.
    async fn new(
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        inventory: Arc<dyn Inventory>,
        property_delegate: Arc<dyn PropertyDelegate>,
    ) -> Self {
        struct BeaconScreenListener;
        impl crate::screen_handler::ScreenHandlerListener for BeaconScreenListener {
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
            inventory: inventory.clone(),
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Beacon)),
            _property_delegate: property_delegate.clone(),
        };

        inventory.on_open().await;

        // Levels (index 0), primary effect (index 1), secondary effect (index 2)
        handler.add_property(ScreenProperty::new(property_delegate.clone(), 0));
        handler.add_property(ScreenProperty::new(property_delegate.clone(), 1));
        handler.add_property(ScreenProperty::new(property_delegate.clone(), 2));

        handler.add_listener(Arc::new(BeaconScreenListener)).await;

        // Add the single payment slot for the beacon (slot 0)
        handler.add_slot(Arc::new(BeaconPaymentSlot::new(
            handler.inventory.clone(),
            0,
        )));

        // Add the player's inventory slots (27 slots + 9 hotbar)
        let player_inventory_arc: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inventory_arc);

        handler
    }

    /// `BeaconMenu.hasPayment` (`BeaconMenu.java:162-164`).
    pub async fn has_payment(&self) -> bool {
        !self.inventory.get_stack(0).await.is_empty()
    }
}

impl ScreenHandler for BeaconScreenHandler {
    /// Port of `BeaconMenu.java:68-70`: the block at the opening position must still be
    /// `Blocks.BEACON` and the player must still be within
    /// `blockInteractionRange() + 4.0` (`AbstractContainerMenu.java:93-95`).
    fn container_access(&self) -> crate::screen_handler::ContainerAccess {
        crate::screen_handler::ContainerAccess::Block(|block| {
            block.id == pumpkin_data::Block::BEACON.id
        })
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

    /// `BeaconMenu.removed` (`BeaconMenu.java:56-65`): unlike a block-entity container
    /// (chest, barrel, ...) the payment slot's contents are not left in the beacon when the
    /// GUI closes -- they are dropped back to the player, since payment is only actually
    /// spent through `updateEffects`.
    fn on_closed<'a>(&'a mut self, player: &'a dyn InventoryPlayer) -> ScreenHandlerFuture<'a, ()> {
        Box::pin(async move {
            self.default_on_closed(player).await;
            self.drop_inventory(player, self.inventory.clone()).await;
            self.inventory.on_close().await;
        })
    }

    /// `BeaconMenu.quickMoveStack` (`BeaconMenu.java:79-121`).
    fn quick_move<'a>(
        &'a mut self,
        _player: &'a dyn InventoryPlayer,
        slot_index: i32,
    ) -> ItemStackFuture<'a> {
        Box::pin(async move {
            let mut stack_left = ItemStack::EMPTY.clone();
            let slot = self.get_behaviour().slots[slot_index as usize].clone();
            let total_slots = self.get_behaviour().slots.len() as i32;

            if slot.has_stack().await {
                let mut slot_stack = slot.get_stack().await;
                stack_left = slot_stack.clone();

                if slot_index == 0 {
                    // `slotIndex == 0`: move out of the payment slot into the full player
                    // inventory (`BeaconMenu.java:86`).
                    if !self
                        .insert_item(&mut slot_stack, 1, total_slots, true)
                        .await
                    {
                        return ItemStack::EMPTY.clone();
                    }
                } else {
                    let payment_slot = self.get_behaviour().slots[0].clone();
                    let payment_empty = !payment_slot.has_stack().await;
                    let may_pay = payment_slot.can_insert(&slot_stack).await;
                    if payment_empty && may_pay && slot_stack.item_count == 1 {
                        // Eligible payment item, and the payment slot is free: offer it there
                        // first (`BeaconMenu.java:91-94`).
                        if !self.insert_item(&mut slot_stack, 0, 1, false).await {
                            return ItemStack::EMPTY.clone();
                        }
                    } else if (INV_SLOT_START..INV_SLOT_END).contains(&slot_index) {
                        // From the main inventory: shift into the hotbar (`BeaconMenu.java:95-98`).
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
                    } else if (USE_ROW_SLOT_START..USE_ROW_SLOT_END).contains(&slot_index) {
                        // From the hotbar: shift into the main inventory (`BeaconMenu.java:99-102`).
                        if !self
                            .insert_item(&mut slot_stack, INV_SLOT_START, INV_SLOT_END, false)
                            .await
                        {
                            return ItemStack::EMPTY.clone();
                        }
                    } else if !self
                        .insert_item(&mut slot_stack, 1, total_slots, false)
                        .await
                    {
                        // Fallback: anywhere in the player inventory (`BeaconMenu.java:103-105`).
                        return ItemStack::EMPTY.clone();
                    }
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
