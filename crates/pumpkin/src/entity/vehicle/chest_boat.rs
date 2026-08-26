//! Vanilla `AbstractChestBoat`: a boat variant that carries a 27-slot chest
//! container with loot-table support.
//!
//! (`net/minecraft/world/entity/vehicle/boat/AbstractChestBoat.java`). The
//! chest-vehicle storage helpers it relies on (`addChestVehicleSaveData`,
//! `chestVehicleDestroyed`, ...) are the same `ContainerEntity` default methods the chest
//! minecart uses, so this reuses [`MinecartInventory`] exactly like
//! `crate::entity::vehicle::minecart::chest::ChestMinecart` does.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam::atomic::AtomicCell;

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::GameMode;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::Inventory;

use crate::entity::player::Player;
use crate::entity::vehicle::minecart::container::{self, MinecartInventory};
use crate::entity::vehicle::vehicle::VehicleEntity;
use crate::entity::{Entity, EntityBase, EntityBaseFuture, NBTStorage, living::LivingEntity};
use crate::server::Server;

/// `AbstractChestBoat.getMaxPassengers` (`AbstractChestBoat.java:47-50`) caps chest boats at a
/// single passenger, unlike plain boats which carry two.
const MAX_PASSENGERS: usize = 1;

pub struct ChestBoatEntity {
    pub vehicle: VehicleEntity,
    ticks_underwater: AtomicCell<f32>,
    left_paddle_moving: AtomicBool,
    right_paddle_moving: AtomicBool,
    inventory: Arc<MinecartInventory>,
}

impl ChestBoatEntity {
    pub fn new(entity: Entity) -> Self {
        Self {
            vehicle: VehicleEntity::new(entity),
            ticks_underwater: AtomicCell::new(0.0),
            left_paddle_moving: AtomicBool::new(false),
            right_paddle_moving: AtomicBool::new(false),
            inventory: Arc::new(MinecartInventory::new(27)),
        }
    }

    /// The item dropped when the boat breaks. Each concrete vanilla subclass supplies one via
    /// the `dropItem` supplier passed to the `AbstractChestBoat` constructor
    /// (`AbstractChestBoat.java:38-40`).
    const fn drop_item(&self) -> &'static Item {
        match self.vehicle.entity.entity_type.id {
            id if id == EntityType::ACACIA_CHEST_BOAT.id => &Item::ACACIA_CHEST_BOAT,
            id if id == EntityType::BAMBOO_CHEST_RAFT.id => &Item::BAMBOO_CHEST_RAFT,
            id if id == EntityType::BIRCH_CHEST_BOAT.id => &Item::BIRCH_CHEST_BOAT,
            id if id == EntityType::CHERRY_CHEST_BOAT.id => &Item::CHERRY_CHEST_BOAT,
            id if id == EntityType::DARK_OAK_CHEST_BOAT.id => &Item::DARK_OAK_CHEST_BOAT,
            id if id == EntityType::JUNGLE_CHEST_BOAT.id => &Item::JUNGLE_CHEST_BOAT,
            id if id == EntityType::MANGROVE_CHEST_BOAT.id => &Item::MANGROVE_CHEST_BOAT,
            id if id == EntityType::OAK_CHEST_BOAT.id => &Item::OAK_CHEST_BOAT,
            id if id == EntityType::PALE_OAK_CHEST_BOAT.id => &Item::PALE_OAK_CHEST_BOAT,
            id if id == EntityType::SPRUCE_CHEST_BOAT.id => &Item::SPRUCE_CHEST_BOAT,
            _ => &Item::OAK_CHEST_BOAT,
        }
    }

    fn send_wobble_metadata(&self) {
        self.vehicle.send_wobble_metadata();
    }
}

impl NBTStorage for ChestBoatEntity {
    /// `AbstractChestBoat.addAdditionalSaveData`
    /// (`AbstractChestBoat.java:52-56`) saves the parent boat data plus the chest vehicle data
    /// (27 item slots or a deferred LootTable/LootTableSeed pair).
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async move {
            self.vehicle.entity.write_nbt(nbt).await;
            self.inventory.write_nbt(nbt).await;
        })
    }

    /// `AbstractChestBoat.readAdditionalSaveData`
    /// (`AbstractChestBoat.java:58-62`) restores items/LootTable after the base entity reads.
    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async move {
            self.vehicle.entity.read_nbt_non_mut(nbt).await;
            self.inventory.read_nbt(nbt).await;
        })
    }
}

impl EntityBase for ChestBoatEntity {
    fn get_entity(&self) -> &Entity {
        &self.vehicle.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn is_pickable(&self) -> bool {
        self.vehicle.entity.is_alive()
    }

    fn tick<'a>(
        &'a self,
        _caller: &'a Arc<dyn EntityBase>,
        _server: &'a Server,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.vehicle.tick();

            let underwater = self.ticks_underwater.load();
            if self.vehicle.entity.touching_water.load(Ordering::Relaxed) {
                self.ticks_underwater.store((underwater + 1.0).min(60.0));
            } else if underwater > 0.0 {
                self.ticks_underwater.store((underwater - 1.0).max(0.0));
            }
        })
    }

    fn init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.send_wobble_metadata();
        })
    }

    fn can_hit(&self) -> bool {
        self.vehicle.entity.is_alive()
    }

    fn is_collidable(&self, _entity: Option<Box<dyn EntityBase>>) -> bool {
        true
    }

    fn can_be_collided_with(&self) -> bool {
        true
    }

    /// Breaking the boat spills the chest contents (`chestVehicleDestroyed` reached from
    /// `AbstractChestBoat.destroy`, `AbstractChestBoat.java:65-68`; the same spill happens on
    /// any destroy-removal in `remove`, `AbstractChestBoat.java:71-77`) and drops the wooden
    /// chest-boat item for non-creative attackers.
    fn damage_with_context<'a>(
        &'a self,
        _caller: &'a dyn EntityBase,
        amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&'a dyn EntityBase>,
        _cause: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let creative = source
                .and_then(EntityBase::get_player)
                .is_some_and(|player| player.gamemode.load() == GameMode::Creative);
            let damaged = self.vehicle.damage_with_context(amount, source).await;

            if self.vehicle.entity.is_removed() {
                let world = self.vehicle.entity.world.load();
                let position = self.vehicle.entity.block_pos.load();
                // Contents always spill on destroy, creative hit included
                // (`AbstractChestBoat.remove`, AbstractChestBoat.java:71-77).
                if self.inventory.claim_drops() {
                    self.inventory.unpack_loot().await;
                    let inventory: Arc<dyn Inventory> = self.inventory.clone();
                    world.scatter_inventory(&position, &inventory).await;
                }
                if !creative && world.level_info.load().game_rules.entity_drops {
                    world
                        .drop_stack(&position, ItemStack::new(1, self.drop_item()))
                        .await;
                }
            }

            damaged
        })
    }

    /// `AbstractChestBoat.interact` (`AbstractChestBoat.java:80-97`): riding is tried first
    /// through the plain-boat path (`AbstractBoat.interact`, AbstractBoat.java:691-700); only
    /// when that yields PASS -- the player is sneaking or no passenger slot is free --
    /// does the container screen open (`interactWithContainerVehicle` ->
    /// `openCustomInventoryScreen`, AbstractChestBoat.java:100-106).
    fn interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        _item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let sneaking = player.get_entity().sneaking.load(Ordering::Relaxed);
            let has_room = self.vehicle.entity.passengers.lock().await.len() < MAX_PASSENGERS;

            if !sneaking && has_room && self.ticks_underwater.load() < 60.0 {
                let world = self.vehicle.entity.world.load();
                let Some(vehicle) = world.get_entity_by_id(self.vehicle.entity.entity_id) else {
                    return false;
                };
                let Some(passenger) = world.get_player_by_id(player.entity_id()) else {
                    return false;
                };
                self.vehicle
                    .entity
                    .add_passenger(vehicle, passenger as Arc<dyn EntityBase>)
                    .await;
                return true;
            }

            let java_key = format!(
                "entity.minecraft.{}",
                self.vehicle
                    .entity
                    .entity_type
                    .resource_name
                    .strip_prefix("minecraft:")
                    .unwrap_or(self.vehicle.entity.entity_type.resource_name)
            );
            container::open(
                &self.vehicle.entity,
                player,
                &self.inventory,
                TextComponent::translate(java_key, []),
                false,
            )
            .await
        })
    }

    fn set_paddle_state(&self, left: bool, right: bool) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.left_paddle_moving.store(left, Ordering::Relaxed);
            self.right_paddle_moving.store(right, Ordering::Relaxed);

            self.vehicle.entity.send_meta_data(
                &[
                    pumpkin_protocol::java::client::play::Metadata::new(
                        pumpkin_data::tracked_data::boat::ID_PADDLE_LEFT,
                        left,
                    ),
                    pumpkin_protocol::java::client::play::Metadata::new(
                        pumpkin_data::tracked_data::boat::ID_PADDLE_RIGHT,
                        right,
                    ),
                ],
                None,
            );
        })
    }

    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushable(&self) -> bool {
        true
    }

    /// Same ride-height rule as [`crate::entity::vehicle::boat::BoatEntity`]: chest rafts use
    /// the raft ratio (`ChestRaft.java:14-17`) while chest boats keep `height / 3.0`
    /// (`ChestBoat.java:14-17`).
    fn get_passengers_riding_offset(&self) -> f64 {
        let height = f64::from(self.vehicle.entity.entity_dimension.load().height);
        let entity_type = self.vehicle.entity.entity_type;
        if entity_type == &EntityType::BAMBOO_CHEST_RAFT {
            height * 0.888_888_9
        } else {
            height / 3.0
        }
    }
}
