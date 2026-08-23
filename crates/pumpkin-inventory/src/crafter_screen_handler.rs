//! Crafter screen handler.
//!
//! Port of `CrafterMenu.java`. A crafter is not a plain 3x3 container: it
//! carries a ten-entry `ContainerData` (`CrafterMenu.java:26`) holding one
//! disabled flag per input slot plus the redstone-powered flag
//! (`CrafterMenu.java:56-68`), its input slots are `CrafterSlot`s that refuse
//! insertion while disabled (`CrafterSlot.java:14-17`), and it carries a
//! non-interactive recipe-preview slot after the player slots
//! (`CrafterMenu.java:51`).

use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

use pumpkin_data::{item_stack::ItemStack, screen::WindowType};
use pumpkin_world::{block::entities::PropertyDelegate, inventory::Inventory};

use crate::{
    player::player_inventory::PlayerInventory,
    screen_handler::{
        InventoryPlayer, ItemStackFuture, ScreenHandler, ScreenHandlerBehaviour,
        ScreenHandlerFuture, ScreenProperty,
    },
    slot::{BoxFuture, Slot},
};

/// Input slots of a crafter (`CrafterMenu.java:13`).
pub const CRAFTER_SLOT_COUNT: usize = 9;
/// First player-inventory slot (`CrafterMenu.java:14`).
pub const INV_SLOT_START: i32 = 9;
/// One past the last player hotbar slot (`CrafterMenu.java:17`).
pub const USE_ROW_SLOT_END: i32 = 45;
/// Size of the crafter's `ContainerData`: nine disabled flags plus `powered`
/// (`CrafterMenu.java:26`, read back at `:63` and `:67`).
pub const CRAFTER_PROPERTY_COUNT: i32 = 10;
/// `ContainerData` index of the redstone-powered flag (`CrafterMenu.java:67`).
pub const POWERED_PROPERTY_INDEX: i32 = 9;

/// An input slot of a crafter (`CrafterSlot.java`).
///
/// Vanilla reaches the disabled flag through `CrafterMenu.isSlotDisabled`,
/// which is just a read of the shared `ContainerData`; this holds the delegate
/// directly to avoid a slot -> handler back-reference.
pub struct CrafterSlot {
    inventory: Arc<dyn Inventory>,
    index: usize,
    properties: Arc<dyn PropertyDelegate>,
    id: AtomicU8,
}

impl CrafterSlot {
    #[must_use]
    pub fn new(
        inventory: Arc<dyn Inventory>,
        index: usize,
        properties: Arc<dyn PropertyDelegate>,
    ) -> Self {
        Self {
            inventory,
            index,
            properties,
            id: AtomicU8::new(0),
        }
    }
}

impl Slot for CrafterSlot {
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

    /// `CrafterSlot.java:14-17`: a disabled slot accepts nothing.
    fn can_insert<'a>(&'a self, _stack: &'a ItemStack) -> BoxFuture<'a, bool> {
        Box::pin(async move { self.properties.get_property(self.index as i32) != 1 })
    }
}

/// The crafter's recipe preview slot (`NonInteractiveResultSlot.java`).
///
/// It is display-only: nothing may be placed in it and nothing may be taken
/// out (`NonInteractiveResultSlot.java:18-56`).
pub struct NonInteractiveResultSlot {
    inventory: Arc<dyn Inventory>,
    index: usize,
    id: AtomicU8,
}

impl NonInteractiveResultSlot {
    #[must_use]
    pub fn new(inventory: Arc<dyn Inventory>, index: usize) -> Self {
        Self {
            inventory,
            index,
            id: AtomicU8::new(0),
        }
    }
}

impl Slot for NonInteractiveResultSlot {
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

    /// `NonInteractiveResultSlot.java:47-49`.
    fn can_insert<'a>(&'a self, _stack: &'a ItemStack) -> BoxFuture<'a, bool> {
        Box::pin(async move { false })
    }

    /// `NonInteractiveResultSlot.java:18-20` (`mayPickup`).
    fn can_take_items(&self, _player: &dyn InventoryPlayer) -> BoxFuture<'_, bool> {
        Box::pin(async move { false })
    }

    /// `NonInteractiveResultSlot.java:41-43` (`allowModification`).
    fn allow_modification<'a>(&'a self, _player: &'a dyn InventoryPlayer) -> BoxFuture<'a, bool> {
        Box::pin(async move { false })
    }

    /// `NonInteractiveResultSlot.java:51-53` (`remove`).
    fn take_stack(&self, _amount: u8) -> BoxFuture<'_, ItemStack> {
        Box::pin(async move { ItemStack::EMPTY.clone() })
    }

    /// `NonInteractiveResultSlot.java:22-24` (`tryRemove`).
    fn try_take_stack_range<'a>(
        &'a self,
        _min: u8,
        _max: u8,
        _player: &'a dyn InventoryPlayer,
    ) -> BoxFuture<'a, Option<ItemStack>> {
        Box::pin(async move { None })
    }

    /// `NonInteractiveResultSlot.isFake` (NonInteractiveResultSlot.java:66-68):
    /// the preview slot is a fake/recipe-book slot.
    fn is_fake(&self) -> bool {
        true
    }

    /// `NonInteractiveResultSlot.isHighlightable` (NonInteractiveResultSlot.java:62-64):
    /// the preview slot should not be highlighted by the recipe book.
    fn is_highlightable(&self) -> bool {
        false
    }
}

/// Screen handler for a crafter block (`CrafterMenu.java`).
pub struct CrafterScreenHandler {
    /// The crafter's nine input slots.
    pub inventory: Arc<dyn Inventory>,
    /// One-slot inventory backing the recipe preview (`ResultContainer`,
    /// `CrafterMenu.java:18`).
    pub result_inventory: Arc<dyn Inventory>,
    /// Nine disabled flags plus `powered` (`CrafterMenu.java:26`).
    pub properties: Arc<dyn PropertyDelegate>,
    behaviour: ScreenHandlerBehaviour,
}

impl CrafterScreenHandler {
    /// Creates a crafter screen handler (`CrafterMenu.java:31-54`).
    ///
    /// Slot order is the vanilla one: nine `CrafterSlot`s, the 36 standard
    /// player slots, then the non-interactive result slot at index 45.
    pub async fn new(
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        inventory: Arc<dyn Inventory>,
        result_inventory: Arc<dyn Inventory>,
        properties: Arc<dyn PropertyDelegate>,
    ) -> Self {
        let mut handler = Self {
            inventory: inventory.clone(),
            result_inventory: result_inventory.clone(),
            properties: properties.clone(),
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Crafter3x3)),
        };

        inventory.on_open().await;

        for index in 0..CRAFTER_SLOT_COUNT {
            handler.add_slot(Arc::new(CrafterSlot::new(
                inventory.clone(),
                index,
                properties.clone(),
            )));
        }

        let player_inv: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inv);

        handler.add_slot(Arc::new(NonInteractiveResultSlot::new(result_inventory, 0)));

        // `CrafterMenu.java:52`: addDataSlots(this.containerData).
        for index in 0..CRAFTER_PROPERTY_COUNT {
            handler.add_property(ScreenProperty::new(properties.clone(), index as u8));
        }

        handler
    }

    /// `CrafterMenu.java:62-64`.
    #[must_use]
    pub fn is_slot_disabled(&self, slot_id: i32) -> bool {
        slot_id > -1
            && slot_id < CRAFTER_SLOT_COUNT as i32
            && self.properties.get_property(slot_id) == 1
    }

    /// `CrafterMenu.java:56-60`. Vanilla stores 0 for enabled and 1 for
    /// disabled, then broadcasts the change.
    pub fn set_slot_state(&self, slot_id: i32, enabled: bool) {
        if slot_id > -1 && slot_id < CRAFTER_SLOT_COUNT as i32 {
            self.properties.set_property(slot_id, i32::from(!enabled));
        }
    }

    /// `CrafterMenu.java:66-68`.
    #[must_use]
    pub fn is_powered(&self) -> bool {
        self.properties.get_property(POWERED_PROPERTY_INDEX) == 1
    }
}

impl ScreenHandler for CrafterScreenHandler {
    /// Port of `CrafterMenu.java:102-104`, which delegates to
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

    /// `CrafterMenu.java:70-99`. Input slots push to the whole player area
    /// (reversed); everything else pushes back into the nine input slots. The
    /// result slot sits at index 45 but yields nothing, because
    /// `NonInteractiveResultSlot` refuses to give up its stack.
    fn quick_move<'a>(
        &'a mut self,
        player: &'a dyn InventoryPlayer,
        slot_index: i32,
    ) -> ItemStackFuture<'a> {
        Box::pin(async move {
            let mut stack_left = ItemStack::EMPTY.clone();
            let slot = self.get_behaviour().slots[slot_index as usize].clone();

            if slot.has_stack().await {
                let mut slot_stack = slot.get_stack().await;
                stack_left = slot_stack.clone();

                if slot_index < CRAFTER_SLOT_COUNT as i32 {
                    if !self
                        .insert_item(&mut slot_stack, INV_SLOT_START, USE_ROW_SLOT_END, true)
                        .await
                    {
                        return ItemStack::EMPTY.clone();
                    }
                } else if !self
                    .insert_item(&mut slot_stack, 0, CRAFTER_SLOT_COUNT as i32, false)
                    .await
                {
                    return ItemStack::EMPTY.clone();
                }

                if slot_stack.is_empty() {
                    slot.set_stack(ItemStack::EMPTY.clone()).await;
                } else {
                    slot.set_stack(slot_stack.clone()).await;
                }

                // `CrafterMenu.java:91-95`.
                if slot_stack.item_count == stack_left.item_count {
                    return ItemStack::EMPTY.clone();
                }
                slot.on_take_item(player, &slot_stack).await;
            }

            stack_left
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicI32, Ordering as AtomicOrdering},
    };

    use pumpkin_data::item::Item;
    use pumpkin_world::inventory::SimpleInventory;
    use tokio::sync::Mutex;

    use crate::{entity_equipment::EntityEquipment, screen_handler::PlayerFuture};

    use super::*;

    /// Stand-in for `CrafterBlockEntity`'s `ContainerData`.
    struct TestProperties {
        values: [AtomicI32; CRAFTER_PROPERTY_COUNT as usize],
    }

    impl TestProperties {
        fn new() -> Self {
            Self {
                values: std::array::from_fn(|_| AtomicI32::new(0)),
            }
        }
    }

    impl PropertyDelegate for TestProperties {
        fn get_property(&self, index: i32) -> i32 {
            self.values[index as usize].load(AtomicOrdering::Relaxed)
        }
        fn set_property(&self, index: i32, value: i32) {
            self.values[index as usize].store(value, AtomicOrdering::Relaxed);
        }
        fn get_properties_size(&self) -> i32 {
            CRAFTER_PROPERTY_COUNT
        }
    }

    struct TestPlayer {
        inventory: Arc<PlayerInventory>,
    }

    impl InventoryPlayer for TestPlayer {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn drop_item(&self, _item: ItemStack, _retain_ownership: bool) -> PlayerFuture<'_, ()> {
            Box::pin(async {})
        }
        fn get_inventory(&self) -> Arc<PlayerInventory> {
            self.inventory.clone()
        }
        fn play_sound(&self, _sound: pumpkin_data::sound::Sound) -> PlayerFuture<'_, ()> {
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
        fn add_experience_levels(&self, _levels: i32) -> PlayerFuture<'_, ()> {
            Box::pin(async {})
        }
        fn enchantment_seed(&self) -> i32 {
            0
        }
        fn set_enchantment_seed(&self, _seed: i32) -> PlayerFuture<'_, ()> {
            Box::pin(async {})
        }
        fn enqueue_inventory_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetContainerContent,
            _window_type: Option<WindowType>,
        ) -> PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_slot_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetContainerSlot,
            _window_type: Option<WindowType>,
            _total_slots: usize,
        ) -> PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_cursor_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetCursorItem,
        ) -> PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_property_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetContainerProperty,
        ) -> PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_slot_set_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetPlayerInventory,
        ) -> PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_set_held_item_packet<'a>(
            &'a self,
            _packet: &'a pumpkin_protocol::java::client::play::CSetSelectedSlot,
        ) -> PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn enqueue_equipment_change<'a>(
            &'a self,
            _slot: &'a pumpkin_data::data_component_impl::EquipmentSlot,
            _stack: &'a ItemStack,
        ) -> PlayerFuture<'a, ()> {
            Box::pin(async {})
        }
        fn award_experience(&self, _amount: i32) -> PlayerFuture<'_, ()> {
            Box::pin(async {})
        }
        fn increment_stat(
            &self,
            _category: pumpkin_data::statistic::StatisticCategory,
            _stat_id: i32,
            _amount: i32,
        ) -> PlayerFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    async fn handler() -> (
        CrafterScreenHandler,
        Arc<SimpleInventory>,
        Arc<TestProperties>,
        TestPlayer,
    ) {
        let player_inventory = Arc::new(PlayerInventory::new(
            Arc::new(Mutex::new(EntityEquipment::new())),
            Arc::new(HashMap::new()),
        ));
        let inventory = Arc::new(SimpleInventory::new(CRAFTER_SLOT_COUNT));
        let result = Arc::new(SimpleInventory::new(1));
        let properties = Arc::new(TestProperties::new());
        let handler = CrafterScreenHandler::new(
            0,
            &player_inventory,
            inventory.clone(),
            result,
            properties.clone(),
        )
        .await;
        let player = TestPlayer {
            inventory: player_inventory,
        };
        (handler, inventory, properties, player)
    }

    #[tokio::test]
    async fn layout_is_nine_inputs_then_player_slots_then_result() {
        let (handler, ..) = handler().await;
        // `CrafterMenu.java:42-51`: 9 + 36 + 1.
        assert_eq!(handler.get_behaviour().slots.len(), 46);
        assert_eq!(handler.window_type(), Some(WindowType::Crafter3x3));
        assert_eq!(
            handler.get_behaviour().properties.len(),
            CRAFTER_PROPERTY_COUNT as usize
        );
    }

    #[tokio::test]
    async fn slot_state_round_trips_through_the_container_data() {
        let (handler, _, properties, _) = handler().await;
        assert!(!handler.is_slot_disabled(4));

        handler.set_slot_state(4, false);
        assert_eq!(properties.get_property(4), 1);
        assert!(handler.is_slot_disabled(4));

        handler.set_slot_state(4, true);
        assert_eq!(properties.get_property(4), 0);
        assert!(!handler.is_slot_disabled(4));
    }

    #[tokio::test]
    async fn out_of_range_slot_ids_are_never_disabled() {
        let (handler, ..) = handler().await;
        assert!(!handler.is_slot_disabled(-1));
        // Index 9 is the `powered` flag, not an input slot.
        handler.set_slot_state(POWERED_PROPERTY_INDEX, false);
        assert!(!handler.is_slot_disabled(POWERED_PROPERTY_INDEX));
    }

    #[tokio::test]
    async fn powered_reads_property_nine() {
        let (handler, _, properties, _) = handler().await;
        assert!(!handler.is_powered());
        properties.set_property(POWERED_PROPERTY_INDEX, 1);
        assert!(handler.is_powered());
    }

    #[tokio::test]
    async fn disabled_input_slots_reject_items() {
        let (handler, _, properties, _) = handler().await;
        let slot = handler.get_behaviour().slots[3].clone();
        assert!(slot.can_insert(&ItemStack::new(1, &Item::STONE)).await);

        properties.set_property(3, 1);
        assert!(!slot.can_insert(&ItemStack::new(1, &Item::STONE)).await);
        // Its neighbours are unaffected.
        assert!(
            handler.get_behaviour().slots[4]
                .can_insert(&ItemStack::new(1, &Item::STONE))
                .await
        );
    }

    #[tokio::test]
    async fn result_slot_is_display_only() {
        let (handler, _, _, player) = handler().await;
        let result = handler.get_behaviour().slots[45].clone();
        assert!(!result.can_insert(&ItemStack::new(1, &Item::STONE)).await);
        assert!(!result.can_take_items(&player).await);
        assert!(result.take_stack(1).await.is_empty());
        assert!(result.try_take_stack_range(1, 64, &player).await.is_none());
    }

    #[tokio::test]
    async fn quick_move_from_input_reaches_player_inventory() {
        let (mut handler, inventory, _, player) = handler().await;
        inventory
            .set_stack(0, ItemStack::new(5, &Item::STONE))
            .await;

        handler.quick_move(&player, 0).await;

        assert!(inventory.get_stack(0).await.is_empty());
        // `insert_item(.., true)` fills from the end of the player area, which
        // for a crafter stops at index 44 - the result slot is index 45.
        assert_eq!(
            handler.get_behaviour().slots[(USE_ROW_SLOT_END - 1) as usize]
                .get_stack()
                .await
                .item_count,
            5
        );
        assert!(
            handler.get_behaviour().slots[45]
                .get_stack()
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn quick_move_from_player_reaches_the_inputs() {
        let (mut handler, inventory, _, player) = handler().await;
        handler.get_behaviour().slots[INV_SLOT_START as usize]
            .set_stack(ItemStack::new(2, &Item::STONE))
            .await;

        handler.quick_move(&player, INV_SLOT_START).await;

        assert_eq!(inventory.get_stack(0).await.item_count, 2);
    }

    #[tokio::test]
    async fn quick_move_skips_disabled_inputs() {
        let (mut handler, inventory, properties, player) = handler().await;
        for index in 0..4 {
            properties.set_property(index, 1);
        }
        handler.get_behaviour().slots[INV_SLOT_START as usize]
            .set_stack(ItemStack::new(2, &Item::STONE))
            .await;

        handler.quick_move(&player, INV_SLOT_START).await;

        for index in 0..4 {
            assert!(inventory.get_stack(index as usize).await.is_empty());
        }
        assert_eq!(inventory.get_stack(4).await.item_count, 2);
    }
}
