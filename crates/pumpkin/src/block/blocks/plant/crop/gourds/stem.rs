use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, CanPlaceAtArgs, GetCloneItemStackArgs,
    GetStateForNeighborUpdateArgs, RandomTickArgs,
    blocks::plant::{
        PlantBlockBase,
        crop::{CropBlockBase, MIN_GROWTH_LIGHT, get_available_moisture},
    },
};
use pumpkin_data::{
    Block, BlockDirection, BlockId, BlockStateId,
    block_properties::{
        BlockProperties, HorizontalFacing, WallTorchLikeProperties, WheatLikeProperties,
    },
    item::Item,
    item_stack::ItemStack,
    tag::{self, Taggable},
};
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, xoroshiro128::Xoroshiro},
};
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::RngExt;

type StemProperties = WheatLikeProperties;
type AttachedStemProperties = WallTorchLikeProperties;

pub struct StemBlock;

impl BlockMetadata for StemBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::PUMPKIN_STEM, BlockId::MELON_STEM].into()
    }
}

impl StemBlock {
    fn state_with_age(block: &Block, state: BlockStateId, age: i32) -> BlockStateId {
        let mut props = StemProperties::from_state_id(state, block);
        props.age = age as u8;
        props.to_state_id(block)
    }

    fn get_attached_stem(dir: HorizontalFacing, block: &Block) -> BlockStateId {
        let attached_block = match block.id {
            id if id == Block::PUMPKIN_STEM.id => &Block::ATTACHED_PUMPKIN_STEM,
            id if id == Block::MELON_STEM.id => &Block::ATTACHED_MELON_STEM,
            _ => &Block::ATTACHED_MELON_STEM, // Should never happen
        };
        let mut props = AttachedStemProperties::default(attached_block);
        props.facing = dir;
        props.to_state_id(attached_block)
    }

    fn get_gourd(block: &Block) -> &Block {
        match block.id {
            id if id == Block::PUMPKIN_STEM.id => &Block::PUMPKIN,
            id if id == Block::MELON_STEM.id => &Block::MELON,
            _ => &Block::MELON, // Should never happen
        }
    }
}

impl BlockBehaviour for StemBlock {
    fn is_valid_bonemeal_target(&self, args: crate::block::BonemealArgs<'_>) -> bool {
        <Self as CropBlockBase>::is_valid_bonemeal_target(self, args.world, args.position)
    }

    /// `StemBlock.getCloneItemStack` (StemBlock.java:110-112): middle-clicking a stem block
    /// yields its seed item (pumpkin seeds / melon seeds) rather than the stem block itself.
    fn get_clone_item_stack(&self, args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        let seed = if args.block.id == Block::PUMPKIN_STEM.id {
            &Item::PUMPKIN_SEEDS
        } else {
            &Item::MELON_SEEDS
        };
        Some(ItemStack::new(1, seed))
    }

    fn perform_bonemeal<'a>(&'a self, args: crate::block::BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            <Self as CropBlockBase>::perform_bonemeal(self, args.world, args.position).await;
            let (_, state) = args.world.get_block_and_state_id(args.position);
            if StemProperties::from_state_id(state, args.block).age == 7 {
                BlockBehaviour::random_tick(
                    self,
                    RandomTickArgs {
                        world: args.world,
                        block: args.block,
                        position: args.position,
                    },
                )
                .await;
            }
        })
    }

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

    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.world.get_raw_brightness(args.position, 0) < MIN_GROWTH_LIGHT {
                return;
            }
            let f: f32 = get_available_moisture(args.world, args.position, args.block).await;
            if rand::rng().random_range(0..=(25.0 / f).floor() as i32) == 0 {
                let (block, state) = args.world.get_block_and_state_id(args.position);
                let props = StemProperties::from_state_id(state, block);
                let age = i32::from(props.age);
                if age < 7 {
                    args.world
                        .set_block_state(
                            args.position,
                            Self::state_with_age(block, state, age + 1),
                            BlockFlags::NOTIFY_NEIGHBORS,
                        )
                        .await;
                } else {
                    let dir = BlockDirection::random_horizontal(&mut RandomGenerator::Xoroshiro(
                        Xoroshiro::from_seed(rand::rng().random()),
                    ));
                    let plant_block_pos = args.position.offset(dir.to_offset());
                    let plant_block_state = args.world.get_block_state(&plant_block_pos);
                    let under_block: &Block = args.world.get_block(&plant_block_pos.down());
                    // `StemBlock.randomTick`: the fruit needs a block from the stem's own
                    // `supports_*_stem_fruit` tag beneath it, which covers grass, podzol,
                    // mycelium, mud and moss as well as dirt and farmland.
                    let fruit_support = if block == &Block::PUMPKIN_STEM {
                        &tag::Block::MINECRAFT_SUPPORTS_PUMPKIN_STEM_FRUIT
                    } else {
                        &tag::Block::MINECRAFT_SUPPORTS_MELON_STEM_FRUIT
                    };
                    if plant_block_state.is_air() && under_block.has_tag(fruit_support) {
                        let attached_stem = Self::get_attached_stem(dir, block);
                        let gourd = Self::get_gourd(block);
                        args.world
                            .set_block_state(
                                &plant_block_pos,
                                gourd.default_state.id,
                                BlockFlags::NOTIFY_NEIGHBORS,
                            )
                            .await;
                        args.world
                            .set_block_state(
                                args.position,
                                attached_stem,
                                BlockFlags::NOTIFY_NEIGHBORS,
                            )
                            .await;
                    }
                }
            }
        })
    }
}

impl PlantBlockBase for StemBlock {
    /// `StemBlock` extends `CropBlock`, so it inherits `CropBlock.canSurvive`
    /// (`CropBlock.java:145-147`) with its own `mayPlaceOn` (`StemBlock.java:78-80`).
    fn can_place_at(&self, block_accessor: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
        <Self as CropBlockBase>::crop_can_survive(self, block_accessor, block_pos)
    }

    fn can_plant_on_top(&self, block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        let block = block_accessor.get_block(pos);
        if block == &Block::PUMPKIN_STEM {
            block.has_tag(&tag::Block::MINECRAFT_SUPPORTS_PUMPKIN_STEM)
        } else {
            block.has_tag(&tag::Block::MINECRAFT_SUPPORTS_MELON_STEM)
        }
    }
}

impl CropBlockBase for StemBlock {}
