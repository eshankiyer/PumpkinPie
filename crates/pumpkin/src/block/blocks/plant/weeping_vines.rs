use std::sync::Arc;

use crate::block::blocks::plant::{
    PlantBlockBase, connected_plant_head,
};
use crate::block::{
    BlockBehaviour, BlockMetadata, BonemealArgs, BrokenArgs, CanPlaceAtArgs,
    GetStateForNeighborUpdateArgs, PlacedArgs, RandomTickArgs,
};
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, KelpLikeProperties};
use pumpkin_data::{Block, BlockId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::RngExt;

pub struct WeepingVinesBlock;
impl BlockMetadata for WeepingVinesBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::WEEPING_VINES, BlockId::WEEPING_VINES_PLANT].into()
    }
}

/// `NetherVines.GROW_PER_TICK_PROBABILITY` (`NetherVines.java:8`), passed to the
/// `GrowingPlantHeadBlock` constructor at `WeepingVinesBlock.java:20`.
const GROW_PER_TICK_PROBABILITY: f64 = 0.1;

fn nether_vines_blocks_to_grow_sync() -> usize {
    let mut random = rand::rng();
    let mut probability = 1.0;
    let mut count = 0;
    while random.random::<f64>() < probability {
        count += 1;
        probability *= 0.826;
    }
    count
}

fn bonemeal_grow_plant_head_sync(
    world: &Arc<crate::world::World>,
    mut head_pos: BlockPos,
    head: &'static Block,
    body: &'static Block,
    growth_direction: pumpkin_data::BlockDirection,
) {
    let state_id = world.get_block_state_id(&head_pos);
    let state = KelpLikeProperties::from_state_id(state_id, head);
    let mut forward_pos = head_pos.offset(growth_direction.to_offset());
    let blocks_to_grow = nether_vines_blocks_to_grow_sync();
    let mut next_age = state.age.saturating_add(1).min(25);

    for _ in 0..blocks_to_grow {
        if !world.is_in_height_limit(forward_pos.0.y)
            || !world.get_block_state(&forward_pos).is_air()
        {
            break;
        }

        let mut grown = KelpLikeProperties::default(head);
        grown.age = next_age;
        world.set_block_state(
            &forward_pos,
            grown.to_state_id(head),
            BlockFlags::NOTIFY_NEIGHBORS,
        );
        world.set_block_state(
            &head_pos,
            body.default_state.id,
            BlockFlags::NOTIFY_NEIGHBORS,
        );
        head_pos = forward_pos;
        forward_pos = forward_pos.offset(growth_direction.to_offset());
        next_age = next_age.saturating_add(1).min(25);
    }
}

fn grow_plant_head_sync(
    world: &Arc<crate::world::World>,
    pos: &BlockPos,
    head: &'static Block,
    body: &'static Block,
    growth_direction: pumpkin_data::BlockDirection,
    grow_per_tick_probability: f64,
) {
    let (block, state_id) = world.get_block_and_state_id(pos);
    if block != head {
        return;
    }
    let age = KelpLikeProperties::from_state_id(state_id, block).age;
    if !super::should_grow_plant_head(age, rand::rng().random::<f64>(), grow_per_tick_probability) {
        return;
    }

    let grow_pos = pos.offset(growth_direction.to_offset());
    if !world.is_in_height_limit(grow_pos.0.y) || !world.get_block(&grow_pos).is_air() {
        return;
    }

    let grown = KelpLikeProperties { age: age + 1 };
    world.set_block_state(
        &grow_pos,
        grown.to_state_id(head),
        BlockFlags::NOTIFY_NEIGHBORS,
    );
    world.set_block_state(pos, body.default_state.id, BlockFlags::NOTIFY_NEIGHBORS);
}

impl BlockBehaviour for WeepingVinesBlock {
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        let Some((head_pos, _)) = connected_plant_head(
            args.world,
            args.position,
            &Block::WEEPING_VINES,
            &Block::WEEPING_VINES_PLANT,
            pumpkin_data::BlockDirection::Down,
        ) else {
            return false;
        };
        args.world.is_in_height_limit(head_pos.0.y - 1)
            && args.world.get_block_state(&head_pos.down()).is_air()
    }

    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    /// `WeepingVinesBlock` inherits `GrowingPlantHeadBlock.performBonemeal`
    /// (`GrowingPlantHeadBlock.java:125-134`) and uses `NetherVines.java:14-22` for
    /// the geometric growth count.
    fn perform_bonemeal(&self, args: BonemealArgs<'_>) {
        if let Some((head_pos, _)) = connected_plant_head(
            args.world,
            args.position,
            &Block::WEEPING_VINES,
            &Block::WEEPING_VINES_PLANT,
            pumpkin_data::BlockDirection::Down,
        ) {
            bonemeal_grow_plant_head_sync(
                args.world,
                head_pos,
                &Block::WEEPING_VINES,
                &Block::WEEPING_VINES_PLANT,
                pumpkin_data::BlockDirection::Down,
            );
        }
    }

    /// `GrowingPlantHeadBlock.randomTick` with `NetherVines.isValidGrowthState`
    /// (`NetherVines.java:10-12`: the target must be air).
    fn random_tick(&self, args: RandomTickArgs<'_>) {
        grow_plant_head_sync(
            args.world,
            args.position,
            &Block::WEEPING_VINES,
            &Block::WEEPING_VINES_PLANT,
            pumpkin_data::BlockDirection::Down,
            GROW_PER_TICK_PROBABILITY,
        );
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        <Self as PlantBlockBase>::can_place_at(self, args.block_accessor, args.position)
    }
    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        <Self as PlantBlockBase>::get_state_for_neighbor_update(
            self,
            args.world,
            args.position,
            args.state_id,
        )
    }
    fn placed(&self, args: PlacedArgs<'_>) {
        {
            let support_pos = args.position.up();
            let support_block = args.world.get_block(&support_pos);
            if support_block == &Block::WEEPING_VINES {
                args.world.set_block_state(
                    &support_pos,
                    Block::WEEPING_VINES_PLANT.default_state.id,
                    BlockFlags::empty(),
                );
            }
        }
    }
    fn broken(&self, args: BrokenArgs<'_>) {
        {
            let support_pos = args.position.up();
            let support_block = args.world.get_block(&support_pos);
            if support_block == &Block::WEEPING_VINES_PLANT {
                args.world.set_block_state(
                    &support_pos,
                    Block::WEEPING_VINES.default_state.id,
                    BlockFlags::empty(),
                );
            }
        }
    }
}

impl PlantBlockBase for WeepingVinesBlock {
    fn can_place_at(
        &self,
        block_accessor: &dyn pumpkin_world::world::BlockAccessor,
        pos: &pumpkin_util::math::position::BlockPos,
    ) -> bool {
        // Determine support block
        let support_pos = pos.up();
        let (support_block, support_block_state) = block_accessor.get_block_and_state(&support_pos);

        if support_block == &Block::WEEPING_VINES || support_block == &Block::WEEPING_VINES_PLANT {
            return true;
        }
        if support_block_state.is_side_solid(pumpkin_data::BlockDirection::Down)
            && support_block.is_solid()
        {
            return true;
        }
        false
    }
    fn get_state_for_neighbor_update(
        &self,
        block_accessor: &dyn BlockAccessor,
        block_pos: &BlockPos,
        block_state: BlockStateId,
    ) -> BlockStateId {
        if !<Self as PlantBlockBase>::can_place_at(self, block_accessor, block_pos) {
            return Block::AIR.default_state.id;
        }
        block_state
    }
}
