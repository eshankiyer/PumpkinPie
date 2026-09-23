use crate::block::blocks::plant::PlantBlockBase;
use crate::block::{
    BlockBehaviour, BonemealArgs, CanPlaceAtArgs, GetStateForNeighborUpdateArgs, OnPlaceArgs,
    PlacedArgs,
};
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{
    BlockProperties, DoubleBlockHalf, SmallDripleafLikeProperties,
};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, tag};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::{BlockAccessor, BlockFlags};

#[pumpkin_block("minecraft:small_dripleaf")]
pub struct SmallDripleafBlock;

impl BlockBehaviour for SmallDripleafBlock {
    /// Vanilla `SmallDripleafBlock.isValidBonemealTarget`
    /// (`SmallDripleafBlock.java:115-117`): always a valid target.
    fn is_valid_bonemeal_target(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    /// Vanilla `SmallDripleafBlock.isBonemealSuccess`
    /// (`SmallDripleafBlock.java:120-122`): always succeeds.
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    fn perform_bonemeal(&self, args: BonemealArgs<'_>) {
        // Vanilla `SmallDripleafBlock.performBonemeal`
        // (`SmallDripleafBlock.java:125-133`): bone-mealing the lower half clears the
        // upper half to its fluid's legacy block and grows a big dripleaf column of
        // random height in its place; bone-mealing the upper half re-runs against the
        // lower half.
        let props = SmallDripleafLikeProperties::from_state_id(args.state_id, args.block);
        if props.half != DoubleBlockHalf::Lower {
            let below_pos = args.position.down();
            let (below_block, below_state_id) = args.world.get_block_and_state_id(&below_pos);
            self.perform_bonemeal(BonemealArgs {
                world: args.world,
                block: below_block,
                position: &below_pos,
                state_id: below_state_id,
            });
            return;
        }

        // `level.setBlock(above, level.getFluidState(above).createLegacyBlock(), 18)`
        // (`SmallDripleafBlock.java:128`): restore air or source water above. Flag 18
        // suppresses neighbor reactions so the upper-half removal does not cascade
        // before the column below is replaced; NOTIFY_LISTENERS alone keeps clients in
        // sync here.
        let above = args.position.up();
        let cleared = if args.world.get_block(&above) == &Block::WATER {
            Block::WATER.default_state.id
        } else {
            BlockStateId::AIR
        };
        args.world
            .set_block_state(&above, cleared, BlockFlags::NOTIFY_LISTENERS);

        super::big_dripleaf::place_with_random_height(args.world, args.position, props.facing);
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        <Self as PlantBlockBase>::can_place_at(self, args.block_accessor, args.position)
    }
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let facing = args
            .player
            .living_entity
            .entity
            .get_horizontal_facing()
            .opposite();
        let mut small_dripleaf_props = SmallDripleafLikeProperties::default(args.block);

        small_dripleaf_props.facing = facing;
        small_dripleaf_props.waterlogged = args.replacing.water_source();
        small_dripleaf_props.half = DoubleBlockHalf::Lower;

        small_dripleaf_props.to_state_id(args.block)
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
        let lower_small_dripleaf_props =
            SmallDripleafLikeProperties::from_state_id(args.state_id, args.block);
        if lower_small_dripleaf_props.half != DoubleBlockHalf::Lower {
            return;
        }

        let mut upper_small_dripleaf_props =
            SmallDripleafLikeProperties::default(&Block::SMALL_DRIPLEAF);

        let upper_block = args.world.get_block(&args.position.up());
        upper_small_dripleaf_props.facing = lower_small_dripleaf_props.facing;
        upper_small_dripleaf_props.waterlogged = upper_block == &Block::WATER;
        upper_small_dripleaf_props.half = DoubleBlockHalf::Upper;

        args.world.set_block_state(
            &args.position.up(),
            upper_small_dripleaf_props.to_state_id(&Block::SMALL_DRIPLEAF),
            BlockFlags::NOTIFY_ALL | BlockFlags::SKIP_BLOCK_ADDED_CALLBACK,
        );
    }
}
fn is_small_dripleaf_waterlogged(state_id: BlockStateId) -> bool {
    let dripleaf_props =
        SmallDripleafLikeProperties::from_state_id(state_id, &Block::SMALL_DRIPLEAF);
    dripleaf_props.waterlogged
}
impl PlantBlockBase for SmallDripleafBlock {
    fn can_plant_on_top(&self, block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        let support_block = block_accessor.get_block(pos);

        if support_block == &Block::SMALL_DRIPLEAF {
            return true;
        }
        let upper_block = block_accessor.get_block(&pos.up_height(2));
        if upper_block != &Block::AIR
            && upper_block != &Block::WATER
            && upper_block != &Block::SMALL_DRIPLEAF
        {
            return false;
        }
        let (replacing_block, replacing_block_state) =
            block_accessor.get_block_and_state(&pos.up());
        if replacing_block == &Block::SMALL_DRIPLEAF && replacing_block_state.is_waterlogged() {
            //in case of neighbor update check
            supports_small_dripleaf(support_block, true)
        } else {
            supports_small_dripleaf(support_block, replacing_block == &Block::WATER)
        }
    }

    #[allow(clippy::unused_async_trait_impl)]
    fn get_state_for_neighbor_update(
        &self,
        block_accessor: &dyn BlockAccessor,
        block_pos: &BlockPos,
        block_state: BlockStateId,
    ) -> BlockStateId {
        if !<Self as PlantBlockBase>::can_place_at(self, block_accessor, block_pos) {
            if is_small_dripleaf_waterlogged(block_state) {
                return Block::WATER.default_state.id;
            }
            return Block::AIR.default_state.id;
        }
        let upper_block = block_accessor.get_block(&block_pos.up());
        let below_blow = block_accessor.get_block(&block_pos.down());
        if upper_block != &Block::SMALL_DRIPLEAF && below_blow != &Block::SMALL_DRIPLEAF {
            if is_small_dripleaf_waterlogged(block_state) {
                return Block::WATER.default_state.id;
            }
            return Block::AIR.default_state.id;
        }
        block_state
    }
}
fn supports_small_dripleaf(support_block: &Block, underwater: bool) -> bool {
    if support_block.has_tag(&tag::Block::MINECRAFT_SUPPORTS_SMALL_DRIPLEAF) {
        return true;
    }
    underwater && support_block.has_tag(&tag::Block::MINECRAFT_SUPPORTS_BIG_DRIPLEAF)
}
