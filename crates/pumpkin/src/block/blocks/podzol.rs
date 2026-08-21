use pumpkin_data::BlockStateId;
use pumpkin_data::{
    Block,
    block_properties::{BlockProperties, GrassBlockLikeProperties},
    tag::{self, Taggable},
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;

use crate::block::{BlockBehaviour, BlockFuture, GetStateForNeighborUpdateArgs, OnPlaceArgs};
use crate::world::World;

/// `SnowyBlock` (`net/minecraft/world/level/block/SnowyBlock.java:17`), which podzol is registered
/// with in `Blocks.java`.
///
/// Unlike `GrassBlock`/`MyceliumBlock` (`SpreadingSnowyBlock`), podzol neither spreads nor dies
/// back: `SnowyBlock` overrides only `getStateForPlacement` and `updateShape`, both of which just
/// keep the `snowy` property in sync with the block covering it.
#[pumpkin_block("minecraft:podzol")]
pub struct PodzolBlock;

impl BlockBehaviour for PodzolBlock {
    /// `SnowyBlock#getStateForPlacement` (SnowyBlock.java:47-51).
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = GrassBlockLikeProperties::default(args.block);
            props.snowy = is_snowy_setting(args.world, args.position);
            props.to_state_id(args.block)
        })
    }

    /// `SnowyBlock#updateShape` (SnowyBlock.java:31-45): only the neighbour above matters, and the
    /// value it produces is a function of that block alone, so recomputing it unconditionally from
    /// the block above gives the same state for every direction.
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = GrassBlockLikeProperties::from_state_id(args.state_id, &Block::PODZOL);
            let should_be_snowy = is_snowy_setting(args.world, args.position);
            if props.snowy == should_be_snowy {
                return args.state_id;
            }
            props.snowy = should_be_snowy;
            props.to_state_id(&Block::PODZOL)
        })
    }
}

/// `SnowyBlock#isSnowySetting` (SnowyBlock.java:53-55) applied to the block covering `position`.
fn is_snowy_setting(world: &World, position: &BlockPos) -> bool {
    world
        .get_block(&position.up())
        .has_tag(&tag::Block::MINECRAFT_SNOW)
}

#[cfg(test)]
mod test {
    use pumpkin_data::{
        Block,
        block_properties::{BlockProperties, GrassBlockLikeProperties},
        tag::{self, Taggable},
    };

    /// `isSnowySetting` keys off `BlockTags.SNOW`, which holds both the layer block and the full
    /// snow block; dirt-like ground above podzol must not set it.
    #[test]
    fn snow_tag_membership_matches_vanilla() {
        assert!(Block::SNOW.has_tag(&tag::Block::MINECRAFT_SNOW));
        assert!(Block::SNOW_BLOCK.has_tag(&tag::Block::MINECRAFT_SNOW));
        assert!(!Block::DIRT.has_tag(&tag::Block::MINECRAFT_SNOW));
        assert!(!Block::AIR.has_tag(&tag::Block::MINECRAFT_SNOW));
    }

    /// Podzol carries the same single `snowy` property `SnowyBlock` declares
    /// (SnowyBlock.java:57-60), and its default state is the un-snowy one (SnowyBlock.java:28).
    #[test]
    fn podzol_has_a_snowy_property_defaulting_to_false() {
        let props =
            GrassBlockLikeProperties::from_state_id(Block::PODZOL.default_state.id, &Block::PODZOL);
        assert!(!props.snowy);

        let mut snowy = GrassBlockLikeProperties::default(&Block::PODZOL);
        snowy.snowy = true;
        assert_ne!(
            snowy.to_state_id(&Block::PODZOL),
            Block::PODZOL.default_state.id
        );
    }
}
