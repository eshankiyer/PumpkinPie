use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam::atomic::AtomicCell;

use crate::entity::mob::is_animal_entity;
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase, EntityBaseFuture, NBTStorage, living::LivingEntity};
use crate::server::Server;

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;

use pumpkin_protocol::java::client::play::Metadata;

use pumpkin_util::math::{boundingbox::EntityAttachments, vector3::Vector3, wrap_degrees};

use crate::entity::vehicle::vehicle::VehicleEntity;

pub struct BoatEntity {
    pub vehicle: VehicleEntity,
    ticks_underwater: AtomicCell<f32>,
    left_paddle_moving: AtomicBool,
    right_paddle_moving: AtomicBool,
}

/// `AbstractBoat.clampRotation` limits the rider's yaw relative to the boat
/// (`AbstractBoat.java:666-673`).
pub(crate) fn clamp_passenger_yaw(boat_yaw: f32, passenger_yaw: f32) -> f32 {
    let delta = wrap_degrees(passenger_yaw - boat_yaw);
    let target_delta = delta.clamp(-105.0, 105.0);
    passenger_yaw + target_delta - delta
}

/// `AbstractBoat.getPassengerAttachmentPoint` (`AbstractBoat.java:135-151`): a lone rider sits
/// at `single_offset` (`getSinglePassengerXOffset`, 0 for boats and 0.15 for chest boats,
/// `AbstractBoat.java:611-613`; `AbstractChestBoat.java:43-45`); with two, the first sits at 0.2
/// and the other at -0.6, an `Animal` 0.2 further forward. The float offset is rotated by the
/// boat's yaw.
pub(crate) fn boat_passenger_attachment_point(
    ride_height: f32,
    single_offset: f32,
    passenger_is_animal: bool,
    passenger_index: i32,
    passenger_count: usize,
    boat_yaw: f32,
) -> Vector3<f64> {
    let mut offset = single_offset;
    if passenger_count > 1 {
        offset = if passenger_index == 0 { 0.2 } else { -0.6 };
        if passenger_is_animal {
            offset += 0.2;
        }
    }
    EntityAttachments::rotate_y(
        Vector3::new(0.0, f64::from(ride_height), f64::from(offset)),
        boat_yaw,
    )
}

impl BoatEntity {
    pub const fn new(entity: Entity) -> Self {
        Self {
            vehicle: VehicleEntity::new(entity),
            ticks_underwater: AtomicCell::new(0.0),
            left_paddle_moving: AtomicBool::new(false),
            right_paddle_moving: AtomicBool::new(false),
        }
    }

    pub fn set_paddles(&self, left: bool, right: bool) {
        self.left_paddle_moving.store(left, Ordering::Relaxed);
        self.right_paddle_moving.store(right, Ordering::Relaxed);

        self.vehicle.entity.send_meta_data(
            &[
                Metadata::new(pumpkin_data::tracked_data::boat::ID_PADDLE_LEFT, left),
                Metadata::new(pumpkin_data::tracked_data::boat::ID_PADDLE_RIGHT, right),
            ],
            None,
        );
    }

    fn send_wobble_metadata(&self) {
        self.vehicle.send_wobble_metadata();
    }
}

impl NBTStorage for BoatEntity {}

impl EntityBase for BoatEntity {
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

    fn damage_with_context<'a>(
        &'a self,
        _caller: &'a dyn EntityBase,
        amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&'a dyn EntityBase>,
        _cause: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move { self.vehicle.damage_with_context(amount, source).await })
    }

    fn interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        _item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            if !player.get_entity().can_start_riding().await {
                return false;
            }

            if self.ticks_underwater.load() >= 60.0 {
                return false;
            }

            if self.vehicle.entity.passengers.lock().await.len() >= 2 {
                return false;
            }

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

            true
        })
    }

    fn set_paddle_state(&self, left: bool, right: bool) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.set_paddles(left, right);
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

    /// `AbstractBoat.getPassengerAttachmentPoint` (`AbstractBoat.java:135-151`) with
    /// `Boat.rideHeight` = `dimensions.height() / 3.0F` (`Boat.java:14-17`), or
    /// `Raft.rideHeight` = `dimensions.height() * 0.8888889F` for both raft variants
    /// (`Raft.java:14-17`; `ChestRaft.java:14-17`).
    fn get_passenger_attachment_point<'a>(
        &'a self,
        passenger: &'a dyn EntityBase,
        passenger_index: i32,
        passenger_count: usize,
    ) -> EntityBaseFuture<'a, Vector3<f64>> {
        Box::pin(async move {
            let entity = &self.vehicle.entity;
            let height = entity.entity_dimension.load().height;
            let entity_type = entity.entity_type;
            let ride_height = if entity_type == &EntityType::BAMBOO_RAFT
                || entity_type == &EntityType::BAMBOO_CHEST_RAFT
            {
                height * 0.888_888_9
            } else {
                height / 3.0
            };
            boat_passenger_attachment_point(
                ride_height,
                0.0,
                is_animal_entity(passenger.get_entity().entity_type.id),
                passenger_index,
                passenger_count,
                entity.yaw.load(),
            )
        })
    }

    /// `AbstractBoat.onPassengerTurned` clamps a rider to 105 degrees from the boat
    /// (`AbstractBoat.java:666-678`).
    fn on_passenger_turned(&self, passenger: &Entity) {
        let boat_yaw = self.vehicle.entity.yaw.load();
        passenger.body_yaw.store(boat_yaw);
        passenger
            .yaw
            .store(clamp_passenger_yaw(boat_yaw, passenger.yaw.load()));
        passenger.head_yaw.store(passenger.yaw.load());
    }
}

#[cfg(test)]
mod tests {
    use super::{boat_passenger_attachment_point, clamp_passenger_yaw};

    #[test]
    fn two_riders_sit_fore_and_aft() {
        let height = 0.5625f32 / 3.0;
        let lone = boat_passenger_attachment_point(height, 0.0, false, 0, 1, 0.0);
        assert_eq!(lone.y, f64::from(height));
        assert_eq!(lone.z, 0.0);
        let driver = boat_passenger_attachment_point(height, 0.0, false, 0, 2, 0.0);
        assert_eq!(driver.z, f64::from(0.2f32));
        let back = boat_passenger_attachment_point(height, 0.0, false, 1, 2, 0.0);
        assert_eq!(back.z, f64::from(-0.6f32));
        let animal = boat_passenger_attachment_point(height, 0.0, true, 1, 2, 0.0);
        assert_eq!(animal.z, f64::from(-0.6f32 + 0.2f32));
        let chest = boat_passenger_attachment_point(height, 0.15, false, 0, 1, 0.0);
        assert_eq!(chest.z, f64::from(0.15f32));
    }

    #[test]
    fn passenger_yaw_is_clamped_to_boat() {
        // Vanilla does not renormalize the result to +-180, so 200 clamped to the 105-degree
        // limit stays on the unwrapped side (255, equivalent to -105) rather than snapping to 105.
        assert_eq!(clamp_passenger_yaw(0.0, 200.0), 255.0);
        assert_eq!(clamp_passenger_yaw(0.0, -200.0), -255.0);
        assert_eq!(clamp_passenger_yaw(15.0, 60.0), 60.0);
    }
}
