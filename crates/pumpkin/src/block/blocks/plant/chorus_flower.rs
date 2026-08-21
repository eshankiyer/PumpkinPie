use std::sync::Arc;

use pumpkin_data::{
    Block, BlockDirection, BlockStateId,
    block_properties::{BlockProperties, ChorusFlowerLikeProperties, HorizontalFacing},
    tag::{self, Taggable},
    world::WorldEvent,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::{
    tick::TickPriority,
    world::{BlockAccessor, BlockFlags},
};
use rand::RngExt;

use crate::block::{
    BlockBehaviour, BlockFuture, CanPlaceAtArgs, GetStateForNeighborUpdateArgs,
    OnProjectileHitArgs, OnScheduledTickArgs, RandomTickArgs,
};
use crate::world::World;

/// `ChorusFlowerBlock.DEAD_AGE` (`ChorusFlowerBlock.java:30`).
const DEAD_AGE: u8 = 5;

#[pumpkin_block("minecraft:chorus_flower")]
pub struct ChorusFlowerBlock;

impl BlockBehaviour for ChorusFlowerBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_survive(args.block_accessor, args.position)
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if args.direction != BlockDirection::Up && !can_survive(args.world, args.position) {
                args.world
                    .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
            }
            args.state_id
        })
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !can_survive(args.world.as_ref(), args.position) {
                args.world
                    .break_block(args.position, None, BlockFlags::empty())
                    .await;
            }
        })
    }

    /// `ChorusFlowerBlock.randomTick` (`ChorusFlowerBlock.java:64-126`).
    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let world = args.world;
            let pos = *args.position;
            let above = pos.up();
            if !world.get_block_state(&above).is_air() || above.0.y > world.get_top_y() {
                return;
            }

            let state_id = world.get_block_state_id(&pos);
            let age = ChorusFlowerLikeProperties::from_state_id(state_id, args.block).age;
            if age >= DEAD_AGE {
                return;
            }

            let mut grow_upwards = false;
            let mut pillar_on_support_block = false;
            let below = world.get_block(&pos.down());
            if below.has_tag(&tag::Block::MINECRAFT_SUPPORTS_CHORUS_FLOWER) {
                grow_upwards = true;
            } else if below == &Block::CHORUS_PLANT {
                // Walk at most four blocks down the stem, recording whether the
                // pillar bottoms out on a chorus-flower support block.
                let mut height = 1;
                for _ in 0..4 {
                    let test = world.get_block(&pos.down_height(height + 1));
                    if test != &Block::CHORUS_PLANT {
                        if test.has_tag(&tag::Block::MINECRAFT_SUPPORTS_CHORUS_FLOWER) {
                            pillar_on_support_block = true;
                        }
                        break;
                    }
                    height += 1;
                }
                let roll = rand::rng().random_range(0..pillar_roll_bound(pillar_on_support_block));
                grow_upwards = pillar_allows_growth(height, roll);
            } else if below.is_air() {
                grow_upwards = true;
            }

            if grow_upwards
                && all_neighbors_empty(world.as_ref(), &above, None)
                && world.get_block_state(&pos.up_height(2)).is_air()
            {
                place_stem(world, &pos).await;
                place_grown_flower(world, &above, age).await;
            } else if age < DEAD_AGE - 1 {
                let mut attempts = rand::rng().random_range(0..4);
                if pillar_on_support_block {
                    attempts += 1;
                }

                let mut created_branch = false;
                for _ in 0..attempts {
                    let direction =
                        BlockDirection::horizontal_worldgen()[rand::rng().random_range(0..4usize)];
                    let target = pos.offset(direction.to_offset());
                    if world.get_block_state(&target).is_air()
                        && world.get_block_state(&target.down()).is_air()
                        && all_neighbors_empty(world.as_ref(), &target, Some(direction.opposite()))
                    {
                        place_grown_flower(world, &target, age + 1).await;
                        created_branch = true;
                    }
                }

                if created_branch {
                    place_stem(world, &pos).await;
                } else {
                    place_dead_flower(world, &pos).await;
                }
            } else {
                place_dead_flower(world, &pos).await;
            }
        })
    }

    /// `ChorusFlowerBlock.onProjectileHit` (`ChorusFlowerBlock.java:256-262`).
    fn on_projectile_hit<'a>(&'a self, args: OnProjectileHitArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world
                .break_block(args.position, None, BlockFlags::NOTIFY_ALL)
                .await;
        })
    }
}

/// `ChorusFlowerBlock.java:89` - the exclusive bound of the vanilla `nextInt` draw.
const fn pillar_roll_bound(pillar_on_support_block: bool) -> i32 {
    if pillar_on_support_block { 5 } else { 4 }
}

/// `ChorusFlowerBlock.java:89`: `height < 2 || height <= random.nextInt(bound)`.
const fn pillar_allows_growth(height: i32, roll: i32) -> bool {
    height < 2 || height <= roll
}

/// `ChorusFlowerBlock.java:97`: the flower converts itself into a connected stem.
async fn place_stem(world: &Arc<World>, pos: &BlockPos) {
    let state =
        super::chorus_plant::get_state_with_connections(world.as_ref(), &Block::CHORUS_PLANT, pos);
    world
        .set_block_state(pos, state, BlockFlags::NOTIFY_LISTENERS)
        .await;
}

/// `ChorusFlowerBlock.placeGrownFlower` (`ChorusFlowerBlock.java:128-131`).
async fn place_grown_flower(world: &Arc<World>, pos: &BlockPos, age: u8) {
    let props = ChorusFlowerLikeProperties { age };
    world
        .set_block_state(
            pos,
            props.to_state_id(&Block::CHORUS_FLOWER),
            BlockFlags::NOTIFY_LISTENERS,
        )
        .await;
    world.sync_world_event(WorldEvent::SoundChorusGrow, *pos, 0);
}

/// `ChorusFlowerBlock.placeDeadFlower` (`ChorusFlowerBlock.java:133-136`).
async fn place_dead_flower(world: &Arc<World>, pos: &BlockPos) {
    let props = ChorusFlowerLikeProperties { age: DEAD_AGE };
    world
        .set_block_state(
            pos,
            props.to_state_id(&Block::CHORUS_FLOWER),
            BlockFlags::NOTIFY_LISTENERS,
        )
        .await;
    world.sync_world_event(WorldEvent::SoundChorusDeath, *pos, 0);
}

/// `ChorusFlowerBlock.allNeighborsEmpty` (`ChorusFlowerBlock.java:138-146`).
fn all_neighbors_empty(
    block_accessor: &dyn BlockAccessor,
    pos: &BlockPos,
    ignore: Option<HorizontalFacing>,
) -> bool {
    for direction in BlockDirection::horizontal_worldgen() {
        if Some(direction) != ignore
            && !block_accessor
                .get_block_state(&pos.offset(direction.to_offset()))
                .is_air()
        {
            return false;
        }
    }
    true
}

fn can_survive(block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
    let block_below = block_accessor.get_block(&pos.down());

    if block_below == &Block::CHORUS_PLANT
        || block_below.has_tag(&tag::Block::MINECRAFT_SUPPORTS_CHORUS_FLOWER)
    {
        return true;
    }

    if !block_below.is_air() {
        return false;
    }

    // Below is air: the flower is the tip of a horizontal branch.
    // Exactly one horizontal neighbor must be a chorus plant stem.
    let mut plant_count = 0u32;
    for dir in BlockDirection::horizontal() {
        let neighbor = block_accessor.get_block(&pos.offset(dir.to_offset()));
        if neighbor == &Block::CHORUS_PLANT {
            plant_count += 1;
            if plant_count > 1 {
                return false;
            }
        } else if !neighbor.is_air() {
            return false;
        }
    }

    plant_count == 1
}

#[cfg(test)]
mod tests {
    use super::{DEAD_AGE, pillar_allows_growth, pillar_roll_bound};

    #[test]
    fn dead_age_matches_vanilla() {
        assert_eq!(DEAD_AGE, 5);
    }

    #[test]
    fn roll_bound_widens_on_a_supported_pillar() {
        assert_eq!(pillar_roll_bound(false), 4);
        assert_eq!(pillar_roll_bound(true), 5);
    }

    #[test]
    fn short_pillars_always_grow() {
        // height < 2 short-circuits before the random draw.
        assert!(pillar_allows_growth(1, 0));
    }

    #[test]
    fn tall_pillars_grow_only_when_the_roll_reaches_their_height() {
        // With bound 4 the draws are 0..=3, so a height-4 pillar never grows on
        // stone but can on end stone, where the bound is 5. Matches
        // ChorusFlowerBlock.java:89.
        assert!(!pillar_allows_growth(4, 3));
        assert!(pillar_allows_growth(4, 4));
        for roll in 0..pillar_roll_bound(false) {
            assert!(!pillar_allows_growth(4, roll));
        }
        assert!(pillar_allows_growth(4, pillar_roll_bound(true) - 1));
    }
}
