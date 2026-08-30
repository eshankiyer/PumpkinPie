use super::BlockEntity;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

use crate::world::World;

pub struct CalibratedSculkSensorBlockEntity {
    pub position: BlockPos,
    pub last_vibration_frequency: Mutex<i32>,
    dirty: AtomicBool,
}

impl BlockEntity for CalibratedSculkSensorBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    /// Vanilla inherits the sensor block entity's listener construction for calibrated
    /// sensors (`CalibratedSculkSensorBlockEntity.java:14-21`). Re-register the listener
    /// after loading because Pumpkin stores listeners separately from block entities.
    fn tick<'a>(&'a self, world: &'a Arc<World>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            crate::block::blocks::redstone::sculk_sensor::ensure_listener_registered(
                world,
                &self.position,
            )
            .await;
        })
    }

    /// Vanilla inherits the sensor's `setChanged` callback for vibration updates
    /// (`SculkSensorBlockEntity.java:111-133`) and keeps the calibrated listener's
    /// frequency behavior (`CalibratedSculkSensorBlockEntity.java:14-40`).
    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Release);
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let last_vibration_frequency = nbt.get_int("last_vibration_frequency").unwrap_or(0);
        Self {
            position,
            last_vibration_frequency: Mutex::new(last_vibration_frequency),
            dirty: AtomicBool::new(false),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            nbt.put_int(
                "last_vibration_frequency",
                *self.last_vibration_frequency.lock().await,
            );
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        nbt.put_int(
            "last_vibration_frequency",
            *self.last_vibration_frequency.try_lock().ok()?,
        );
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl CalibratedSculkSensorBlockEntity {
    pub const ID: &'static str = "minecraft:calibrated_sculk_sensor";
    // Vanilla `CalibratedSculkSensorBlockEntity.VibrationUser.getListenerRadius`
    // (`CalibratedSculkSensorBlockEntity.java:29-32`).
    pub const LISTENER_RADIUS: i32 = 16;

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            last_vibration_frequency: Mutex::new(0),
            dirty: AtomicBool::new(false),
        }
    }

    pub async fn set_last_vibration_frequency(&self, frequency: i32) {
        *self.last_vibration_frequency.lock().await = frequency;
        self.dirty.store(true, Ordering::Release);
    }
}
