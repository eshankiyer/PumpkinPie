use pumpkin_data::{Block, BlockId};
use pumpkin_world::world::BlockFlags;

use crate::block::{BlockBehaviour, BlockFuture, BlockMetadata, RandomTickArgs};

/// Above this light dampening, nylium is considered covered and reverts to netherrack.
const MAX_LIGHT_LEVEL: u8 = 15;

/// `NyliumBlock`: nylium dies back to netherrack once the block above it blocks all light.
///
/// Bone-mealing nylium into nether vegetation is NOT implemented. `NyliumBlock.performBonemeal`
/// places a configured feature, and nothing here bridges a live world to feature placement -
/// the same gap that leaves moss bone meal and sapling growth unported.
pub struct NyliumBlock;

impl BlockMetadata for NyliumBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::CRIMSON_NYLIUM, BlockId::WARPED_NYLIUM].into()
    }
}

impl BlockBehaviour for NyliumBlock {
    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if can_be_nylium(args.world, args.position) {
                return;
            }
            args.world
                .set_block_state(
                    args.position,
                    Block::NETHERRACK.default_state.id,
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
        })
    }
}

/// `NyliumBlock.canBeNylium`: the light dampening into the face above must stay below 15.
///
/// Vanilla runs this through `LightEngine.getLightDampeningInto`, which reports a full block
/// when the two touching faces occlude each other. Pumpkin has no face-occlusion lookup, so
/// only the raw opacity is available - the same divergence `grass_block.rs` documents, and it
/// errs the same way: something that occludes downwards while carrying a low opacity, such as a
/// bottom slab, lets the nylium survive where vanilla would kill it. Nylium under a full opaque
/// block still reverts, which is the common case.
fn can_be_nylium(
    world: &crate::world::World,
    position: &pumpkin_util::math::position::BlockPos,
) -> bool {
    world.get_block_state(&position.up()).opacity < MAX_LIGHT_LEVEL
}
