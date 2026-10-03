use std::pin::Pin;
use std::{any::Any, sync::Arc};

use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{
    BeesImpl, BlockEntityDataImpl, ContainerImpl, ContainerLootImpl, CustomNameImpl,
    DataComponentImpl, NoteBlockSoundImpl, PotDecorationsImpl, ProfileImpl, read_data,
};
use pumpkin_data::{Block, BlockStateId, block_properties::BLOCK_ENTITY_TYPES};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use std::array::from_fn;

use crate::world::World;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_world::inventory::Inventory;

pub mod barrel;
pub mod beacon;
pub mod bed;
pub mod bell;
pub mod blasting_furnace;
pub mod brewing_stand;
pub mod chest;
pub mod chest_like_block_entity;
pub mod chiseled_bookshelf;
pub mod command_block;
pub mod comparator;
pub mod daylight_detector;
pub mod dropper;
pub mod end_portal;
pub mod ender_chest;
pub mod furnace;
pub mod furnace_like_block_entity;
pub mod hopper;
pub mod jigsaw_block;
pub mod jukebox;
pub mod lectern;
pub mod map;
pub mod mob_spawner;
pub mod piston;
pub mod shulker_box;
pub mod sign;
pub mod smoker;
pub mod trapped_chest;

pub mod banner;
pub mod beehive;
pub mod brushable_block;
pub mod calibrated_sculk_sensor;
pub mod campfire;
pub mod conduit;
pub mod copper_golem_statue;
pub mod crafter;
pub mod creaking_heart;
pub mod decorated_pot;
pub mod dispenser;
pub mod enchanting_table;
pub mod end_gateway;
pub mod hanging_sign;
pub mod potent_sulfur;
pub mod sculk_catalyst;
pub mod sculk_sensor;
pub mod sculk_shrieker;
pub mod shelf;
pub mod skull;
pub mod structure_block;
pub mod test_block;
pub mod test_instance_block;
pub mod trial_spawner;
pub mod vault;

pub use furnace_like_block_entity::ExperienceContainer;
pub use pumpkin_world::block::entities::PropertyDelegate;

//TODO: We need a mark_dirty for chests
pub trait BlockEntity: Any + Send + Sync {
    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized;
    fn tick<'a>(&'a self, _world: &'a Arc<World>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn resource_location(&self) -> &'static str;
    fn get_position(&self) -> BlockPos;

    /// Atomically takes the pending loot-table key and seed from this block entity.
    ///
    /// Returns `Some((key, seed))` if a deferred loot table was set, clearing it in the
    /// process. Returns `None` for entities that do not support loot tables, or if the
    /// loot has already been generated.
    fn take_loot_table(&self) -> Option<(String, i64)> {
        None
    }

    /// Returns `true` if this block entity has a pending deferred loot table that has
    /// not yet been unpacked. Does not consume the loot table.
    fn has_loot_table(&self) -> bool {
        false
    }

    fn write_internal<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            nbt.put_string("id", self.resource_location().to_string());
            let position = self.get_position();
            nbt.put_int("x", position.0.x);
            nbt.put_int("y", position.0.y);
            nbt.put_int("z", position.0.z);
            self.write_nbt(nbt).await;
        })
    }
    fn get_id(&self) -> u32 {
        let name = self
            .resource_location()
            .split(':')
            .next_back()
            .unwrap_or("");
        pumpkin_data::block_properties::BLOCK_ENTITY_TYPES
            .iter()
            .position(|block_entity_name| *block_entity_name == name)
            .unwrap_or(0) as u32
    }

    /// Mirrors `BlockEntity.isValidBlockState`: the NBT type must belong to the
    /// block state currently occupying the position.
    fn is_valid_block_state(&self, block_state: BlockStateId) -> bool {
        let block_entity_type = pumpkin_data::BlockState::from_id(block_state).block_entity_type;
        block_entity_type != u16::MAX && block_entity_type == self.get_id() as u16
    }

    /// Obtain NBT data for sending to the client in `ChunkData`
    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        None
    }

    /// Obtain the client update tag with the block state available when vanilla
    /// derives state-dependent fields.
    fn chunk_data_nbt_with_state(&self, _block_state: BlockStateId) -> Option<NbtCompound> {
        self.chunk_data_nbt()
    }

    /// Obtain block actor NBT for fields Bedrock does not include in its block state.
    fn bedrock_block_actor_data(&self, _state_id: BlockStateId) -> Option<NbtCompound> {
        None
    }

    fn get_inventory(self: Arc<Self>) -> Option<Arc<dyn Inventory>> {
        None
    }
    fn set_block_state(&mut self, _block_state: BlockStateId) {}

    /// Mirrors `BlockEntity.preRemoveSideEffects` (`BlockEntity.java:233-237`): the base
    /// implementation drops every slot of a container block entity. `LevelChunk.setBlockState`
    /// runs it before the entity is removed, and only while the update flags lack
    /// `UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS` (`LevelChunk.java:305-316`), which
    /// `BlockFlags::SKIP_BLOCK_ENTITY_REPLACED_CALLBACK` models.
    fn pre_remove_side_effects<'a>(
        self: Arc<Self>,
        world: Arc<World>,
        position: BlockPos,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            if let Some(inventory) = self.get_inventory() {
                world.scatter_inventory(&position, &inventory).await;
            }
        })
    }

    /// Runs whenever the block entity is removed from its chunk, whatever the update flags
    /// (`LevelChunk.removeBlockEntity`, `LevelChunk.java:470-481`): the `setRemoved` overrides
    /// (beacon deactivation, jukebox stop event) and game event listener removal. Dropping
    /// contents belongs to [`Self::pre_remove_side_effects`].
    fn on_block_replaced<'a>(
        self: Arc<Self>,
        _world: Arc<World>,
        _position: BlockPos,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(async {})
    }
    fn is_dirty(&self) -> bool {
        false
    }

    fn clear_dirty(&self) {
        // Default implementation does nothing
        // Override in implementations that have a dirty flag
    }

    fn as_any(&self) -> &dyn Any;
    fn to_property_delegate(self: Arc<Self>) -> Option<Arc<dyn PropertyDelegate>> {
        None
    }
    fn to_experience_container(self: Arc<Self>) -> Option<Arc<dyn ExperienceContainer>> {
        None
    }
}

/// Applies modeled block-entity components from a placed item.
///
/// `BlockItem.updateBlockEntityComponents` calls
/// `BlockEntity.applyComponentsFromItemStack` before `setPlacedBy` and the block-place game
/// event; randomizable containers apply their `ContainerLoot` component as the vanilla
/// `LootTable` fields (`BlockItem.java:101-106`; `BlockEntity.java:276-300`;
/// `RandomizableContainerBlockEntity.java:98-112`).
#[must_use]
/// `SkullBlockEntity.applyImplicitComponents` restores the profile, note block sound and
/// custom name from the placed item (`SkullBlockEntity.java:97-103`).
fn apply_skull_components(
    entity: &dyn BlockEntity,
    stack: &ItemStack,
) -> Option<Arc<dyn BlockEntity>> {
    let profile = stack
        .get_data_component::<ProfileImpl>()
        .map(pumpkin_data::data_component_impl::DataComponentImpl::write_data);
    let note_block_sound = stack
        .get_data_component::<NoteBlockSoundImpl>()
        .map(pumpkin_data::data_component_impl::DataComponentImpl::write_data);
    let custom_name = stack
        .get_data_component::<CustomNameImpl>()
        .map(pumpkin_data::data_component_impl::DataComponentImpl::write_data);

    if profile.is_some() || note_block_sound.is_some() || custom_name.is_some() {
        // `SkullBlockEntity.applyImplicitComponents` copies these three components
        // (`SkullBlockEntity.java:82-87`); `BlockItem.updateBlockEntityComponents` invokes it
        // for the freshly placed entity (`BlockItem.java:101-106`).
        let position = entity.get_position();
        let block_entity_data = stack.get_data_component::<BlockEntityDataImpl>();
        let mut nbt = block_entity_data.map_or_else(NbtCompound::new, |data| data.nbt.clone());
        if let Some(id) = nbt.get_string("id")
            && id != entity.resource_location()
        {
            return None;
        }
        nbt.put_string("id", entity.resource_location().to_string());
        nbt.put_int("x", position.0.x);
        nbt.put_int("y", position.0.y);
        nbt.put_int("z", position.0.z);
        if let Some(profile) = profile {
            nbt.put("profile", profile);
        }
        if let Some(note_block_sound) = note_block_sound {
            nbt.put("note_block_sound", note_block_sound);
        }
        if let Some(custom_name) = custom_name {
            nbt.put("custom_name", custom_name);
        }
        return block_entity_from_nbt_at(&nbt, position);
    }

    None
}

pub fn apply_components_from_item_stack(
    entity: &dyn BlockEntity,
    stack: &ItemStack,
) -> Option<Arc<dyn BlockEntity>> {
    apply_components_from_item_stack_with_permission(entity, stack, true)
}

/// Applies item components while enforcing the placement permission for typed block-entity data
/// (`BlockItem.java:101-106, 148-170`).
#[expect(clippy::too_many_lines)]
pub(crate) fn apply_components_from_item_stack_with_permission(
    entity: &dyn BlockEntity,
    stack: &ItemStack,
    can_use_game_master_blocks: bool,
) -> Option<Arc<dyn BlockEntity>> {
    // Falls through to the generic component path when the skull carries none of its three
    // implicit components, matching the original single-function control flow.
    if entity.as_any().is::<skull::SkullBlockEntity>()
        && let Some(applied) = apply_skull_components(entity, stack)
    {
        return Some(applied);
    }

    // `BeehiveBlockEntity.applyImplicitComponents` replaces stored occupants from the item
    // component (`BeehiveBlockEntity.java:309-315`).
    if entity.as_any().is::<beehive::BeehiveBlockEntity>() {
        let data = stack.get_data_component::<BeesImpl>()?;
        let position = entity.get_position();
        let mut nbt = NbtCompound::new();
        nbt.put_string("id", entity.resource_location().to_string());
        nbt.put_int("x", position.0.x);
        nbt.put_int("y", position.0.y);
        nbt.put_int("z", position.0.z);
        nbt.put("bees", data.write_data());
        return block_entity_from_nbt_at(&nbt, position);
    }

    // `ShelfBlockEntity.applyImplicitComponents` copies the item container into its three slots
    // (`ShelfBlockEntity.java:104-107`), and BlockItem invokes that hook before `setChanged`
    // (`BlockItem.java:101-106`).
    if entity.as_any().is::<shelf::ShelfBlockEntity>() {
        let position = entity.get_position();
        let mut nbt = NbtCompound::new();
        nbt.put_string("id", entity.resource_location().to_string());
        nbt.put_int("x", position.0.x);
        nbt.put_int("y", position.0.y);
        nbt.put_int("z", position.0.z);
        let container = stack.get_data_component::<ContainerImpl>();
        let items: [ItemStack; shelf::ShelfBlockEntity::INVENTORY_SIZE] = from_fn(|slot| {
            container
                .and_then(|container| {
                    container
                        .items
                        .iter()
                        .find(|(item_slot, _)| usize::from(*item_slot) == slot)
                        .map(|(_, item)| item.clone())
                })
                .unwrap_or_else(|| ItemStack::EMPTY.clone())
        });
        pumpkin_world::inventory::sync_write_items_to_nbt(&items, &mut nbt);
        return block_entity_from_nbt_at(&nbt, position);
    }

    // `ChiseledBookShelfBlockEntity.applyImplicitComponents` copies the container component into
    // its six slots (`ChiseledBookShelfBlockEntity.java:122-126`), before the placed block is
    // finalized by the live `BlockItem` placement path.
    if entity
        .as_any()
        .is::<chiseled_bookshelf::ChiseledBookshelfBlockEntity>()
    {
        let position = entity.get_position();
        let mut nbt = NbtCompound::new();
        nbt.put_string("id", entity.resource_location().to_string());
        nbt.put_int("x", position.0.x);
        nbt.put_int("y", position.0.y);
        nbt.put_int("z", position.0.z);
        let container = stack.get_data_component::<ContainerImpl>();
        let items: [ItemStack; chiseled_bookshelf::ChiseledBookshelfBlockEntity::INVENTORY_SIZE] =
            from_fn(|slot| {
                container
                    .and_then(|container| {
                        container
                            .items
                            .iter()
                            .find(|(item_slot, _)| usize::from(*item_slot) == slot)
                            .map(|(_, item)| item.clone())
                    })
                    .unwrap_or_else(|| ItemStack::EMPTY.clone())
            });
        pumpkin_world::inventory::sync_write_items_to_nbt(&items, &mut nbt);
        return block_entity_from_nbt_at(&nbt, position);
    }

    if entity.as_any().is::<campfire::CampfireBlockEntity>()
        && let Some(container) = stack.get_data_component::<ContainerImpl>()
    {
        // `CampfireBlockEntity.applyImplicitComponents` copies CONTAINER into its four slots
        // (`CampfireBlockEntity.java:207-210`) during `BlockItem.updateBlockEntityComponents`.
        let position = entity.get_position();
        let items: [ItemStack; 4] = from_fn(|slot| {
            container
                .items
                .iter()
                .find(|(item_slot, _)| usize::from(*item_slot) == slot)
                .map_or_else(|| ItemStack::EMPTY.clone(), |(_, item)| item.clone())
        });
        let mut nbt = stack
            .get_data_component::<BlockEntityDataImpl>()
            .map_or_else(NbtCompound::new, |data| data.nbt.clone());
        if let Some(id) = nbt.get_string("id")
            && id != entity.resource_location()
        {
            return None;
        }
        nbt.put_string("id", entity.resource_location().to_string());
        nbt.put_int("x", position.0.x);
        nbt.put_int("y", position.0.y);
        nbt.put_int("z", position.0.z);
        pumpkin_world::inventory::sync_write_items_to_nbt(&items, &mut nbt);
        return block_entity_from_nbt_at(&nbt, position);
    }

    if entity
        .as_any()
        .is::<decorated_pot::DecoratedPotBlockEntity>()
        && (stack.get_data_component::<PotDecorationsImpl>().is_some()
            || stack.get_data_component::<ContainerImpl>().is_some())
    {
        let position = entity.get_position();
        let mut nbt = NbtCompound::new();
        nbt.put_string("id", entity.resource_location().to_string());
        nbt.put_int("x", position.0.x);
        nbt.put_int("y", position.0.y);
        nbt.put_int("z", position.0.z);
        if let Some(decorations) = stack.get_data_component::<PotDecorationsImpl>() {
            nbt.put_list(
                "sherds",
                decorations
                    .decorations
                    .iter()
                    .map(|decoration| NbtTag::String(decoration.to_string().into_boxed_str()))
                    .collect(),
            );
        }
        if let Some(container) = stack.get_data_component::<ContainerImpl>()
            && let Some((_, item)) = container.items.iter().find(|(slot, _)| *slot == 0)
        {
            let mut item_nbt = NbtCompound::new();
            item.write_item_stack(&mut item_nbt);
            nbt.put_compound("item", item_nbt);
        }
        // `DecoratedPotBlockEntity.applyImplicitComponents` restores POT_DECORATIONS and the
        // one-slot CONTAINER component (`DecoratedPotBlockEntity.java:119-123`) during the live
        // placement hook (`BlockItem.java:101-106`).
        return block_entity_from_nbt_at(&nbt, position);
    }

    if entity.as_any().is::<shulker_box::ShulkerBoxBlockEntity>()
        && (stack.get_data_component::<ContainerImpl>().is_some()
            || stack.get_data_component::<ContainerLootImpl>().is_some())
    {
        let position = entity.get_position();
        let container = stack.get_data_component::<ContainerImpl>();
        let items: [ItemStack; shulker_box::ShulkerBoxBlockEntity::INVENTORY_SIZE] =
            from_fn(|slot| {
                container
                    .and_then(|container| {
                        container
                            .items
                            .iter()
                            .find(|(item_slot, _)| usize::from(*item_slot) == slot)
                            .map(|(_, item)| item.clone())
                    })
                    .unwrap_or_else(|| ItemStack::EMPTY.clone())
            });
        let mut nbt = stack
            .get_data_component::<BlockEntityDataImpl>()
            .map_or_else(NbtCompound::new, |data| data.nbt.clone());
        if let Some(id) = nbt.get_string("id")
            && id != entity.resource_location()
        {
            return None;
        }
        nbt.put_string("id", entity.resource_location().to_string());
        nbt.put_int("x", position.0.x);
        nbt.put_int("y", position.0.y);
        nbt.put_int("z", position.0.z);
        pumpkin_world::inventory::sync_write_items_to_nbt(&items, &mut nbt);
        if let Some(loot) = stack.get_data_component::<ContainerLootImpl>() {
            nbt.put_string("LootTable", loot.loot_table.clone());
            if loot.seed != 0 {
                nbt.put_long("LootTableSeed", loot.seed);
            }
        }
        // `BaseContainerBlockEntity.applyImplicitComponents` and
        // `RandomizableContainerBlockEntity.applyImplicitComponents`
        // (`BaseContainerBlockEntity.java:149-164`; `RandomizableContainerBlockEntity.java:97-105`)
        // apply both container contents and deferred loot during placement.
        return block_entity_from_nbt_at(&nbt, position);
    }

    // `BlockItem.updateCustomBlockEntityTag` rejects typed data for op-only block entities unless
    // the player can use game-master blocks (`BlockItem.java:148-170`; `BlockEntityTypes.java:211-211`).
    let block_entity_data = (can_apply_custom_block_entity_data(
        entity.resource_location(),
        can_use_game_master_blocks,
    ))
    .then(|| stack.get_data_component::<BlockEntityDataImpl>())
    .flatten();
    let container_loot = stack.get_data_component::<ContainerLootImpl>();
    let custom_name = stack.get_data_component::<CustomNameImpl>();
    // `BaseContainerBlockEntity.applyImplicitComponents` copies CONTAINER into the entity's
    // slots (`BaseContainerBlockEntity.java:149-154`).
    let container = container_kind(entity.resource_location())
        .and_then(|_| stack.get_data_component::<ContainerImpl>());
    if block_entity_data.is_none()
        && container_loot.is_none()
        && custom_name.is_none()
        && container.is_none()
    {
        return None;
    }

    let position = entity.get_position();
    // `BlockItem.updateCustomBlockEntityTag` validates and loads the typed block-entity
    // component before `updateBlockEntityComponents` applies implicit components
    // (`BlockItem.java:101-106, 148-170`). Rebuild the existing entity from both modeled
    // component payloads so the live placement path preserves both kinds of data.
    let mut nbt = block_entity_data.map_or_else(NbtCompound::new, |data| data.nbt.clone());
    if let Some(id) = nbt.get_string("id")
        && id != entity.resource_location()
    {
        return None;
    }
    nbt.put_string("id", entity.resource_location().to_string());
    nbt.put_int("x", position.0.x);
    nbt.put_int("y", position.0.y);
    nbt.put_int("z", position.0.z);
    if let Some(data) = container_loot {
        nbt.put_string("LootTable", data.loot_table.clone());
        if data.seed != 0 {
            nbt.put_long("LootTableSeed", data.seed);
        }
    }
    if let Some(container) = container {
        // `ItemContainerContents.copyInto` fills the entity's own slots only, so a slot the
        // entity does not have is dropped when it reads `Items` back.
        let items = container
            .items
            .iter()
            .map(|(slot, item)| {
                let mut item_nbt = NbtCompound::new();
                item_nbt.put_byte("Slot", *slot as i8);
                item.write_item_stack(&mut item_nbt);
                NbtTag::Compound(item_nbt)
            })
            .collect();
        nbt.put_list("Items", items);
    }
    if matches!(
        entity.resource_location(),
        banner::BannerBlockEntity::ID | beacon::BeaconBlockEntity::ID
    ) {
        // Banner and beacon take their name from CUSTOM_NAME alone
        // (`BannerBlockEntity.java:89-93`; `BeaconBlockEntity.java:369-374`).
        nbt.child_tags.remove("CustomName");
        if let Some(name) = custom_name
            && let Ok(name) = serde_json::to_string(&name.name)
        {
            nbt.put_string("CustomName", name);
        }
    }
    let rebuilt = block_entity_from_nbt_at(&nbt, position)?;
    if let Some(command_block) = rebuilt
        .as_any()
        .downcast_ref::<command_block::CommandBlockEntity>()
    {
        // `BlockEntity.applyImplicitComponents` is invoked by
        // `BlockItem.updateBlockEntityComponents` after the typed payload is loaded
        // (`CommandBlockEntity.java:160-164`; `BlockItem.java:101-106`).
        command_block.apply_implicit_components(stack);
    } else if let Some(enchanting_table) = rebuilt
        .as_any()
        .downcast_ref::<enchanting_table::EnchantingTableBlockEntity>(
    ) {
        // `EnchantingTableBlockEntity.applyImplicitComponents` copies CUSTOM_NAME
        // (`EnchantingTableBlockEntity.java:123-126`) during `BlockItem.updateBlockEntityComponents`
        // (`BlockItem.java:101-106`).
        enchanting_table.apply_implicit_components(stack);
    }
    Some(rebuilt)
}

// `BlockItem.updateCustomBlockEntityTag` uses the `OP_ONLY_CUSTOM_DATA` set for this gate
// (`BlockItem.java:162-166`; `BlockEntityTypes.java:211-211`).
fn can_apply_custom_block_entity_data(
    resource_location: &str,
    can_use_game_master_blocks: bool,
) -> bool {
    can_use_game_master_blocks
        || !matches!(
            resource_location,
            "minecraft:command_block"
                | "minecraft:lectern"
                | "minecraft:sign"
                | "minecraft:hanging_sign"
                | "minecraft:mob_spawner"
                | "minecraft:trial_spawner"
        )
}

/// The component types a block entity class reads in its `applyImplicitComponents` override.
/// Vanilla records them through the getter it hands the entity (`BlockEntity.java:285-297`);
/// `BLOCK_ENTITY_DATA` and `BLOCK_STATE` are always implicit (`BlockEntity.java:282-283`).
fn implicit_components(resource_location: &str) -> &'static [DataComponent] {
    match resource_location {
        // `BannerBlockEntity.java:89-93`.
        banner::BannerBlockEntity::ID => {
            &[DataComponent::BannerPatterns, DataComponent::CustomName]
        }
        // `BaseContainerBlockEntity.java:149-154` plus, for the randomizable classes,
        // `RandomizableContainerBlockEntity.java:98-105`. These classes also read CUSTOM_NAME and
        // LOCK, but the Pumpkin entities have no name or lock field to hold them, so both are
        // left out here and stay in the stored `components` map, where `collect_components`
        // exports them again (a renamed chest item still drops renamed).
        chest::ChestBlockEntity::ID
        | trapped_chest::TrappedChestBlockEntity::ID
        | barrel::BarrelBlockEntity::ID
        | shulker_box::ShulkerBoxBlockEntity::ID
        | dispenser::DispenserBlockEntity::ID
        | dropper::DropperBlockEntity::ID
        | hopper::HopperBlockEntity::ID
        | crafter::CrafterBlockEntity::ID => {
            &[DataComponent::Container, DataComponent::ContainerLoot]
        }
        // `BeaconBlockEntity.java:369-374`.
        beacon::BeaconBlockEntity::ID => &[DataComponent::CustomName, DataComponent::Lock],
        // `BeehiveBlockEntity.java:310-315`.
        beehive::BeehiveBlockEntity::ID => &[DataComponent::Bees],
        // `CampfireBlockEntity.java:207-210`; `ChiseledBookShelfBlockEntity.java:123-127`;
        // `ShelfBlockEntity.java:104-107`; the non-randomizable `BaseContainerBlockEntity`
        // classes (`BaseContainerBlockEntity.java:149-154`) read CONTAINER alone here.
        campfire::CampfireBlockEntity::ID
        | chiseled_bookshelf::ChiseledBookshelfBlockEntity::ID
        | shelf::ShelfBlockEntity::ID
        | furnace::FurnaceBlockEntity::ID
        | blasting_furnace::BlastingFurnaceBlockEntity::ID
        | smoker::SmokerBlockEntity::ID
        | brewing_stand::BrewingStandBlockEntity::ID => &[DataComponent::Container],
        // `CommandBlockEntity.java:161-164`; `EnchantingTableBlockEntity.java:123-126`.
        command_block::CommandBlockEntity::ID
        | enchanting_table::EnchantingTableBlockEntity::ID => &[DataComponent::CustomName],
        // `DecoratedPotBlockEntity.java:119-123`.
        decorated_pot::DecoratedPotBlockEntity::ID => {
            &[DataComponent::PotDecorations, DataComponent::Container]
        }
        // `SkullBlockEntity.java:82-87`.
        skull::SkullBlockEntity::ID => &[
            DataComponent::Profile,
            DataComponent::NoteBlockSound,
            DataComponent::CustomName,
        ],
        _ => &[],
    }
}

/// `BlockEntity.applyComponents` (`BlockEntity.java:298-300`): the item components the entity did
/// not consume as implicit ones become the entity's own `components` map, serialized like
/// `DataComponentMap.CODEC`. Removal markers, and components without a serialized value (the
/// payload-less ones), have no entry in such a map.
pub(crate) fn leftover_components(entity: &dyn BlockEntity, stack: &ItemStack) -> NbtCompound {
    let implicit = implicit_components(entity.resource_location());
    let mut components = NbtCompound::new();
    for (component, data) in &stack.patch {
        let Some(data) = data else {
            continue;
        };
        if matches!(
            component,
            DataComponent::BlockEntityData | DataComponent::BlockState
        ) || implicit.contains(component)
        {
            continue;
        }
        let value = data.write_data();
        if !matches!(value, NbtTag::End) {
            components.put(component.to_name(), value);
        }
    }
    components
}

/// Reads a stored `components` map back into typed components. Entries this build cannot decode
/// stay in the stored map; they are only left out of the result.
pub(crate) fn components_from_nbt(
    components: &NbtCompound,
) -> Vec<(DataComponent, Option<Box<dyn DataComponentImpl>>)> {
    let mut decoded: Vec<_> = components
        .child_tags
        .iter()
        .filter_map(|(name, value)| {
            let component = DataComponent::try_from_name(name)?;
            Some((component, Some(read_data(component, value)?)))
        })
        .collect();
    decoded.sort_by_key(|(component, _)| component.to_id());
    decoded
}

/// `BlockEntity.collectComponents` (`BlockEntity.java:309-314`): the stored `components` map, then
/// the implicit components, which replace a stored component of the same type.
pub(crate) async fn collect_components(
    world: &World,
    entity: &dyn BlockEntity,
) -> Vec<(DataComponent, Option<Box<dyn DataComponentImpl>>)> {
    let implicit = collect_components_from_block_entity(entity).await;
    let mut components = world
        .get_block_entity_components(&entity.get_position())
        .map(|stored| components_from_nbt(&stored))
        .unwrap_or_default();
    components.retain(|(component, _)| implicit.iter().all(|(other, _)| other != component));
    components.extend(implicit);
    components
}

/// The container classes whose `Items` travel as the `CONTAINER` component. `Randomizable` marks
/// the `RandomizableContainerBlockEntity` subclasses, which also carry `CONTAINER_LOOT`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ContainerKind {
    Base,
    Randomizable,
}

/// `BaseContainerBlockEntity` and `RandomizableContainerBlockEntity` subclasses; the shelf-like
/// and single-item containers have their own handling.
fn container_kind(resource_location: &str) -> Option<ContainerKind> {
    match resource_location {
        chest::ChestBlockEntity::ID
        | trapped_chest::TrappedChestBlockEntity::ID
        | barrel::BarrelBlockEntity::ID
        | shulker_box::ShulkerBoxBlockEntity::ID
        | dispenser::DispenserBlockEntity::ID
        | dropper::DropperBlockEntity::ID
        | hopper::HopperBlockEntity::ID
        | crafter::CrafterBlockEntity::ID => Some(ContainerKind::Randomizable),
        furnace::FurnaceBlockEntity::ID
        | blasting_furnace::BlastingFurnaceBlockEntity::ID
        | smoker::SmokerBlockEntity::ID
        | brewing_stand::BrewingStandBlockEntity::ID => Some(ContainerKind::Base),
        _ => None,
    }
}

/// The `CUSTOM_NAME` component for a name a block entity keeps as text or JSON.
pub(crate) fn custom_name_component(
    name: String,
) -> (DataComponent, Option<Box<dyn DataComponentImpl>>) {
    let name = serde_json::from_str::<pumpkin_util::text::TextComponent>(&name)
        .unwrap_or_else(|_| pumpkin_util::text::TextComponent::text(name));
    (
        DataComponent::CustomName,
        Some(Box::new(CustomNameImpl { name }).to_dyn()),
    )
}

/// `BaseContainerBlockEntity.collectImplicitComponents` and, for the randomizable classes,
/// `RandomizableContainerBlockEntity.collectImplicitComponents`
/// (`BaseContainerBlockEntity.java:157-165`; `RandomizableContainerBlockEntity.java:107-113`),
/// read from the entity's saved `Items` and loot table so every container shares one path.
async fn collect_container_components(
    entity: &dyn BlockEntity,
    kind: ContainerKind,
) -> Vec<(DataComponent, Option<Box<dyn DataComponentImpl>>)> {
    let mut nbt = NbtCompound::new();
    entity.write_nbt(&mut nbt).await;
    let items = nbt
        .get_list("Items")
        .unwrap_or_default()
        .iter()
        .filter_map(|tag| {
            let item = tag.extract_compound()?;
            let slot = item.get_byte("Slot")? as u8;
            Some((slot, ItemStack::read_item_stack(item)?))
        })
        .collect();
    let mut components = vec![(
        DataComponent::Container,
        Some(Box::new(ContainerImpl { items }).to_dyn()),
    )];
    if kind == ContainerKind::Randomizable
        && let Some(loot_table) = nbt.get_string("LootTable")
    {
        components.push((
            DataComponent::ContainerLoot,
            Some(
                Box::new(ContainerLootImpl {
                    loot_table: loot_table.to_string(),
                    seed: nbt.get_long("LootTableSeed").unwrap_or(0),
                })
                .to_dyn(),
            ),
        ));
    }
    components
}

/// Collects the component used by the beehive creative-break item round trip. This is the live
/// `collectImplicitComponents` path (`BeehiveBlockEntity.java:317-321`).
#[expect(clippy::too_many_lines)]
pub(crate) async fn collect_components_from_block_entity(
    entity: &dyn BlockEntity,
) -> Vec<(
    pumpkin_data::data_component::DataComponent,
    Option<Box<dyn DataComponentImpl>>,
)> {
    if let Some(enchanting_table) = entity
        .as_any()
        .downcast_ref::<enchanting_table::EnchantingTableBlockEntity>()
    {
        // `EnchantingTableBlockEntity.collectImplicitComponents` exports CUSTOM_NAME
        // (`EnchantingTableBlockEntity.java:128-132`); the live caller is the creative
        // include-data pick-item path (`ServerGamePacketListenerImpl.java:715-723`).
        let Some(name) = enchanting_table.custom_name.lock().await.clone() else {
            return Vec::new();
        };
        let name = serde_json::from_str::<pumpkin_util::text::TextComponent>(&name)
            .unwrap_or_else(|_| pumpkin_util::text::TextComponent::text(name));
        return vec![(
            pumpkin_data::data_component::DataComponent::CustomName,
            Some(Box::new(CustomNameImpl { name }).to_dyn()),
        )];
    }

    if let Some(skull) = entity.as_any().downcast_ref::<skull::SkullBlockEntity>() {
        // `SkullBlockEntity.collectImplicitComponents` exports PROFILE, NOTE_BLOCK_SOUND, and
        // CUSTOM_NAME (`SkullBlockEntity.java:90-95`); the pick-block path consumes these
        // components after `removeComponentsFromTag` removes them from the raw tag
        // (`SkullBlockEntity.java:97-103`).
        let mut components = Vec::new();
        let profile_value = skull.profile.lock().await.clone();
        if let Some(profile) = profile_value {
            components.push((
                pumpkin_data::data_component::DataComponent::Profile,
                ProfileImpl::read_data(&pumpkin_nbt::tag::NbtTag::Compound(profile))
                    .map(|profile| Box::new(profile).to_dyn()),
            ));
        }
        let note_block_sound_value = skull.note_block_sound.lock().await.clone();
        if let Some(sound) = note_block_sound_value {
            components.push((
                pumpkin_data::data_component::DataComponent::NoteBlockSound,
                Some(Box::new(NoteBlockSoundImpl { sound }).to_dyn()),
            ));
        }
        let custom_name_value = skull.custom_name.lock().await.clone();
        if let Some(name) = custom_name_value {
            let name = serde_json::from_str::<pumpkin_util::text::TextComponent>(&name)
                .unwrap_or_else(|_| pumpkin_util::text::TextComponent::text(name));
            components.push((
                pumpkin_data::data_component::DataComponent::CustomName,
                Some(Box::new(CustomNameImpl { name }).to_dyn()),
            ));
        }
        return components;
    }

    if let Some(campfire) = entity
        .as_any()
        .downcast_ref::<campfire::CampfireBlockEntity>()
    {
        // `CampfireBlockEntity.collectImplicitComponents` exports CONTAINER from all four slots
        // (`CampfireBlockEntity.java:212-215`) for the creative include-data pick path.
        let mut items = Vec::new();
        for (slot, item) in campfire.items.iter().enumerate() {
            let item = item.lock().await;
            if !item.is_empty() {
                items.push((slot as u8, item.clone()));
            }
        }
        return vec![(
            pumpkin_data::data_component::DataComponent::Container,
            Some(Box::new(ContainerImpl { items }).to_dyn()),
        )];
    }

    if let Some(pot) = entity
        .as_any()
        .downcast_ref::<decorated_pot::DecoratedPotBlockEntity>()
    {
        // `DecoratedPotBlockEntity.collectImplicitComponents` exports POT_DECORATIONS and
        // CONTAINER (`DecoratedPotBlockEntity.java:112-116`); the live collector is used by
        // the creative include-data pick-item path (`ServerGamePacketListenerImpl.java:699-709`).
        return pot.collect_implicit_components().await;
    }

    if let Some(shulker) = entity
        .as_any()
        .downcast_ref::<shulker_box::ShulkerBoxBlockEntity>()
    {
        let items = shulker.items.read().await;
        let container = ContainerImpl {
            items: items
                .iter()
                .enumerate()
                .filter(|(_, item)| !item.is_empty())
                .map(|(slot, item)| (slot as u8, item.clone()))
                .collect(),
        };
        let mut components = vec![(
            pumpkin_data::data_component::DataComponent::Container,
            Some(Box::new(container).to_dyn()),
        )];
        let loot_table = shulker
            .loot_table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(loot_table) = loot_table {
            // `RandomizableContainerBlockEntity.collectImplicitComponents`
            // (`RandomizableContainerBlockEntity.java:107-113`) exports deferred loot alongside
            // the base container component (`BaseContainerBlockEntity.java:157-165`).
            components.push((
                pumpkin_data::data_component::DataComponent::ContainerLoot,
                Some(
                    Box::new(ContainerLootImpl {
                        loot_table,
                        seed: shulker.loot_table_seed,
                    })
                    .to_dyn(),
                ),
            ));
        }
        return components;
    }

    if let Some(banner) = entity.as_any().downcast_ref::<banner::BannerBlockEntity>() {
        // `BannerBlockEntity.collectImplicitComponents` exports BANNER_PATTERNS and CUSTOM_NAME
        // (`BannerBlockEntity.java:96-100`). `BannerPatternsImpl` has no payload yet, so the
        // layers stay in the raw tag (see `block_entity_data_component`) and only the name is
        // exported.
        let name = banner.custom_name.lock().await.clone();
        return name.map(custom_name_component).into_iter().collect();
    }

    if let Some(beacon) = entity.as_any().downcast_ref::<beacon::BeaconBlockEntity>() {
        // `BeaconBlockEntity.collectImplicitComponents` exports CUSTOM_NAME and a non-default
        // LOCK (`BeaconBlockEntity.java:376-382`); the lock stays in the raw tag.
        let name = beacon.custom_name.lock().await.clone();
        return name.map(custom_name_component).into_iter().collect();
    }

    if let Some(command_block) = entity
        .as_any()
        .downcast_ref::<command_block::CommandBlockEntity>()
    {
        // `CommandBlockEntity.collectImplicitComponents` exports CUSTOM_NAME
        // (`CommandBlockEntity.java:167-171`).
        let name = command_block
            .custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        return name
            .map(|name| {
                (
                    DataComponent::CustomName,
                    Some(Box::new(CustomNameImpl { name }).to_dyn()),
                )
            })
            .into_iter()
            .collect();
    }

    // `ChiseledBookShelfBlockEntity.java:129-133` and `ShelfBlockEntity.java:110-114` export
    // CONTAINER; the plain containers add their own (`BaseContainerBlockEntity.java:157-165`).
    if entity
        .as_any()
        .is::<chiseled_bookshelf::ChiseledBookshelfBlockEntity>()
        || entity.as_any().is::<shelf::ShelfBlockEntity>()
    {
        return collect_container_components(entity, ContainerKind::Base).await;
    }
    if let Some(kind) = container_kind(entity.resource_location()) {
        return collect_container_components(entity, kind).await;
    }

    let Some(hive) = entity
        .as_any()
        .downcast_ref::<beehive::BeehiveBlockEntity>()
    else {
        return Vec::new();
    };
    vec![(
        pumpkin_data::data_component::DataComponent::Bees,
        Some(Box::new(hive.bees_component().await).to_dyn()),
    )]
}

/// Serializes the custom block-entity payload used by creative pick-block. Vanilla writes
/// `saveCustomOnly`, removes fields exported as implicit components, and stores the remainder as
/// block-entity data (`ServerGamePacketListenerImpl.java:715-724`; `BlockEntity.java:141-151,
/// 302-314`).
pub(crate) async fn block_entity_data_component(
    entity: &dyn BlockEntity,
) -> Option<(
    pumpkin_data::data_component::DataComponent,
    Option<Box<dyn DataComponentImpl>>,
)> {
    let mut nbt = NbtCompound::new();
    entity.write_nbt(&mut nbt).await;

    // These fields are represented by the implicit components collected above. The removals
    // match the concrete vanilla overrides (`BeehiveBlockEntity.java:317-327`;
    // `SkullBlockEntity.java:90-103`; `EnchantingTableBlockEntity.java:129-137`).
    if entity.as_any().is::<beehive::BeehiveBlockEntity>() {
        nbt.child_tags.remove("bees");
    } else if entity.as_any().is::<skull::SkullBlockEntity>() {
        nbt.child_tags.remove("profile");
        nbt.child_tags.remove("note_block_sound");
        nbt.child_tags.remove("custom_name");
    } else if entity
        .as_any()
        .is::<enchanting_table::EnchantingTableBlockEntity>()
    {
        nbt.child_tags.remove("CustomName");
    } else if entity.as_any().is::<campfire::CampfireBlockEntity>() {
        // `CampfireBlockEntity.removeComponentsFromTag` removes Items because that field is
        // represented by CONTAINER (`CampfireBlockEntity.java:219-220`).
        nbt.child_tags.remove("Items");
    } else if entity.as_any().is::<shulker_box::ShulkerBoxBlockEntity>() {
        // `BaseContainerBlockEntity.removeComponentsFromTag` and
        // `RandomizableContainerBlockEntity.removeComponentsFromTag`
        // (`BaseContainerBlockEntity.java:167-172`; `RandomizableContainerBlockEntity.java:115-120`)
        // keep implicit container data out of the custom block-entity component.
        nbt.child_tags.remove("Items");
        nbt.child_tags.remove("LootTable");
        nbt.child_tags.remove("LootTableSeed");
    } else if let Some(kind) = container_kind(entity.resource_location()) {
        // The same removals for the other containers. CustomName and lock have no field on
        // these entities yet, so there is nothing of them to remove.
        nbt.child_tags.remove("Items");
        if kind == ContainerKind::Randomizable {
            nbt.child_tags.remove("LootTable");
            nbt.child_tags.remove("LootTableSeed");
        }
    } else if entity
        .as_any()
        .is::<chiseled_bookshelf::ChiseledBookshelfBlockEntity>()
        || entity.as_any().is::<shelf::ShelfBlockEntity>()
    {
        // `ChiseledBookShelfBlockEntity.java:135-137`; `ShelfBlockEntity.java:116-118`.
        nbt.child_tags.remove("Items");
    } else if entity
        .as_any()
        .is::<decorated_pot::DecoratedPotBlockEntity>()
    {
        // `DecoratedPotBlockEntity.java:126-129`.
        nbt.child_tags.remove("sherds");
        nbt.child_tags.remove("item");
    } else if entity.as_any().is::<banner::BannerBlockEntity>()
        || entity.as_any().is::<beacon::BeaconBlockEntity>()
    {
        // `BannerBlockEntity.java:103-106` and `BeaconBlockEntity.java:385-388` also discard
        // `patterns` and `lock`; those are not exported as components yet, so they stay.
        nbt.child_tags.remove("CustomName");
    } else if entity.as_any().is::<command_block::CommandBlockEntity>() {
        // `CommandBlockEntity.java:173-177`.
        nbt.child_tags.remove("CustomName");
        nbt.child_tags.remove("conditionMet");
        nbt.child_tags.remove("powered");
    }

    (!nbt.is_empty()).then_some((
        pumpkin_data::data_component::DataComponent::BlockEntityData,
        Some(Box::new(BlockEntityDataImpl { nbt }).to_dyn()),
    ))
}

#[must_use]
pub fn block_entity_from_generic<T: BlockEntity>(nbt: &NbtCompound) -> T {
    let x = nbt.get_int("x").unwrap_or(0);
    let y = nbt.get_int("y").unwrap_or(0);
    let z = nbt.get_int("z").unwrap_or(0);
    T::from_nbt(nbt, BlockPos::new(x, y, z))
}

#[must_use]
pub fn block_entity_from_nbt(nbt: &NbtCompound) -> Option<Arc<dyn BlockEntity>> {
    let x = nbt.get_int("x")?;
    let y = nbt.get_int("y")?;
    let z = nbt.get_int("z")?;
    block_entity_from_nbt_at(nbt, BlockPos::new(x, y, z))
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn block_entity_from_nbt_at(
    nbt: &NbtCompound,
    position: BlockPos,
) -> Option<Arc<dyn BlockEntity>> {
    let id = nbt.get_string("id")?;
    let pos = position;
    match id {
        barrel::BarrelBlockEntity::ID => {
            Some(Arc::new(barrel::BarrelBlockEntity::from_nbt(nbt, pos)))
        }
        chest::ChestBlockEntity::ID => Some(Arc::new(chest::ChestBlockEntity::from_nbt(nbt, pos))),
        trapped_chest::TrappedChestBlockEntity::ID => Some(Arc::new(
            trapped_chest::TrappedChestBlockEntity::from_nbt(nbt, pos),
        )),
        ender_chest::EnderChestBlockEntity::ID => Some(Arc::new(
            ender_chest::EnderChestBlockEntity::from_nbt(nbt, pos),
        )),
        furnace::FurnaceBlockEntity::ID => {
            Some(Arc::new(furnace::FurnaceBlockEntity::from_nbt(nbt, pos)))
        }
        blasting_furnace::BlastingFurnaceBlockEntity::ID => Some(Arc::new(
            blasting_furnace::BlastingFurnaceBlockEntity::from_nbt(nbt, pos),
        )),
        smoker::SmokerBlockEntity::ID => {
            Some(Arc::new(smoker::SmokerBlockEntity::from_nbt(nbt, pos)))
        }
        brewing_stand::BrewingStandBlockEntity::ID => Some(Arc::new(
            brewing_stand::BrewingStandBlockEntity::from_nbt(nbt, pos),
        )),
        hopper::HopperBlockEntity::ID => {
            Some(Arc::new(hopper::HopperBlockEntity::from_nbt(nbt, pos)))
        }
        jukebox::JukeboxBlockEntity::ID => {
            Some(Arc::new(jukebox::JukeboxBlockEntity::from_nbt(nbt, pos)))
        }
        mob_spawner::MobSpawnerBlockEntity::ID => Some(Arc::new(
            mob_spawner::MobSpawnerBlockEntity::from_nbt(nbt, pos),
        )),
        sign::SignBlockEntity::ID => Some(Arc::new(sign::SignBlockEntity::from_nbt(nbt, pos))),
        piston::PistonBlockEntity::ID => {
            Some(Arc::new(piston::PistonBlockEntity::from_nbt(nbt, pos)))
        }
        chiseled_bookshelf::ChiseledBookshelfBlockEntity::ID => Some(Arc::new(
            chiseled_bookshelf::ChiseledBookshelfBlockEntity::from_nbt(nbt, pos),
        )),
        dropper::DropperBlockEntity::ID => {
            Some(Arc::new(dropper::DropperBlockEntity::from_nbt(nbt, pos)))
        }
        command_block::CommandBlockEntity::ID => Some(Arc::new(
            command_block::CommandBlockEntity::from_nbt(nbt, pos),
        )),
        jigsaw_block::JigsawBlockEntity::ID => Some(Arc::new(
            jigsaw_block::JigsawBlockEntity::from_nbt(nbt, pos),
        )),
        comparator::ComparatorBlockEntity::ID => Some(Arc::new(
            comparator::ComparatorBlockEntity::from_nbt(nbt, pos),
        )),
        daylight_detector::DaylightDetectorBlockEntity::ID => Some(Arc::new(
            daylight_detector::DaylightDetectorBlockEntity::from_nbt(nbt, pos),
        )),
        end_portal::EndPortalBlockEntity::ID => Some(Arc::new(
            end_portal::EndPortalBlockEntity::from_nbt(nbt, pos),
        )),
        beacon::BeaconBlockEntity::ID => {
            Some(Arc::new(beacon::BeaconBlockEntity::from_nbt(nbt, pos)))
        }
        bed::BedBlockEntity::ID => Some(Arc::new(bed::BedBlockEntity::from_nbt(nbt, pos))),
        bell::BellBlockEntity::ID => Some(Arc::new(bell::BellBlockEntity::from_nbt(nbt, pos))),
        shulker_box::ShulkerBoxBlockEntity::ID => Some(Arc::new(
            shulker_box::ShulkerBoxBlockEntity::from_nbt(nbt, pos),
        )),
        lectern::LecternBlockEntity::ID => {
            Some(Arc::new(lectern::LecternBlockEntity::from_nbt(nbt, pos)))
        }
        dispenser::DispenserBlockEntity::ID => Some(Arc::new(
            dispenser::DispenserBlockEntity::from_nbt(nbt, pos),
        )),
        hanging_sign::HangingSignBlockEntity::ID => Some(Arc::new(
            hanging_sign::HangingSignBlockEntity::from_nbt(nbt, pos),
        )),
        creaking_heart::CreakingHeartBlockEntity::ID => Some(Arc::new(
            creaking_heart::CreakingHeartBlockEntity::from_nbt(nbt, pos),
        )),
        enchanting_table::EnchantingTableBlockEntity::ID => Some(Arc::new(
            enchanting_table::EnchantingTableBlockEntity::from_nbt(nbt, pos),
        )),
        skull::SkullBlockEntity::ID => Some(Arc::new(skull::SkullBlockEntity::from_nbt(nbt, pos))),
        banner::BannerBlockEntity::ID => {
            Some(Arc::new(banner::BannerBlockEntity::from_nbt(nbt, pos)))
        }
        structure_block::StructureBlockBlockEntity::ID => Some(Arc::new(
            structure_block::StructureBlockBlockEntity::from_nbt(nbt, pos),
        )),
        end_gateway::EndGatewayBlockEntity::ID => Some(Arc::new(
            end_gateway::EndGatewayBlockEntity::from_nbt(nbt, pos),
        )),
        conduit::ConduitBlockEntity::ID => {
            Some(Arc::new(conduit::ConduitBlockEntity::from_nbt(nbt, pos)))
        }
        map::MAP_BLOCK_ENTITY_ID => Some(Arc::new(map::MapBlockEntity::from_nbt(nbt, pos))),
        campfire::CampfireBlockEntity::ID => {
            Some(Arc::new(campfire::CampfireBlockEntity::from_nbt(nbt, pos)))
        }
        beehive::BeehiveBlockEntity::ID => {
            Some(Arc::new(beehive::BeehiveBlockEntity::from_nbt(nbt, pos)))
        }
        sculk_sensor::SculkSensorBlockEntity::ID => Some(Arc::new(
            sculk_sensor::SculkSensorBlockEntity::from_nbt(nbt, pos),
        )),
        calibrated_sculk_sensor::CalibratedSculkSensorBlockEntity::ID => Some(Arc::new(
            calibrated_sculk_sensor::CalibratedSculkSensorBlockEntity::from_nbt(nbt, pos),
        )),
        sculk_catalyst::SculkCatalystBlockEntity::ID => Some(Arc::new(
            sculk_catalyst::SculkCatalystBlockEntity::from_nbt(nbt, pos),
        )),
        sculk_shrieker::SculkShriekerBlockEntity::ID => Some(Arc::new(
            sculk_shrieker::SculkShriekerBlockEntity::from_nbt(nbt, pos),
        )),
        shelf::ShelfBlockEntity::ID => Some(Arc::new(shelf::ShelfBlockEntity::from_nbt(nbt, pos))),
        brushable_block::BrushableBlockBlockEntity::ID => Some(Arc::new(
            brushable_block::BrushableBlockBlockEntity::from_nbt(nbt, pos),
        )),
        decorated_pot::DecoratedPotBlockEntity::ID => Some(Arc::new(
            decorated_pot::DecoratedPotBlockEntity::from_nbt(nbt, pos),
        )),
        crafter::CrafterBlockEntity::ID => {
            Some(Arc::new(crafter::CrafterBlockEntity::from_nbt(nbt, pos)))
        }
        trial_spawner::TrialSpawnerBlockEntity::ID => Some(Arc::new(
            trial_spawner::TrialSpawnerBlockEntity::from_nbt(nbt, pos),
        )),
        vault::VaultBlockEntity::ID => Some(Arc::new(vault::VaultBlockEntity::from_nbt(nbt, pos))),
        test_block::TestBlockBlockEntity::ID => Some(Arc::new(
            test_block::TestBlockBlockEntity::from_nbt(nbt, pos),
        )),
        test_instance_block::TestInstanceBlockBlockEntity::ID => Some(Arc::new(
            test_instance_block::TestInstanceBlockBlockEntity::from_nbt(nbt, pos),
        )),
        copper_golem_statue::CopperGolemStatueBlockEntity::ID => Some(Arc::new(
            copper_golem_statue::CopperGolemStatueBlockEntity::from_nbt(nbt, pos),
        )),
        potent_sulfur::PotentSulfurBlockEntity::ID => Some(Arc::new(
            potent_sulfur::PotentSulfurBlockEntity::from_nbt(nbt, pos),
        )),
        _ => None,
    }
}

#[must_use]
pub fn has_block_block_entity(block: &Block) -> bool {
    BLOCK_ENTITY_TYPES.contains(&block.name)
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn create_block_entity(
    block_entity_type_id: u16,
    position: BlockPos,
) -> Option<Arc<dyn BlockEntity>> {
    use pumpkin_data::block_properties::FacingHopper;
    if block_entity_type_id == u16::MAX {
        return None;
    }
    let name = BLOCK_ENTITY_TYPES.get(block_entity_type_id as usize)?;
    match *name {
        "furnace" => Some(Arc::new(furnace::FurnaceBlockEntity::new(position))),
        "chest" => Some(Arc::new(chest::ChestBlockEntity::new(position))),
        "trapped_chest" => Some(Arc::new(trapped_chest::TrappedChestBlockEntity::new(
            position,
        ))),
        "ender_chest" => Some(Arc::new(ender_chest::EnderChestBlockEntity::new(position))),
        "jukebox" => Some(Arc::new(jukebox::JukeboxBlockEntity::new(position))),
        "dispenser" => Some(Arc::new(dispenser::DispenserBlockEntity::new(position))),
        "dropper" => Some(Arc::new(dropper::DropperBlockEntity::new(position))),
        "sign" => Some(Arc::new(sign::SignBlockEntity::empty(position))),
        "hanging_sign" => Some(Arc::new(hanging_sign::HangingSignBlockEntity::empty(
            position,
        ))),
        "mob_spawner" => Some(Arc::new(mob_spawner::MobSpawnerBlockEntity::new(
            position, None,
        ))),
        "creaking_heart" => Some(Arc::new(creaking_heart::CreakingHeartBlockEntity::new(
            position,
        ))),
        "piston" => Some(Arc::new(piston::PistonBlockEntity::from_nbt(
            &pumpkin_nbt::compound::NbtCompound::new(),
            position,
        ))),
        "brewing_stand" => Some(Arc::new(brewing_stand::BrewingStandBlockEntity::new(
            position,
        ))),
        "enchanting_table" => Some(Arc::new(enchanting_table::EnchantingTableBlockEntity::new(
            position,
        ))),
        "end_portal" => Some(Arc::new(end_portal::EndPortalBlockEntity::new(position))),
        "beacon" => Some(Arc::new(beacon::BeaconBlockEntity::new(position))),
        "skull" => Some(Arc::new(skull::SkullBlockEntity::new(position))),
        "daylight_detector" => Some(Arc::new(
            daylight_detector::DaylightDetectorBlockEntity::new(position),
        )),
        "hopper" => Some(Arc::new(hopper::HopperBlockEntity::new(
            position,
            FacingHopper::Down,
        ))),
        "comparator" => Some(Arc::new(comparator::ComparatorBlockEntity::new(position))),
        "banner" => Some(Arc::new(banner::BannerBlockEntity::new(position))),
        "structure_block" => Some(Arc::new(structure_block::StructureBlockBlockEntity::new(
            position,
        ))),
        "end_gateway" => Some(Arc::new(end_gateway::EndGatewayBlockEntity::new(position))),
        "command_block" => Some(Arc::new(command_block::CommandBlockEntity::new(
            position, true, false,
        ))),
        "shulker_box" => Some(Arc::new(shulker_box::ShulkerBoxBlockEntity::new(position))),
        "conduit" => Some(Arc::new(conduit::ConduitBlockEntity::new(position))),
        "barrel" => Some(Arc::new(barrel::BarrelBlockEntity::new(position))),
        "smoker" => Some(Arc::new(smoker::SmokerBlockEntity::new(position))),
        "blast_furnace" => Some(Arc::new(blasting_furnace::BlastingFurnaceBlockEntity::new(
            position,
        ))),
        "lectern" => Some(Arc::new(lectern::LecternBlockEntity::new(position))),
        "bell" => Some(Arc::new(bell::BellBlockEntity::new(position))),
        "jigsaw" => Some(Arc::new(jigsaw_block::JigsawBlockEntity::new(position))),
        "campfire" => Some(Arc::new(campfire::CampfireBlockEntity::new(position))),
        "beehive" => Some(Arc::new(beehive::BeehiveBlockEntity::new(position))),
        "sculk_sensor" => Some(Arc::new(sculk_sensor::SculkSensorBlockEntity::new(
            position,
        ))),
        "calibrated_sculk_sensor" => Some(Arc::new(
            calibrated_sculk_sensor::CalibratedSculkSensorBlockEntity::new(position),
        )),
        "sculk_catalyst" => Some(Arc::new(sculk_catalyst::SculkCatalystBlockEntity::new(
            position,
        ))),
        "sculk_shrieker" => Some(Arc::new(sculk_shrieker::SculkShriekerBlockEntity::new(
            position,
        ))),
        "chiseled_bookshelf" => Some(Arc::new(
            chiseled_bookshelf::ChiseledBookshelfBlockEntity::new(position),
        )),
        "shelf" => Some(Arc::new(shelf::ShelfBlockEntity::new(position))),
        "brushable_block" => Some(Arc::new(brushable_block::BrushableBlockBlockEntity::new(
            position,
        ))),
        "decorated_pot" => Some(Arc::new(decorated_pot::DecoratedPotBlockEntity::new(
            position,
        ))),
        "crafter" => Some(Arc::new(crafter::CrafterBlockEntity::new(position))),
        "trial_spawner" => Some(Arc::new(trial_spawner::TrialSpawnerBlockEntity::new(
            position,
        ))),
        "vault" => Some(Arc::new(vault::VaultBlockEntity::new(position))),
        "test_block" => Some(Arc::new(test_block::TestBlockBlockEntity::new(position))),
        "test_instance_block" => Some(Arc::new(
            test_instance_block::TestInstanceBlockBlockEntity::new(position),
        )),
        "copper_golem_statue" => Some(Arc::new(
            copper_golem_statue::CopperGolemStatueBlockEntity::new(position),
        )),
        "potent_sulfur" => Some(Arc::new(potent_sulfur::PotentSulfurBlockEntity::new(
            position,
        ))),
        "map" => Some(Arc::new(map::MapBlockEntity::new(position, 0))),
        _ => None,
    }
}

#[cfg(test)]
mod test {
    use super::{
        BlockEntity, apply_components_from_item_stack, beehive::BeehiveBlockEntity,
        block_entity_data_component, block_entity_from_nbt, chest::ChestBlockEntity,
        collect_components_from_block_entity, decorated_pot::DecoratedPotBlockEntity,
        furnace::FurnaceBlockEntity, skull::SkullBlockEntity, test_block::TestBlockBlockEntity,
    };
    use pumpkin_data::data_component_impl::{
        BlockEntityDataImpl, ContainerImpl, ContainerLootImpl, CustomNameImpl, DataComponentImpl,
        PotDecorationsImpl, ProfileImpl,
    };
    use pumpkin_data::{data_component::DataComponent, item::Item, item_stack::ItemStack};
    use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_util::text::TextComponent;
    use pumpkin_world::inventory::Inventory;
    use std::sync::Arc;

    /// A loaded block entity is serialized back into its chunk with
    /// `write_internal`, so whatever it holds has to survive that round trip or
    /// it is gone the next time the chunk is read.
    #[tokio::test]
    async fn furnace_contents_survive_a_chunk_round_trip() {
        let position = BlockPos::new(0, 100, 0);
        let furnace = Arc::new(FurnaceBlockEntity::new(position));
        furnace
            .set_stack(0, ItemStack::new(5, &Item::DIAMOND))
            .await;

        let mut nbt = NbtCompound::new();
        furnace.write_internal(&mut nbt).await;

        let inventory = block_entity_from_nbt(&nbt).and_then(BlockEntity::get_inventory);
        assert!(
            inventory.is_some(),
            "furnace should be readable back from its own NBT"
        );

        if let Some(inventory) = inventory {
            let stack = inventory.get_stack(0).await;
            assert_eq!(stack.get_item().id, Item::DIAMOND.id);
            assert_eq!(stack.item_count, 5);
        }
    }

    #[test]
    fn placed_container_loot_component_is_applied() {
        // BlockItem.updateBlockEntityComponents applies the item component before the
        // placement callbacks (`BlockItem.java:101-106`; RandomizableContainerBlockEntity.java:98-112).
        let position = BlockPos::new(3, 64, -2);
        let stack = ItemStack::new_with_component(
            1,
            &Item::CHEST,
            vec![(
                pumpkin_data::data_component::DataComponent::ContainerLoot,
                Some(Box::new(ContainerLootImpl {
                    loot_table: "minecraft:chests/simple_dungeon".to_string(),
                    seed: 7,
                })),
            )],
        );
        let entity: Arc<dyn BlockEntity> = Arc::new(ChestBlockEntity::new(position));
        let applied = apply_components_from_item_stack(entity.as_ref(), &stack)
            .expect("container loot should rebuild the placed entity");
        assert!(applied.has_loot_table());
        assert_eq!(
            applied.take_loot_table(),
            Some(("minecraft:chests/simple_dungeon".to_string(), 7))
        );
        assert_eq!(applied.get_position(), position);
    }

    #[test]
    fn op_only_block_entity_data_requires_game_master_permission() {
        // `OP_ONLY_CUSTOM_DATA` is limited to these types unless the player has the permission
        // checked by `BlockItem.updateCustomBlockEntityTag` (`BlockEntityTypes.java:211-211`;
        // `BlockItem.java:162-166`).
        assert!(!super::can_apply_custom_block_entity_data(
            "minecraft:command_block",
            false
        ));
        assert!(super::can_apply_custom_block_entity_data(
            "minecraft:command_block",
            true
        ));
        assert!(super::can_apply_custom_block_entity_data(
            "minecraft:chest",
            false
        ));
    }

    #[tokio::test]
    async fn placed_block_entity_data_component_is_applied() {
        // `BlockItem.updateCustomBlockEntityTag` loads the typed payload into the freshly
        // placed entity before `updateBlockEntityComponents` applies the implicit components
        // (`BlockItem.java:76-80, 148-170`; `TypedEntityData.java:139-150`).
        let position = BlockPos::new(3, 64, -2);
        let mut test_nbt = NbtCompound::new();
        test_nbt.put_string("message", "from item".to_string());
        let stack = ItemStack::new_with_component(
            1,
            &Item::TEST_BLOCK,
            vec![(
                pumpkin_data::data_component::DataComponent::BlockEntityData,
                Some(Box::new(BlockEntityDataImpl { nbt: test_nbt })),
            )],
        );
        let entity: Arc<dyn BlockEntity> = Arc::new(TestBlockBlockEntity::new(position));
        let applied = apply_components_from_item_stack(entity.as_ref(), &stack)
            .expect("block entity data should rebuild the placed entity");
        let test_block = applied
            .as_any()
            .downcast_ref::<TestBlockBlockEntity>()
            .expect("the rebuilt entity should stay a test block");
        // `TestBlockEntity.loadAdditional` reads `message` (`TestBlockEntity.java:40-42`).
        assert_eq!(test_block.get_message().await, "from item");

        // A chest item's prototype carries an empty CONTAINER (`Items.java:421`), and
        // `BaseContainerBlockEntity.applyImplicitComponents` copies it over every slot after the
        // payload is loaded (`BaseContainerBlockEntity.java:149-154`; `BlockEntity.java:280-297`;
        // `ItemContainerContents.java:118-122`), so raw `Items` in the payload do not survive.
        let stored = ItemStack::new(5, &Item::DIAMOND);
        let mut item_nbt = NbtCompound::new();
        stored.write_item_stack(&mut item_nbt);
        item_nbt.put_byte("Slot", 0);

        let mut entity_nbt = NbtCompound::new();
        entity_nbt.put_list("Items", vec![NbtTag::Compound(item_nbt)]);
        let stack = ItemStack::new_with_component(
            1,
            &Item::CHEST,
            vec![(
                pumpkin_data::data_component::DataComponent::BlockEntityData,
                Some(Box::new(BlockEntityDataImpl { nbt: entity_nbt })),
            )],
        );
        let entity: Arc<dyn BlockEntity> = Arc::new(ChestBlockEntity::new(position));
        let applied = apply_components_from_item_stack(entity.as_ref(), &stack)
            .expect("block entity data should rebuild the placed entity");
        let inventory = applied
            .get_inventory()
            .expect("chest block entity should expose its inventory");
        let restored = inventory.get_stack(0).await;
        assert!(restored.is_empty());
    }

    #[tokio::test]
    async fn creative_pick_preserves_custom_block_entity_data() {
        // `addBlockDataToItem` stores the result of `saveCustomOnly` as block-entity data
        // (`ServerGamePacketListenerImpl.java:715-724`; `BlockEntity.java:141-151`).
        let entity = TestBlockBlockEntity::new(BlockPos::new(3, 64, -2));
        entity.set_message("keep me".to_string()).await;

        let Some((_, Some(component))) = super::block_entity_data_component(&entity).await else {
            panic!("custom block-entity data should be serialized for pick-block");
        };
        let NbtTag::Compound(data) = component.write_data() else {
            panic!("block-entity data component should contain an NBT compound");
        };
        assert_eq!(data.get_string("message"), Some("keep me"));
    }

    #[tokio::test]
    async fn placed_shelf_container_component_is_applied() {
        // `ShelfBlockEntity.applyImplicitComponents` copies `DataComponents.CONTAINER` into the
        // shelf slots (`ShelfBlockEntity.java:104-107`).
        let position = BlockPos::new(3, 64, -2);
        let stack = ItemStack::new_with_component(
            1,
            &Item::ACACIA_SHELF,
            vec![(
                pumpkin_data::data_component::DataComponent::Container,
                Some(Box::new(pumpkin_data::data_component_impl::ContainerImpl {
                    items: vec![(1, ItemStack::new(3, &Item::DIAMOND))],
                })),
            )],
        );
        let entity: Arc<dyn BlockEntity> = Arc::new(super::shelf::ShelfBlockEntity::new(position));
        let applied = apply_components_from_item_stack(entity.as_ref(), &stack)
            .expect("shelf container should rebuild the placed entity");
        let inventory = applied
            .get_inventory()
            .expect("shelf should expose its inventory");
        assert!(inventory.get_stack(0).await.is_empty());
        let slot = inventory.get_stack(1).await;
        assert_eq!(slot.get_item().id, Item::DIAMOND.id);
        assert_eq!(slot.item_count, 3);
        assert!(inventory.get_stack(2).await.is_empty());
    }

    #[tokio::test]
    async fn campfire_container_component_round_trips_without_raw_items() {
        // `CampfireBlockEntity` collects CONTAINER, applies it to its slots, and removes Items
        // from block-entity data (`CampfireBlockEntity.java:207-220`).
        let position = BlockPos::new(3, 64, -2);
        let campfire = super::campfire::CampfireBlockEntity::new(position);
        *campfire.items[1].lock().await = ItemStack::new(1, &Item::BEEF);

        let components = collect_components_from_block_entity(&campfire).await;
        let container = components
            .iter()
            .find(|(id, _)| *id == DataComponent::Container)
            .and_then(|(_, component)| component.as_ref())
            .and_then(|component| component.as_any().downcast_ref::<ContainerImpl>())
            .expect("campfire container component");
        assert_eq!(container.items[0].0, 1);
        assert_eq!(container.items[0].1.get_item().id, Item::BEEF.id);

        let stack = ItemStack::new_with_component(
            1,
            &Item::CAMPFIRE,
            vec![(DataComponent::Container, Some(container.clone().to_dyn()))],
        );
        let placed = apply_components_from_item_stack(
            &super::campfire::CampfireBlockEntity::new(position),
            &stack,
        )
        .expect("campfire container should rebuild the placed entity");
        let placed_inventory = placed.get_inventory().expect("placed campfire inventory");
        let placed_item = placed_inventory.get_stack(1).await;
        assert_eq!(placed_item.get_item().id, Item::BEEF.id);

        let Some((_, Some(block_entity_data))) = block_entity_data_component(&campfire).await
        else {
            panic!("campfire cooking data should remain after Items is removed");
        };
        let NbtTag::Compound(block_entity_data) = block_entity_data.write_data() else {
            panic!("block entity data should be an NBT compound");
        };
        assert!(block_entity_data.get_list("Items").is_none());
        assert!(block_entity_data.get_int_array("CookingTimes").is_some());
    }

    #[tokio::test]
    async fn shulker_container_component_round_trips_without_raw_items() {
        // `BaseContainerBlockEntity.collectImplicitComponents`/`applyImplicitComponents`
        // (`BaseContainerBlockEntity.java:149-165`) carry shulker contents in CONTAINER.
        let position = BlockPos::new(3, 64, -2);
        let shulker = super::shulker_box::ShulkerBoxBlockEntity::new(position);
        shulker
            .set_stack(3, ItemStack::new(5, &Item::DIAMOND))
            .await;

        let components = collect_components_from_block_entity(&shulker).await;
        let container = components
            .iter()
            .find(|(id, _)| *id == DataComponent::Container)
            .and_then(|(_, component)| component.as_ref())
            .and_then(|component| component.as_any().downcast_ref::<ContainerImpl>())
            .expect("shulker container component");
        assert_eq!(container.items.len(), 1);
        assert_eq!(container.items[0].0, 3);
        assert_eq!(container.items[0].1.get_item().id, Item::DIAMOND.id);
        assert_eq!(container.items[0].1.item_count, 5);

        let stack = ItemStack::new_with_component(
            1,
            &Item::SHULKER_BOX,
            vec![(DataComponent::Container, Some(container.clone().to_dyn()))],
        );
        let placed = apply_components_from_item_stack(
            &super::shulker_box::ShulkerBoxBlockEntity::new(position),
            &stack,
        )
        .expect("shulker container should rebuild the placed entity");
        let placed_item = placed
            .get_inventory()
            .expect("placed shulker inventory")
            .get_stack(3)
            .await;
        assert_eq!(placed_item.get_item().id, Item::DIAMOND.id);
        assert_eq!(placed_item.item_count, 5);
    }

    #[tokio::test]
    async fn placed_beehive_bees_component_is_applied() {
        // `BeehiveBlockEntity.applyImplicitComponents` reads `DataComponents.BEES`
        // (`BeehiveBlockEntity.java:309-315`).
        let position = BlockPos::new(3, 64, -2);
        let mut entity_data = NbtCompound::new();
        entity_data.put_string("id", "minecraft:bee".to_string());
        let mut occupant = NbtCompound::new();
        occupant.put_compound("entity_data", entity_data);
        occupant.put_int("ticks_in_hive", 12);
        occupant.put_int("min_ticks_in_hive", 600);
        let stack = ItemStack::new_with_component(
            1,
            &Item::BEEHIVE,
            vec![(
                pumpkin_data::data_component::DataComponent::Bees,
                Some(Box::new(pumpkin_data::data_component_impl::BeesImpl {
                    bees: std::borrow::Cow::Owned(vec![occupant]),
                })),
            )],
        );
        let entity: Arc<dyn BlockEntity> = Arc::new(BeehiveBlockEntity::new(position));
        let applied = apply_components_from_item_stack(entity.as_ref(), &stack)
            .expect("bees component should rebuild the hive entity");
        let hive = applied
            .as_any()
            .downcast_ref::<BeehiveBlockEntity>()
            .expect("component application should preserve the hive type");
        assert_eq!(hive.occupant_count().await, 1);
    }

    #[tokio::test]
    async fn skull_implicit_components_are_collected() {
        // `SkullBlockEntity.collectImplicitComponents` exports the three modeled components
        // (`SkullBlockEntity.java:90-95`).
        let entity = SkullBlockEntity::new(BlockPos::new(3, 64, -2));
        let mut profile = NbtCompound::new();
        profile.put_string("name", "Steve".to_string());
        *entity.profile.lock().await = Some(profile);
        *entity.note_block_sound.lock().await = Some("minecraft:block.note_block.harp".to_string());
        *entity.custom_name.lock().await = Some("Skull".to_string());

        let components = collect_components_from_block_entity(&entity).await;

        assert_eq!(components.len(), 3);
        assert!(
            components
                .iter()
                .any(|(id, _)| { *id == pumpkin_data::data_component::DataComponent::Profile })
        );
        assert!(
            components.iter().any(|(id, _)| {
                *id == pumpkin_data::data_component::DataComponent::NoteBlockSound
            })
        );
        assert!(
            components
                .iter()
                .any(|(id, _)| { *id == pumpkin_data::data_component::DataComponent::CustomName })
        );
    }

    #[tokio::test]
    async fn decorated_pot_implicit_components_are_collected() {
        // `DecoratedPotBlockEntity.collectImplicitComponents` exports the decoration and
        // one-slot container components (`DecoratedPotBlockEntity.java:112-116`).
        let entity = DecoratedPotBlockEntity::new(BlockPos::new(3, 64, -2));
        *entity.sherds.lock().await = Some(vec![
            NbtTag::String("minecraft:brick".into()),
            NbtTag::String("minecraft:brick".into()),
            NbtTag::String("minecraft:brick".into()),
            NbtTag::String("minecraft:brick".into()),
        ]);
        *entity.item.lock().await = Some(ItemStack::new(2, &Item::DIAMOND));

        let components = collect_components_from_block_entity(&entity).await;

        assert_eq!(components.len(), 2);
        let decorations = components
            .iter()
            .find(|(id, _)| *id == DataComponent::PotDecorations)
            .and_then(|(_, component)| component.as_ref())
            .and_then(|component| component.as_any().downcast_ref::<PotDecorationsImpl>())
            .expect("pot decorations should be collected");
        assert_eq!(decorations.decorations.len(), 4);

        let container = components
            .iter()
            .find(|(id, _)| *id == DataComponent::Container)
            .and_then(|(_, component)| component.as_ref())
            .and_then(|component| component.as_any().downcast_ref::<ContainerImpl>())
            .expect("pot item should be collected as a container component");
        assert_eq!(container.items[0].0, 0);
        assert_eq!(container.items[0].1.get_item().id, Item::DIAMOND.id);
        assert_eq!(container.items[0].1.item_count, 2);

        let stack = ItemStack::new_with_component(1, &Item::DECORATED_POT, components);
        let placed = apply_components_from_item_stack(
            &DecoratedPotBlockEntity::new(BlockPos::new(3, 64, -2)),
            &stack,
        )
        .expect("pot components should be applied during placement");
        let placed = placed
            .as_any()
            .downcast_ref::<DecoratedPotBlockEntity>()
            .expect("component application should preserve the pot type");
        assert_eq!(
            placed
                .decorations()
                .expect("decorations should round-trip")
                .len(),
            4
        );
        let placed_item = placed
            .get_item()
            .await
            .expect("pot item should round-trip through the container component");
        assert_eq!(placed_item.get_item().id, Item::DIAMOND.id);
        assert_eq!(placed_item.item_count, 2);
    }

    #[tokio::test]
    async fn enchanting_table_custom_name_components_round_trip() {
        // EnchantingTableBlockEntity.collectImplicitComponents and applyImplicitComponents carry
        // CUSTOM_NAME (EnchantingTableBlockEntity.java:123-132).
        let position = BlockPos::new(3, 64, -2);
        let entity = super::enchanting_table::EnchantingTableBlockEntity::new(position);
        *entity.custom_name.lock().await = Some("Arcane".to_string());

        let components = collect_components_from_block_entity(&entity).await;
        assert!(
            components
                .iter()
                .any(|(id, _)| { *id == pumpkin_data::data_component::DataComponent::CustomName })
        );

        let stack = ItemStack::new_with_component(
            1,
            &Item::ENCHANTING_TABLE,
            vec![(
                pumpkin_data::data_component::DataComponent::CustomName,
                Some(Box::new(CustomNameImpl {
                    name: TextComponent::text("Placed"),
                })),
            )],
        );
        let applied = apply_components_from_item_stack(&entity, &stack)
            .expect("custom name should rebuild the enchanting table entity");
        let applied = applied
            .as_any()
            .downcast_ref::<super::enchanting_table::EnchantingTableBlockEntity>()
            .expect("component application should preserve the enchanting table type");
        assert_eq!(
            applied.custom_name.lock().await.as_deref(),
            Some("{\"text\":\"Placed\"}")
        );
    }

    #[tokio::test]
    async fn placed_skull_profile_component_is_applied() {
        // `SkullBlockEntity.applyImplicitComponents` loads PROFILE into the placed entity
        // (`SkullBlockEntity.java:82-87`).
        let position = BlockPos::new(3, 64, -2);
        let stack = ItemStack::new_with_component(
            1,
            &Item::PLAYER_HEAD,
            vec![(
                pumpkin_data::data_component::DataComponent::Profile,
                Some(Box::new(ProfileImpl {
                    name: Some("Steve".to_string()),
                    ..Default::default()
                })),
            )],
        );
        let entity: Arc<dyn BlockEntity> = Arc::new(SkullBlockEntity::new(position));
        let applied = apply_components_from_item_stack(entity.as_ref(), &stack)
            .expect("profile component should rebuild the placed skull");
        let skull = applied
            .as_any()
            .downcast_ref::<SkullBlockEntity>()
            .expect("component application should preserve the skull type");
        assert_eq!(
            skull
                .profile
                .lock()
                .await
                .as_ref()
                .and_then(|profile| profile.get_string("name")),
            Some("Steve")
        );
    }

    #[test]
    fn leftover_components_skip_implicit_and_reserved_types() {
        // `BlockEntity.applyComponents` stores only the added components the entity did not
        // query, and never BLOCK_ENTITY_DATA or BLOCK_STATE (`BlockEntity.java:280-304`).
        let stack = ItemStack::new_with_component(
            1,
            &Item::CHEST,
            vec![
                (
                    DataComponent::CustomName,
                    Some(
                        CustomNameImpl {
                            name: TextComponent::text("named"),
                        }
                        .to_dyn(),
                    ),
                ),
                (
                    DataComponent::ItemName,
                    Some(
                        pumpkin_data::data_component_impl::ItemNameImpl {
                            name: "Boxy".into(),
                        }
                        .to_dyn(),
                    ),
                ),
                (
                    DataComponent::BlockEntityData,
                    Some(
                        BlockEntityDataImpl {
                            nbt: NbtCompound::new(),
                        }
                        .to_dyn(),
                    ),
                ),
                (DataComponent::Container, None),
            ],
        );

        // CUSTOM_NAME is implicit for an enchanting table (it has a name field) and ITEM_NAME is
        // not; the removal marker and the block entity data are skipped.
        let table =
            super::enchanting_table::EnchantingTableBlockEntity::new(BlockPos::new(0, 64, 0));
        let leftover = super::leftover_components(&table, &stack);
        assert_eq!(leftover.child_tags.len(), 1);
        assert!(leftover.get_compound("minecraft:item_name").is_some());

        // A chest has no name field, so its CUSTOM_NAME stays in the stored map (and is
        // exported again by `collect_components`); CONTAINER is consumed by its slots.
        let chest = ChestBlockEntity::new(BlockPos::new(0, 64, 0));
        let leftover = super::leftover_components(&chest, &stack);
        assert_eq!(leftover.child_tags.len(), 2);
        assert!(leftover.has("minecraft:custom_name"));
        assert!(leftover.has("minecraft:item_name"));

        // A class without an `applyImplicitComponents` override keeps CUSTOM_NAME as well.
        let bell = super::bell::BellBlockEntity::new(BlockPos::new(0, 64, 0));
        let leftover = super::leftover_components(&bell, &stack);
        assert!(leftover.has("minecraft:custom_name"));
        assert!(leftover.has("minecraft:item_name"));
        assert!(!leftover.has("minecraft:block_entity_data"));
    }

    #[test]
    fn leftover_components_are_empty_when_everything_is_implicit() {
        // A profiled head and a filled shulker box consume every component they carry.
        let skull = SkullBlockEntity::new(BlockPos::new(0, 64, 0));
        let head = ItemStack::new_with_component(
            1,
            &Item::PLAYER_HEAD,
            vec![(
                DataComponent::Profile,
                Some(Box::new(ProfileImpl {
                    name: Some("Steve".to_string()),
                    ..Default::default()
                })),
            )],
        );
        assert!(super::leftover_components(&skull, &head).is_empty());

        let shulker = super::shulker_box::ShulkerBoxBlockEntity::new(BlockPos::new(0, 64, 0));
        let filled = ItemStack::new_with_component(
            1,
            &Item::SHULKER_BOX,
            vec![(
                DataComponent::Container,
                Some(Box::new(ContainerImpl {
                    items: vec![(0, ItemStack::new(1, &Item::DIAMOND))],
                })),
            )],
        );
        assert!(super::leftover_components(&shulker, &filled).is_empty());
    }

    #[test]
    fn stored_components_decode_back_into_typed_components() {
        let bell = super::bell::BellBlockEntity::new(BlockPos::new(0, 64, 0));
        let stack = ItemStack::new_with_component(
            1,
            &Item::BELL,
            vec![(
                DataComponent::CustomName,
                Some(Box::new(CustomNameImpl {
                    name: TextComponent::text("named"),
                })),
            )],
        );
        let mut stored = super::leftover_components(&bell, &stack);
        // An entry this build cannot decode stays in the map and is left out of the result.
        stored.put_string("minecraft:not_a_component", "kept".to_string());

        let decoded = super::components_from_nbt(&stored);

        assert_eq!(decoded.len(), 1);
        assert!(decoded[0].0 == DataComponent::CustomName);
        assert!(stored.has("minecraft:not_a_component"));
    }

    #[tokio::test]
    async fn chest_container_component_round_trips_without_raw_items() {
        // `BaseContainerBlockEntity` carries a chest's contents in CONTAINER and removes
        // `Items` from the custom tag (`BaseContainerBlockEntity.java:149-172`); slots the
        // chest does not have are dropped when the component is applied.
        let position = BlockPos::new(3, 64, -2);
        let chest = ChestBlockEntity::new(position);
        chest.set_stack(3, ItemStack::new(5, &Item::DIAMOND)).await;

        let components = collect_components_from_block_entity(&chest).await;
        let container = components
            .iter()
            .find(|(id, _)| *id == DataComponent::Container)
            .and_then(|(_, component)| component.as_ref())
            .and_then(|component| component.as_any().downcast_ref::<ContainerImpl>())
            .expect("chest container component");
        assert_eq!(container.items.len(), 1);
        assert_eq!(container.items[0].0, 3);
        assert!(
            !components
                .iter()
                .any(|(id, _)| *id == DataComponent::ContainerLoot)
        );

        let mut items = container.items.clone();
        items.push((200, ItemStack::new(1, &Item::DIRT)));
        let stack = ItemStack::new_with_component(
            1,
            &Item::CHEST,
            vec![(
                DataComponent::Container,
                Some(ContainerImpl { items }.to_dyn()),
            )],
        );
        let placed = apply_components_from_item_stack(&ChestBlockEntity::new(position), &stack)
            .expect("chest container should rebuild the placed entity");
        let inventory = placed.get_inventory().expect("placed chest inventory");
        let placed_item = inventory.get_stack(3).await;
        assert_eq!(placed_item.get_item().id, Item::DIAMOND.id);
        assert_eq!(placed_item.item_count, 5);

        let block_entity_data = block_entity_data_component(&chest).await;
        assert!(
            block_entity_data.is_none(),
            "Items is exported as CONTAINER, so nothing else remains"
        );
    }

    #[tokio::test]
    async fn pending_loot_table_is_exported_as_container_loot() {
        // `RandomizableContainerBlockEntity.collectImplicitComponents` exports the deferred
        // loot table with its seed (`RandomizableContainerBlockEntity.java:107-113`), and
        // `removeComponentsFromTag` drops both keys from the custom tag.
        let position = BlockPos::new(3, 64, -2);
        let mut nbt = NbtCompound::new();
        nbt.put_string("id", "minecraft:chest".to_string());
        nbt.put_int("x", position.0.x);
        nbt.put_int("y", position.0.y);
        nbt.put_int("z", position.0.z);
        nbt.put_string("LootTable", "minecraft:chests/simple_dungeon".to_string());
        nbt.put_long("LootTableSeed", 7);
        let chest = block_entity_from_nbt(&nbt).expect("chest from nbt");

        let components = collect_components_from_block_entity(chest.as_ref()).await;
        let loot = components
            .iter()
            .find(|(id, _)| *id == DataComponent::ContainerLoot)
            .and_then(|(_, component)| component.as_ref())
            .and_then(|component| component.as_any().downcast_ref::<ContainerLootImpl>())
            .expect("container loot component");
        assert_eq!(loot.loot_table, "minecraft:chests/simple_dungeon");
        assert_eq!(loot.seed, 7);
        assert!(block_entity_data_component(chest.as_ref()).await.is_none());
    }

    #[tokio::test]
    async fn banner_and_beacon_custom_names_round_trip_out_of_the_raw_tag() {
        // `BannerBlockEntity` and `BeaconBlockEntity` carry CUSTOM_NAME as a component and
        // discard `CustomName` from the custom tag (`BannerBlockEntity.java:96-106`;
        // `BeaconBlockEntity.java:376-389`).
        let position = BlockPos::new(3, 64, -2);
        let banner = super::banner::BannerBlockEntity::new(position);
        *banner.custom_name.lock().await = Some("\"Flag\"".to_string());
        let beacon = super::beacon::BeaconBlockEntity::new(position);
        *beacon.custom_name.lock().await = Some("\"Light\"".to_string());

        for entity in [&banner as &dyn BlockEntity, &beacon] {
            let components = collect_components_from_block_entity(entity).await;
            assert_eq!(components.len(), 1);
            assert!(components[0].0 == DataComponent::CustomName);
            let raw = block_entity_data_component(entity).await;
            let has_name = raw.is_some_and(|(_, data)| {
                data.is_some_and(|data| match data.write_data() {
                    NbtTag::Compound(nbt) => nbt.has("CustomName"),
                    _ => false,
                })
            });
            assert!(!has_name);
        }

        let stack = ItemStack::new_with_component(
            1,
            &Item::WHITE_BANNER,
            vec![(
                DataComponent::CustomName,
                Some(Box::new(CustomNameImpl {
                    name: TextComponent::text("Placed"),
                })),
            )],
        );
        let placed = apply_components_from_item_stack(&banner, &stack)
            .expect("custom name should rebuild the banner");
        let placed = placed
            .as_any()
            .downcast_ref::<super::banner::BannerBlockEntity>()
            .expect("component application should preserve the banner type");
        assert_eq!(
            placed.custom_name.lock().await.as_deref(),
            Some("{\"text\":\"Placed\"}")
        );
    }

    #[tokio::test]
    async fn decorated_pot_components_are_removed_from_the_raw_tag() {
        // `DecoratedPotBlockEntity.removeComponentsFromTag` discards `sherds` and `item`
        // (`DecoratedPotBlockEntity.java:125-128`), which POT_DECORATIONS and CONTAINER carry.
        let pot = DecoratedPotBlockEntity::new(BlockPos::new(3, 64, -2));
        *pot.sherds.lock().await = Some(vec![NbtTag::String("minecraft:brick".into()); 4]);
        *pot.item.lock().await = Some(ItemStack::new(2, &Item::DIAMOND));

        assert!(block_entity_data_component(&pot).await.is_none());
    }

    #[test]
    fn created_block_entities_report_their_own_type_index() {
        // The tick loop skips an entity whose type does not accept the block at its position
        // (`LevelChunk.java:747`), so every creatable entity must map to the index the block
        // states use for its type.
        for (index, name) in super::BLOCK_ENTITY_TYPES.iter().enumerate() {
            let created = super::create_block_entity(index as u16, BlockPos::new(0, 64, 0));
            let Some(created) = created else {
                panic!("block entity type {name} cannot be created");
            };
            assert_eq!(created.get_id() as usize, index, "{name}");
        }
    }
}
