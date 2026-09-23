use crate::block::blocks::plant::{PlantBlockBase, connected_plant_head, grow_plant_head};
use crate::block::{
    BlockBehaviour, BlockMetadata, BonemealArgs, BrokenArgs, CanPlaceAtArgs,
    GetStateForNeighborUpdateArgs, PlacedArgs, RandomTickArgs,
};
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, KelpLikeProperties, WaterLikeProperties};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockId, tag};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
pub struct KelpBlock;

impl BlockMetadata for KelpBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::KELP, BlockId::KELP_PLANT].into()
    }
}

/// `KelpBlock.GROW_PER_TICK_PROBABILITY` (`KelpBlock.java:23`).
const GROW_PER_TICK_PROBABILITY: f64 = 0.14;

impl BlockBehaviour for KelpBlock {
    /// `GrowingPlantHeadBlock.randomTick` with `KelpBlock.canGrowInto`
    /// (`KelpBlock.java:36-38`: the target must be water).
    fn random_tick(&self, args: RandomTickArgs<'_>) {
        grow_plant_head(
            args.world,
            args.position,
            &Block::KELP,
            &Block::KELP_PLANT,
            pumpkin_data::BlockDirection::Up,
            GROW_PER_TICK_PROBABILITY,
            |block| block == &Block::WATER,
        );
    }

    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        // `GrowingPlantBodyBlock.isValidBonemealTarget` (`GrowingPlantBodyBlock.java:68-76`)
        // resolves the head first via `getHeadPos`, then applies the head-side check
        // `GrowingPlantHeadBlock.isValidBonemealTarget` (`GrowingPlantHeadBlock.java:117-120`)
        // with `KelpBlock.canGrowInto` (`KelpBlock.java:36-38`: the target must be water).
        let Some((head_pos, _)) = connected_plant_head(
            args.world,
            args.position,
            &Block::KELP,
            &Block::KELP_PLANT,
            pumpkin_data::BlockDirection::Up,
        ) else {
            return false;
        };
        args.world.is_in_height_limit(head_pos.0.y + 1)
            && args.world.get_block(&head_pos.up()) == &Block::WATER
    }

    /// `GrowingPlantHeadBlock.isBonemealSuccess` (`GrowingPlantHeadBlock.java:122-124`),
    /// inherited unchanged by the body (`GrowingPlantBodyBlock.java:78-81`).
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    /// `GrowingPlantBodyBlock.performBonemeal` (`GrowingPlantBodyBlock.java:84-90`) delegates
    /// to the head's `GrowingPlantHeadBlock.performBonemeal` (`GrowingPlantHeadBlock.java:126-140`)
    /// with `KelpBlock.getBlocksToGrowWhenBonemealed` = 1 (`KelpBlock.java:66-68`). Vanilla
    /// converts the old head to body through `updateShape`; pumpkin has no such update chain
    /// for these blocks, so it is applied explicitly, exactly as `bonemeal_grow_plant_head` does.
    fn perform_bonemeal(&self, args: BonemealArgs<'_>) {
        let Some((head_pos, head_state_id)) = connected_plant_head(
            args.world,
            args.position,
            &Block::KELP,
            &Block::KELP_PLANT,
            pumpkin_data::BlockDirection::Up,
        ) else {
            return;
        };

        let forward_pos = head_pos.up();
        if !args.world.is_in_height_limit(forward_pos.0.y)
            || args.world.get_block(&forward_pos) != &Block::WATER
        {
            return;
        }

        let mut grown = KelpLikeProperties::from_state_id(head_state_id, &Block::KELP);
        grown.age = grown.age.saturating_add(1).min(25);
        args.world.set_block_state(
            &forward_pos,
            grown.to_state_id(&Block::KELP),
            BlockFlags::NOTIFY_NEIGHBORS,
        );
        args.world.set_block_state(
            &head_pos,
            Block::KELP_PLANT.default_state.id,
            BlockFlags::NOTIFY_NEIGHBORS,
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
        let support_pos = args.position.down();
        let support_block = args.world.get_block(&support_pos);
        if support_block == &Block::KELP {
            args.world.set_block_state(
                &support_pos,
                Block::KELP_PLANT.default_state.id,
                BlockFlags::empty(),
            );
        }
    }
    fn broken(&self, args: BrokenArgs<'_>) {
        let support_pos = args.position.down();
        let support_block = args.world.get_block(&support_pos);
        if support_block == &Block::KELP_PLANT {
            args.world.set_block_state(
                &support_pos,
                Block::KELP.default_state.id,
                BlockFlags::empty(),
            );
            args.world.set_block_state(
                args.position,
                Block::WATER.default_state.id,
                BlockFlags::empty(),
            );
        }
    }
}

impl PlantBlockBase for KelpBlock {
    fn can_plant_on_top(
        &self,
        block_accessor: &dyn pumpkin_world::world::BlockAccessor,
        pos: &pumpkin_util::math::position::BlockPos,
    ) -> bool {
        // Determine support block
        let support_pos = pos;
        let (replacing_block, replacing_block_state) =
            block_accessor.get_block_and_state(&pos.up());
        let (support_block, support_block_state) = block_accessor.get_block_and_state(support_pos);
        if replacing_block == &Block::WATER {
            let water_props =
                WaterLikeProperties::from_state_id(replacing_block_state.id, replacing_block);

            //Only allow placing kelp on either full water or downward flowing water
            if water_props.level != 0 && water_props.level != 8 {
                return false;
            }
        } else {
            //Replacing block can also be a kelp_plant or kelp in case this is an neighbour update check
            if replacing_block != &Block::KELP_PLANT && replacing_block != &Block::KELP {
                return false;
            }
        }
        // If placing the base kelp block, allow placement on water or on other kelp segments.
        if support_block == &Block::KELP || support_block == &Block::KELP_PLANT {
            return true;
        }
        if support_block.has_tag(&tag::Block::MINECRAFT_CANNOT_SUPPORT_KELP) {
            return false;
        }
        if support_block_state.is_side_solid(pumpkin_data::BlockDirection::Up)
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
            return Block::WATER.default_state.id;
        }
        block_state
    }
}
