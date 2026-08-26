use std::sync::Arc;

use crate::block::entities::mob_spawner::MobSpawnerBlockEntity;
use crate::entity::experience_orb::ExperienceOrbEntity;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::GameMode;
use rand::RngExt;

use crate::block::{BlockBehaviour, BlockFuture, BrokenArgs, OnSyncedBlockEventArgs, PlacedArgs};

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

    /// `SpawnerBlock.spawnAfterBreak` (`SpawnerBlock.java:41-47`) awards two independent
    /// `nextInt(15)` rolls plus 15 experience when the break is allowed to drop experience.
    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.player.gamemode.load() == GameMode::Creative
                || !args.world.level_info.load().game_rules.block_drops
            {
                return;
            }

            let amount = {
                let mut random = rand::rng();
                15 + random.random_range(0..15) + random.random_range(0..15)
            };
            // `popExperience` (`Block.java:445-449`): `ExperienceOrb.award(level,
            // Vec3.atCenterOf(pos), amount)`.
            ExperienceOrbEntity::spawn(args.world, args.position.to_centered_f64(), amount as u32)
                .await;
        })
    }
}
