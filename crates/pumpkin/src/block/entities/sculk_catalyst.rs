//! `SculkCatalystBlockEntity` (`world/level/block/entity/SculkCatalystBlockEntity.java`).
//!
//! Vanilla holds a `SculkSpreader` (created by `CatalystListener`, line 66), persists it
//! through `loadAdditional`/`saveAdditional` (lines 41-51) and drives it every server tick
//! from `serverTick` (lines 37-39). The previous port had neither: it stored a `decay_delay`
//! int that vanilla does not have on this block entity at all.

use super::BlockEntity;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::xoroshiro128::Xoroshiro;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

use crate::block::blocks::sculk::sculk_catalyst::CatalystListener;
use crate::block::blocks::sculk::sculk_spreader::SculkSpreader;
use crate::block::blocks::sculk_vein::WorldSpreadTarget;
use crate::world::World;

pub struct SculkCatalystBlockEntity {
    pub position: BlockPos,
    /// `CatalystListener.sculkSpreader` — `SculkSpreader.createLevelSpreader()`.
    pub spreader: Mutex<SculkSpreader>,
    /// Guards the lazy `GameEventListener` registration performed on the first tick.
    listener_registered: AtomicBool,
}

impl BlockEntity for SculkCatalystBlockEntity {
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
        // `loadAdditional`: `this.catalystListener.sculkSpreader.load(input)`.
        let mut spreader = SculkSpreader::level_spreader();
        spreader.load_nbt(nbt);
        Self {
            position,
            spreader: Mutex::new(spreader),
            listener_registered: AtomicBool::new(false),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.spreader.lock().await.save_nbt(nbt);
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        self.spreader.try_lock().ok()?.save_nbt(&mut nbt);
        Some(nbt)
    }

    /// `SculkCatalystBlockEntity.serverTick` (lines 37-39):
    /// `getSculkSpreader().updateCursors(level, pos, level.getRandom(), true)`.
    fn tick<'a>(&'a self, world: &'a Arc<World>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.ensure_listener_registered(world).await;

            let mut spreader = self.spreader.lock().await;
            if spreader.cursors().is_empty() {
                return;
            }
            let mut random =
                RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::random::<u64>()));
            let target = WorldSpreadTarget { world };
            let events = spreader
                .update_cursors(&target, self.position, &mut random, true)
                .await;
            drop(spreader);

            for event in events {
                world.sync_world_event(
                    pumpkin_data::world::WorldEvent::ParticlesSculkCharge,
                    event.pos,
                    event.data,
                );
            }
        })
    }

    fn on_block_replaced<'a>(
        self: Arc<Self>,
        world: Arc<World>,
        position: BlockPos,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            world.unregister_game_event_listener_at(&position).await;
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl SculkCatalystBlockEntity {
    pub const ID: &'static str = "minecraft:sculk_catalyst";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            spreader: Mutex::new(SculkSpreader::level_spreader()),
            listener_registered: AtomicBool::new(false),
        }
    }

    /// Vanilla re-creates the catalyst's `GameEventListener` whenever the block entity is
    /// constructed, and rebuilds its per-chunk-section listener registry on chunk load.
    /// This codebase's registry is a flat per-world `Vec` populated from `placed()`, which
    /// a catalyst loaded from disk never calls — so registration happens lazily here, on
    /// the block entity's first tick. The scan makes it idempotent: a listener leaked by a
    /// chunk unload (nothing unregisters on unload) is reused rather than duplicated, and
    /// such a leaked listener is inert because it resolves through `get_block_entity`.
    async fn ensure_listener_registered(&self, world: &Arc<World>) {
        if self.listener_registered.swap(true, Ordering::AcqRel) {
            return;
        }
        let already_present = world
            .game_event_listeners
            .lock()
            .await
            .iter()
            .any(|listener| {
                matches!(
                    listener.listener_source(),
                    crate::world::game_event::PositionSource::Block(pos) if pos == self.position
                )
            });
        if !already_present {
            world
                .register_game_event_listener(Arc::new(CatalystListener { pos: self.position }))
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::sculk_behaviour::ChargeCursor;

    #[tokio::test]
    async fn cursors_survive_an_nbt_round_trip() {
        let entity = SculkCatalystBlockEntity::new(BlockPos::new(4, 5, 6));
        entity
            .spreader
            .lock()
            .await
            .add_cursors(BlockPos::new(4, 6, 6), 1500);

        let mut nbt = NbtCompound::new();
        entity.write_nbt(&mut nbt).await;

        let loaded = SculkCatalystBlockEntity::from_nbt(&nbt, BlockPos::new(4, 5, 6));
        let cursors: Vec<i32> = loaded
            .spreader
            .lock()
            .await
            .cursors()
            .iter()
            .map(ChargeCursor::charge)
            .collect();
        assert_eq!(cursors, vec![1000, 500]);
    }

    #[tokio::test]
    async fn a_fresh_catalyst_has_no_cursors() {
        let entity = SculkCatalystBlockEntity::new(BlockPos::new(0, 0, 0));
        assert!(entity.spreader.lock().await.cursors().is_empty());
    }
}
