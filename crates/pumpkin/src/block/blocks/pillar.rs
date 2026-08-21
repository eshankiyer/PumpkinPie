use pumpkin_data::BlockId;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::BlockProperties;

use crate::block::OnPlaceArgs;
use crate::block::{BlockBehaviour, BlockFuture, BlockMetadata};

type PillarProperties = pumpkin_data::block_properties::PaleOakWoodLikeProperties;

/// `RotatedPillarBlock`: the axis follows the face the block was placed against.
///
/// `logs.rs` already does this for the `minecraft:logs` tag. These twelve are
/// `RotatedPillarBlock` in vanilla too but belong to no shared tag, so without this they
/// always placed on the Y axis however they were clicked.
pub struct PillarBlock;

impl BlockMetadata for PillarBlock {
    fn ids() -> Box<[BlockId]> {
        [
            BlockId::BAMBOO_BLOCK,
            BlockId::STRIPPED_BAMBOO_BLOCK,
            BlockId::BASALT,
            BlockId::POLISHED_BASALT,
            BlockId::BONE_BLOCK,
            BlockId::DEEPSLATE,
            BlockId::MUDDY_MANGROVE_ROOTS,
            BlockId::OCHRE_FROGLIGHT,
            BlockId::PEARLESCENT_FROGLIGHT,
            BlockId::VERDANT_FROGLIGHT,
            BlockId::PURPUR_PILLAR,
            BlockId::QUARTZ_PILLAR,
        ]
        .into()
    }
}

impl BlockBehaviour for PillarBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = PillarProperties::default(args.block);
            props.axis = args.direction.to_axis();
            props.to_state_id(args.block)
        })
    }
}
