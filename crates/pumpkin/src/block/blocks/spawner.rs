use std::sync::Arc;

use crate::block::entities::mob_spawner::MobSpawnerBlockEntity;
use crate::entity::experience_orb::ExperienceOrbEntity;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::GameMode;
use rand::RngExt;

use crate::block::{BlockBehaviour, BlockFuture, BrokenArgs, PlacedArgs};

#[pumpkin_block("minecraft:spawner")]
pub struct SpawnerBlock;

impl BlockBehaviour for SpawnerBlock {
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
