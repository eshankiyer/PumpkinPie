use pumpkin_data::{
    Block,
    tag::{self, Taggable},
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::block::{BlockBehaviour, BlockFuture, BonemealArgs};
use crate::world::World;

/// `NetherrackBlock` (`net/minecraft/world/level/block/NetherrackBlock.java:13`).
///
/// Netherrack itself is inert; the only behaviour it has is `BonemealableBlock`. Bone meal on a
/// netherrack block that touches nylium turns it into nylium of the neighbouring kind. Unlike
/// nylium's own bone meal (see `nylium.rs`), this places no configured feature - it is a plain
/// `setBlock` - so it is implementable without the `GenerationCache` a live world lacks.
#[pumpkin_block("minecraft:netherrack")]
pub struct NetherrackBlock;

/// Which nylium kinds sit in the 3x3x3 cube centred on `position`.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
struct NyliumNeighbours {
    crimson: bool,
    warped: bool,
}

impl NyliumNeighbours {
    const fn any(self) -> bool {
        self.crimson || self.warped
    }
}

/// `NetherrackBlock#performBonemeal` (NetherrackBlock.java:46-63) scans
/// `BlockPos.betweenClosed(pos.offset(-1, -1, -1), pos.offset(1, 1, 1))`, which is the full
/// 3x3x3 cube including the netherrack itself.
fn scan_nylium_neighbours(world: &World, position: &BlockPos) -> NyliumNeighbours {
    let mut found = NyliumNeighbours::default();
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                let block = world.get_block(
                    &position.offset(pumpkin_util::math::vector3::Vector3::new(x, y, z)),
                );
                if block == &Block::WARPED_NYLIUM {
                    found.warped = true;
                } else if block == &Block::CRIMSON_NYLIUM {
                    found.crimson = true;
                }
                if found.crimson && found.warped {
                    return found;
                }
            }
        }
    }
    found
}

impl BlockBehaviour for NetherrackBlock {
    /// `NetherrackBlock#isValidBonemealTarget` (NetherrackBlock.java:26-38): the block above must
    /// let skylight through, and some nylium must be in range.
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        if !propagates_skylight_down(args.world, &args.position.up()) {
            return false;
        }

        let mut in_range = false;
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    if args
                        .world
                        .get_block(
                            &args
                                .position
                                .offset(pumpkin_util::math::vector3::Vector3::new(x, y, z)),
                        )
                        .has_tag(&tag::Block::MINECRAFT_NYLIUM)
                    {
                        in_range = true;
                    }
                }
            }
        }
        in_range
    }

    /// `NetherrackBlock#isBonemealSuccess` (NetherrackBlock.java:40-43) is unconditionally true.
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let found = scan_nylium_neighbours(args.world, args.position);
            if !found.any() {
                return;
            }

            // NetherrackBlock.java:65-71: with both kinds adjacent vanilla picks one at random.
            let target = if found.crimson && found.warped {
                if rand::rng().random::<bool>() {
                    &Block::WARPED_NYLIUM
                } else {
                    &Block::CRIMSON_NYLIUM
                }
            } else if found.warped {
                &Block::WARPED_NYLIUM
            } else {
                &Block::CRIMSON_NYLIUM
            };

            args.world
                .set_block_state(
                    args.position,
                    target.default_state.id,
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
        })
    }
}

/// `BlockState#propagatesSkylightDown`, whose default is
/// `!Block.isShapeFullBlock(shape) && fluidState.isEmpty()`
/// (`BlockBehaviour.java:397-399`).
///
/// The generated data does not carry that flag, but it does carry the light dampening it feeds:
/// `getLightDampening` (BlockBehaviour.java:298-304) is 15 for a solid-render block, 0 when the
/// state propagates skylight and 1 otherwise, so `opacity == 0 && !solid_render` recovers the
/// flag for every block that does not override `getLightDampening`. The one 26.2 block that does
/// override it while not propagating is tinted glass (`TintedGlassBlock.java:24-27`), which
/// reports 15 and so is correctly rejected here too.
fn propagates_skylight_down(world: &World, position: &BlockPos) -> bool {
    let state = world.get_block_state(position);
    state.opacity == 0 && !state.solid_render
}

#[cfg(test)]
mod test {
    use pumpkin_data::{
        Block,
        tag::{self, Taggable},
    };

    /// Both nyliums are in `BlockTags.NYLIUM`, which is what `isValidBonemealTarget` tests
    /// (NetherrackBlock.java:32), and netherrack itself is not.
    #[test]
    fn nylium_tag_holds_both_kinds() {
        assert!(Block::CRIMSON_NYLIUM.has_tag(&tag::Block::MINECRAFT_NYLIUM));
        assert!(Block::WARPED_NYLIUM.has_tag(&tag::Block::MINECRAFT_NYLIUM));
        assert!(!Block::NETHERRACK.has_tag(&tag::Block::MINECRAFT_NYLIUM));
    }

    /// The `opacity == 0 && !solid_render` stand-in for `propagatesSkylightDown` must accept air
    /// and reject a full opaque block; `propagates_skylight_down` itself needs a world, so this
    /// checks the two data fields it reads.
    #[test]
    fn skylight_fields_separate_air_from_stone() {
        assert_eq!(Block::AIR.default_state.opacity, 0);
        assert!(!Block::AIR.default_state.solid_render);
        assert!(Block::STONE.default_state.solid_render);
        assert_eq!(Block::NETHERRACK.default_state.opacity, 15);
        // Tinted glass overrides getLightDampening to 15 while not propagating skylight.
        assert_eq!(Block::TINTED_GLASS.default_state.opacity, 15);
    }
}
