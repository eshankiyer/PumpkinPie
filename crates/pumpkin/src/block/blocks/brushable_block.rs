use std::sync::Arc;

use pumpkin_data::block_properties::{BlockProperties, SuspiciousSandLikeProperties};
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockId, BlockStateId};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

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

impl BrushableBlock {
    pub fn brush(
        world: &Arc<crate::world::World>,
        pos: &pumpkin_util::math::position::BlockPos,
        block: &Block,
    ) {
        let state = world.get_block_state(pos);
        let mut props = SuspiciousSandLikeProperties::from_state_id(state.id, block);
        let is_gravel = block.id == BlockId::SUSPICIOUS_GRAVEL;

        if let Some(be) = world.get_block_entity(pos)
            && let Some(brush_be) = be.as_any().downcast_ref::<BrushableBlockBlockEntity>()
        {
            let mut hits = brush_be
                .hits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *hits += 1;

            if *hits >= 4 {
                let item = brush_be
                    .item
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(item_stack) = item {
                    world.drop_stack(pos, item_stack);
                }
                let target_block = if is_gravel { &Block::GRAVEL } else { &Block::SAND };
                world.set_block_state(pos, target_block.default_state.id, BlockFlags::NOTIFY_ALL);
                let sound = if is_gravel {
                    Sound::ItemBrushBrushingGravelComplete
                } else {
                    Sound::ItemBrushBrushingSandComplete
                };
                world.play_sound(sound, SoundCategory::Blocks, &pos.to_f64());
            } else {
                props.dusted = (*hits as u8).min(3);
                world.set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
                let sound = if is_gravel {
                    Sound::ItemBrushBrushingGravel
                } else {
                    Sound::ItemBrushBrushingSand
                };
                world.play_sound(sound, SoundCategory::Blocks, &pos.to_f64());
            }
        }
    }
}

impl BlockBehaviour for BrushableBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        SuspiciousSandLikeProperties::default(args.block).to_state_id(args.block)
    }

    /// `BrushableBlock.onPlace` (`BrushableBlock.java:62-65`) schedules the tick that
    /// drives `checkReset`.
    fn placed(&self, args: PlacedArgs<'_>) {
        args.world
            .add_block_entity(Arc::new(BrushableBlockBlockEntity::new(*args.position)));
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
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(brush_be.check_reset(args.world, game_time));
            });
        }
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        if let Some(be) = args.world.get_block_entity(args.position)
            && let Some(brush_be) = be.as_any().downcast_ref::<BrushableBlockBlockEntity>()
        {
            let contained = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(brush_be.take_item())
            });
            if let Some(contained) = contained {
                args.world.drop_stack(args.position, contained);
            }
        }
    }
}
