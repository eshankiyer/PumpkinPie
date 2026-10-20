use std::sync::Arc;

use crate::block::entities::mob_spawner::MobSpawnerBlockEntity;
use pumpkin_macros::pumpkin_block;
use rand::RngExt;

use crate::block::{
    BlockBehaviour, BlockFuture, OnSyncedBlockEventArgs, PlacedArgs, SpawnAfterBreakArgs,
    pop_experience,
};

#[pumpkin_block("minecraft:spawner")]
pub struct SpawnerBlock;

impl BlockBehaviour for SpawnerBlock {
    /// Vanilla `BaseSpawner.onEventTriggered` (`BaseSpawner.java:249-259`) accepts
    /// event 1, and `SpawnerBlockEntity.broadcastEvent` sends that event through
    /// the spawner block (`SpawnerBlockEntity.java:21-25`). The client uses it to
    /// reset the visual spawn delay; accepting it here also makes the queued Java
    /// block event reachable through `World::flush_synced_block_events`.
    fn on_synced_block_event<'a>(
        &'a self,
        args: OnSyncedBlockEventArgs<'a>,
    ) -> BlockFuture<'a, bool> {
        Box::pin(async move { args.r#type == 1 })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let hopper_block_entity = MobSpawnerBlockEntity::new(*args.position, None);
            args.world.add_block_entity(Arc::new(hopper_block_entity));
        })
    }

    /// `SpawnerBlock.spawnAfterBreak` (`SpawnerBlock.java:41-48`).
    fn spawn_after_break<'a>(&'a self, args: SpawnAfterBreakArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !args.drop_experience {
                return;
            }
            let amount = {
                let mut random = rand::rng();
                15 + random.random_range(0..15) + random.random_range(0..15)
            };
            pop_experience(args.world, args.position, amount).await;
        })
    }
}
