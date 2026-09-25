use pumpkin_data::entity::EntityType;
use pumpkin_data::fluid::Fluid;
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
    BlockBehaviour, CanPlaceAtArgs, GetStateForNeighborUpdateArgs, OnEntityCollisionArgs,
    OnScheduledTickArgs, PlacedArgs,
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
    let fluid = block_accessor.get_fluid(below);
    let fluid_above = block_accessor.get_fluid(&below.up());
    // FrogspawnBlock.java:102-106 checks fluids, so an existing dry frogspawn survives too.
    (fluid.has_tag(&tag::Fluid::MINECRAFT_SUPPORTS_FROGSPAWN)
        || block.has_tag(&tag::Block::MINECRAFT_SUPPORTS_FROGSPAWN))
        && fluid_above == Fluid::EMPTY
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
    fn placed(&self, args: PlacedArgs<'_>) {
        args.world.schedule_block_tick_long(
            args.block,
            *args.position,
            hatch_delay(),
            TickPriority::Normal,
        );
    }

    /// `FrogspawnBlock.updateShape` (`FrogspawnBlock.java:68-82`): the block pops instantly rather
    /// than scheduling a tick.
    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if can_survive(args.world, args.position) {
            args.state_id
        } else {
            Block::AIR.default_state.id
        }
    }

    /// `FrogspawnBlock.tick` (`FrogspawnBlock.java:84-91`).
    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        if !can_survive(args.world.as_ref(), args.position) {
            destroy(args.world, args.position);
            return;
        }

        // `hatchFrogspawn` (`FrogspawnBlock.java:108-112`).
        destroy(args.world, args.position);
        args.world.play_sound(
            Sound::BlockFrogspawnHatch,
            SoundCategory::Blocks,
            &args.position.to_f64(),
        );
        spawn_tadpoles(args.world, args.position);
    }

    /// `FrogspawnBlock.entityInside` (`FrogspawnBlock.java:93-100`).
    fn on_entity_collision(&self, args: OnEntityCollisionArgs<'_>) {
        if args.entity.get_entity().entity_type == &EntityType::FALLING_BLOCK {
            destroy(args.world, args.position);
        }
    }
}

/// `FrogspawnBlock.destroyBlock` (`FrogspawnBlock.java:114-116`): `dropBlock` is false.
fn destroy(world: &std::sync::Arc<crate::world::World>, position: &BlockPos) {
    world.break_block(position, None, BlockFlags::SKIP_DROPS);
}

/// `FrogspawnBlock.spawnTadpoles` (`FrogspawnBlock.java:118-132`).
fn spawn_tadpoles(world: &std::sync::Arc<crate::world::World>, position: &BlockPos) {
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
        world.spawn_entity(tadpole);
    }
}

/// `FrogspawnBlock.getRandomTadpolePositionOffset` (`FrogspawnBlock.java:134-137`).
fn random_tadpole_position_offset() -> f64 {
    rng()
        .random::<f64>()
        .clamp(f64::from(0.2f32), 0.799_999_997_019_767_8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::BlockState;

    struct FrogspawnAccessor {
        support: &'static Block,
        support_fluid: Fluid,
        spawn_fluid: Fluid,
    }

    impl BlockAccessor for FrogspawnAccessor {
        fn get_block(&self, position: &BlockPos) -> &'static Block {
            if position.0.y == 0 {
                self.support
            } else {
                &Block::FROGSPAWN
            }
        }

        fn get_block_state(&self, position: &BlockPos) -> &'static BlockState {
            self.get_block(position).default_state
        }

        fn get_block_state_id(&self, position: &BlockPos) -> BlockStateId {
            self.get_block_state(position).id
        }

        fn get_block_and_state(
            &self,
            position: &BlockPos,
        ) -> (&'static Block, &'static BlockState) {
            (self.get_block(position), self.get_block_state(position))
        }

        fn get_fluid(&self, position: &BlockPos) -> Fluid {
            if position.0.y == 0 {
                self.support_fluid.clone()
            } else {
                self.spawn_fluid.clone()
            }
        }
    }

    #[test]
    fn existing_frogspawn_survives_above_water_and_waterlogged_support() {
        let position = BlockPos::new(0, 1, 0);
        for support in [&Block::WATER, &Block::OAK_SLAB] {
            assert!(can_survive(
                &FrogspawnAccessor {
                    support,
                    support_fluid: Fluid::WATER,
                    spawn_fluid: Fluid::EMPTY,
                },
                &position,
            ));
        }
    }

    #[test]
    fn frogspawn_rejects_missing_water_support_and_fluid_at_its_own_position() {
        let position = BlockPos::new(0, 1, 0);
        for (support, support_fluid, spawn_fluid) in [
            (&Block::STONE, Fluid::EMPTY, Fluid::EMPTY),
            (&Block::WATER, Fluid::WATER, Fluid::WATER),
        ] {
            assert!(!can_survive(
                &FrogspawnAccessor {
                    support,
                    support_fluid,
                    spawn_fluid
                },
                &position,
            ));
        }
    }
}
