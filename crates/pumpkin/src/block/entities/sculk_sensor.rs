use super::BlockEntity;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

use crate::world::World;
use crate::world::game_event::vibration::VibrationData;

pub struct SculkSensorBlockEntity {
    pub position: BlockPos,
    pub last_vibration_frequency: Mutex<i32>,
    /// Vanilla's `VibrationSystem.Data` (`SculkSensorBlockEntity.java:20`), shared with the
    /// vibration listener that queues candidates and the tick that delivers them.
    pub(crate) vibration_data: Mutex<VibrationData>,
    dirty: AtomicBool,
}

impl BlockEntity for SculkSensorBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    /// Vanilla creates the vibration listener with the block entity
    /// (`SculkSensorBlockEntity.java:25-30`) and keeps its block position source
    /// (`SculkSensorBlockEntity.java:72-94`). Re-register it when a persisted entity is
    /// first ticked because Pumpkin's listener registry is maintained separately from the
    /// block entity map.
    ///
    /// The block's ticker is `VibrationSystem.Ticker.tick` (`SculkSensorBlock.java:157-165`).
    fn tick<'a>(&'a self, world: &'a Arc<World>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            crate::block::blocks::redstone::sculk_sensor::ensure_listener_registered(
                world,
                &self.position,
            )
            .await;
            if crate::block::blocks::redstone::sculk_sensor::tick_vibration(
                world,
                &self.position,
                &self.vibration_data,
                Self::LISTENER_RADIUS,
            )
            .await
            {
                self.dirty.store(true, Ordering::Release);
            }
        })
    }

    /// Vanilla saves the listener data and frequency (`SculkSensorBlockEntity.java:41-52`)
    /// and calls `setChanged` after a vibration (`SculkSensorBlockEntity.java:111-133`).
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
        // `input.read("listener", Data.CODEC).orElseGet(Data::new)`.
        let vibration_data = nbt
            .get_compound("listener")
            .and_then(VibrationData::from_nbt)
            .unwrap_or_default();
        Self {
            position,
            last_vibration_frequency: Mutex::new(last_vibration_frequency),
            vibration_data: Mutex::new(vibration_data),
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
            nbt.put_compound("listener", self.vibration_data.lock().await.to_nbt());
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

impl SculkSensorBlockEntity {
    pub const ID: &'static str = "minecraft:sculk_sensor";
    // Vanilla `SculkSensorBlockEntity.VibrationUser.LISTENER_RANGE`
    // (`SculkSensorBlockEntity.java:77`).
    pub const LISTENER_RADIUS: i32 = 8;

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            last_vibration_frequency: Mutex::new(0),
            vibration_data: Mutex::new(VibrationData::default()),
            dirty: AtomicBool::new(false),
        }
    }

    pub async fn set_last_vibration_frequency(&self, frequency: i32) {
        *self.last_vibration_frequency.lock().await = frequency;
        self.dirty.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::entities::BlockEntity;

    #[test]
    fn frequency_update_marks_sensor_dirty_until_cleared() {
        let sensor = SculkSensorBlockEntity::new(BlockPos::ZERO);
        assert!(!sensor.is_dirty());

        futures::executor::block_on(sensor.set_last_vibration_frequency(7));
        assert!(sensor.is_dirty());

        sensor.clear_dirty();
        assert!(!sensor.is_dirty());
    }
}
