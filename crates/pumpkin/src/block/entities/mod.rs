use std::pin::Pin;
use std::{any::Any, sync::Arc};

use pumpkin_data::data_component_impl::{
    BeesImpl, BlockEntityDataImpl, ContainerImpl, ContainerLootImpl, CustomNameImpl,
    DataComponentImpl, NoteBlockSoundImpl, ProfileImpl,
};
use pumpkin_data::{Block, BlockStateId, block_properties::BLOCK_ENTITY_TYPES};
use pumpkin_nbt::compound::NbtCompound;
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
    fn on_block_replaced<'a>(
        self: Arc<Self>,
        world: Arc<World>,
        position: BlockPos,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            if let Some(inventory) = self.get_inventory() {
                // Assuming scatter_inventory is an async method on World
                world.scatter_inventory(&position, &inventory).await;
            }
        })
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
pub fn apply_components_from_item_stack(
    entity: &dyn BlockEntity,
    stack: &ItemStack,
) -> Option<Arc<dyn BlockEntity>> {
    if entity.as_any().is::<skull::SkullBlockEntity>() {
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

    let block_entity_data = stack.get_data_component::<BlockEntityDataImpl>();
    let container_loot = stack.get_data_component::<ContainerLootImpl>();
    let custom_name = stack.get_data_component::<CustomNameImpl>();
    if block_entity_data.is_none() && container_loot.is_none() && custom_name.is_none() {
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
    let rebuilt = block_entity_from_nbt_at(&nbt, position)?;
    if let Some(command_block) = rebuilt
        .as_any()
        .downcast_ref::<command_block::CommandBlockEntity>()
    {
        // `BlockEntity.applyImplicitComponents` is invoked by
        // `BlockItem.updateBlockEntityComponents` after the typed payload is loaded
        // (`CommandBlockEntity.java:160-164`; `BlockItem.java:101-106`).
        command_block.apply_implicit_components(stack);
    }
    Some(rebuilt)
}

/// Collects the component used by the beehive creative-break item round trip. This is the live
/// `collectImplicitComponents` path (`BeehiveBlockEntity.java:317-321`).
pub(crate) async fn collect_components_from_block_entity(
    entity: &dyn BlockEntity,
) -> Vec<(
    pumpkin_data::data_component::DataComponent,
    Option<Box<dyn DataComponentImpl>>,
)> {
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
        block_entity_from_nbt, chest::ChestBlockEntity, collect_components_from_block_entity,
        furnace::FurnaceBlockEntity, skull::SkullBlockEntity,
    };
    use pumpkin_data::data_component_impl::{BlockEntityDataImpl, ContainerLootImpl, ProfileImpl};
    use pumpkin_data::{item::Item, item_stack::ItemStack};
    use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
    use pumpkin_util::math::position::BlockPos;
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

    #[tokio::test]
    async fn placed_block_entity_data_component_is_applied() {
        // `BlockItem.updateCustomBlockEntityTag` loads the typed payload into the freshly
        // placed entity before the remaining placement callbacks (`BlockItem.java:76-80,
        // 148-170`).
        let position = BlockPos::new(3, 64, -2);
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
        assert_eq!(restored.item.id, Item::DIAMOND.id);
        assert_eq!(restored.item_count, 5);
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
}
