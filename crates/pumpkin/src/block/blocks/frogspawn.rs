use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::{RngExt, rng};
use uuid::Uuid;

use crate::block::{
    BlockBehaviour, BlockFuture, CanPlaceAtArgs, GetStateForNeighborUpdateArgs,
    OnEntityCollisionArgs, OnScheduledTickArgs, PlacedArgs,
};

/// `FrogspawnBlock.DEFAULT_MIN_HATCH_TICK_DELAY` (`FrogspawnBlock.java:34`).
const MIN_HATCH_TICK_DELAY: i64 = 3600;
/// `FrogspawnBlock.DEFAULT_MAX_HATCH_TICK_DELAY` (`FrogspawnBlock.java:35`).
const MAX_HATCH_TICK_DELAY: i64 = 12000;

#[pumpkin_block("minecraft:frogspawn")]
pub struct FrogspawnBlock;

/// `FrogspawnBlock.mayPlaceOn` (`FrogspawnBlock.java:102-106`): the block below must hold water
/// (`minecraft:supports_frogspawn`) and the frogspawn's own position must be fluid-free.
fn may_place_on(block_accessor: &dyn BlockAccessor, below: &BlockPos) -> bool {
    let block = block_accessor.get_block(below);
    let above = block_accessor.get_block(&below.up());
    (block.has_tag(&tag::Fluid::MINECRAFT_SUPPORTS_FROGSPAWN)
        || block.has_tag(&tag::Block::MINECRAFT_SUPPORTS_FROGSPAWN))
        && above.is_air()
}

/// `FrogspawnBlock.canSurvive` (`FrogspawnBlock.java:54-57`).
fn can_survive(block_accessor: &dyn BlockAccessor, position: &BlockPos) -> bool {
    may_place_on(block_accessor, &position.down())
}

/// `FrogspawnBlock.getFrogspawnHatchDelay` (`FrogspawnBlock.java:64-66`).
fn hatch_delay() -> i64 {
    rng().random_range(MIN_HATCH_TICK_DELAY..MAX_HATCH_TICK_DELAY)
}

impl BlockBehaviour for FrogspawnBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_survive(args.block_accessor, args.position)
    }

    /// `FrogspawnBlock.onPlace` (`FrogspawnBlock.java:59-62`).
    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world.schedule_block_tick_long(
                args.block,
                *args.position,
                hatch_delay(),
                TickPriority::Normal,
            );
        })
    }

    /// `FrogspawnBlock.updateShape` (`FrogspawnBlock.java:68-82`): the block pops instantly rather
    /// than scheduling a tick.
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if can_survive(args.world, args.position) {
                args.state_id
            } else {
                Block::AIR.default_state.id
            }
        })
    }

    /// `FrogspawnBlock.tick` (`FrogspawnBlock.java:84-91`).
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !can_survive(args.world.as_ref(), args.position) {
                destroy(args.world, args.position).await;
                return;
            }

            // `hatchFrogspawn` (`FrogspawnBlock.java:108-112`).
            destroy(args.world, args.position).await;
            args.world.play_sound(
                Sound::BlockFrogspawnHatch,
                SoundCategory::Blocks,
                &args.position.to_f64(),
            );
            spawn_tadpoles(args.world, args.position).await;
        })
    }

    /// `FrogspawnBlock.entityInside` (`FrogspawnBlock.java:93-100`).
    fn on_entity_collision<'a>(&'a self, args: OnEntityCollisionArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.entity.get_entity().entity_type == &EntityType::FALLING_BLOCK {
                destroy(args.world, args.position).await;
            }
        })
    }
}

/// `FrogspawnBlock.destroyBlock` (`FrogspawnBlock.java:114-116`): `dropBlock` is false.
async fn destroy(world: &std::sync::Arc<crate::world::World>, position: &BlockPos) {
    world
        .break_block(position, None, BlockFlags::SKIP_DROPS)
        .await;
}

/// `FrogspawnBlock.spawnTadpoles` (`FrogspawnBlock.java:118-132`).
async fn spawn_tadpoles(world: &std::sync::Arc<crate::world::World>, position: &BlockPos) {
    let amount = rng().random_range(2..6);
    for _ in 0..amount {
        let x = f64::from(position.0.x) + random_tadpole_position_offset();
        let z = f64::from(position.0.z) + random_tadpole_position_offset();
        let y = f64::from(position.0.y) - 0.5;
        let yaw = rng().random_range(1..361) as f32;

        let tadpole = crate::entity::r#type::from_type(
            &EntityType::TADPOLE,
            pumpkin_util::math::vector3::Vector3::new(x, y, z),
            world,
            Uuid::new_v4(),
        );
        tadpole.get_entity().set_rotation(yaw, 0.0);
        tadpole
            .get_entity()
            .persistence_required
            .store(true, std::sync::atomic::Ordering::Relaxed);
        world.spawn_entity(tadpole).await;
    }
}

/// `FrogspawnBlock.getRandomTadpolePositionOffset` (`FrogspawnBlock.java:134-137`).
fn random_tadpole_position_offset() -> f64 {
    rng()
        .random::<f64>()
        .clamp(f64::from(0.2f32), 0.799_999_997_019_767_8)
}
