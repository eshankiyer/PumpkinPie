use std::sync::Arc;

use crate::block::entities::mob_spawner::MobSpawnerBlockEntity;
use crate::entity::experience_orb::ExperienceOrbEntity;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::random::{RandomGenerator, RandomImpl, get_seed, xoroshiro128::Xoroshiro};

use crate::block::{BlockBehaviour, BlockFuture, BrokenArgs, PlacedArgs};

#[pumpkin_block("minecraft:spawner")]
pub struct SpawnerBlock;

/// `SpawnerBlock#spawnAfterBreak` in vanilla: `15 + random.nextInt(15) + random.nextInt(15)`,
/// giving a range of 15-43 inclusive. Unlike ore blocks, the spawner's override does not
/// route through `Block#tryDropExperience`/`EnchantmentHelper#processBlockExperience`, so
/// Silk Touch does not suppress this drop (only the block item itself is never obtainable).
fn spawner_break_experience(random: &mut RandomGenerator) -> i32 {
    15 + random.next_bounded_i32(15) + random.next_bounded_i32(15)
}

impl BlockBehaviour for SpawnerBlock {
    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let hopper_block_entity = MobSpawnerBlockEntity::new(*args.position, None);
            args.world.add_block_entity(Arc::new(hopper_block_entity));
        })
    }

    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(get_seed()));
            let amount = spawner_break_experience(&mut random);
            if amount > 0 {
                ExperienceOrbEntity::spawn(args.world, args.position.to_f64(), amount as u32).await;
            }
        })
    }
}

#[cfg(test)]
mod test {
    use super::spawner_break_experience;
    use pumpkin_util::random::{RandomGenerator, xoroshiro128::Xoroshiro};

    #[test]
    fn experience_is_within_vanilla_range() {
        for seed in 0..1000u64 {
            let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(seed));
            let amount = spawner_break_experience(&mut random);
            assert!((15..=43).contains(&amount), "amount {amount} out of range");
        }
    }
}
