use std::sync::Arc;

use crate::block::BlockBehaviour;
use crate::block::CanPlaceAtArgs;
use crate::block::GetStateForNeighborUpdateArgs;
use crate::block::OnPlaceArgs;
use crate::block::OnScheduledTickArgs;
use crate::world::World;
use pumpkin_data::Block;
use pumpkin_data::BlockDirection;
use pumpkin_data::BlockStateId;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockAccessor;
use pumpkin_world::world::BlockFlags;

#[pumpkin_block("minecraft:dirt_path")]
pub struct DirtPathBlock;

impl BlockBehaviour for DirtPathBlock {
    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        push_up_entities(args.world, args.position);
        args.world.set_block_state(
            args.position,
            Block::DIRT.default_state.id,
            BlockFlags::NOTIFY_ALL,
        );
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        if !can_place_at(args.world, args.position) {
            return Block::DIRT.default_state.id;
        }

        args.block.default_state.id
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if args.direction == BlockDirection::Up && !can_place_at(args.world, args.position) {
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
        }
        args.state_id
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_place_at(args.block_accessor, args.position)
    }
}

fn can_place_at(world: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
    let (block, state) = world.get_block_and_state(&block_pos.up());
    !state.is_solid() || block.has_tag(&tag::Block::C_FENCE_GATES)
}

/// Simplified port of vanilla's `Block.pushEntitiesUp`: teleports any entity
/// overlapping the 1x1x1 column at `block_pos` up by one block, rather than
/// vanilla's exact old-shape/new-shape collision diff.
fn push_up_entities(world: &Arc<World>, block_pos: &BlockPos) {
    let min = Vector3::new(
        f64::from(block_pos.0.x),
        f64::from(block_pos.0.y),
        f64::from(block_pos.0.z),
    );
    let max = Vector3::new(min.x + 1.0, min.y + 1.0, min.z + 1.0);
    let aabb = BoundingBox::new(min, max);
    for entity in world.get_entities_at_box(&aabb) {
        let entity = entity.get_entity();
        let pos = entity.pos.load();
        entity.pos.store(Vector3::new(pos.x, pos.y + 1.0, pos.z));
    }
}
