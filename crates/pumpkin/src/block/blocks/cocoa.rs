use pumpkin_data::block_properties::{BlockProperties, CocoaLikeProperties};
use pumpkin_data::{
    Block, BlockDirection, BlockStateId, HorizontalFacingExt,
    tag::{self, Taggable},
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::RngExt;

use crate::block::{
    BlockBehaviour, BlockFuture, BonemealArgs, CanPlaceAtArgs, GetStateForNeighborUpdateArgs,
    OnPlaceArgs, RandomTickArgs,
};

/// `CocoaBlock` (`net/minecraft/world/level/block/CocoaBlock.java:28`).
///
/// Cocoa had no behaviour at all: pods never ripened, never responded to bone meal, and stayed
/// floating when the jungle log they hung on was mined.
#[pumpkin_block("minecraft:cocoa")]
pub struct CocoaBlock;

/// `CocoaBlock.MAX_AGE` (CocoaBlock.java:30).
const MAX_AGE: u8 = 2;

/// `CocoaBlock#randomTick` grows on a 1-in-5 roll (CocoaBlock.java:53).
const GROWTH_CHANCE: i32 = 5;

/// `CocoaBlock#canSurvive` (CocoaBlock.java:61-65): the block the pod FACES must be in
/// `BlockTags.SUPPORTS_COCOA` - unlike most wall-mounted blocks, cocoa's `FACING` points AT its
/// support rather than away from it.
fn can_survive(world: &dyn BlockAccessor, position: &BlockPos, facing: BlockDirection) -> bool {
    world
        .get_block(&position.offset(facing.to_offset()))
        .has_tag(&tag::Block::MINECRAFT_SUPPORTS_COCOA)
}

fn facing_of(state_id: BlockStateId) -> BlockDirection {
    CocoaLikeProperties::from_state_id(state_id, &Block::COCOA)
        .facing
        .to_block_direction()
}

impl BlockBehaviour for CocoaBlock {
    /// `CocoaBlock#getStateForPlacement` (CocoaBlock.java:72-88). Pumpkin hands `on_place` the
    /// direction from the pod toward the block it was placed against, which is exactly the
    /// `FACING` vanilla stores.
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = CocoaLikeProperties::default(args.block);
            let Some(facing) = args.direction.to_horizontal_facing() else {
                return BlockStateId::AIR;
            };
            props.facing = facing;
            props.to_state_id(args.block)
        })
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        let facing = match args.direction {
            Some(direction) if direction.to_horizontal_facing().is_some() => direction,
            _ => facing_of(args.state.id),
        };
        can_survive(args.block_accessor, args.position, facing)
    }

    /// `CocoaBlock#randomTick` (CocoaBlock.java:51-59). `isRandomlyTicking` stops at age 2
    /// (CocoaBlock.java:46-49), which the age check below reproduces.
    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if rand::rng().random_range(0..GROWTH_CHANCE) != 0 {
                return;
            }
            let state_id = args.world.get_block_state_id(args.position);
            let mut props = CocoaLikeProperties::from_state_id(state_id, args.block);
            if props.age >= MAX_AGE {
                return;
            }
            props.age += 1;
            args.world
                .set_block_state(
                    args.position,
                    props.to_state_id(args.block),
                    BlockFlags::NOTIFY_LISTENERS,
                )
                .await;
        })
    }

    /// `CocoaBlock#updateShape` (CocoaBlock.java:90-104): only the neighbour the pod faces can
    /// break it, and it breaks immediately rather than scheduling a tick.
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let facing = facing_of(args.state_id);
            if args.direction == facing && !can_survive(args.world, args.position, facing) {
                return BlockStateId::AIR;
            }
            args.state_id
        })
    }

    /// `CocoaBlock#isValidBonemealTarget` (CocoaBlock.java:106-109).
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        CocoaLikeProperties::from_state_id(args.state_id, args.block).age < MAX_AGE
    }

    /// `CocoaBlock#isBonemealSuccess` (CocoaBlock.java:111-114) is unconditionally true.
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    /// `CocoaBlock#performBonemeal` (CocoaBlock.java:116-119) advances one age step, with no
    /// feature placement, so it is fully portable.
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let mut props = CocoaLikeProperties::from_state_id(args.state_id, args.block);
            if props.age >= MAX_AGE {
                return;
            }
            props.age += 1;
            args.world
                .set_block_state(
                    args.position,
                    props.to_state_id(args.block),
                    BlockFlags::NOTIFY_LISTENERS,
                )
                .await;
        })
    }
}

#[cfg(test)]
mod test {
    use super::{MAX_AGE, facing_of};
    use pumpkin_data::block_properties::{BlockProperties, CocoaLikeProperties, HorizontalFacing};
    use pumpkin_data::{
        Block, BlockDirection,
        tag::{self, Taggable},
    };

    /// `AGE_2` gives three ages and `FACING` four horizontals: twelve states
    /// (CocoaBlock.java:121-124).
    #[test]
    fn cocoa_has_three_ages_in_four_directions() {
        assert_eq!(MAX_AGE, 2);
        assert_eq!(Block::COCOA.states.len(), 12);
    }

    /// Vanilla keys survival off `BlockTags.SUPPORTS_COCOA` (CocoaBlock.java:64), which is the
    /// jungle log family, not any solid block.
    #[test]
    fn supports_cocoa_tag_is_jungle_logs() {
        assert!(Block::JUNGLE_LOG.has_tag(&tag::Block::MINECRAFT_SUPPORTS_COCOA));
        assert!(Block::JUNGLE_WOOD.has_tag(&tag::Block::MINECRAFT_SUPPORTS_COCOA));
        assert!(!Block::STONE.has_tag(&tag::Block::MINECRAFT_SUPPORTS_COCOA));
        assert!(!Block::OAK_LOG.has_tag(&tag::Block::MINECRAFT_SUPPORTS_COCOA));
    }

    /// Cocoa's `FACING` points AT the log, so the support is one step along the facing, not
    /// against it - the opposite convention from wall banners and amethyst clusters.
    #[test]
    fn facing_points_at_the_support() {
        for (facing, expected) in [
            (HorizontalFacing::North, BlockDirection::North),
            (HorizontalFacing::South, BlockDirection::South),
            (HorizontalFacing::East, BlockDirection::East),
            (HorizontalFacing::West, BlockDirection::West),
        ] {
            let mut props = CocoaLikeProperties::default(&Block::COCOA);
            props.facing = facing;
            assert_eq!(facing_of(props.to_state_id(&Block::COCOA)), expected);
        }
    }
}
