use crate::block::blocks::falling::FallingBlock;
use crate::block::registry::BlockActionResult;
use crate::block::{BlockBehaviour, BrokenArgs, NormalUseArgs, PlacedArgs};
use crate::world::World;
use pumpkin_data::BlockStateId;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use rand::{RngExt, rng};
use std::sync::Arc;

#[pumpkin_block("minecraft:dragon_egg")]
pub struct DragonEggBlock;

impl DragonEggBlock {
    fn teleport(world: &Arc<World>, pos: &BlockPos, state_id: BlockStateId) {
        let min_y = world.dimension.min_y;
        let max_y = min_y + world.dimension.height;
        for _ in 0..1000 {
            let (x, y, z) = {
                let mut rng = rng();
                (
                    pos.0.x + rng.random_range(0..16) - rng.random_range(0..16),
                    pos.0.y + rng.random_range(0..8) - rng.random_range(0..8),
                    pos.0.z + rng.random_range(0..16) - rng.random_range(0..16),
                )
            };
            if y < min_y || y >= max_y {
                continue;
            }
            let test_pos = BlockPos::new(x, y, z);

            let state = world.get_block_state(&test_pos);
            let below_state = world.get_block_state(&test_pos.down());

            if state.is_air()
                && !below_state.is_air()
                && world.worldborder.lock().contains_block(x, z)
            {
                world.set_block_state(
                    &test_pos,
                    state_id,
                    pumpkin_world::world::BlockFlags::NOTIFY_LISTENERS,
                );
                // Do not erase a replacement that arrived at the source while moving.
                if world.get_block_state(pos).id != state_id {
                    return;
                }
                world.set_block_state(
                    pos,
                    pumpkin_data::Block::AIR.default_state.id,
                    pumpkin_world::world::BlockFlags::NOTIFY_ALL,
                );
                return;
            }
        }
    }
}

impl BlockBehaviour for DragonEggBlock {
    fn placed(&self, args: PlacedArgs<'_>) {
        args.world
            .schedule_block_tick(args.block, *args.position, 5, TickPriority::Normal);
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        let state_id = args.world.get_block_state(args.position).id;
        Self::teleport(args.world, args.position, state_id);
        BlockActionResult::Success
    }

    // Dragon egg is typically teleported when attacked
    fn broken(&self, args: BrokenArgs<'_>) {
        let state_id = args.world.get_block_state(args.position).id;
        Self::teleport(args.world, args.position, state_id);
    }

    fn on_scheduled_tick(&self, args: crate::block::OnScheduledTickArgs<'_>) {
        FallingBlock::on_scheduled_tick(&FallingBlock, args);
    }
}
