use crate::block::{
    AttackArgs, BlockBehaviour, BlockFuture, BlockMetadata, BrokenArgs, NormalUseArgs,
    OnEntityStepArgs, RandomTickArgs, UseWithItemArgs, registry::BlockActionResult,
};
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::world::World;
use pumpkin_data::block_properties::{BlockProperties, RedstoneOreLikeProperties};
use pumpkin_data::particle::Particle;
use pumpkin_data::{Block, BlockId, BlockState};
use pumpkin_protocol::ser::NetworkWriteExt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use rand::{RngExt, rng};
use std::sync::Arc;

pub struct RedstoneOreBlock;

impl BlockMetadata for RedstoneOreBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::REDSTONE_ORE, BlockId::DEEPSLATE_REDSTONE_ORE].into()
    }
}

impl RedstoneOreBlock {
    fn spawn_particles(world: &Arc<World>, pos: &BlockPos) {
        let mut random = rng();
        let mut data = Vec::with_capacity(8);
        let _ = data.write_i32_be(0x00FF0000);
        let _ = data.write_f32_be(1.0);

        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            let relative = BlockPos::new(pos.0.x + dx, pos.0.y + dy, pos.0.z + dz);
            let adjacent_state = world.get_block_state(&relative);
            let adjacent_block = Block::from_state_id(adjacent_state.id);
            if adjacent_state.is_full_cube()
                && adjacent_state.opacity >= 15
                && adjacent_block.name != "tinted_glass"
            {
                continue;
            }
            let x = if dx != 0 {
                0.5 + 0.5625 * f64::from(dx)
            } else {
                random.random::<f64>()
            };
            let y = if dy != 0 {
                0.5 + 0.5625 * f64::from(dy)
            } else {
                random.random::<f64>()
            };
            let z = if dz != 0 {
                0.5 + 0.5625 * f64::from(dz)
            } else {
                random.random::<f64>()
            };
            let particle_pos = Vector3::new(
                f64::from(pos.0.x) + x,
                f64::from(pos.0.y) + y,
                f64::from(pos.0.z) + z,
            );
            for player in world.players.load().iter() {
                if player.position().squared_distance_to_vec(&particle_pos) <= 32.0 * 32.0 {
                    player.spawn_particle_with_data(
                        particle_pos,
                        Vector3::new(0.0, 0.0, 0.0),
                        0.0,
                        1,
                        Particle::Dust,
                        &data,
                    );
                }
            }
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

    async fn interact(world: &Arc<World>, pos: &BlockPos, block: &Block, state: &BlockState) {
        Self::spawn_particles(world, pos);
        Self::light_up(world, pos, block, state).await;
    }
}

impl BlockBehaviour for RedstoneOreBlock {
    fn attack<'a>(&'a self, args: AttackArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            Self::interact(args.world, args.position, args.block, args.state).await;
        })
    }

    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            Self::interact(args.world, args.position, args.block, state).await;
            BlockActionResult::Success
        })
    }

    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            Self::interact(args.world, args.position, args.block, state).await;

            // RedStoneOreBlock.useItemOn lights the ore before allowing a placeable block item
            // to continue through BlockItem's placement path. `BlockPlaceContext.canPlace()` only
            // checks whether the clicked block or the adjacent block can be replaced.
            let can_place = state.replaceable()
                || args
                    .world
                    .get_block_state(&BlockPos::new(
                        args.position.0.x + args.hit.face.to_offset().x,
                        args.position.0.y + args.hit.face.to_offset().y,
                        args.position.0.z + args.hit.face.to_offset().z,
                    ))
                    .replaceable();
            if Block::from_item_id(args.item_stack.item.id).is_some() && can_place {
                BlockActionResult::Pass
            } else {
                BlockActionResult::Success
            }
        })
    }

    fn on_entity_step<'a>(&'a self, args: OnEntityStepArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.entity.get_entity().is_sneaking() {
                return;
            }
            let state = args.world.get_block_state(args.position);
            Self::interact(args.world, args.position, args.block, state).await;
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

    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // RedStoneOreBlock.spawnAfterBreak awards 1..=5 XP when normal block drops are
            // enabled and the break is eligible for drops. Silk Touch is handled by the vanilla
            // block-experience enchantment effect and therefore suppresses this award.
            if args.player.gamemode.load() == pumpkin_util::GameMode::Creative
                || !args.world.level_info.load().game_rules.block_drops
                || !args
                    .player
                    .can_harvest(args.state, Block::from_state_id(args.state.id))
                    .await
            {
                return;
            }

            let tool = args.player.inventory().held_item().await;
            if tool.get_enchantment_level(&pumpkin_data::Enchantment::SILK_TOUCH) > 0 {
                return;
            }

            let amount = rand::rng().random_range(1..=5);
            ExperienceOrbEntity::spawn(args.world, args.position.to_centered_f64(), amount).await;
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
