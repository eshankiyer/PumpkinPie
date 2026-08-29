use pumpkin_data::item::Item;
use pumpkin_data::{
    BlockDirection, BlockStateId,
    block_properties::{BlockProperties, CandleLikeProperties},
    entity::EntityPose,
    fluid::Fluid,
    game_event::GameEvent,
    sound::{Sound, SoundCategory},
};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockAccessor;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;

use crate::block::{GetStateForNeighborUpdateArgs, OnScheduledTickArgs};
use crate::{
    block::{
        BlockIsReplacing,
        registry::BlockActionResult,
        {
            BlockBehaviour, CanPlaceAtArgs, CanUpdateAtArgs, NormalUseArgs, OnPlaceArgs,
            UseWithItemArgs,
        },
    },
    entity::EntityBase,
    world::game_event::{GameEventContext, emit_game_event},
};

#[pumpkin_block_from_tag("minecraft:candles")]
pub struct CandleBlock;

impl CandleBlock {
    /// Port of `CandleBlock.placeLiquid` (`CandleBlock.java:150-164`): waterlogs a dry
    /// candle and extinguishes it before scheduling the water tick.
    pub(crate) async fn place_liquid(
        world: &Arc<crate::world::World>,
        position: &BlockPos,
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
        fluid: &Fluid,
    ) -> bool {
        if !fluid.matches_type(&Fluid::WATER) {
            return false;
        }

        let mut properties = CandleLikeProperties::from_state_id(state_id, block);
        if properties.waterlogged {
            return false;
        }

        if properties.lit {
            world.play_block_sound(
                Sound::BlockCandleExtinguish,
                SoundCategory::Blocks,
                *position,
            );
            emit_game_event(
                world,
                GameEvent::BlockChange,
                position.to_centered_f64(),
                GameEventContext::none(),
            )
            .await;
        }

        properties.waterlogged = true;
        properties.lit = false;
        world
            .set_block_state(
                position,
                properties.to_state_id(block),
                BlockFlags::NOTIFY_ALL,
            )
            .await;
        world.schedule_fluid_tick(
            &Fluid::WATER,
            *position,
            Fluid::WATER.flow_speed as u8,
            TickPriority::Normal,
        );
        true
    }
}

impl BlockBehaviour for CandleBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        if args.player.get_entity().pose.load() != EntityPose::Crouching
            && let BlockIsReplacing::Itself(state_id) = args.replacing
        {
            let mut properties = CandleLikeProperties::from_state_id(state_id, args.block);
            if properties.candles < 4 {
                properties.candles += 1;
            }
            return properties.to_state_id(args.block);
        }

        let mut properties = CandleLikeProperties::default(args.block);
        properties.waterlogged = args.replacing.water_source();
        properties.to_state_id(args.block)
    }

    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        {
            let state = args.world.get_block_state(args.position);
            let mut properties = CandleLikeProperties::from_state_id(state.id, args.block);

            let item = args.item_stack.item;

            match item.id {
                id if (Item::CANDLE.id..=Item::BLACK_CANDLE.id).contains(&id)
                    && item.id == args.block.item_id =>
                {
                    let was_lit = properties.lit;

                    if properties.candles < 4 {
                        properties.candles += 1;
                    }

                    properties.lit = was_lit;

                    args.world.set_block_state(
                        args.position,
                        properties.to_state_id(args.block),
                        BlockFlags::NOTIFY_ALL,
                    );

                    BlockActionResult::Consume
                }
                _ => {
                    // Vanilla `CandleBlock.useItemOn` delegates non-empty-hand uses to
                    // `BlockBehaviour.useItemOn`, which requests the empty-hand fallback.
                    // The caller then invokes `normal_use` for the main hand.
                    BlockActionResult::PassToDefaultBlockAction
                }
            }
        }
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        {
            let state_id = args.world.get_block_state_id(args.position);
            let mut properties = CandleLikeProperties::from_state_id(state_id, args.block);

            // Vanilla `CandleBlock.useItemOn` (`CandleBlock.java:80-94`) only handles an empty
            // hand when the player may build and the candle is lit.
            if !args
                .player
                .abilities
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .allow_modify_world
                || !properties.lit
            {
                return BlockActionResult::Pass;
            }

            properties.lit = false;

            args.world.set_block_state(
                args.position,
                properties.to_state_id(args.block),
                BlockFlags::NOTIFY_ALL,
            );

            args.world.play_sound(
                Sound::BlockCandleExtinguish,
                SoundCategory::Blocks,
                &args.position.to_centered_f64(),
            );
            let world = args.world.clone();
            let position = args.position.to_centered_f64();
            let player = args.player.clone();
            tokio::spawn(async move {
                emit_game_event(
                    &world,
                    GameEvent::BlockChange,
                    position,
                    GameEventContext::of_entity(player),
                )
                .await;
            });

            BlockActionResult::Success
        }
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_place_at(args.block_accessor, args.position)
    }

    fn can_update_at(&self, args: CanUpdateAtArgs<'_>) -> bool {
        let b = BlockAccessor::get_block(args.world, args.position);
        args.player.get_entity().pose.load() != EntityPose::Crouching
            && CandleLikeProperties::from_state_id(args.state_id, args.block).candles != 4
            && args.block.id == b.id
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        if !can_place_at(args.world.as_ref(), args.position) {
            args.world
                .break_block(args.position, None, BlockFlags::empty());
        }
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if !can_place_at(args.world, args.position) {
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
        }
        args.state_id
    }
}

fn can_place_at(block_accessor: &dyn BlockAccessor, position: &BlockPos) -> bool {
    let (support_block, state) = block_accessor.get_block_and_state(&position.down());
    !support_block.is_waterlogged(state.id) && state.is_center_solid(BlockDirection::Up)
}
