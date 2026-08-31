use std::sync::Arc;

use crate::block::entities::mob_spawner::MobSpawnerBlockEntity;
use crate::entity::experience_orb::ExperienceOrbEntity;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::GameMode;
use rand::RngExt;

use crate::block::{BlockBehaviour, BlockFuture, BrokenArgs, OnSyncedBlockEventArgs, PlacedArgs};

/// Vanilla `SpawnerBlock.spawnAfterBreak` (`SpawnerBlock.java:41-47`) only awards
/// experience when the break may drop it; `Block.popExperience` also requires
/// block drops (`Block.java:446-449`).
const fn should_drop_experience(
    drop_experience: bool,
    block_drops: bool,
    game_mode: GameMode,
) -> bool {
    drop_experience && block_drops && !matches!(game_mode, GameMode::Creative | GameMode::Spectator)
}

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

    /// Vanilla `SpawnerBlock.spawnAfterBreak` (`SpawnerBlock.java:41-47`) awards two
    /// independent `nextInt(15)` rolls plus 15 experience when allowed to drop experience.
    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let block_drops = args.world.level_info.load().game_rules.block_drops;
            let game_mode = args.player.gamemode.load();
            if !should_drop_experience(args.drop_experience, block_drops, game_mode) {
                return;
            }

            let amount = {
                let mut random = rand::rng();
                15 + random.random_range(0..15) + random.random_range(0..15)
            };
            // `popExperience` (`Block.java:446-449`): `ExperienceOrb.award(level,
            // Vec3.atCenterOf(pos), amount)`.
            ExperienceOrbEntity::spawn(args.world, args.position.to_centered_f64(), amount as u32)
                .await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::should_drop_experience;
    use pumpkin_util::GameMode;

    // Vanilla `SpawnerBlock.spawnAfterBreak` (`SpawnerBlock.java:41-47`) is
    // gated by drop experience, block drops, and the player's game mode.
    #[test]
    fn spawner_experience_requires_drop_experience_and_block_drops() {
        assert!(should_drop_experience(true, true, GameMode::Survival));
        assert!(!should_drop_experience(false, true, GameMode::Survival));
        assert!(!should_drop_experience(true, false, GameMode::Survival));
        assert!(!should_drop_experience(true, true, GameMode::Creative));
        assert!(!should_drop_experience(true, true, GameMode::Spectator));
    }
}
