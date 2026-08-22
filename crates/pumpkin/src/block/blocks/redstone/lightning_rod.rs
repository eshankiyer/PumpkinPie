use std::sync::Arc;

use crate::block::blocks::copper_weathering;
use crate::block::{
    BlockBehaviour, BlockFuture, EmitsRedstonePowerArgs, GetRedstonePowerArgs, OnPlaceArgs,
    OnScheduledTickArgs, PlacedArgs, RandomTickArgs,
};
use crate::world::World;
use pumpkin_data::block_properties::{BlockProperties, LightningRodLikeProperties};
use pumpkin_data::{BlockStateId, FacingExt};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

/// `LightningRodBlock` (`net/minecraft/world/level/block/LightningRodBlock.java:26`).
///
/// `Blocks.java:5303-5315` registers this as a `WeatheringCopperCollection`, so all eight ids
/// exist: the four weathering stages are `WeatheringLightningRodBlock` and the four waxed ones are
/// plain `LightningRodBlock`. Both extend `LightningRodBlock`, which is what
/// `LightningBolt.java:70` instance-checks and what `PoiTypes.java:54-58` collects into the
/// `lightning_rod` POI, so every variant conducts lightning and emits redstone. Registering only
/// `minecraft:lightning_rod` left the other seven with no behaviour at all.
///
/// `minecraft:lightning_rods` is the same set vanilla itself uses in `ServerLevel.java:552`.
#[pumpkin_block_from_tag("minecraft:lightning_rods")]
pub struct LightningRodBlock;

impl LightningRodBlock {
    pub async fn trigger(world: &Arc<World>, pos: &BlockPos) {
        let (block, state_id) = world.get_block_and_state_id(pos);
        let mut props = LightningRodLikeProperties::from_state_id(state_id, block);
        if !props.powered {
            props.powered = true;
            world
                .set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
                .await;

            Self::update_neighbors(world, pos, &props).await;

            // In vanilla, it stays powered for 8 ticks (4 redstone ticks) before scheduled tick turns it off.
            world.schedule_block_tick(block, *pos, 8, TickPriority::Normal);
        }
    }

    async fn update_neighbors(
        world: &Arc<World>,
        pos: &BlockPos,
        props: &LightningRodLikeProperties,
    ) {
        world.update_neighbors(pos, None).await;
        // The block it is attached to is in the opposite of the facing direction
        let attached_pos = pos.offset(props.facing.opposite().to_block_direction().to_offset());
        world.update_neighbors(&attached_pos, None).await;
    }
}

impl BlockBehaviour for LightningRodBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            // `LightningRodBlock#getStateForPlacement` (LightningRodBlock.java:45-49).
            let mut props = LightningRodLikeProperties::default(args.block);
            props.facing = args.direction.to_facing().opposite();
            props.waterlogged = args.replacing.water_source();
            props.to_state_id(args.block)
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // `LightningRodBlock.onPlace` (LightningRodBlock.java:97-103): a rod that ARRIVES
            // already POWERED with no tick queued has nothing left to switch it off, so it emits
            // 15 forever. Vanilla re-queues the 8-tick reset.
            if pumpkin_data::Block::from_state_id(args.old_state_id) == args.block {
                return;
            }
            let props = LightningRodLikeProperties::from_state_id(args.state_id, args.block);
            if props.powered
                && !args
                    .world
                    .is_block_tick_scheduled(args.position, args.block)
            {
                args.world
                    .schedule_block_tick(args.block, *args.position, 8, TickPriority::Normal);
            }
        })
    }

    fn emits_redstone_power<'a>(
        &'a self,
        _args: EmitsRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, bool> {
        Box::pin(async move { true })
    }

    fn get_weak_redstone_power<'a>(
        &'a self,
        args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(async move {
            let props = LightningRodLikeProperties::from_state_id(args.state.id, args.block);
            if props.powered { 15 } else { 0 }
        })
    }

    fn get_strong_redstone_power<'a>(
        &'a self,
        args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(async move {
            let props = LightningRodLikeProperties::from_state_id(args.state.id, args.block);
            // It emits strong power only in its facing direction (the direction pointing outward)
            if props.powered && props.facing.to_block_direction() == args.direction {
                15
            } else {
                0
            }
        })
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let mut props = LightningRodLikeProperties::from_state_id(state.id, args.block);
            if props.powered {
                props.powered = false;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
                Self::update_neighbors(args.world, args.position, &props).await;
            }
        })
    }

    /// Only the four non-waxed rods oxidize. A waxed rod also reaches this hook, but it is absent
    /// from `oxidation_stages` below, so `try_oxidize_copper` finds no level for it and returns
    /// without changing the block - the same outcome as vanilla, where a waxed rod is a plain
    /// `LightningRodBlock` and never implements `WeatheringCopper` (`Blocks.java:5306-5307`).
    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let oxidation_stages = [
                &pumpkin_data::Block::LIGHTNING_ROD,
                &pumpkin_data::Block::EXPOSED_LIGHTNING_ROD,
                &pumpkin_data::Block::WEATHERED_LIGHTNING_ROD,
                &pumpkin_data::Block::OXIDIZED_LIGHTNING_ROD,
            ];

            let current_state_id = args.world.get_block_state_id(args.position);
            let current_props =
                LightningRodLikeProperties::from_state_id(current_state_id, args.block);

            copper_weathering::try_oxidize_copper(
                args.world,
                args.position,
                args.block,
                &oxidation_stages,
                |next_block| {
                    let mut new_props = LightningRodLikeProperties::default(next_block);
                    new_props.facing = current_props.facing;
                    new_props.powered = current_props.powered;
                    new_props.to_state_id(next_block)
                },
            )
            .await;
        })
    }
}
