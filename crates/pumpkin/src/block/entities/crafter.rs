use crate::block::entities::{BlockEntity, PropertyDelegate};
use crate::world::World;
use pumpkin_data::BlockDirection;
use pumpkin_data::block_properties::{BlockProperties, CrafterLikeProperties};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_inventory::crafting::recipes::RecipeInputInventory;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::inventory::{Clearable, Inventory, InventoryFuture, sync_write_items_to_nbt};
use pumpkin_world::world::BlockFlags;
use std::any::Any;
use std::array::from_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

pub struct CrafterBlockEntity {
    pub position: BlockPos,
    pub items: tokio::sync::RwLock<[ItemStack; Self::INVENTORY_SIZE]>,
    pub crafting_ticks_remaining: AtomicI32,
    pub triggered: AtomicBool,
    pub disabled_slots: [AtomicBool; Self::INVENTORY_SIZE],
    pub dirty: AtomicBool,
}

impl BlockEntity for CrafterBlockEntity {
    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let items = self.items.read().await;
            sync_write_items_to_nbt(items.as_slice(), nbt);
            nbt.put_int(
                "crafting_ticks_remaining",
                self.crafting_ticks_remaining.load(Ordering::Relaxed),
            );
            nbt.put_bool("triggered", self.triggered.load(Ordering::Relaxed));

            let disabled_indices: Vec<i32> = self
                .disabled_slots
                .iter()
                .enumerate()
                .filter(|(_, disabled)| disabled.load(Ordering::Relaxed))
                .map(|(slot, _)| slot as i32)
                .collect();
            if !disabled_indices.is_empty() {
                nbt.put("disabled_slots", NbtTag::IntArray(disabled_indices));
            }
        })
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let disabled_slots: [AtomicBool; Self::INVENTORY_SIZE] =
            from_fn(|_| AtomicBool::new(false));
        if let Some(disabled_indices) = nbt.get_int_array("disabled_slots") {
            for &index in disabled_indices {
                if let Ok(slot) = usize::try_from(index)
                    && slot < Self::INVENTORY_SIZE
                {
                    disabled_slots[slot].store(true, Ordering::Relaxed);
                }
            }
        }

        let crafter = Self {
            position,
            items: tokio::sync::RwLock::new(from_fn(|_| ItemStack::EMPTY.clone())),
            crafting_ticks_remaining: AtomicI32::new(
                nbt.get_int("crafting_ticks_remaining").unwrap_or(0),
            ),
            triggered: AtomicBool::new(nbt.get_bool("triggered").unwrap_or(false)),
            disabled_slots,
            dirty: AtomicBool::new(false),
        };

        let mut items = futures::executor::block_on(crafter.items.write());
        crafter.read_data(nbt, &mut *items);
        drop(items);

        // Vanilla loadAdditional only keeps a loaded disabled flag if the slot is
        // still empty once items are loaded (slotCanBeDisabled gates both directions).
        for slot in 0..Self::INVENTORY_SIZE {
            if crafter.disabled_slots[slot].load(Ordering::Relaxed)
                && !futures::executor::block_on(crafter.items.read())[slot].is_empty()
            {
                crafter.disabled_slots[slot].store(false, Ordering::Relaxed);
            }
        }
        crafter
    }

    /// Vanilla `CrafterBlockEntity.serverTick` (`CrafterBlockEntity.java:236-245`):
    /// counts the crafting animation down and clears `CRAFTING` when it reaches zero.
    fn tick<'a>(&'a self, world: &'a Arc<World>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let remaining = self.crafting_ticks_remaining.load(Ordering::Relaxed) - 1;
            if remaining < 0 {
                return;
            }
            self.crafting_ticks_remaining
                .store(remaining, Ordering::Relaxed);
            if remaining != 0 {
                return;
            }
            let block = world.get_block(&self.position);
            let state = world.get_block_state(&self.position);
            let mut props = CrafterLikeProperties::from_state_id(state.id, block);
            if !props.crafting {
                return;
            }
            props.crafting = false;
            world
                .set_block_state(
                    &self.position,
                    props.to_state_id(block),
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
        })
    }

    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn get_inventory(self: Arc<Self>) -> Option<Arc<dyn Inventory>> {
        Some(self)
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Relaxed);
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        let items = futures::executor::block_on(self.items.read());
        sync_write_items_to_nbt(items.as_slice(), &mut nbt);
        nbt.put_int(
            "crafting_ticks_remaining",
            self.crafting_ticks_remaining.load(Ordering::Relaxed),
        );
        nbt.put_bool("triggered", self.triggered.load(Ordering::Relaxed));
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn to_property_delegate(self: Arc<Self>) -> Option<Arc<dyn PropertyDelegate>> {
        Some(self)
    }
}

/// Vanilla's anonymous `ContainerData` on `CrafterBlockEntity`
/// (`CrafterBlockEntity.java:38-59`): ten entries, indices 0-8 are the per-slot disabled
/// flags and index 9 is `triggered`.
impl PropertyDelegate for CrafterBlockEntity {
    fn get_property(&self, index: i32) -> i32 {
        match index {
            Self::TRIGGERED_PROPERTY_INDEX => i32::from(self.triggered.load(Ordering::Relaxed)),
            _ => usize::try_from(index)
                .ok()
                .and_then(|slot| self.disabled_slots.get(slot))
                .map_or(0, |disabled| i32::from(disabled.load(Ordering::Relaxed))),
        }
    }

    fn set_property(&self, index: i32, value: i32) {
        match index {
            Self::TRIGGERED_PROPERTY_INDEX => {
                self.triggered.store(value == 1, Ordering::Relaxed);
            }
            _ => {
                if let Ok(slot) = usize::try_from(index)
                    && let Some(disabled) = self.disabled_slots.get(slot)
                {
                    disabled.store(value == 1, Ordering::Relaxed);
                }
            }
        }
    }

    fn get_properties_size(&self) -> i32 {
        Self::PROPERTY_COUNT
    }
}

impl CrafterBlockEntity {
    pub const INVENTORY_SIZE: usize = 9;
    pub const ID: &'static str = "minecraft:crafter";
    /// `ContainerData` index of the `triggered` flag (`CrafterBlockEntity.java:44`).
    pub const TRIGGERED_PROPERTY_INDEX: i32 = 9;
    /// `ContainerData.getCount` (`CrafterBlockEntity.java:57`).
    pub const PROPERTY_COUNT: i32 = 10;

    /// Vanilla `CrafterBlockEntity.setTriggered` (`CrafterBlockEntity.java:228-230`),
    /// called from `CrafterBlock.setBlockEntityTriggered` (`CrafterBlock.java:100-104`).
    pub fn set_triggered(&self, triggered: bool) {
        self.triggered.store(triggered, Ordering::Relaxed);
        self.mark_dirty();
    }

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            items: tokio::sync::RwLock::new(from_fn(|_| ItemStack::EMPTY.clone())),
            crafting_ticks_remaining: AtomicI32::new(0),
            triggered: AtomicBool::new(false),
            disabled_slots: from_fn(|_| AtomicBool::new(false)),
            dirty: AtomicBool::new(false),
        }
    }

    /// Vanilla `CrafterBlockEntity.slotCanBeDisabled`: a slot can only be toggled
    /// while it currently holds no item.
    pub async fn slot_can_be_disabled(&self, slot: usize) -> bool {
        slot < Self::INVENTORY_SIZE && self.items.read().await[slot].is_empty()
    }

    /// Vanilla `CrafterBlockEntity.setSlotState`.
    pub async fn set_slot_state(&self, slot: usize, enabled: bool) {
        if self.slot_can_be_disabled(slot).await {
            self.disabled_slots[slot].store(!enabled, Ordering::Relaxed);
            self.mark_dirty();
        }
    }

    /// Vanilla `CrafterBlockEntity.setCraftingTicksRemaining`
    /// (`CrafterBlockEntity.java:247-249`).
    pub fn set_crafting_ticks_remaining(&self, ticks: i32) {
        self.crafting_ticks_remaining
            .store(ticks, Ordering::Relaxed);
    }

    /// Vanilla `CrafterBlockEntity.isSlotDisabled`.
    #[must_use]
    pub fn is_slot_disabled(&self, slot: usize) -> bool {
        slot < Self::INVENTORY_SIZE && self.disabled_slots[slot].load(Ordering::Relaxed)
    }
}

impl Inventory for CrafterBlockEntity {
    fn size(&self) -> usize {
        Self::INVENTORY_SIZE
    }

    fn is_empty(&self) -> InventoryFuture<'_, bool> {
        Box::pin(async move {
            let items = self.items.read().await;
            items.iter().all(ItemStack::is_empty)
        })
    }

    fn get_stack(&self, slot: usize) -> InventoryFuture<'_, ItemStack> {
        Box::pin(async move {
            let items = self.items.read().await;
            items[slot].clone()
        })
    }

    fn remove_stack(&self, slot: usize) -> InventoryFuture<'_, ItemStack> {
        Box::pin(async move {
            let mut items = self.items.write().await;
            let removed = std::mem::replace(&mut items[slot], ItemStack::EMPTY.clone());
            self.mark_dirty();
            removed
        })
    }

    fn remove_stack_specific(&self, slot: usize, amount: u8) -> InventoryFuture<'_, ItemStack> {
        Box::pin(async move {
            let mut items = self.items.write().await;
            let res = if !items[slot].is_empty() && amount > 0 {
                items[slot].split(amount)
            } else {
                ItemStack::EMPTY.clone()
            };
            self.mark_dirty();
            res
        })
    }

    fn set_stack(&self, slot: usize, stack: ItemStack) -> InventoryFuture<'_, ()> {
        Box::pin(async move {
            // Vanilla `setItem`: placing an item into a disabled slot re-enables it.
            if self.is_slot_disabled(slot) {
                self.set_slot_state(slot, true).await;
            }
            let mut items = self.items.write().await;
            items[slot] = stack;
            self.mark_dirty();
        })
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn can_insert_through_face<'a>(
        &'a self,
        slot: usize,
        _stack: &'a ItemStack,
        _direction: BlockDirection,
    ) -> InventoryFuture<'a, bool> {
        Box::pin(async move { !self.is_slot_disabled(slot) })
    }
}

/// Vanilla `CrafterBlockEntity.getWidth`/`getHeight` (`CrafterBlockEntity.java:195-203`):
/// the crafter is a `CraftingContainer`, so its nine slots are a 3x3 recipe grid.
/// Disabled slots are simply empty ones, so they need no special handling here -
/// `asCraftInput` (`CraftingContainer.java:15-21`) reads the raw item list.
impl RecipeInputInventory for CrafterBlockEntity {
    fn get_width(&self) -> usize {
        3
    }

    fn get_height(&self) -> usize {
        3
    }
}

impl Clearable for CrafterBlockEntity {
    fn clear(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let mut items = self.items.write().await;
            items.fill_with(|| ItemStack::EMPTY.clone());
            self.mark_dirty();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CrafterBlockEntity;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_inventory::crafting::crafting_screen_handler::match_crafting_recipe;
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_world::inventory::Inventory;

    fn crafter() -> CrafterBlockEntity {
        CrafterBlockEntity::new(BlockPos::new(0, 0, 0))
    }

    /// The crafter is a `CraftingContainer`, so `CrafterBlock.dispenseFrom` can look a
    /// recipe up straight out of its nine slots (`CrafterBlock.java:152-153`).
    #[tokio::test]
    async fn crafter_contents_are_a_recipe_input() {
        let crafter = crafter();
        for slot in [0, 1, 3, 4] {
            crafter
                .set_stack(slot, ItemStack::new(1, &Item::OAK_PLANKS))
                .await;
        }
        let result = match_crafting_recipe(&crafter, None)
            .await
            .expect("four planks are a crafting table");
        assert_eq!(result.item_id, "minecraft:crafting_table");
    }

    /// A disabled slot is an empty one, so it just shrinks the trimmed input.
    #[tokio::test]
    async fn a_disabled_slot_does_not_block_a_match() {
        let crafter = crafter();
        crafter.set_slot_state(8, false).await;
        for slot in [0, 1, 3, 4] {
            crafter
                .set_stack(slot, ItemStack::new(1, &Item::OAK_PLANKS))
                .await;
        }
        assert!(crafter.is_slot_disabled(8));
        assert!(match_crafting_recipe(&crafter, None).await.is_some());
    }

    #[tokio::test]
    async fn an_unmatched_grid_yields_no_recipe() {
        let crafter = crafter();
        crafter.set_stack(0, ItemStack::new(1, &Item::DIRT)).await;
        crafter.set_stack(4, ItemStack::new(1, &Item::DIRT)).await;
        assert!(match_crafting_recipe(&crafter, None).await.is_none());
    }
}
