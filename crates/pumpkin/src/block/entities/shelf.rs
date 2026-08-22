use crate::block::entities::BlockEntity;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::inventory::{Clearable, Inventory, InventoryFuture, sync_write_items_to_nbt};
use std::any::Any;
use std::array::from_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ShelfBlockEntity {
    pub position: BlockPos,
    pub items: tokio::sync::RwLock<[ItemStack; Self::INVENTORY_SIZE]>,
    /// `ShelfBlockEntity.alignItemsToBottom` (`ShelfBlockEntity.java:36`), saved and loaded
    /// under `align_items_to_bottom` (`ShelfBlockEntity.java:47,54`) and repeated in the
    /// update tag (`:66`) because the renderer reads it.
    pub align_items_to_bottom: AtomicBool,
    pub dirty: AtomicBool,
}

impl BlockEntity for ShelfBlockEntity {
    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.write_inventory_nbt(nbt, true).await;
            nbt.put_bool(
                "align_items_to_bottom",
                self.align_items_to_bottom.load(Ordering::Relaxed),
            );
        })
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let mut shelf = Self {
            position,
            items: tokio::sync::RwLock::new(from_fn(|_| ItemStack::EMPTY.clone())),
            align_items_to_bottom: AtomicBool::new(
                nbt.get_bool("align_items_to_bottom").unwrap_or(false),
            ),
            dirty: AtomicBool::new(false),
        };

        pumpkin_world::inventory::sync_read_items_from_nbt(nbt, shelf.items.get_mut());

        shelf
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
        nbt.put_bool(
            "align_items_to_bottom",
            self.align_items_to_bottom.load(Ordering::Relaxed),
        );
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ShelfBlockEntity {
    pub const INVENTORY_SIZE: usize = 3;
    pub const ID: &'static str = "minecraft:shelf";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            items: tokio::sync::RwLock::new(from_fn(|_| ItemStack::EMPTY.clone())),
            align_items_to_bottom: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
        }
    }
}

impl Inventory for ShelfBlockEntity {
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
}

impl Clearable for ShelfBlockEntity {
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
    use super::ShelfBlockEntity;
    use crate::block::entities::BlockEntity;
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_util::math::position::BlockPos;
    use std::sync::atomic::Ordering;

    #[test]
    fn align_items_to_bottom_round_trips_through_nbt() {
        let pos = BlockPos::new(1, 64, 2);
        let entity = ShelfBlockEntity::new(pos);
        entity.align_items_to_bottom.store(true, Ordering::Relaxed);

        let mut nbt = NbtCompound::new();
        futures::executor::block_on(entity.write_nbt(&mut nbt));
        assert_eq!(nbt.get_bool("align_items_to_bottom"), Some(true));

        let loaded = ShelfBlockEntity::from_nbt(&nbt, pos);
        assert!(loaded.align_items_to_bottom.load(Ordering::Relaxed));
    }

    #[test]
    fn align_items_to_bottom_defaults_to_false() {
        let loaded = ShelfBlockEntity::from_nbt(&NbtCompound::new(), BlockPos::new(0, 0, 0));
        assert!(!loaded.align_items_to_bottom.load(Ordering::Relaxed));
    }
}
