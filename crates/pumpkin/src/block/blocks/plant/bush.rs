use pumpkin_data::{BlockDirection, BlockId, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, BonemealArgs, CanPlaceAtArgs,
    GetStateForNeighborUpdateArgs, blocks::plant::PlantBlockBase,
};

pub struct BushBlock;

impl BlockMetadata for BushBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::BUSH, BlockId::FIREFLY_BUSH].into()
    }
}

impl BlockBehaviour for BushBlock {
    /// `FireflyBushBlock.isValidBonemealTarget` (`FireflyBushBlock.java:48-50`) delegates to
    /// `BonemealableBlock.hasSpreadableNeighbourPos` (`BonemealableBlock.java:13-15`): a
    /// horizontal neighbour must be empty and able to support this bush. The same behavior is
    /// inherited by the ordinary `BushBlock` (`BushBlock.java:33-36`).
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        find_spreadable_neighbour(self, args.world, args.position, false).is_some()
    }

    /// `BushBlock.isBonemealSuccess` (`BushBlock.java:38-41`) always succeeds; the firefly
    /// subclass has the same implementation (`FireflyBushBlock.java:55-58`).
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    /// `BushBlock.performBonemeal` (`BushBlock.java:43-46`) places this block's default state at
    /// the shuffled horizontal spread position selected by
    /// `BonemealableBlock.findSpreadableNeighbourPos` (`BonemealableBlock.java:17-20`).
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let Some(position) = find_spreadable_neighbour(self, args.world, args.position, true)
            else {
                return;
            };
            args.world
                .set_block_state(
                    &position,
                    args.block.default_state.id,
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
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
}

impl PlantBlockBase for BushBlock {}

/// Finds an empty horizontal neighbour that can survive as this bush. Vanilla uses a fixed
/// direction order for the validity probe and a random permutation for the actual placement;
/// `random_start` gives the latter a uniform choice without mutating a shared direction table
/// (`BonemealableBlock.java:27-40`).
fn find_spreadable_neighbour(
    bush: &BushBlock,
    world: &crate::world::World,
    position: &BlockPos,
    random_start: bool,
) -> Option<BlockPos> {
    let mut directions = [
        BlockDirection::North,
        BlockDirection::East,
        BlockDirection::South,
        BlockDirection::West,
    ];
    if random_start {
        let offset = rand::rng().random_range(0..directions.len());
        directions.rotate_left(offset);
    }

    directions.into_iter().find_map(|direction| {
        let neighbour = position.offset(direction.to_offset());
        (world.is_in_height_limit(neighbour.0.y)
            && world.is_loaded(&neighbour)
            && world.get_block_state(&neighbour).is_air()
            && <BushBlock as PlantBlockBase>::can_place_at(bush, world, &neighbour))
        .then_some(neighbour)
    })
}
