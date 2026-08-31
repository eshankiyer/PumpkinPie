//! Screen handler module.
//!
//! This module defines the core screen handler system for container UIs.
//! A screen handler manages the server-side state of a container interface,
//! handling slot layout, click processing, item transfer, and synchronization
//! with the client.
//!
//! # Core Components
//!
//! - [`ScreenHandler`] - The main trait for container screen handlers
//! - [`ScreenHandlerBehaviour`] - Shared state for all screen handlers
//! - [`InventoryPlayer`] - Interface for player interactions with containers
//! - [`ScreenProperty`] - Container UI properties (progress bars, etc.)
//!
//! # Screen Handler Lifecycle
//!
//! 1. Creation - Screen handler is created with slots and sync ID
//! 2. Opening - Player opens the container, sync handler attaches
//! 3. Interaction - Click packets are processed, items move between slots
//! 4. Closing - Container closes, cursor item is dropped/given to player
//!
//! # Slot Indexing
//!
//! Slots are indexed from 0 within each screen handler. Special values:
//! - `-1` - Cursor slot (held item)
//! - `-999` - Outside inventory (drop to world)

use crate::{
    container_click::MouseClick,
    player::player_inventory::PlayerInventory,
    slot::{NormalSlot, Slot},
    sync_handler::{SyncHandler, TrackedStack},
};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{
    Block, Enchantment,
    data_component_impl::{EquipmentSlot, EquipmentType, EquippableImpl},
    screen::WindowType,
    sound::Sound,
    statistic::StatisticCategory,
};
use pumpkin_protocol::{
    codec::item_stack_seralizer::OptionalItemStackHash,
    java::{
        client::play::{
            CSetContainerContent, CSetContainerProperty, CSetContainerSlot, CSetCursorItem,
            CSetPlayerInventory, CSetSelectedSlot,
        },
        server::play::SlotActionType,
    },
};
use pumpkin_util::text::TextComponent;
use pumpkin_world::{
    block::entities::PropertyDelegate,
    inventory::{ComparableInventory, Inventory},
};
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{any::Any, collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use tracing::warn;

/// Slot index indicating a click outside the inventory.
const SLOT_INDEX_OUTSIDE: i32 = -999;

fn bundle_secondary_click_applies(cursor_stack: &ItemStack, slot_stack: &ItemStack) -> bool {
    // `BundleItem` only overrides secondary clicks when the other stack is empty
    // (`BundleItem.java:59-136`).
    (cursor_stack.is_empty()
        && slot_stack
            .get_data_component::<pumpkin_data::data_component_impl::BundleContentsImpl>()
            .is_some())
        || (slot_stack.is_empty()
            && cursor_stack
                .get_data_component::<pumpkin_data::data_component_impl::BundleContentsImpl>()
                .is_some())
}

/// A tracked property for container UI elements.
///
/// Properties are used to synchronize UI state like furnace progress bars,
/// enchantment levels, and other visual indicators between server and client.
pub struct ScreenProperty {
    old_value: i32,
    index: u8,
    value: Arc<dyn PropertyDelegate>,
}

impl ScreenProperty {
    /// Creates a new screen property.
    ///
    /// # Arguments
    /// - `value` - The property delegate that holds the actual value
    /// - `index` - The property index for multi-value delegates
    pub fn new(value: Arc<dyn PropertyDelegate>, index: u8) -> Self {
        Self {
            old_value: value.get_property(i32::from(index)),
            index,
            value,
        }
    }

    /// Gets the current property value.
    #[must_use]
    pub fn get(&self) -> i32 {
        self.value.get_property(i32::from(self.index))
    }

    /// Sets the property value.
    pub fn set(&mut self, value: i32) {
        self.value.set_property(i32::from(self.index), value);
    }

    /// Checks if the value has changed since the last check.
    ///
    /// Updates the old value to the current value.
    pub fn has_changed(&mut self) -> bool {
        let value = self.get();
        let has_changed = !value.eq(&self.old_value);
        self.old_value = value;
        has_changed
    }
}

/// Type alias for async player operations.
/// Type alias for async player operations.
pub type PlayerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Interface for player interactions with containers.
///
/// This trait abstracts the player's ability to:
/// - Drop items into the world
/// - Receive inventory packets
/// - Change equipment
/// - Receive experience
///
/// Implementors are typically player entities that can open containers.
/// Server-side analogue of vanilla's `ContainerLevelAccess` predicate
/// (`net/minecraft/world/inventory/ContainerLevelAccess.java:9-37`), describing what
/// still has to hold at the position a menu was opened against for the menu to stay
/// usable.
///
/// Vanilla splits menu validation into two families:
///
/// * `AbstractContainerMenu.stillValid(ContainerLevelAccess, Player, Block)`
///   (`AbstractContainerMenu.java:93-95`) - the block at the opening position must
///   still be the expected block, and the player must still be within
///   `blockInteractionRange() + 4.0` of it (`Player.java:2014-2016`). Crafting table,
///   enchanting table, beacon, grindstone, loom, stonecutter, cartography table and
///   the `ItemCombinerMenu` subclasses (`ItemCombinerMenu.java:110-112`) use this.
/// * `Container.stillValidBlockEntity` (`Container.java:94-101`) - the block entity at
///   the opening position must still be the same object, plus the same range check.
///   Chest, furnace family, brewing stand, hopper, shulker box, crafter and lectern use
///   this via `container.stillValid(player)`.
///
/// Pumpkin has no stable block-entity identity to compare against here, so the second
/// family is modelled as [`ContainerAccess::RangeOnly`]; the "block entity replaced"
/// half is covered separately by `World::close_container_screens_at`.
#[derive(Clone, Copy)]
pub enum ContainerAccess {
    /// The menu has no backing world position and is never invalidated by movement
    /// (the player's own inventory, merchant menus, plugin GUIs). Equivalent to
    /// `ContainerLevelAccess.NULL`, whose `evaluate` yields `Optional.empty()` and so
    /// falls through to `stillValid`'s `.orElse(true)`
    /// (`ContainerLevelAccess.java:10-15`, `AbstractContainerMenu.java:93-95`).
    None,
    /// Range check only, mirroring `Container.stillValidBlockEntity`
    /// (`Container.java:94-101`).
    RangeOnly,
    /// Range check plus a predicate on the block currently at the opening position,
    /// mirroring `AbstractContainerMenu.stillValid`'s `state.is(block)` test
    /// (`AbstractContainerMenu.java:93-95`) and `ItemCombinerMenu.isValidBlock`
    /// (`ItemCombinerMenu.java:32`, `AnvilMenu.java:65-67`, `SmithingMenu.java:66-68`).
    Block(fn(&Block) -> bool),
}

impl ContainerAccess {
    /// Whether this access has a backing world position at all. `false` corresponds to
    /// `ContainerLevelAccess.NULL` (`ContainerLevelAccess.java:10-15`), which makes
    /// `stillValid` fall through to `.orElse(true)`.
    #[must_use]
    pub const fn requires_position(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Applies the block half of `AbstractContainerMenu.stillValid`
    /// (`AbstractContainerMenu.java:93-95`) to the block currently at the opening
    /// position. Accesses with no block requirement accept anything.
    #[must_use]
    pub fn accepts_block(self, block: &Block) -> bool {
        match self {
            Self::None | Self::RangeOnly => true,
            Self::Block(predicate) => predicate(block),
        }
    }
}

pub trait InventoryPlayer: Send + Sync {
    fn as_any(&self) -> &dyn std::any::Any;

    /// Evaluates a [`ContainerAccess`] against the position this player's currently
    /// open menu was opened at.
    ///
    /// This is the `ContainerLevelAccess.evaluate` half of vanilla's
    /// `AbstractContainerMenu.stillValid(access, player, block)`
    /// (`AbstractContainerMenu.java:93-95`): the implementor supplies the world and the
    /// opening position, applies the caller's block predicate to the block currently
    /// there, and range-checks the player with
    /// `Player.isWithinBlockInteractionRange(pos, 4.0)` (`Player.java:2014-2016`,
    /// i.e. squared distance from the eye position to the block's box below
    /// `(blockInteractionRange() + 4.0)^2`).
    ///
    /// Implementations with no opening position must return `true`, matching
    /// `ContainerLevelAccess.NULL` falling through to `.orElse(true)`
    /// (`ContainerLevelAccess.java:10-15`).
    ///
    /// The default returns `true` so that test doubles need not model a world; the
    /// real server player overrides it.
    fn evaluate_container_access(&self, access: ContainerAccess) -> bool {
        let _ = access;
        true
    }
    /// Drops an item into the world.
    ///
    /// # Arguments
    /// - `item` - The item to drop
    /// - `retain_ownership` - If true, the player keeps ownership (for pickup delay)
    fn drop_item(&self, item: ItemStack, retain_ownership: bool) -> PlayerFuture<'_, ()>;

    /// Gets the player's inventory.
    fn get_inventory(&self) -> Arc<PlayerInventory>;

    /// Plays a sound at the player's own position, broadcast to nearby players.
    fn play_sound(&self, sound: Sound) -> PlayerFuture<'_, ()>;

    /// Checks if the player has infinite materials (creative mode).
    fn has_infinite_materials(&self) -> bool;

    /// Checks if the player is in creative mode.
    fn is_creative(&self) -> bool;

    /// Gets the player's experience level.
    fn experience_level(&self) -> i32;

    /// Adds or removes experience levels.
    fn add_experience_levels(&self, levels: i32) -> PlayerFuture<'_, ()>;

    /// Gets the player's enchantment seed.
    fn enchantment_seed(&self) -> i32;

    /// Sets the player's enchantment seed.
    fn set_enchantment_seed(&self, seed: i32) -> PlayerFuture<'_, ()>;

    /// Sends a full container content packet.
    fn enqueue_inventory_packet<'a>(
        &'a self,
        packet: &'a CSetContainerContent,
        window_type: Option<WindowType>,
    ) -> PlayerFuture<'a, ()>;

    /// Sends a single slot update packet.
    fn enqueue_slot_packet<'a>(
        &'a self,
        packet: &'a CSetContainerSlot,
        window_type: Option<WindowType>,
        total_slots: usize,
    ) -> PlayerFuture<'a, ()>;

    /// Sends a cursor item update packet.
    fn enqueue_cursor_packet<'a>(&'a self, packet: &'a CSetCursorItem) -> PlayerFuture<'a, ()>;

    /// Sends a property update packet.
    fn enqueue_property_packet<'a>(
        &'a self,
        packet: &'a CSetContainerProperty,
    ) -> PlayerFuture<'a, ()>;

    /// Sends a player inventory slot update.
    fn enqueue_slot_set_packet<'a>(
        &'a self,
        packet: &'a CSetPlayerInventory,
    ) -> PlayerFuture<'a, ()>;

    /// Sends a selected slot update.
    fn enqueue_set_held_item_packet<'a>(
        &'a self,
        packet: &'a CSetSelectedSlot,
    ) -> PlayerFuture<'a, ()>;

    /// Sends an equipment change packet.
    fn enqueue_equipment_change<'a>(
        &'a self,
        slot: &'a EquipmentSlot,
        stack: &'a ItemStack,
    ) -> PlayerFuture<'a, ()>;

    /// Awards experience points to the player (used for furnace smelting, etc.)
    fn award_experience(&self, amount: i32) -> PlayerFuture<'_, ()>;

    /// Increments a statistic for the player.
    fn increment_stat(
        &self,
        category: StatisticCategory,
        stat_id: i32,
        amount: i32,
    ) -> PlayerFuture<'_, ()>;

    /// Applies item post-processing after a crafted result has been taken.
    ///
    /// `ItemStack.onCraftedBySystem` (`ItemStack.java:727-729`) is a server-side
    /// callback. The default keeps non-world-backed test players inert.
    fn process_item_stack_after_crafting<'a>(
        &'a self,
        _stack: &'a mut ItemStack,
    ) -> PlayerFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Fires a prepare item enchant event. Returns true if cancelled.
    fn fire_prepare_item_enchant_event<'a>(
        &'a self,
        _item: &'a ItemStack,
        _level_requirements: &'a mut [i32; 3],
        _enchantment_id: &'a mut [i32; 3],
        _enchantment_level: &'a mut [i32; 3],
        _bookshelf_count: i32,
    ) -> PlayerFuture<'a, bool> {
        Box::pin(async move { false })
    }

    /// Fires an enchant item event. Returns true if cancelled.
    fn fire_enchant_item_event<'a>(
        &'a self,
        _item: &'a ItemStack,
        _option: i32,
        _exp_level_cost: i32,
        _enchantments_to_add: &'a mut Vec<(&'static Enchantment, i32)>,
    ) -> PlayerFuture<'a, bool> {
        Box::pin(async move { false })
    }
}

/// Gives a stack to the player or drops it if inventory is full.
///
/// Tries to insert the stack into the player's inventory first,
/// and drops it in the world if there's no room.
pub async fn offer_or_drop_stack(player: &dyn InventoryPlayer, stack: ItemStack) {
    // TODO: Super weird disconnect logic in vanilla, investigate this later
    player
        .get_inventory()
        .offer_or_drop_stack(stack, player)
        .await;
}

/// Type alias for async screen handler operations.
pub type ScreenHandlerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Future type that returns an `ItemStack` (used by `quick_move`).
pub type ItemStackFuture<'a> = ScreenHandlerFuture<'a, ItemStack>;

/// Future type that returns an optional slot index.
pub type OptionUsizeFuture<'a> = ScreenHandlerFuture<'a, Option<usize>>;

/// The main trait for container screen handlers.
///
/// Screen handlers manage the server-side state of container UIs like chests,
/// furnaces, crafting tables, etc. They handle:
/// - Slot layout and management
/// - Click processing
/// - Item transfer logic (shift-click)
/// - Client synchronization
///
/// # Implementation
///
/// Implementors must provide:
/// - [`get_behaviour`](ScreenHandler::get_behaviour) and [`get_behaviour_mut`](ScreenHandler::get_behaviour_mut)
/// - [`quick_move`](ScreenHandler::quick_move) for shift-click behavior
/// - [`as_any`](ScreenHandler::as_any) for downcasting
// ScreenHandler.java
// TODO: Fully implement this
pub trait ScreenHandler: Send + Sync {
    // --- Synchronous Methods ---

    /// Gets the window type for this screen handler.
    fn window_type(&self) -> Option<WindowType> {
        self.get_behaviour().window_type
    }

    /// Returns this screen handler as an Any reference.
    fn as_any(&self) -> &dyn Any;

    /// Returns this screen handler as a mutable Any reference.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Gets the sync ID for this screen handler.
    fn sync_id(&self) -> u8 {
        self.get_behaviour().sync_id
    }

    /// What must still hold at the position this menu was opened against.
    ///
    /// Vanilla equivalent: the `ContainerLevelAccess`/`Block` pair a menu passes to
    /// `AbstractContainerMenu.stillValid` (`AbstractContainerMenu.java:93-95`), or the
    /// backing container for the `Container.stillValidBlockEntity` family
    /// (`Container.java:94-101`). Defaults to [`ContainerAccess::None`], i.e. a menu
    /// with no backing position.
    fn container_access(&self) -> ContainerAccess {
        ContainerAccess::None
    }

    /// Checks if the player can still use this container.
    ///
    /// Port of `AbstractContainerMenu.stillValid(Player)`
    /// (`AbstractContainerMenu.java:635`). The default routes
    /// [`ScreenHandler::container_access`] through the player, so a menu stays valid
    /// only while the player is in range of - and the expected block still stands at -
    /// the position the menu was opened against.
    fn can_use(&self, player: &dyn InventoryPlayer) -> bool {
        player.evaluate_container_access(self.container_access())
    }

    /// Gets a reference to the screen handler behaviour.
    fn get_behaviour(&self) -> &ScreenHandlerBehaviour;

    /// Gets a mutable reference to the screen handler behaviour.
    fn get_behaviour_mut(&mut self) -> &mut ScreenHandlerBehaviour;

    /// Adds a slot to this screen handler.
    ///
    /// Assigns an ID and sets up tracking for the slot.
    fn add_slot(&mut self, slot: Arc<dyn Slot>) -> Arc<dyn Slot> {
        let behaviour = self.get_behaviour_mut();
        slot.set_id(behaviour.slots.len());
        behaviour.slots.push(slot.clone());
        behaviour.tracked_stacks.push(ItemStack::EMPTY.clone());
        behaviour.previous_tracked_stacks.push(TrackedStack::EMPTY);

        slot
    }

    /// Adds hotbar slots (0-8) from the player inventory.
    fn add_player_hotbar_slots(&mut self, player_inventory: &Arc<dyn Inventory>) {
        for i in 0..9 {
            self.add_slot(Arc::new(NormalSlot::new(player_inventory.clone(), i)));
        }
    }

    /// Adds main inventory slots (9-35) from the player inventory.
    fn add_player_inventory_slots(&mut self, player_inventory: &Arc<dyn Inventory>) {
        for i in 0..3 {
            for j in 0..9 {
                self.add_slot(Arc::new(NormalSlot::new(
                    player_inventory.clone(),
                    j + (i + 1) * 9,
                )));
            }
        }
    }

    /// Adds all player inventory slots (main + hotbar).
    fn add_player_slots(&mut self, player_inventory: &Arc<dyn Inventory>) {
        self.add_player_inventory_slots(player_inventory);
        self.add_player_hotbar_slots(player_inventory);
    }

    /// Records a received hash for a slot (for sync tracking).
    fn set_received_hash(&mut self, slot: usize, hash: OptionalItemStackHash) {
        let behaviour = self.get_behaviour_mut();
        if slot < behaviour.previous_tracked_stacks.len() {
            behaviour.previous_tracked_stacks[slot].set_received_hash(hash);
        } else {
            warn!(
                "Incorrect slot index: {} available slots: {}",
                slot,
                behaviour.previous_tracked_stacks.len()
            );
        }
    }

    /// Records a received stack for a slot (for sync tracking).
    fn set_received_stack(&mut self, slot: usize, stack: ItemStack) {
        let behaviour = self.get_behaviour_mut();
        behaviour.previous_tracked_stacks[slot].set_received_stack(stack);
    }

    /// Records a received cursor hash (for sync tracking).
    fn set_received_cursor_hash(&mut self, hash: OptionalItemStackHash) {
        let behaviour = self.get_behaviour_mut();
        behaviour.previous_cursor_stack.set_received_hash(hash);
    }

    /// Adds a property to track.
    fn add_property(&mut self, property: ScreenProperty) {
        let behaviour = self.get_behaviour_mut();
        behaviour.properties.push(property);
        behaviour.tracked_property_values.push(0);
    }

    /// Adds multiple properties to track.
    fn add_properties(&mut self, properties: Vec<ScreenProperty>) {
        for property in properties {
            self.add_property(property);
        }
    }

    // --- Asynchronous Methods ---

    /// Called when the container is closed by the player.
    ///
    /// Default implementation drops the cursor item.
    fn on_closed<'a>(&'a mut self, player: &'a dyn InventoryPlayer) -> ScreenHandlerFuture<'a, ()> {
        Box::pin(async move {
            self.default_on_closed(player).await;
        })
    }

    /// Default close behavior - drops the cursor item.
    fn default_on_closed<'a>(
        &'a mut self,
        player: &'a dyn InventoryPlayer,
    ) -> ScreenHandlerFuture<'a, ()> {
        Box::pin(async move {
            let behaviour = self.get_behaviour_mut();

            // Lock and clone are performed inside the async block
            let mut cursor_stack_lock = behaviour.cursor_stack.lock().await;

            if !cursor_stack_lock.is_empty() {
                offer_or_drop_stack(player, cursor_stack_lock.clone()).await;
                *cursor_stack_lock = ItemStack::EMPTY.clone();
            }
        })
    }

    /// Mirrors `AbstractContainerMenu.canTakeItemForPickAll`'s per-menu hook
    /// (`AbstractContainerMenu.java:534-545,583-585`).
    fn can_take_item_for_pick_all(&self, _carried: &ItemStack, _target: &dyn Slot) -> bool {
        true
    }

    /// Drops all items from an inventory into the world.
    fn drop_inventory<'a>(
        &'a self,
        player: &'a dyn InventoryPlayer,
        inventory: Arc<dyn Inventory>,
    ) -> ScreenHandlerFuture<'a, ()> {
        Box::pin(async move {
            for i in 0..inventory.size() {
                offer_or_drop_stack(player, inventory.remove_stack(i).await).await;
            }
        })
    }

    /// Copies tracked slot state from another screen handler.
    ///
    /// Used when reopening a container to restore previous state.
    fn copy_shared_slots(
        &mut self,
        other: Arc<Mutex<dyn ScreenHandler>>,
    ) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            let mut table: HashMap<ComparableInventory, HashMap<usize, usize>> = HashMap::new();
            let other_binding = other.lock().await;
            let other_behaviour = other_binding.get_behaviour();

            for i in 0..other_behaviour.slots.len() {
                let other_slot = other_behaviour.slots[i].clone();
                let mut hash_map = HashMap::new();
                hash_map.insert(other_slot.get_index(), i);
                table.insert(
                    ComparableInventory(other_slot.get_inventory().clone()),
                    hash_map,
                );
            }

            for i in 0..self.get_behaviour().slots.len() {
                let slot = self.get_behaviour().slots[i].clone();
                let inventory = slot.get_inventory();
                let index = slot.get_index();

                if let Some(hash_map) = table.get(&ComparableInventory(inventory.clone()))
                    && let Some(other_index) = hash_map.get(&index)
                {
                    self.get_behaviour_mut().tracked_stacks[i] =
                        other_behaviour.tracked_stacks[*other_index].clone();
                    self.get_behaviour_mut().previous_tracked_stacks[i] =
                        other_behaviour.previous_tracked_stacks[*other_index].clone();
                }
            }
        })
    }

    /// Synchronizes the full state to the client.
    ///
    /// Captures current slot states and sends a full update packet.
    fn sync_state(&mut self) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            let behaviour = self.get_behaviour_mut();
            let mut previous_tracked_stacks = Vec::new();

            for i in 0..behaviour.slots.len() {
                let stack = behaviour.slots[i].get_cloned_stack().await;
                previous_tracked_stacks.push(stack.clone());
                behaviour.previous_tracked_stacks[i].set_received_stack(stack);
            }

            let cursor_stack = behaviour.cursor_stack.lock().await.clone();
            behaviour
                .previous_cursor_stack
                .set_received_stack(cursor_stack.clone());

            for i in 0..behaviour.properties.len() {
                let property_val = behaviour.properties[i].get();
                behaviour.tracked_property_values[i] = property_val;
            }

            let next_revision = behaviour.next_revision();

            if let Some(sync_handler) = behaviour.sync_handler.as_ref() {
                sync_handler
                    .update_state(
                        behaviour,
                        &previous_tracked_stacks,
                        &cursor_stack,
                        behaviour.tracked_property_values.clone(),
                        next_revision,
                    )
                    .await;
            }
        })
    }

    /// Adds a listener for slot and property changes.
    fn add_listener(
        &mut self,
        listener: Arc<dyn ScreenHandlerListener>,
    ) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            self.get_behaviour_mut().listeners.push(listener);
            self.send_content_updates().await;
        })
    }

    /// Attaches a sync handler and performs initial sync.
    fn update_sync_handler(
        &mut self,
        sync_handler: Arc<SyncHandler>,
    ) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            let behaviour = self.get_behaviour_mut();
            behaviour.sync_handler = Some(sync_handler.clone());
            self.sync_state().await;
        })
    }

    /// Sends all updates to the client.
    ///
    /// Updates tracked slots and properties.
    fn update_to_client(&mut self) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            for i in 0..self.get_behaviour().slots.len() {
                let behaviour = self.get_behaviour_mut();
                let slot = behaviour.slots[i].clone();
                let stack = slot.get_cloned_stack().await;
                self.update_tracked_slot(i, stack).await;
            }

            let behaviour = self.get_behaviour_mut();
            let mut prop_vec = vec![];
            for (idx, prop) in behaviour.properties.iter_mut().enumerate() {
                let value = prop.get();
                if prop.has_changed() {
                    prop_vec.push((idx, value));
                }
            }

            for (idx, value) in prop_vec {
                self.update_tracked_properties(idx as i32, value).await;
                self.check_property_updates(idx as i32, value).await;
            }

            self.sync_state().await;
        })
    }

    /// Updates a tracked property value.
    fn update_tracked_properties(&mut self, idx: i32, value: i32) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            let behaviour = self.get_behaviour_mut();
            if idx <= behaviour.tracked_property_values.len() as i32 {
                behaviour.tracked_property_values[idx as usize] = value;
                for listener in &behaviour.listeners {
                    listener
                        .on_property_update(behaviour, idx as u8, value)
                        .await;
                }
            }
        })
    }

    /// Checks if a property needs to be synced to the client.
    fn check_property_updates(&mut self, idx: i32, value: i32) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            let behaviour = self.get_behaviour_mut();
            if !behaviour.disable_sync
                && let Some(old_value) = behaviour.tracked_property_values.get(idx as usize)
            {
                let old_value = *old_value;
                if old_value != value {
                    behaviour
                        .tracked_property_values
                        .insert(idx as usize, value);
                    if let Some(ref sync_handler) = behaviour.sync_handler {
                        sync_handler.update_property(behaviour, idx, value).await;
                    }
                }
            }
        })
    }

    /// Updates the tracked state of a slot.
    fn update_tracked_slot(
        &mut self,
        slot: usize,
        stack: ItemStack,
    ) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            let behaviour = self.get_behaviour_mut();
            let other_stack = &behaviour.tracked_stacks[slot];
            if !other_stack.are_equal(&stack) {
                behaviour.tracked_stacks[slot] = stack.clone();

                for listener in &behaviour.listeners {
                    listener
                        .on_slot_update(behaviour, slot as u8, stack.clone())
                        .await;
                }
            }
        })
    }

    /// Checks if a slot needs to be synced to the client.
    fn check_slot_updates(&mut self, slot: usize, stack: ItemStack) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            let behaviour = self.get_behaviour_mut();
            if !behaviour.disable_sync {
                let prev_stack = &mut behaviour.previous_tracked_stacks[slot];

                if !prev_stack.is_in_sync(&stack) {
                    prev_stack.set_received_stack(stack.clone());
                    let next_revision = behaviour.next_revision();
                    if let Some(sync_handler) = behaviour.sync_handler.as_ref() {
                        sync_handler
                            .update_slot(behaviour, slot, &stack, next_revision)
                            .await;
                    }
                }
            }
        })
    }

    /// Checks if the cursor stack needs to be synced.
    fn check_cursor_stack_updates(&mut self) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            let behaviour = self.get_behaviour_mut();
            if !behaviour.disable_sync {
                let cursor_stack = behaviour.cursor_stack.lock().await;
                if !behaviour.previous_cursor_stack.is_in_sync(&cursor_stack) {
                    behaviour
                        .previous_cursor_stack
                        .set_received_stack(cursor_stack.clone());
                    if let Some(sync_handler) = behaviour.sync_handler.as_ref() {
                        sync_handler
                            .update_cursor_stack(behaviour, &cursor_stack)
                            .await;
                    }
                }
            }
        })
    }

    /// Sends all content updates to listeners and sync handler.
    fn send_content_updates(&mut self) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            let slots_len = self.get_behaviour().slots.len();

            for i in 0..slots_len {
                let slot = self.get_behaviour().slots[i].clone();
                let stack = slot.get_cloned_stack().await;

                self.update_tracked_slot(i, stack.clone()).await;
                self.check_slot_updates(i, stack).await;
            }

            self.check_cursor_stack_updates().await;

            let behaviour = self.get_behaviour_mut();
            let mut prop_vec = vec![];
            for (idx, prop) in behaviour.properties.iter_mut().enumerate() {
                let value = prop.get();
                if prop.has_changed() {
                    prop_vec.push((idx, value));
                }
            }

            for (idx, value) in prop_vec {
                self.update_tracked_properties(idx as i32, value).await;
                self.check_property_updates(idx as i32, value).await;
            }
        })
    }

    /// Checks if a slot index is valid.
    fn is_slot_valid(&self, slot: i32) -> ScreenHandlerFuture<'_, bool> {
        Box::pin(async move {
            slot == -1 || slot == -999 || slot < self.get_behaviour().slots.len() as i32
        })
    }

    /// Disables synchronization (for batch operations).
    fn disable_sync(&mut self) {
        let behaviour = self.get_behaviour_mut();
        behaviour.disable_sync = true;
    }

    /// Re-enables synchronization.
    fn enable_sync(&mut self) {
        let behaviour = self.get_behaviour_mut();
        behaviour.disable_sync = false;
    }

    /// Gets the screen handler slot index for an inventory slot.
    fn get_slot_index<'a>(
        &'a self,
        inventory: &'a Arc<dyn Inventory>,
        slot: usize,
    ) -> OptionUsizeFuture<'a> {
        Box::pin(async move {
            (0..self.get_behaviour().slots.len()).find(|&i| {
                Arc::ptr_eq(&self.get_behaviour().slots[i].get_inventory(), inventory)
                    && self.get_behaviour().slots[i].get_index() == slot
            })
        })
    }

    /// Performs a quick move (shift-click) from a slot.
    ///
    /// Must be implemented by concrete screen handlers to define
    /// where items go when shift-clicked from specific slots.
    fn quick_move<'a>(
        &'a mut self,
        player: &'a dyn InventoryPlayer,
        slot_index: i32,
    ) -> ItemStackFuture<'a>;

    /// Handles a button click event (e.g., enchantment selection, beacon effects).
    fn on_button_click<'a>(
        &'a mut self,
        _player: &'a dyn InventoryPlayer,
        _button_id: i32,
    ) -> ScreenHandlerFuture<'a, bool> {
        Box::pin(async { false })
    }

    /// Inserts an item into a range of slots.
    ///
    /// First tries to stack with existing items, then fills empty slots.
    fn insert_item<'a>(
        &'a mut self,
        stack: &'a mut ItemStack,
        start_index: i32,
        end_index: i32,
        from_last: bool,
    ) -> ScreenHandlerFuture<'a, bool> {
        Box::pin(async move {
            let mut success = false;
            let mut current_index = if from_last {
                end_index - 1
            } else {
                start_index
            };

            if stack.is_stackable() {
                while !stack.is_empty()
                    && (if from_last {
                        current_index >= start_index
                    } else {
                        current_index < end_index
                    })
                {
                    let slot = self.get_behaviour().slots[current_index as usize].clone();
                    let mut slot_stack = slot.get_stack().await;

                    if !slot_stack.is_empty() && slot_stack.are_items_and_components_equal(stack) {
                        let combined_count = slot_stack.item_count + stack.item_count;
                        let max_slot_count = slot.get_max_item_count_for_stack(&slot_stack).await;
                        if combined_count <= max_slot_count {
                            stack.set_count(0);
                            slot_stack.set_count(combined_count);
                            slot.set_stack(slot_stack).await;
                            success = true;
                        } else if slot_stack.item_count < max_slot_count {
                            stack.decrement(max_slot_count - slot_stack.item_count);
                            slot_stack.set_count(max_slot_count);
                            slot.set_stack(slot_stack).await;
                            success = true;
                        }
                    }

                    if from_last {
                        current_index -= 1;
                    } else {
                        current_index += 1;
                    }
                }
            }

            if !stack.is_empty() {
                if from_last {
                    current_index = end_index - 1;
                } else {
                    current_index = start_index;
                }

                while if from_last {
                    current_index >= start_index
                } else {
                    current_index < end_index
                } {
                    let slot = self.get_behaviour().slots[current_index as usize].clone();
                    let slot_stack = slot.get_stack().await;

                    if slot_stack.is_empty() && slot.can_insert(stack).await {
                        let max_count = slot.get_max_item_count_for_stack(stack).await;
                        slot.set_stack(stack.split(max_count.min(stack.item_count)))
                            .await;
                        slot.mark_dirty().await;
                        success = true;
                        break;
                    }

                    if from_last {
                        current_index -= 1;
                    } else {
                        current_index += 1;
                    }
                }
            }

            success
        })
    }

    /// Handles a slot click event.
    ///
    /// Override for custom click handling. Return true to prevent default handling.
    fn handle_slot_click<'a>(
        &'a self,
        _player: &'a dyn InventoryPlayer,
        _click_type: MouseClick,
        _slot: Arc<dyn Slot>,
        _slot_stack: ItemStack,
        _cursor_stack: ItemStack,
    ) -> ScreenHandlerFuture<'a, bool> {
        Box::pin(async {
            // TODO: required for bundle in the future
            false
        })
    }

    /// Cancels any client-side changes and resynchronizes the state.
    fn cancel(&mut self) -> ScreenHandlerFuture<'_, ()> {
        Box::pin(async move {
            self.sync_state().await;
        })
    }

    /// Public entry point for slot click handling.
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

    /// Internal slot click handling implementation.
    ///
    /// Handles all click types: pickup, quick move, swap, throw, drag, clone.
    #[expect(clippy::too_many_lines)]
    fn internal_on_slot_click<'a>(
        &'a mut self,
        slot_index: i32,
        button: i32,
        action_type: SlotActionType,
        player: &'a dyn InventoryPlayer,
    ) -> ScreenHandlerFuture<'a, ()> {
        Box::pin(async move {
            // Vanilla pickup-all checks the menu hook for every target
            // (`AbstractContainerMenu.java:534-545`).
            if action_type == SlotActionType::PickupAll && button == 0 {
                let slots = self.get_behaviour().slots.clone();
                let cursor_stack_handle = self.get_behaviour().cursor_stack.clone();
                let mut cursor_stack = cursor_stack_handle.lock().await;
                let mut to_pick_up = cursor_stack.get_max_stack_size() - cursor_stack.item_count;

                for slot in &slots {
                    if to_pick_up == 0 {
                        break;
                    }

                    if !self.can_take_item_for_pick_all(&cursor_stack, slot.as_ref()) {
                        continue;
                    }

                    let item_stack = slot.get_cloned_stack().await;
                    if !item_stack.are_items_and_components_equal(&cursor_stack) {
                        continue;
                    }

                    if !slot.allow_modification(player).await {
                        continue;
                    }

                    let taken_stack = slot
                        .safe_take(
                            item_stack.item_count.min(to_pick_up),
                            cursor_stack.get_max_stack_size() - cursor_stack.item_count,
                            player,
                        )
                        .await;
                    to_pick_up -= taken_stack.item_count;
                    cursor_stack.increment(taken_stack.item_count);
                }
            } else if action_type == SlotActionType::QuickCraft {
                let drag_type = button & 3;
                let drag_button = (button >> 2) & 3;
                let behaviour = self.get_behaviour_mut();
                if drag_type == 0 {
                    behaviour.drag_slots.clear();
                } else if drag_type == 1 {
                    if slot_index < 0 {
                        warn!("Invalid slot index for drag action: {slot_index}. Must be >= 0");
                        return;
                    }
                    let cursor_stack = behaviour.cursor_stack.lock().await;

                    let slot = &behaviour.slots[slot_index as usize];
                    let stack = slot.get_stack().await;
                    if !cursor_stack.is_empty()
                        && slot.can_insert(&cursor_stack).await
                        && (stack.are_items_and_components_equal(&cursor_stack) || stack.is_empty())
                        && slot.get_max_item_count_for_stack(&stack).await > stack.item_count
                    {
                        behaviour.drag_slots.push(slot_index as u32);
                    }
                } else if drag_type == 2 && !behaviour.drag_slots.is_empty() {
                    // process drag end
                    if behaviour.drag_slots.len() == 1 {
                        let slot = behaviour.drag_slots[0] as i32;
                        behaviour.drag_slots.clear();
                        let _ = behaviour;
                        self.internal_on_slot_click(
                            slot,
                            drag_button,
                            SlotActionType::Pickup,
                            player,
                        )
                        .await;

                        return;
                    }
                    if drag_button == 2 && !player.has_infinite_materials() {
                        return; // Only creative
                    }

                    let mut cursor_stack = behaviour.cursor_stack.lock().await;
                    let initial_count = cursor_stack.item_count;
                    let slots_count = behaviour.drag_slots.len();
                    for slot_index in &behaviour.drag_slots {
                        let Some(slot) = behaviour.slots.get(*slot_index as usize).cloned() else {
                            continue;
                        };
                        let stack = slot.get_stack().await;

                        if (stack.are_items_and_components_equal(&cursor_stack) || stack.is_empty())
                            && slot.can_insert(&cursor_stack).await
                        {
                            let mut inserting_count = match drag_button {
                                0 => (initial_count as usize)
                                    .checked_div(slots_count)
                                    .map_or(0, |c| c as u8),
                                1 => 1,
                                2 => {
                                    cursor_stack.item_count = cursor_stack.get_max_stack_size();
                                    cursor_stack.item_count
                                }
                                _ => 0,
                            };
                            // `AbstractContainerMenu.java:384-386` caps against
                            // `min(source.getMaxStackSize(), slot.getMaxStackSize(source))`,
                            // i.e. the CARRIED stack's limit, and does the headroom
                            // subtraction on a signed int. Using the slot's own (possibly
                            // empty) stack and a `u8` subtraction both diverge: the latter
                            // underflows whenever the slot already holds more than the
                            // carried stack's max size permits.
                            let headroom = slot
                                .get_max_item_count_for_stack(&cursor_stack)
                                .await
                                .saturating_sub(stack.item_count);
                            inserting_count =
                                inserting_count.min(headroom).min(cursor_stack.item_count);
                            if inserting_count > 0 {
                                let mut new_stack = stack.clone();
                                if new_stack.is_empty() {
                                    new_stack = cursor_stack.copy_with_count(0);
                                }
                                new_stack.increment(inserting_count);
                                slot.set_stack(new_stack).await;
                                if drag_button != 2 {
                                    cursor_stack.decrement(inserting_count);
                                }
                                if cursor_stack.is_empty() {
                                    *cursor_stack = ItemStack::EMPTY.clone();
                                    break;
                                }
                            }
                        }
                    }

                    if drag_button == 2 {
                        *cursor_stack = ItemStack::EMPTY.clone();
                    }
                    behaviour.drag_slots.clear();
                }
            } else if action_type == SlotActionType::Throw {
                if slot_index >= 0 && self.get_behaviour().cursor_stack.lock().await.is_empty() {
                    let slot = self.get_behaviour().slots[slot_index as usize].clone();
                    let prev_stack = slot.get_cloned_stack().await;
                    if !prev_stack.is_empty() {
                        if button == 1 {
                            // Throw all
                            while slot
                                .get_cloned_stack()
                                .await
                                .are_items_and_components_equal(&prev_stack)
                            {
                                let drop_stack =
                                    slot.safe_take(prev_stack.item_count, u8::MAX, player).await;
                                player.drop_item(drop_stack, true).await;
                                // player.handleCreativeModeItemDrop(itemStack);
                            }
                        } else {
                            let drop_stack = slot.safe_take(1, u8::MAX, player).await;
                            if !drop_stack.is_empty() {
                                slot.on_take_item(player, &drop_stack).await;
                                player.drop_item(drop_stack, true).await;
                            }
                        }
                    }
                }
            } else if action_type == SlotActionType::Clone {
                if player.has_infinite_materials() && slot_index >= 0 {
                    let behaviour = self.get_behaviour_mut();
                    let mut cursor_stack = behaviour.cursor_stack.lock().await;
                    if !cursor_stack.is_empty() {
                        return;
                    }
                    let slot = behaviour.slots[slot_index as usize].clone();
                    // `AbstractContainerMenu` delegates creative cloning to `Slot.safeClone`
                    // (`AbstractContainerMenu.java:508-511`), which also rejects empty slots.
                    *cursor_stack = slot.safe_clone(player).await;
                }
            } else if (action_type == SlotActionType::Pickup
                || action_type == SlotActionType::QuickMove)
                && (button == 0 || button == 1)
            {
                let click_type = if button == 0 {
                    MouseClick::Left
                } else {
                    MouseClick::Right
                };

                // Drop item if outside inventory
                if slot_index == SLOT_INDEX_OUTSIDE {
                    let mut cursor_stack = self.get_behaviour().cursor_stack.lock().await;
                    if !cursor_stack.is_empty() {
                        if click_type == MouseClick::Left {
                            player.drop_item(cursor_stack.clone(), true).await;
                            *cursor_stack = ItemStack::EMPTY.clone();
                        } else {
                            player.drop_item(cursor_stack.split(1), true).await;
                        }
                    }
                } else if action_type == SlotActionType::QuickMove {
                    if slot_index < 0 {
                        return;
                    }

                    let slot = self.get_behaviour().slots[slot_index as usize].clone();

                    if !slot.can_take_items(player).await {
                        return;
                    }

                    let mut moved_stack = self.quick_move(player, slot_index).await;

                    // `AbstractContainerMenu.doClick` (AbstractContainerMenu.java:425) loops
                    // while `ItemStack.isSameItem`, which compares the item only
                    // (ItemStack.java:634-636) - not its components.
                    while !moved_stack.is_empty()
                        && slot.get_cloned_stack().await.is_same_item(&moved_stack)
                    {
                        moved_stack = self.quick_move(player, slot_index).await;
                    }
                } else {
                    // Pickup
                    if slot_index < 0 {
                        return;
                    }

                    let slot = self.get_behaviour().slots[slot_index as usize].clone();

                    if click_type == MouseClick::Left {
                        slot.on_click(player).await;
                    }

                    let slot_stack = slot.get_cloned_stack().await;
                    let mut cursor_stack = self.get_behaviour().cursor_stack.lock().await;

                    // Vanilla `BundleItem#overrideStackedOnOther`/`#overrideOtherStackedOnMe`:
                    // left-click moves as much of the *other* stack into the bundle as fits
                    // (right-click, handled below, only ever moves a single item).
                    if click_type == MouseClick::Left {
                        // Cursor holds the bundle ("self"); slot is "other". Requires the
                        // slot to be non-empty, matching `!other.isEmpty()`.
                        let intercepted = if !cursor_stack.is_empty()
                            && !slot_stack.is_empty()
                            && cursor_stack
                                .get_data_component::<pumpkin_data::data_component_impl::BundleContentsImpl>()
                                .is_some()
                        {
                            // `BundleContents.Mutable.tryTransfer` uses `Slot.safeTake`, so
                            // slot permissions and the slot's transfer callbacks must run
                            // before the extracted stack enters the bundle
                            // (`BundleContents.java:217-225`).
                            let max_amount = cursor_stack
                                .get_data_component::<pumpkin_data::data_component_impl::BundleContentsImpl>()
                                .map_or(0, |bundle| {
                                    let weight_per_item =
                                        (64 / slot_stack.get_max_stack_size() as u32).max(1);
                                    ((64u32.saturating_sub(bundle.get_weight()) / weight_per_item)
                                        .min(u32::from(slot_stack.item_count)))
                                        as u8
                                });
                            let mut inner_slot_stack = if max_amount == 0 {
                                ItemStack::EMPTY.clone()
                            } else {
                                slot.safe_take(slot_stack.item_count, max_amount, player)
                                    .await
                            };
                            let inserted = cursor_stack
                                .get_data_component_mut::<pumpkin_data::data_component_impl::BundleContentsImpl>()
                                .is_some_and(|bundle| bundle.try_insert(&mut inner_slot_stack));
                            if !inner_slot_stack.is_empty() {
                                let _ = slot.insert_stack(inner_slot_stack).await;
                            }
                            player
                                .play_sound(if inserted {
                                    Sound::ItemBundleInsert
                                } else {
                                    Sound::ItemBundleInsertFail
                                })
                                .await;
                            true
                        } else if !cursor_stack.is_empty() {
                            // Slot holds the bundle ("self"); cursor is "other".
                            let mut inner_slot_stack = slot.get_stack().await;
                            if let Some(bundle) = inner_slot_stack
                                .get_data_component_mut::<pumpkin_data::data_component_impl::BundleContentsImpl>()
                            {
                                let inserted = bundle.try_insert(&mut cursor_stack);
                                slot.set_stack(inner_slot_stack).await;
                                player
                                    .play_sound(if inserted {
                                        Sound::ItemBundleInsert
                                    } else {
                                        Sound::ItemBundleInsertFail
                                    })
                                    .await;
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        };

                        if intercepted {
                            if cursor_stack.item_count == 0 {
                                *cursor_stack = ItemStack::EMPTY.clone();
                            }
                            slot.mark_dirty().await;
                            return;
                        }
                    }

                    // Vanilla's secondary bundle overrides only run when the other side is
                    // empty (`BundleItem.java:59-136`); a non-empty opposite stack falls
                    // through to normal slot handling.
                    if click_type == MouseClick::Right
                        && bundle_secondary_click_applies(&cursor_stack, &slot_stack)
                    {
                        let mut intercepted = false;

                        if !intercepted && cursor_stack.is_empty() {
                            let mut inner_slot_stack = slot.get_stack().await;
                            if let Some(bundle) = inner_slot_stack.get_data_component_mut::<pumpkin_data::data_component_impl::BundleContentsImpl>()
                                && let Some(extracted) = bundle.try_extract() {
                                    slot.set_stack(inner_slot_stack).await;
                                    *cursor_stack = extracted;
                                    player.play_sound(Sound::ItemBundleRemoveOne).await;
                                    intercepted = true;
                                }
                        }

                        if !intercepted && slot_stack.is_empty()
                            && let Some(bundle) = cursor_stack.get_data_component_mut::<pumpkin_data::data_component_impl::BundleContentsImpl>()
                                && let Some(extracted) = bundle.try_extract() {
                                    // `BundleItem.overrideStackedOnOther` delegates the
                                    // extracted stack to `Slot.safeInsert`, preserving slot
                                    // limits and returning any remainder to the bundle
                                    // (`BundleItem.java:59-81`).
                                    let mut remainder = slot.insert_stack(extracted).await;
                                    if !remainder.is_empty()
                                        && let Some(bundle) = cursor_stack.get_data_component_mut::<pumpkin_data::data_component_impl::BundleContentsImpl>()
                                    {
                                        let _ = bundle.try_insert(&mut remainder);
                                    }
                                    player.play_sound(Sound::ItemBundleRemoveOne).await;
                                    intercepted = true;
                                }

                        if intercepted {
                            if cursor_stack.item_count == 0 {
                                *cursor_stack = ItemStack::EMPTY.clone();
                            }
                            slot.mark_dirty().await;
                            return;
                        }
                    }

                    let equipment_slot = cursor_stack
                        .get_data_component::<EquippableImpl>()
                        .map_or(&EquipmentSlot::MAIN_HAND, |equippable| equippable.slot);

                    if self
                        .handle_slot_click(
                            player,
                            click_type.clone(),
                            slot.clone(),
                            slot_stack.clone(),
                            cursor_stack.clone(),
                        )
                        .await
                    {
                        return;
                    }

                    if slot_stack.is_empty() {
                        if !cursor_stack.is_empty() {
                            if equipment_slot.slot_type() == EquipmentType::HumanoidArmor
                                && (5..9).contains(&slot_index)
                            {
                                player
                                    .enqueue_equipment_change(equipment_slot, &cursor_stack)
                                    .await;
                            }

                            let transfer_count = if click_type == MouseClick::Left {
                                cursor_stack.item_count
                            } else {
                                1
                            };
                            *cursor_stack = slot
                                .insert_stack_count(cursor_stack.clone(), transfer_count)
                                .await;
                        }
                    } else if slot.can_take_items(player).await {
                        if cursor_stack.is_empty() {
                            let take_count = if click_type == MouseClick::Left {
                                slot_stack.item_count
                            } else {
                                slot_stack.item_count.div_ceil(2)
                            };
                            let taken =
                                slot.try_take_stack_range(take_count, u8::MAX, player).await;
                            if let Some(taken) = taken {
                                // Reverse order of operations, shouldn't affect anything
                                *cursor_stack = taken.clone();
                                slot.on_take_item(player, &taken).await;

                                if (5..9).contains(&slot_index) {
                                    let equipment_slot = cursor_stack
                                        .get_data_component::<EquippableImpl>()
                                        .map_or(&EquipmentSlot::MAIN_HAND, |equippable| {
                                            equippable.slot
                                        });
                                    player
                                        .enqueue_equipment_change(equipment_slot, ItemStack::EMPTY)
                                        .await;
                                }
                            }
                        } else if slot.can_insert(&cursor_stack).await {
                            if equipment_slot.slot_type() == EquipmentType::HumanoidArmor
                                && (5..9).contains(&slot_index)
                            {
                                player
                                    .enqueue_equipment_change(equipment_slot, &cursor_stack)
                                    .await;
                            }

                            if ItemStack::are_items_and_components_equal(&slot_stack, &cursor_stack)
                            {
                                let insert_count = if click_type == MouseClick::Left {
                                    cursor_stack.item_count
                                } else {
                                    1
                                };
                                *cursor_stack = slot
                                    .insert_stack_count(cursor_stack.clone(), insert_count)
                                    .await;
                            } else if cursor_stack.item_count
                                <= slot.get_max_item_count_for_stack(&cursor_stack).await
                            {
                                let old_cursor_stack = cursor_stack.clone();
                                *cursor_stack = slot_stack.clone();
                                slot.set_stack(old_cursor_stack).await;
                            }
                        } else if ItemStack::are_items_and_components_equal(
                            &slot_stack,
                            &cursor_stack,
                        ) {
                            let taken = slot
                                .try_take_stack_range(
                                    slot_stack.item_count,
                                    cursor_stack
                                        .get_max_stack_size()
                                        .saturating_sub(cursor_stack.item_count),
                                    player,
                                )
                                .await;

                            if let Some(taken) = taken {
                                cursor_stack.increment(taken.item_count);
                                slot.on_take_item(player, &taken).await;
                            }
                        }
                    }

                    slot.mark_dirty().await;
                }
            } else if action_type == SlotActionType::Swap && (0..9).contains(&button)
                || button == 40
            {
                if slot_index < 0 {
                    return;
                }
                let mut button_stack = player.get_inventory().get_stack(button as usize).await;
                let source_slot = self.get_behaviour().slots[slot_index as usize].clone();
                let source_stack = source_slot.get_cloned_stack().await;

                if !button_stack.is_empty() || !source_stack.is_empty() {
                    if button_stack.is_empty() {
                        if source_slot.can_take_items(player).await {
                            player
                                .get_inventory()
                                .set_stack(button as usize, source_stack.clone())
                                .await;
                            source_slot.set_stack(ItemStack::EMPTY.clone()).await;
                            source_slot.on_take_item(player, &source_stack).await;
                        }
                    } else if source_stack.is_empty() && source_slot.can_insert(&button_stack).await
                    {
                        let max_count = source_slot
                            .get_max_item_count_for_stack(&button_stack)
                            .await;
                        if button_stack.item_count > max_count {
                            // button_stack might need to be a ref instead of a clone
                            source_slot.set_stack(button_stack.split(max_count)).await;
                        } else {
                            player
                                .get_inventory()
                                .set_stack(button as usize, ItemStack::EMPTY.clone())
                                .await;
                            source_slot.set_stack(button_stack).await;
                        }
                    }
                }
            }
        })
    }
}

pub trait ScreenHandlerListener: Send + Sync {
    fn on_slot_update<'a>(
        &'a self,
        _screen_handler: &'a ScreenHandlerBehaviour,
        _slot: u8,
        _stack: ItemStack,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn on_property_update<'a>(
        &'a self,
        _screen_handler: &'a ScreenHandlerBehaviour,
        _property: u8,
        _value: i32,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type SharedScreenHandler = Arc<Mutex<dyn ScreenHandler>>;

pub trait ScreenHandlerFactory: Send + Sync {
    fn create_screen_handler<'a>(
        &'a self,
        sync_id: u8,
        player_inventory: &'a Arc<PlayerInventory>,
        player: &'a dyn InventoryPlayer,
    ) -> BoxFuture<'a, Option<SharedScreenHandler>>;
    fn get_display_name(&self) -> TextComponent;
}

pub struct ScreenHandlerBehaviour {
    /// Slots in this screen handler (includes both container and player slots).
    pub slots: Vec<Arc<dyn Slot>>,
    /// Sync ID for client-server matching (matches the window ID in protocol).
    pub sync_id: u8,
    /// Registered listeners for slot/property changes.
    pub listeners: Vec<Arc<dyn ScreenHandlerListener>>,
    /// Sync handler for sending updates to the client.
    pub sync_handler: Option<Arc<SyncHandler>>,
    /// Current tracked stacks for comparison with previous state.
    //TODO: Check if this is needed
    pub tracked_stacks: Vec<ItemStack>,
    /// The item currently held by the player's cursor (held item).
    pub cursor_stack: Arc<Mutex<ItemStack>>,
    /// Previous tracked stacks for detecting changes that need syncing.
    pub previous_tracked_stacks: Vec<TrackedStack>,
    /// Previous cursor stack for detecting cursor changes.
    pub previous_cursor_stack: TrackedStack,
    /// Revision counter for sync tracking (increments on each change).
    pub revision: AtomicU32,
    /// Whether sync is temporarily disabled (for batch operations).
    pub disable_sync: bool,
    /// Container properties (furnace progress, enchantment levels, etc.).
    pub properties: Vec<ScreenProperty>,
    /// Tracked property values for detecting changes.
    pub tracked_property_values: Vec<i32>,
    /// The window type for this container ( determines client UI).
    pub window_type: Option<WindowType>,
    /// Slots selected during a drag operation (for multi-slot distribution).
    pub drag_slots: Vec<u32>,
    /// Whether players can grab items out of the inventory.
    pub allow_grab_items: bool,
    /// Whether players can put items into the inventory from their own.
    pub allow_put_items: bool,
    /// Number of slots that belong to the container (not the player inventory).
    pub container_slots: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickType {
    Left,
    Right,
    ShiftLeft,
    ShiftRight,
    Middle,
    Drop,
    ControlDrop,
    DoubleClick,
    NumberKey(u8),
    Unknown,
}

impl ScreenHandlerBehaviour {
    #[must_use]
    pub fn new(sync_id: u8, window_type: Option<WindowType>) -> Self {
        Self {
            slots: Vec::new(),
            sync_id,
            listeners: Vec::new(),
            sync_handler: None,
            tracked_stacks: Vec::new(),
            cursor_stack: Arc::new(Mutex::new(ItemStack::EMPTY.clone())),
            previous_tracked_stacks: Vec::new(),
            previous_cursor_stack: TrackedStack::EMPTY,
            revision: AtomicU32::new(0),
            disable_sync: false,
            properties: Vec::new(),
            tracked_property_values: Vec::new(),
            window_type,
            drag_slots: Vec::new(),
            allow_grab_items: true,
            allow_put_items: true,
            container_slots: 0,
        }
    }

    pub fn next_revision(&self) -> u32 {
        self.revision.fetch_add(1, Ordering::Relaxed);
        self.revision.fetch_and(32767, Ordering::Relaxed) & 32767
    }
}

#[cfg(test)]
mod container_access_tests {
    use super::ContainerAccess;
    use pumpkin_data::Block;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_data::tag::Taggable;

    #[test]
    fn null_access_has_no_position() {
        assert!(!ContainerAccess::None.requires_position());
        assert!(ContainerAccess::None.accepts_block(&Block::STONE));
    }

    #[test]
    fn block_entity_family_checks_range_but_not_identity() {
        let access = ContainerAccess::RangeOnly;
        assert!(access.requires_position());
        // `Container.stillValidBlockEntity` (`Container.java:94-101`) compares block
        // entities, never block ids, so any block passes the block half here.
        assert!(access.accepts_block(&Block::STONE));
    }

    #[test]
    fn block_family_rejects_a_replaced_block() {
        let access = ContainerAccess::Block(|block| block.id == Block::CRAFTING_TABLE.id);
        assert!(access.requires_position());
        assert!(access.accepts_block(&Block::CRAFTING_TABLE));
        assert!(!access.accepts_block(&Block::STONE));
    }

    /// `AnvilMenu.isValidBlock` tests `BlockTags.ANVIL` (`AnvilMenu.java:65-67`), so a
    /// chipped or damaged anvil must keep the menu open while any other block closes it.
    #[test]
    fn anvil_family_accepts_every_damage_stage() {
        let access = ContainerAccess::Block(|block| {
            block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_ANVIL)
        });
        assert!(access.accepts_block(&Block::ANVIL));
        assert!(access.accepts_block(&Block::CHIPPED_ANVIL));
        assert!(access.accepts_block(&Block::DAMAGED_ANVIL));
        assert!(!access.accepts_block(&Block::STONE));
    }

    /// `BundleItem.overrideStackedOnOther` and `overrideOtherStackedOnMe`
    /// (`BundleItem.java:59-136`) only handle a secondary click when the opposite stack is empty.
    #[test]
    fn bundle_secondary_override_requires_empty_other_stack() {
        let empty = ItemStack::EMPTY.clone();
        let bundle = ItemStack::new(1, &Item::BUNDLE);
        let item = ItemStack::new(1, &Item::STONE);

        assert!(super::bundle_secondary_click_applies(&empty, &bundle));
        assert!(super::bundle_secondary_click_applies(&bundle, &empty));
        assert!(!super::bundle_secondary_click_applies(&bundle, &item));
        assert!(!super::bundle_secondary_click_applies(&item, &bundle));
    }
}
