use crate::block::{
    AttackArgs, BlockBehaviour, BlockFuture, BlockMetadata, NormalUseArgs, OnEntityStepArgs,
    RandomTickArgs,
    registry::BlockActionResult,
};
use crate::world::World;
use bytes::BufMut;
use pumpkin_data::block_properties::{BlockProperties, RedstoneOreLikeProperties};
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState, particle::Particle};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use std::sync::Arc;

pub struct RedstoneOreBlock;

impl BlockMetadata for RedstoneOreBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::REDSTONE_ORE, BlockId::DEEPSLATE_REDSTONE_ORE].into()
    }
}

impl RedstoneOreBlock {
    fn spawn_particles(world: &Arc<World>, pos: &BlockPos) {
        let mut random = rand::rng();
        for direction in BlockDirection::all() {
            let relative = BlockPos::new(
                pos.0.x + direction.to_offset().x,
                pos.0.y + direction.to_offset().y,
                pos.0.z + direction.to_offset().z,
            );
            if world.get_block_state(&relative).is_solid_render() {
                continue;
            }

            let axis = direction.to_offset();
            let x = if axis.x != 0 {
                0.5 + 0.5625 * f64::from(axis.x)
            } else {
                f64::from(random.random::<f32>())
            };
            let y = if axis.y != 0 {
                0.5 + 0.5625 * f64::from(axis.y)
            } else {
                f64::from(random.random::<f32>())
            };
            let z = if axis.z != 0 {
                0.5 + 0.5625 * f64::from(axis.z)
            } else {
                f64::from(random.random::<f32>())
            };
            let mut data = Vec::with_capacity(8);
            data.put_i32(0xFF0000);
            data.put_f32(1.0);
            world.spawn_particle_with_data(
                Vector3::new(
                    f64::from(pos.0.x) + x,
                    f64::from(pos.0.y) + y,
                    f64::from(pos.0.z) + z,
                ),
                Vector3::new(0.0, 0.0, 0.0),
                0.0,
                1,
                Particle::Dust,
                &data,
            );
        }
    }

    async fn light_up(world: &Arc<World>, pos: &BlockPos, block: &Block, state: &BlockState) {
        let mut props = RedstoneOreLikeProperties::from_state_id(state.id, block);
        if !props.lit {
            props.lit = true;
            world
                .set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
                .await;
        }
    }
}

impl BlockBehaviour for RedstoneOreBlock {
    fn attack<'a>(&'a self, args: AttackArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            Self::spawn_particles(args.world, args.position);
            Self::light_up(args.world, args.position, args.block, args.state).await;
        })
    }

    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            Self::light_up(args.world, args.position, args.block, state).await;
            BlockActionResult::Success
        })
    }

    fn on_entity_step<'a>(&'a self, args: OnEntityStepArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.entity.get_entity().is_sneaking() {
                return;
            }
            let state = args.world.get_block_state(args.position);
            Self::light_up(args.world, args.position, args.block, state).await;
        })
    }

    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let mut props = RedstoneOreLikeProperties::from_state_id(state.id, args.block);
            if props.lit {
                props.lit = false;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::block_properties::has_random_ticks;

    #[test]
    fn lit_redstone_ore_is_randomly_ticking() {
        for block in [&Block::REDSTONE_ORE, &Block::DEEPSLATE_REDSTONE_ORE] {
            let mut props = RedstoneOreLikeProperties::default(block);
            props.lit = true;
            assert!(has_random_ticks(props.to_state_id(block)), "lit {block:?}");
            props.lit = false;
            assert!(
                !has_random_ticks(props.to_state_id(block)),
                "unlit {block:?}"
            );
        }
    }
}
