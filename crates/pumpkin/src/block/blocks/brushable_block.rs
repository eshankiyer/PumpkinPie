use std::sync::Arc;

use pumpkin_data::block_properties::{BlockProperties, SuspiciousSandLikeProperties};
use pumpkin_data::sound::Sound;
use pumpkin_data::{Block, BlockId, BlockStateId};
use pumpkin_world::tick::TickPriority;

use crate::block::entities::brushable_block::BrushableBlockBlockEntity;
use crate::block::{
    BlockBehaviour, BlockMetadata, BrokenArgs, OnPlaceArgs, OnScheduledTickArgs, PlacedArgs,
};

pub struct BrushableBlock;

/// `BrushableBlock.java:39` (`TICK_DELAY`).
const TICK_DELAY: u8 = 2;

impl BlockMetadata for BrushableBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::SUSPICIOUS_SAND, BlockId::SUSPICIOUS_GRAVEL].into()
    }
}

/// `BrushableBlock.getBrushSound` (`BrushableBlock.java:123-125`).
///
/// The value is bound by the two vanilla instances; `BrushItem.onUseTick` falls back to
/// `BRUSH_GENERIC` for anything that is not a `BrushableBlock` (`BrushItem.java:72-77`).
#[must_use]
pub fn brush_sound(block: &Block) -> Sound {
    if block.id == BlockId::SUSPICIOUS_GRAVEL {
        Sound::ItemBrushBrushingGravel
    } else if block.id == BlockId::SUSPICIOUS_SAND {
        Sound::ItemBrushBrushingSand
    } else {
        Sound::ItemBrushBrushingGeneric
    }
}

impl BlockBehaviour for BrushableBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let props = SuspiciousSandLikeProperties::default(args.block);
        props.to_state_id(args.block)
    }

    /// `BrushableBlock.onPlace` (`BrushableBlock.java:62-65`) schedules the tick that
    /// drives `checkReset`.
    fn placed(&self, args: PlacedArgs<'_>) {
        let entity = BrushableBlockBlockEntity::new(*args.position);
        args.world.add_block_entity(Arc::new(entity));
        args.world.schedule_block_tick(
            args.block,
            *args.position,
            TICK_DELAY,
            TickPriority::Normal,
        );
    }

    /// `BrushableBlock.tick` (`BrushableBlock.java:82-92`). The `Fallable` half of the
    /// vanilla tick (turning into a `FallingBlockEntity` over air) is not modelled here.
    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        if let Some(be) = args.world.get_block_entity(args.position)
            && let Some(brush_be) = be.as_any().downcast_ref::<BrushableBlockBlockEntity>()
        {
            let game_time = args.world.get_world_age();
            brush_be.check_reset(args.world, game_time);
        }
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        if let Some(be) = args.world.get_block_entity(args.position)
            && let Some(brush_be) = be.as_any().downcast_ref::<BrushableBlockBlockEntity>()
            && let Some(contained) = brush_be.take_item()
        {
            args.world.drop_stack(args.position, contained);
        }
    }
}
