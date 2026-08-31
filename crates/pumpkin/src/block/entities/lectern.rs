use pumpkin_data::data_component_impl::{WritableBookContentImpl, WrittenBookContentImpl};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use std::{
    any::Any,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::sync::Mutex;

use crate::block::entities::BlockEntity;
use pumpkin_world::inventory::{Clearable, Inventory, InventoryFuture};

pub struct LecternBlockEntity {
    pub position: BlockPos,
    pub book: Arc<Mutex<ItemStack>>,
    // Mirrors `hasBook` so synchronous menu validation can read the component state
    // (`LecternBlockEntity.java:134-140`).
    has_book: AtomicBool,
    pub page: AtomicUsize,
    pub dirty: AtomicBool,
}

impl BlockEntity for LecternBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let book_stack = nbt
            .get_compound("Book")
            .and_then(ItemStack::read_item_stack)
            .unwrap_or_else(|| ItemStack::EMPTY.clone());

        // `loadAdditional` derives page data from the loaded book stack
        // (`LecternBlockEntity.java:203-208,248-255`).
        let page_count = Self::page_count_of(&book_stack);
        let has_book = Self::stack_has_book(&book_stack);
        let page = nbt
            .get_int("Page")
            .unwrap_or(0)
            .clamp(0, page_count.saturating_sub(1).max(0)) as usize;
        let book = Arc::new(Mutex::new(book_stack));

        Self {
            position,
            book,
            has_book: AtomicBool::new(has_book),
            page: AtomicUsize::new(page),
            dirty: AtomicBool::new(false),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let book = self.book.lock().await;
            if !book.is_empty() {
                let mut book_nbt = NbtCompound::default();
                book.write_item_stack(&mut book_nbt);
                nbt.put_compound("Book", book_nbt);
            }
            nbt.put_int("Page", self.page.load(Ordering::Relaxed) as i32);
        })
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
        if let Ok(book) = self.book.try_lock()
            && !book.is_empty()
        {
            let mut book_nbt = NbtCompound::new();
            book.write_item_stack(&mut book_nbt);
            nbt.put("Book", NbtTag::Compound(book_nbt));
        }
        nbt.put_int("Page", self.page.load(Ordering::Relaxed) as i32);
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl LecternBlockEntity {
    pub const ID: &'static str = "minecraft:lectern";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            book: Arc::new(Mutex::new(ItemStack::EMPTY.clone())),
            has_book: AtomicBool::new(false),
            page: AtomicUsize::new(0),
            dirty: AtomicBool::new(false),
        }
    }

    /// Number of pages in a writable or written book, `0` for anything else.
    #[must_use]
    pub fn page_count_of(stack: &ItemStack) -> i32 {
        stack
            .get_data_component::<WrittenBookContentImpl>()
            .map(|content| content.pages.len())
            .or_else(|| {
                stack
                    .get_data_component::<WritableBookContentImpl>()
                    .map(|content| content.pages.len())
            })
            .map_or(0, |pages| pages as i32)
    }

    fn stack_has_book(stack: &ItemStack) -> bool {
        // `hasBook` checks components, not merely a non-empty item stack
        // (`LecternBlockEntity.java:134-140`).
        !stack.is_empty()
            && (stack
                .get_data_component::<WritableBookContentImpl>()
                .is_some()
                || stack
                    .get_data_component::<WrittenBookContentImpl>()
                    .is_some())
    }

    /// Vanilla `hasBook` checks for either book-content component
    /// (`LecternBlockEntity.java:134-140`). The atomic mirror keeps that result
    /// available to synchronous menu validation while the item stack remains async.
    #[must_use]
    pub fn has_book(&self) -> bool {
        self.has_book.load(Ordering::Relaxed)
    }

    pub async fn page_count(&self) -> i32 {
        Self::page_count_of(&*self.book.lock().await)
    }

    /// Vanilla comparator output (`LecternBlockEntity.java:172-175`): books with
    /// at most one page use full progress, while multi-page books scale by page.
    pub async fn comparator_output(&self) -> u8 {
        let book = self.book.lock().await;
        if !Self::stack_has_book(&book) {
            return 0;
        }

        let page = self.page.load(Ordering::Relaxed) as f32;
        let page_count = Self::page_count_of(&book);
        let page_progress = if page_count > 1 {
            page / (page_count as f32 - 1.0)
        } else {
            1.0
        };
        (page_progress * 14.0).floor() as u8 + 1
    }
}

impl Inventory for LecternBlockEntity {
    fn size(&self) -> usize {
        1
    }

    fn is_empty(&self) -> InventoryFuture<'_, bool> {
        Box::pin(async move { self.book.lock().await.is_empty() })
    }

    fn get_stack(&self, _slot: usize) -> InventoryFuture<'_, ItemStack> {
        Box::pin(async move { self.book.lock().await.clone() })
    }

    fn remove_stack(&self, _slot: usize) -> InventoryFuture<'_, ItemStack> {
        Box::pin(async move {
            let mut removed = ItemStack::EMPTY.clone();
            let mut guard = self.book.lock().await;
            std::mem::swap(&mut removed, &mut *guard);
            // `removeItemNoUpdate` calls `onBookItemRemove` after clearing slot 0
            // (`LecternBlockEntity.java:69-74`).
            self.has_book.store(false, Ordering::Relaxed);
            self.page.store(0, Ordering::Relaxed);
            self.mark_dirty();
            removed
        })
    }

    fn remove_stack_specific(&self, _slot: usize, amount: u8) -> InventoryFuture<'_, ItemStack> {
        Box::pin(async move {
            let mut stack = self.book.lock().await;
            if stack.is_empty() {
                return ItemStack::EMPTY.clone();
            }
            let res = stack.split(amount);
            if stack.is_empty() {
                // Vanilla `bookAccess.removeItem` calls `onBookItemRemove` when the
                // split empties the slot (`LecternBlockEntity.java:55-60`). The block
                // callback is handled by the screen controller; keep the entity's
                // page state in sync here as well.
                self.page.store(0, Ordering::Relaxed);
                // `removeItem` clears the book state when the split empties slot 0
                // (`LecternBlockEntity.java:55-60`).
                self.has_book.store(false, Ordering::Relaxed);
            }
            self.mark_dirty();
            res
        })
    }

    fn set_stack(&self, _slot: usize, stack: ItemStack) -> InventoryFuture<'_, ()> {
        Box::pin(async move {
            let has_book = Self::stack_has_book(&stack);
            // `setBook` replaces the stack and recomputes its book state
            // (`LecternBlockEntity.java:152-156`).
            *self.book.lock().await = stack;
            self.has_book.store(has_book, Ordering::Relaxed);
            // A freshly placed book always opens on its first page.
            self.page.store(0, Ordering::Relaxed);
            self.mark_dirty();
        })
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// `LecternBlockEntity.bookAccess.canPlaceItem` (`LecternBlockEntity.java:98-100`): the
    /// book slot never accepts an item through this container interface - a book only ever
    /// gets in via `LecternBlock.tryPlaceBook`. Without this a hopper facing a lectern could
    /// insert (or overwrite) whatever item it is holding into slot 0.
    fn can_insert_through_face<'a>(
        &'a self,
        _slot: usize,
        _stack: &'a ItemStack,
        _direction: pumpkin_data::BlockDirection,
    ) -> InventoryFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Clearable for LecternBlockEntity {
    fn clear(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            *self.book.lock().await = ItemStack::EMPTY.clone();
            // `clearContent` delegates to `setBook(ItemStack.EMPTY)`
            // (`LecternBlockEntity.java:220-223,152-156`).
            self.has_book.store(false, Ordering::Relaxed);
            // Vanilla `setBook(ItemStack.EMPTY)` resets both page and page count
            // (`LecternBlockEntity.java:152-156`), which is the outer Clearable
            // behavior behind `clearContent` (`:220-223`).
            self.page.store(0, Ordering::Relaxed);
            self.mark_dirty();
        })
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::DataComponentImpl;
    use pumpkin_data::item::Item;
    use pumpkin_util::math::position::BlockPos;
    use std::sync::atomic::Ordering;

    use super::*;

    #[test]
    fn book_state_and_comparator_follow_vanilla() {
        // `hasBook` checks book components and `getRedstoneSignal` uses page progress
        // (`LecternBlockEntity.java:134-140,172-175`).
        let entity = LecternBlockEntity::new(BlockPos::new(0, 64, 0));
        assert!(!entity.has_book());

        let book = ItemStack::new_with_component(
            1,
            &Item::WRITABLE_BOOK,
            vec![(
                DataComponent::WritableBookContent,
                Some(
                    WritableBookContentImpl {
                        pages: vec!["first".to_owned(), "second".to_owned()],
                    }
                    .to_dyn(),
                ),
            )],
        );
        futures::executor::block_on(entity.set_stack(0, book));
        assert!(entity.has_book());
        assert_eq!(futures::executor::block_on(entity.comparator_output()), 1);

        entity.page.store(1, Ordering::Relaxed);
        assert_eq!(futures::executor::block_on(entity.comparator_output()), 15);

        futures::executor::block_on(entity.set_stack(
            0,
            ItemStack::new_with_component(
                1,
                &Item::WRITABLE_BOOK,
                vec![(
                    DataComponent::WritableBookContent,
                    Some(
                        WritableBookContentImpl {
                            pages: vec!["only".to_owned()],
                        }
                        .to_dyn(),
                    ),
                )],
            ),
        ));
        assert_eq!(futures::executor::block_on(entity.comparator_output()), 15);

        futures::executor::block_on(entity.set_stack(0, ItemStack::new(1, &Item::STONE)));
        assert!(!entity.has_book());
        assert_eq!(futures::executor::block_on(entity.comparator_output()), 0);
    }
}
