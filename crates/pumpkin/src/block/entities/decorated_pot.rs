use super::BlockEntity;
use crate::world::World;
use crate::world::loot::generate_chest_loot;
use pumpkin_data::chest_loot_table::{ChestLootEntry, ChestLootPool, ChestLootTable};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{ContainerImpl, DataComponentImpl, PotDecorationsImpl};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::inventory::{Clearable, Inventory, InventoryFuture};
use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
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

const fn loot_item(
    name: &'static str,
    weight: i32,
    min_count: i32,
    max_count: i32,
) -> ChestLootEntry {
    ChestLootEntry {
        item: name,
        weight,
        min_count,
        max_count,
        enchant_randomly: None,
    }
}

/// `data/minecraft/loot_table/pots/trial_chambers/corridor.json`, transcribed from
/// `assets/datapacks/26_2`: one pool, `rolls: 1`, `set_count` uniform ranges only. The
/// generated chest-loot registry carries only `chests/*`, so the trial-chamber decor pots
/// would otherwise unpack to nothing.
static TRIAL_CHAMBERS_CORRIDOR_POT_POOLS: &[ChestLootPool] = &[ChestLootPool {
    entries: &[
        loot_item("minecraft:emerald", 125, 1, 3),
        loot_item("minecraft:arrow", 100, 2, 8),
        loot_item("minecraft:iron_ingot", 100, 1, 2),
        loot_item("minecraft:trial_key", 10, 1, 1),
        loot_item("minecraft:music_disc_creator_music_box", 5, 1, 1),
        loot_item("minecraft:diamond", 5, 1, 2),
        loot_item("minecraft:emerald_block", 5, 1, 1),
        loot_item("minecraft:diamond_block", 1, 1, 1),
    ],
    min_rolls: 1,
    max_rolls: 1,
    empty_weight: 0,
}];

/// Resolves a pot's `LootTable` key: the transcribed `pots/*` table, else any generated
/// chest table. An unknown key behaves like vanilla's `LootTable.EMPTY`.
fn pot_loot_table(key: &str) -> Option<ChestLootTable> {
    let path = key.strip_prefix("minecraft:").unwrap_or(key);
    if path == "pots/trial_chambers/corridor" {
        return Some(ChestLootTable {
            pools: TRIAL_CHAMBERS_CORRIDOR_POT_POOLS,
        });
    }
    pumpkin_data::chest_loot_table::get_chest_loot_table(&format!("minecraft:{path}")).copied()
}

pub struct DecoratedPotBlockEntity {
    pub position: BlockPos,
    pub sherds: Mutex<Option<Vec<NbtTag>>>,
    pub item: Mutex<Option<ItemStack>>,
    /// `RandomizableContainer` key (`DecoratedPotBlockEntity.java:35`).
    loot_table: StdMutex<Option<String>>,
    /// `DecoratedPotBlockEntity.java:36`; 0 means "roll a random seed".
    loot_table_seed: AtomicI64,
    dirty: AtomicBool,
}

impl BlockEntity for DecoratedPotBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    /// `DecoratedPotBlockEntity.loadAdditional` (`DecoratedPotBlockEntity.java:55-64`): a
    /// pending loot table (`RandomizableContainer.tryLoadLootTable`) replaces the saved item.
    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let sherds = nbt.get_list("sherds").map(<[_]>::to_vec);
        let loot_table = nbt.get_string("LootTable").map(ToString::to_string);
        let loot_table_seed = nbt.get_long("LootTableSeed").unwrap_or(0);
        let item = if loot_table.is_some() {
            None
        } else {
            nbt.get_compound("item")
                .and_then(ItemStack::read_item_stack)
        };
        Self {
            position,
            sherds: Mutex::new(sherds),
            item: Mutex::new(item),
            loot_table: StdMutex::new(loot_table),
            loot_table_seed: AtomicI64::new(loot_table_seed),
            dirty: AtomicBool::new(false),
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
            if !self.try_save_loot_table(nbt)
                && let Some(it) = self.item.lock().await.as_ref()
                && !it.is_empty()
            {
                let mut it_nbt = NbtCompound::new();
                it.write_item_stack(&mut it_nbt);
                nbt.put_compound("item", it_nbt);
            }
        })
    }

    /// `getUpdateTag` is `saveCustomOnly` (`DecoratedPotBlockEntity.java:71-73`).
    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        if let Ok(sherds) = self.sherds.try_lock()
            && let Some(ref sh) = *sherds
        {
            nbt.put_list("sherds", sh.clone());
        }
        if !self.try_save_loot_table(&mut nbt)
            && let Ok(item) = self.item.try_lock()
            && let Some(ref it) = *item
            && !it.is_empty()
        {
            let mut it_nbt = NbtCompound::new();
            it.write_item_stack(&mut it_nbt);
            nbt.put_compound("item", it_nbt);
        }
        Some(nbt)
    }

    fn take_loot_table(&self) -> Option<(String, i64)> {
        let key = self
            .loot_table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()?;
        Some((key, self.loot_table_seed.load(Ordering::Relaxed)))
    }

    fn has_loot_table(&self) -> bool {
        self.loot_table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// `getContainerBlockEntity` (`DecoratedPotBlockEntity.java:155-158`): the pot is a
    /// one-slot `ContainerSingleItem`, so hoppers and `preRemoveSideEffects` see it.
    fn get_inventory(self: Arc<Self>) -> Option<Arc<dyn Inventory>> {
        Some(self)
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Relaxed);
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
            loot_table: StdMutex::new(None),
            loot_table_seed: AtomicI64::new(0),
            dirty: AtomicBool::new(false),
        }
    }

    /// `RandomizableContainer.trySaveLootTable` (`RandomizableContainer.java:57-70`).
    fn try_save_loot_table(&self, nbt: &mut NbtCompound) -> bool {
        let loot_table = self
            .loot_table
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(key) = loot_table.as_ref() else {
            return false;
        };
        nbt.put_string("LootTable", key.clone());
        let seed = self.loot_table_seed.load(Ordering::Relaxed);
        if seed != 0 {
            nbt.put_long("LootTableSeed", seed);
        }
        true
    }

    /// `RandomizableContainer.unpackLootTable` (`RandomizableContainer.java:72-90`), run by
    /// `getTheItem` / `splitTheItem` / `setTheItem` (`DecoratedPotBlockEntity.java:132-153`)
    /// with the item lock held. `LootTable.fill` (`LootTable.java:146-165`) only fills empty
    /// slots, so with one slot the first rolled stack lands in it. There is no player, so
    /// neither luck nor the `GENERATE_LOOT` trigger applies.
    fn unpack_loot_table(&self, item: &mut Option<ItemStack>) {
        let Some(key) = self
            .loot_table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
        };
        if item.as_ref().is_none_or(ItemStack::is_empty)
            && let Some(table) = pot_loot_table(&key)
        {
            let seed = match self.loot_table_seed.load(Ordering::Relaxed) {
                0 => rand::random(),
                seed => seed,
            };
            *item = generate_chest_loot(&table, seed)
                .into_iter()
                .find(|stack| !stack.is_empty());
        }
        self.mark_dirty();
    }

    /// `DecoratedPotBlockEntity.wobble` (`DecoratedPotBlockEntity.java:160-164`): queues a
    /// synced block event so clients play the wobble animation; the client-side
    /// `triggerEvent` (`DecoratedPotBlockEntity.java:167-175`) consumes it.
    pub async fn wobble(&self, world: &Arc<World>, style: WobbleStyle) {
        world
            .add_synced_block_event(self.position, Self::EVENT_POT_WOBBLES, style.to_index())
            .await;
    }

    /// `getTheItem` (`DecoratedPotBlockEntity.java:132-136`).
    pub async fn get_item(&self) -> Option<ItemStack> {
        let mut item = self.item.lock().await;
        self.unpack_loot_table(&mut item);
        item.clone()
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

    /// `getDecorations().ordered()` as used by `DecoratedPotBlock.getDrops`
    /// (`DecoratedPotBlock.java:181-192`). A missing `sherds` tag is `PotDecorations.EMPTY`
    /// (`DecoratedPotBlockEntity.java:39-41,57`), and so is a list the codec rejects (more
    /// than four entries, a non-string or an unknown item; `PotDecorations.java:23-27`).
    /// Sides past the end of a shorter list are empty (`getItem`, `PotDecorations.java:41-48`),
    /// and `ordered` turns every empty side into a brick (`PotDecorations.java:50-52`).
    /// Returns `None` only when the sherds lock is contended.
    pub fn ordered_decorations(&self) -> Option<[Cow<'static, str>; 4]> {
        let sherds = self.sherds.try_lock().ok()?;
        Some(ordered_sherds(sherds.as_deref()))
    }

    /// Exports the item-backed components used by the creative include-data pick path.
    /// `DecoratedPotBlockEntity.collectImplicitComponents` writes both components
    /// (`DecoratedPotBlockEntity.java:112-116`), and the live caller is
    /// `JavaClient::handle_pick_item_from_block` (`ServerGamePacketListenerImpl.java:699-709`).
    /// It reads the field directly and does not unpack a pending loot table.
    pub(crate) async fn collect_implicit_components(
        &self,
    ) -> Vec<(DataComponent, Option<Box<dyn DataComponentImpl>>)> {
        let decorations = {
            let sherds = self.sherds.lock().await;
            sherds.as_ref().and_then(|sherds| {
                sherds
                    .iter()
                    .map(|tag| {
                        tag.extract_string()
                            .map(|value| Cow::Owned(value.to_string()))
                    })
                    .collect::<Option<Vec<_>>>()?
                    .try_into()
                    .ok()
            })
        };
        let item = self.item.lock().await.clone();
        let mut components = Vec::with_capacity(2);
        if let Some(decorations) = decorations {
            components.push((
                DataComponent::PotDecorations,
                Some(Box::new(PotDecorationsImpl { decorations }).to_dyn()),
            ));
        }
        if let Some(item) = item.filter(|item| !item.is_empty()) {
            components.push((
                DataComponent::Container,
                Some(
                    Box::new(ContainerImpl {
                        items: vec![(0, item)],
                    })
                    .to_dyn(),
                ),
            ));
        }
        components
    }

    /// `removeTheItem` (`ContainerSingleItem.java:17-19`).
    pub async fn take_item(&self) -> Option<ItemStack> {
        let mut item = self.item.lock().await;
        self.unpack_loot_table(&mut item);
        let taken = item.take();
        self.mark_dirty();
        taken
    }

    /// The insert branch of `DecoratedPotBlock.useItemOn` (`DecoratedPotBlock.java:110-130`):
    /// accepts one item when the pot is empty, or when it holds the same item and components
    /// below that stack's max size. `consumeAndReturn` leaves the hand untouched for players
    /// with infinite materials. Returns the sound pitch bend (`count / maxStackSize` of the
    /// pot's stack after the insert), or `None` when the pot refuses the item.
    pub async fn try_insert_item(&self, stack: &mut ItemStack, gamemode: GameMode) -> Option<f32> {
        let mut item_guard = self.item.lock().await;
        self.unpack_loot_table(&mut item_guard);
        if stack.is_empty() {
            return None;
        }
        let pitch_bend = if let Some(existing) =
            item_guard.as_mut().filter(|existing| !existing.is_empty())
        {
            if !existing.are_items_and_components_equal(stack)
                || existing.item_count >= existing.get_max_stack_size()
            {
                return None;
            }
            stack.decrement_unless_creative(gamemode, 1);
            existing.item_count += 1;
            f32::from(existing.item_count) / f32::from(existing.get_max_stack_size())
        } else {
            let inserted = stack.copy_with_count(1);
            stack.decrement_unless_creative(gamemode, 1);
            let pitch_bend =
                f32::from(inserted.item_count) / f32::from(inserted.get_max_stack_size());
            *item_guard = Some(inserted);
            pitch_bend
        };
        drop(item_guard);
        // `decoratedPot.setChanged()` (`DecoratedPotBlock.java:130`).
        self.mark_dirty();
        Some(pitch_bend)
    }

    /// `DecoratedPotBlock.getAnalogOutputSignal` (`DecoratedPotBlock.java:240-243`) is
    /// `AbstractContainerMenu.getRedstoneSignalFromBlockEntity`.
    pub async fn get_comparator_output(&self) -> u8 {
        crate::block::calculate_comparator_output(self).await
    }
}

/// `PotDecorations` decode followed by `ordered()`; see
/// [`DecoratedPotBlockEntity::ordered_decorations`].
fn ordered_sherds(sherds: Option<&[NbtTag]>) -> [Cow<'static, str>; 4] {
    const BRICK: &str = "minecraft:brick";
    let decoded = sherds.filter(|list| list.len() <= 4).and_then(|list| {
        list.iter()
            .map(|tag| {
                let id = tag.extract_string()?;
                pumpkin_data::item::Item::from_registry_key(
                    id.strip_prefix("minecraft:").unwrap_or(id),
                )?;
                Some(id.to_string())
            })
            .collect::<Option<Vec<_>>>()
    });
    let list = decoded.unwrap_or_default();
    std::array::from_fn(|i| {
        list.get(i)
            .map_or(Cow::Borrowed(BRICK), |id| Cow::Owned(id.clone()))
    })
}

/// `ContainerSingleItem` defaults (`ContainerSingleItem.java:8-65`) over `getTheItem`,
/// `splitTheItem` and `setTheItem` (`DecoratedPotBlockEntity.java:132-153`).
impl Inventory for DecoratedPotBlockEntity {
    fn size(&self) -> usize {
        1
    }

    fn is_empty(&self) -> InventoryFuture<'_, bool> {
        Box::pin(async move {
            let mut item = self.item.lock().await;
            self.unpack_loot_table(&mut item);
            item.as_ref().is_none_or(ItemStack::is_empty)
        })
    }

    fn get_stack(&self, slot: usize) -> InventoryFuture<'_, ItemStack> {
        Box::pin(async move {
            if slot != 0 {
                return ItemStack::EMPTY.clone();
            }
            let mut item = self.item.lock().await;
            self.unpack_loot_table(&mut item);
            item.clone().unwrap_or_else(|| ItemStack::EMPTY.clone())
        })
    }

    /// `removeItemNoUpdate` splits `getMaxStackSize()` (99), which is the whole stack.
    fn remove_stack(&self, slot: usize) -> InventoryFuture<'_, ItemStack> {
        Box::pin(async move {
            if slot != 0 {
                return ItemStack::EMPTY.clone();
            }
            self.take_item()
                .await
                .unwrap_or_else(|| ItemStack::EMPTY.clone())
        })
    }

    fn remove_stack_specific(&self, slot: usize, amount: u8) -> InventoryFuture<'_, ItemStack> {
        Box::pin(async move {
            if slot != 0 {
                return ItemStack::EMPTY.clone();
            }
            let mut item = self.item.lock().await;
            self.unpack_loot_table(&mut item);
            let Some(stack) = item.as_mut() else {
                return ItemStack::EMPTY.clone();
            };
            let result = stack.split(amount);
            if stack.is_empty() {
                *item = None;
            }
            drop(item);
            self.mark_dirty();
            result
        })
    }

    fn set_stack(&self, slot: usize, stack: ItemStack) -> InventoryFuture<'_, ()> {
        Box::pin(async move {
            if slot != 0 {
                return;
            }
            let mut item = self.item.lock().await;
            self.unpack_loot_table(&mut item);
            *item = if stack.is_empty() { None } else { Some(stack) };
            drop(item);
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

/// `clearContent` is `removeTheItem` (`ContainerSingleItem.java:31-34`).
impl Clearable for DecoratedPotBlockEntity {
    fn clear(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.take_item().await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::DecoratedPotBlockEntity;
    use crate::block::entities::BlockEntity;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_util::GameMode;
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_world::inventory::Inventory;

    /// `DecoratedPotBlock.useItemOn` (`DecoratedPotBlock.java:110-115`) rejects mismatched
    /// item components.
    #[tokio::test]
    async fn insertion_requires_matching_components() {
        let pot = DecoratedPotBlockEntity::new(BlockPos::new(0, 0, 0));
        let mut plain = ItemStack::new(1, &Item::COBBLESTONE);
        assert!(
            pot.try_insert_item(&mut plain, GameMode::Survival)
                .await
                .is_some()
        );

        let mut named = ItemStack::new(1, &Item::COBBLESTONE);
        named.set_custom_name("named".into());
        assert!(
            pot.try_insert_item(&mut named, GameMode::Survival)
                .await
                .is_none()
        );
    }

    /// `DecoratedPotBlock.useItemOn` (`DecoratedPotBlock.java:111-122`) inserts one item per
    /// use, up to the item's max stack size; the comparator then reads a full pot.
    #[tokio::test]
    async fn insertion_uses_the_item_max_stack_size() {
        let pot = DecoratedPotBlockEntity::new(BlockPos::new(0, 0, 0));
        let mut pearls = ItemStack::new(17, &Item::ENDER_PEARL);
        for _ in 0..16 {
            assert!(
                pot.try_insert_item(&mut pearls, GameMode::Survival)
                    .await
                    .is_some()
            );
        }
        assert_eq!(pearls.item_count, 1);
        assert!(
            pot.try_insert_item(&mut pearls, GameMode::Survival)
                .await
                .is_none()
        );
        assert_eq!(pearls.item_count, 1);
        assert_eq!(pot.get_comparator_output().await, 15);
    }

    /// `consumeAndReturn` (`ItemStack.java:1082-1092`) does not shrink for creative players.
    #[tokio::test]
    async fn creative_insertion_keeps_the_hand_stack() {
        let pot = DecoratedPotBlockEntity::new(BlockPos::new(0, 0, 0));
        let mut stone = ItemStack::new(2, &Item::COBBLESTONE);
        assert!(
            pot.try_insert_item(&mut stone, GameMode::Creative)
                .await
                .is_some()
        );
        assert_eq!(stone.item_count, 2);
        assert_eq!(pot.get_stack(0).await.item_count, 1);
    }

    /// `PotDecorations.EMPTY` and the `getItem` padding both drop bricks through
    /// `ordered()` (`PotDecorations.java:41-52`); a list the codec rejects is `EMPTY`.
    #[test]
    fn ordered_decorations_pad_with_bricks() {
        use pumpkin_nbt::tag::NbtTag;
        let pot = DecoratedPotBlockEntity::new(BlockPos::new(0, 0, 0));
        let ordered = pot.ordered_decorations().expect("uncontended lock");
        assert!(ordered.iter().all(|id| id == "minecraft:brick"));

        let short = [NbtTag::String("minecraft:angler_pottery_sherd".into())];
        let ordered = super::ordered_sherds(Some(&short));
        assert_eq!(ordered[0], "minecraft:angler_pottery_sherd");
        assert!(ordered[1..].iter().all(|id| id == "minecraft:brick"));

        let too_long = vec![NbtTag::String("minecraft:angler_pottery_sherd".into()); 5];
        let ordered = super::ordered_sherds(Some(&too_long));
        assert!(ordered.iter().all(|id| id == "minecraft:brick"));

        let not_string = [NbtTag::Int(3)];
        let ordered = super::ordered_sherds(Some(&not_string));
        assert!(ordered.iter().all(|id| id == "minecraft:brick"));
    }

    /// `splitTheItem` (`DecoratedPotBlockEntity.java:138-147`) leaves the remainder.
    #[tokio::test]
    async fn split_leaves_the_remainder() {
        let pot = DecoratedPotBlockEntity::new(BlockPos::new(0, 0, 0));
        pot.set_stack(0, ItemStack::new(2, &Item::COBBLESTONE)).await;
        assert_eq!(pot.remove_stack_specific(0, 1).await.item_count, 1);
        assert_eq!(pot.get_stack(0).await.item_count, 1);
        assert_eq!(pot.remove_stack_specific(0, 1).await.item_count, 1);
        assert!(pot.is_empty().await);
        assert!(pot.item.lock().await.is_none());
    }

    /// `loadAdditional` keeps a pending loot table instead of the item, and the first
    /// container access unpacks it (`DecoratedPotBlockEntity.java:55-64,132-136`).
    #[tokio::test]
    async fn trial_pot_loot_table_unpacks_on_access() {
        let mut nbt = NbtCompound::new();
        nbt.put_string("LootTable", "minecraft:pots/trial_chambers/corridor".to_string());
        nbt.put_long("LootTableSeed", 12345);
        let pot = DecoratedPotBlockEntity::from_nbt(&nbt, BlockPos::new(0, 0, 0));
        assert!(pot.has_loot_table());

        let mut saved = NbtCompound::new();
        pot.write_nbt(&mut saved).await;
        assert!(saved.get_string("LootTable").is_some());
        assert_eq!(saved.get_long("LootTableSeed"), Some(12345));

        assert!(!pot.get_stack(0).await.is_empty());
        assert!(!pot.has_loot_table());
    }
}
