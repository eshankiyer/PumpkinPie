use super::BlockEntity;
use crate::world::World;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;
use std::{borrow::Cow, pin::Pin};
use tokio::sync::Mutex;

/// `DecoratedPotBlockEntity.WobbleStyle` (`DecoratedPotBlockEntity.java:177-186`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WobbleStyle {
    Positive,
    Negative,
}

impl WobbleStyle {
    /// Animation length in ticks (`WobbleStyle.duration`, `DecoratedPotBlockEntity.java:178-179`).
    #[must_use]
    pub const fn duration(self) -> u8 {
        match self {
            Self::Positive => 7,
            Self::Negative => 10,
        }
    }

    /// Ordinal sent as the block-event data (`wobble`, `DecoratedPotBlockEntity.java:160-164`).
    #[must_use]
    pub const fn to_index(self) -> u8 {
        match self {
            Self::Positive => 0,
            Self::Negative => 1,
        }
    }
}

pub struct DecoratedPotBlockEntity {
    pub position: BlockPos,
    pub sherds: Mutex<Option<Vec<NbtTag>>>,
    pub item: Mutex<Option<ItemStack>>,
}

impl BlockEntity for DecoratedPotBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let sherds = nbt.get_list("sherds").map(<[_]>::to_vec);
        let item = nbt
            .get_compound("item")
            .and_then(ItemStack::read_item_stack);
        Self {
            position,
            sherds: Mutex::new(sherds),
            item: Mutex::new(item),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if let Some(sh) = self.sherds.lock().await.as_ref() {
                nbt.put_list("sherds", sh.clone());
            }
            if let Some(it) = self.item.lock().await.as_ref() {
                let mut it_nbt = NbtCompound::new();
                it.write_item_stack(&mut it_nbt);
                nbt.put_compound("item", it_nbt);
            }
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        if let Ok(sherds) = self.sherds.try_lock()
            && let Some(ref sh) = *sherds
        {
            nbt.put_list("sherds", sh.clone());
        }
        if let Ok(item) = self.item.try_lock()
            && let Some(ref it) = *item
        {
            let mut it_nbt = NbtCompound::new();
            it.write_item_stack(&mut it_nbt);
            nbt.put_compound("item", it_nbt);
        }
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl DecoratedPotBlockEntity {
    pub const ID: &'static str = "minecraft:decorated_pot";
    /// `EVENT_POT_WOBBLES` (`DecoratedPotBlockEntity.java:30`).
    pub const EVENT_POT_WOBBLES: u8 = 1;

    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            sherds: Mutex::const_new(None),
            item: Mutex::const_new(None),
        }
    }

    /// `DecoratedPotBlockEntity.wobble` (`DecoratedPotBlockEntity.java:160-164`): queues a
    /// synced block event so clients play the wobble animation; the client-side
    /// `triggerEvent` (`DecoratedPotBlockEntity.java:167-175`) consumes it.
    pub async fn wobble(&self, world: &Arc<World>, style: WobbleStyle) {
        world
            .add_synced_block_event(self.position, Self::EVENT_POT_WOBBLES, style.to_index())
            .await;
    }

    pub async fn get_item(&self) -> Option<ItemStack> {
        self.item.lock().await.clone()
    }

    /// Returns the four serialized sherd identifiers used by
    /// `DecoratedPotBlock.getDrops` and `getCloneItemStack`.
    pub fn decorations(&self) -> Option<[Cow<'static, str>; 4]> {
        let sherds = self.sherds.try_lock().ok()?;
        sherds
            .as_ref()?
            .iter()
            .map(|tag| {
                tag.extract_string()
                    .map(|value| Cow::Owned(value.to_string()))
            })
            .collect::<Option<Vec<_>>>()?
            .try_into()
            .ok()
    }

    pub async fn take_item(&self) -> Option<ItemStack> {
        self.item.lock().await.take()
    }

    pub async fn try_insert_item(&self, stack: &mut ItemStack, count: u8) -> bool {
        let mut item_guard = self.item.lock().await;
        if let Some(existing) = item_guard.as_mut() {
            // Vanilla `DecoratedPotBlock.useItemOn` (`DecoratedPotBlock.java:110-115`) only
            // merges equal items/components and compares against that stack's max size.
            if existing.are_items_and_components_equal(stack) {
                let add = count.min(stack.item_count).min(
                    existing
                        .get_max_stack_size()
                        .saturating_sub(existing.item_count),
                );
                if add > 0 {
                    existing.item_count += add;
                    stack.item_count -= add;
                    return true;
                }
            }
            false
        } else {
            let insert_count = count.min(stack.item_count).min(stack.get_max_stack_size());
            if insert_count == 0 {
                return false;
            }
            let mut inserted = stack.clone();
            inserted.item_count = insert_count;
            *item_guard = Some(inserted);
            stack.item_count -= insert_count;
            true
        }
    }

    pub async fn get_comparator_output(&self) -> u8 {
        self.item.lock().await.as_ref().map_or(0, |item| {
            if item.item_count == 0 {
                0
            } else {
                let max_count = 64f32;
                1 + ((item.item_count as f32 / max_count) * 14.0).floor() as u8
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::DecoratedPotBlockEntity;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_util::math::position::BlockPos;

    /// `DecoratedPotBlock.useItemOn` (`DecoratedPotBlock.java:110-115`) rejects mismatched
    /// item components.
    #[tokio::test]
    async fn insertion_requires_matching_components() {
        let pot = DecoratedPotBlockEntity::new(BlockPos::new(0, 0, 0));
        let mut plain = ItemStack::new(1, &Item::COBBLESTONE);
        assert!(pot.try_insert_item(&mut plain, 1).await);

        let mut named = ItemStack::new(1, &Item::COBBLESTONE);
        named.set_custom_name("named".into());
        assert!(!pot.try_insert_item(&mut named, 1).await);
    }

    /// `DecoratedPotBlock.useItemOn` (`DecoratedPotBlock.java:111-112`) uses the item's max
    /// stack size when deciding whether another item fits.
    #[tokio::test]
    async fn insertion_uses_the_item_max_stack_size() {
        let pot = DecoratedPotBlockEntity::new(BlockPos::new(0, 0, 0));
        let mut pearls = ItemStack::new(16, &Item::ENDER_PEARL);
        assert!(pot.try_insert_item(&mut pearls, 16).await);
        assert_eq!(pearls.item_count, 0);

        let mut extra = ItemStack::new(1, &Item::ENDER_PEARL);
        assert!(!pot.try_insert_item(&mut extra, 1).await);
        assert_eq!(extra.item_count, 1);
    }
}
