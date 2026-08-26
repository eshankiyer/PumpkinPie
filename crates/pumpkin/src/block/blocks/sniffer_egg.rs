use pumpkin_data::block_properties::{BlockProperties, SnifferEggLikeProperties};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{
    Block, BlockStateId,
    tag::{self, Taggable},
};
use pumpkin_macros::pumpkin_block;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use rand::{RngExt, rng};

use crate::block::{
    BlockBehaviour, BlockFuture, BrokenArgs, OnPlaceArgs, OnScheduledTickArgs, PlacedArgs,
};
use crate::entity::Entity;
use crate::entity::EntityBase;
use crate::entity::ageable::AgeableMob;
use crate::entity::passive::sniffer::SnifferEntity;

#[pumpkin_block("minecraft:sniffer_egg")]
pub struct SnifferEggBlock;

/// `SnifferEggBlock.REGULAR_HATCH_TIME_TICKS` (`SnifferEggBlock.java:31`).
const REGULAR_HATCH_TIME_TICKS: i64 = 24000;
/// `SnifferEggBlock.BOOSTED_HATCH_TIME_TICKS` (`SnifferEggBlock.java:32`).
const BOOSTED_HATCH_TIME_TICKS: i64 = 12000;
/// `SnifferEggBlock.RANDOM_HATCH_OFFSET_TICKS` (`SnifferEggBlock.java:33`).
const RANDOM_HATCH_OFFSET_TICKS: i64 = 300;

impl SnifferEggBlock {
    /// `SnifferEggBlock.hatchBoost` (`SnifferEggBlock.java:100-102`): the block below is in
    /// `minecraft:sniffer_egg_no_hatch`'s counterpart tag `sniffer_egg_hatch_boost`.
    fn hatch_boost(
        world: &dyn pumpkin_world::world::BlockAccessor,
        pos: &pumpkin_util::math::position::BlockPos,
    ) -> bool {
        let below_pos = pos.down();
        let state = world.get_block_state(&below_pos);
        Block::from_state_id(state.id).has_tag(&tag::Block::MINECRAFT_SNIFFER_EGG_HATCH_BOOST)
    }

    /// Per-stage delay from `SnifferEggBlock.onPlace` (`SnifferEggBlock.java:88-92`):
    /// `hatchTime / 3 + level.getRandom().nextInt(RANDOM_HATCH_OFFSET_TICKS)`, where
    /// `hatchTime` is 24000 normally and 12000 on a hatching-boost block.
    fn stage_hatch_delay(boosted: bool) -> i64 {
        let hatch_time = if boosted {
            BOOSTED_HATCH_TIME_TICKS
        } else {
            REGULAR_HATCH_TIME_TICKS
        };
        hatch_time / 3 + rng().random_range(0..RANDOM_HATCH_OFFSET_TICKS)
    }
}

impl BlockBehaviour for SnifferEggBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let props = SnifferEggLikeProperties::default(args.block);
            props.to_state_id(args.block)
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world.play_sound(
                Sound::BlockSnifferEggPlop,
                SoundCategory::Blocks,
                &args.position.to_f64(),
            );

            // `SnifferEggBlock.onPlace` (`SnifferEggBlock.java:83-94`): the boost smoke
            // `levelEvent(3009)` fires server-side only when the egg sits on a boost block.
            let boosted = Self::hatch_boost(args.world.as_ref(), args.position);
            if boosted {
                args.world
                    .sync_world_event(WorldEvent::ParticlesEggCrack, *args.position, 0);
            }

            let delay = Self::stage_hatch_delay(boosted);
            args.world.schedule_block_tick_long(
                args.block,
                *args.position,
                delay,
                TickPriority::Normal,
            );
        })
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let mut props = SnifferEggLikeProperties::from_state_id(state_id, args.block);

            if props.hatch < 2 {
                props.hatch += 1;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;

                args.world.play_sound(
                    Sound::BlockSnifferEggCrack,
                    SoundCategory::Blocks,
                    &args.position.to_f64(),
                );

                // Vanilla re-runs `onPlace` on every `setBlock` (`LevelChunk.setBlockState:326-328`),
                // which is what schedules the next stage; pumpkin has no such chain, so it is
                // rescheduled explicitly.
                let boosted = Self::hatch_boost(args.world.as_ref(), args.position);
                let delay = Self::stage_hatch_delay(boosted);
                args.world.schedule_block_tick_long(
                    args.block,
                    *args.position,
                    delay,
                    TickPriority::Normal,
                );
            } else {
                // `SnifferEggBlock.tick` (`SnifferEggBlock.java:70-81`): destroy without drops
                // (`destroyBlock(position, false)`), then hatch a baby sniffer at the block's
                // center with a random yaw (`setBaby` :75, `snapTo` :76).
                args.world
                    .break_block(args.position, None, BlockFlags::SKIP_DROPS)
                    .await;

                args.world.play_sound(
                    Sound::BlockSnifferEggHatch,
                    SoundCategory::Blocks,
                    &args.position.to_f64(),
                );

                let entity = Entity::new(
                    args.world.clone(),
                    args.position.to_centered_f64(),
                    &EntityType::SNIFFER,
                );
                let sniffer = SnifferEntity::new(entity);
                sniffer.set_baby(true);
                let yaw = rng().random_range(0.0..360.0f32);
                sniffer.get_entity().set_rotation(yaw, 0.0);
                args.world.spawn_entity(sniffer).await;
            }
        })
    }

    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world.play_sound(
                Sound::BlockSnifferEggCrack,
                SoundCategory::Blocks,
                &args.position.to_f64(),
            );
            args.world
                .drop_stack(args.position, ItemStack::new(1, &Item::SNIFFER_EGG))
                .await;
        })
    }
}
