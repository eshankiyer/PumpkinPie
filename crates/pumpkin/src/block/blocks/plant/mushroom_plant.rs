use pumpkin_data::tag::Taggable;
use pumpkin_data::{BlockId, BlockStateId, tag};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockAccessor;

use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, CanPlaceAtArgs, GetStateForNeighborUpdateArgs,
    blocks::plant::PlantBlockBase,
};

pub struct MushroomPlantBlock;

impl BlockMetadata for MushroomPlantBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::BROWN_MUSHROOM, BlockId::RED_MUSHROOM].into()
    }
}

impl BlockBehaviour for MushroomPlantBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        <Self as PlantBlockBase>::can_place_at(self, args.block_accessor, args.position)
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            <Self as PlantBlockBase>::get_state_for_neighbor_update(
                self,
                args.world,
                args.position,
                args.state_id,
            )
            .await
        })
    }
}

/// `MushroomBlock.MAX_LIGHT` gate: `canSurvive` needs raw brightness *strictly below* 13
/// (`MushroomBlock.java:86`). Note the direction - mushrooms want darkness, crops want light,
/// which is why an accessor with no light engine must skip the gate rather than assume a value.
const MAX_SURVIVE_LIGHT: u8 = 13;

impl PlantBlockBase for MushroomPlantBlock {
    /// `MushroomBlock.canSurvive` (`MushroomBlock.java:83-87`): a block tagged
    /// `overrides_mushroom_light_requirement` below always works; otherwise the light must be
    /// below 13 and the block below must be `mayPlaceOn`, i.e. `isSolidRender`
    /// (`MushroomBlock.java:78-80`).
    fn can_place_at(&self, block_accessor: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
        let below_pos = block_pos.down();
        let (below, below_state) = block_accessor.get_block_and_state(&below_pos);
        if below.has_tag(&tag::Block::MINECRAFT_OVERRIDES_MUSHROOM_LIGHT_REQUIREMENT) {
            return true;
        }
        let dark_enough = block_accessor
            .get_raw_brightness(block_pos, 0)
            .is_none_or(|light| light < MAX_SURVIVE_LIGHT);
        dark_enough && below_state.is_solid_render()
    }
}
