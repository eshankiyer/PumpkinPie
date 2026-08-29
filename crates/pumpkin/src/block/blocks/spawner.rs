use std::sync::Arc;

use crate::block::entities::mob_spawner::MobSpawnerBlockEntity;
use crate::entity::experience_orb::ExperienceOrbEntity;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::GameMode;
use rand::RngExt;
use crate::block::{BlockBehaviour, BrokenArgs, OnSyncedBlockEventArgs, PlacedArgs};

#[pumpkin_block("minecraft:spawner")]
pub struct SpawnerBlock;

impl BlockBehaviour for SpawnerBlock {
    /// Vanilla `BaseSpawner.onEventTriggered` (`BaseSpawner.java:249-259`) accepts
    /// event 1, and `SpawnerBlockEntity.broadcastEvent` sends that event through
    /// the spawner block (`SpawnerBlockEntity.java:21-25`). The client uses it to
    /// reset the visual spawn delay; accepting it here also makes the queued Java
    /// block event reachable through `World::flush_synced_block_events`.
    fn on_synced_block_event(&self, args: OnSyncedBlockEventArgs<'_>) -> bool {
        args.r#type == 1
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        let spawner_block_entity = MobSpawnerBlockEntity::new(*args.position, None);
        args.world.add_block_entity(Arc::new(spawner_block_entity));
    }

    /// `SpawnerBlock.spawnAfterBreak` (`SpawnerBlock.java:41-47`) awards two independent
    /// `nextInt(15)` rolls plus 15 experience when the break is allowed to drop experience.
    fn broken(&self, args: BrokenArgs<'_>) {
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
                ;
    }
}

