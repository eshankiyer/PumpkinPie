use std::sync::Arc;

use pumpkin_data::{
    Block, BlockStateId,
    block_properties::{BlockProperties, RedstoneOreLikeProperties},
    game_event::GameEvent,
    item::Item,
    item_stack::ItemStack,
    sound::{Sound, SoundCategory},
};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::{GameMode, math::position::BlockPos, math::vector3::Vector3};
use pumpkin_world::{
    tick::TickPriority,
    world::{BlockAccessor, BlockFlags},
};

use crate::{
    block::{
        BlockBehaviour, BlockFuture, GetCloneItemStackArgs, GetComparatorOutputArgs,
        GetStateForNeighborUpdateArgs, NormalUseArgs, OnScheduledTickArgs, UseWithItemArgs,
        blocks::cake::CakeBlock, registry::BlockActionResult,
    },
    entity::player::Player,
    world::World,
};

const CANDLE_MAP: [(&Item, &Block); 17] = [
    (&Item::CANDLE, &Block::CANDLE_CAKE),
    (&Item::WHITE_CANDLE, &Block::WHITE_CANDLE_CAKE),
    (&Item::ORANGE_CANDLE, &Block::ORANGE_CANDLE_CAKE),
    (&Item::MAGENTA_CANDLE, &Block::MAGENTA_CANDLE_CAKE),
    (&Item::LIGHT_BLUE_CANDLE, &Block::LIGHT_BLUE_CANDLE_CAKE),
    (&Item::YELLOW_CANDLE, &Block::YELLOW_CANDLE_CAKE),
    (&Item::LIME_CANDLE, &Block::LIME_CANDLE_CAKE),
    (&Item::PINK_CANDLE, &Block::PINK_CANDLE_CAKE),
    (&Item::GRAY_CANDLE, &Block::GRAY_CANDLE_CAKE),
    (&Item::LIGHT_GRAY_CANDLE, &Block::LIGHT_GRAY_CANDLE_CAKE),
    (&Item::CYAN_CANDLE, &Block::CYAN_CANDLE_CAKE),
    (&Item::PURPLE_CANDLE, &Block::PURPLE_CANDLE_CAKE),
    (&Item::BLUE_CANDLE, &Block::BLUE_CANDLE_CAKE),
    (&Item::BROWN_CANDLE, &Block::BROWN_CANDLE_CAKE),
    (&Item::GREEN_CANDLE, &Block::GREEN_CANDLE_CAKE),
    (&Item::RED_CANDLE, &Block::RED_CANDLE_CAKE),
    (&Item::BLACK_CANDLE, &Block::BLACK_CANDLE_CAKE),
];

#[must_use]
pub fn cake_from_candle(item: &Item) -> &'static Block {
    CANDLE_MAP
        .binary_search_by_key(&item.id, |(key, _)| key.id)
        .map_or(&Block::CAKE, |index| CANDLE_MAP[index].1)
}

#[must_use]
pub fn candle_from_cake(block: &Block) -> &'static Item {
    CANDLE_MAP
        .binary_search_by_key(&block.id, |(_, value)| value.id)
        .map_or(&Item::CANDLE, |index| CANDLE_MAP[index].0)
}

#[pumpkin_block_from_tag("minecraft:candle_cakes")]
pub struct CandleCakeBlock;

impl CandleCakeBlock {
    async fn consume_and_drop_candle(
        block: &Block,
        player: &Arc<Player>,
        location: &BlockPos,
        world: &Arc<World>,
    ) -> BlockActionResult {
        match player.gamemode.load() {
            GameMode::Survival | GameMode::Adventure => {
                if player.hunger_manager.level.load() >= 20 {
                    return BlockActionResult::Pass;
                }
            }
            GameMode::Creative => {}
            GameMode::Spectator => return BlockActionResult::Pass,
        }

        let candle_item = candle_from_cake(block);

        let item_stack = ItemStack::new(1, candle_item);

        world.drop_stack(location, item_stack).await;

        world
            .set_block_state(
                location,
                Block::CAKE.default_state.id,
                BlockFlags::NOTIFY_ALL,
            )
            .await;

        let (block, state) = world.get_block_and_state_id(location);

        CakeBlock::consume_if_hungry(world, player, block, location, state).await
    }
}

impl BlockBehaviour for CandleCakeBlock {
    /// Vanilla `CandleCakeBlock.getCloneItemStack` (`CandleCakeBlock.java:110-113`):
    /// creative pick-block returns a plain cake rather than the candle-cake block item.
    fn get_clone_item_stack(&self, _args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        Some(ItemStack::new(1, &Item::CAKE))
    }

    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let item_id = args.item_stack.item.id;
            match item_id {
                id if id == Item::FIRE_CHARGE.id || id == Item::FLINT_AND_STEEL.id => {
                    BlockActionResult::Pass
                } // Item::FIRE_CHARGE | Item::FLINT_AND_STEEL
                _ if args.item_stack.is_empty()
                    && candle_hit(args.hit.cursor_pos)
                    && RedstoneOreLikeProperties::from_state_id(
                        args.world.get_block_state_id(args.position),
                        args.block,
                    )
                    .lit =>
                {
                    // `CandleCakeBlock.useItemOn` (`CandleCakeBlock.java:79-86`) extinguishes a
                    // lit cake only when an empty-hand hit is above the cake midpoint.
                    let mut properties = RedstoneOreLikeProperties::from_state_id(
                        args.world.get_block_state_id(args.position),
                        args.block,
                    );
                    properties.lit = false;
                    args.world
                        .set_block_state(
                            args.position,
                            properties.to_state_id(args.block),
                            BlockFlags::NOTIFY_ALL,
                        )
                        .await;
                    args.world.play_sound(
                        Sound::BlockCandleExtinguish,
                        SoundCategory::Blocks,
                        &args.position.to_centered_f64(),
                    );
                    crate::world::game_event::emit_game_event(
                        args.world,
                        GameEvent::BlockChange,
                        args.position.to_centered_f64(),
                        crate::world::game_event::GameEventContext::of_entity(args.player.clone()),
                    )
                    .await;
                    BlockActionResult::Success
                }
                _ => BlockActionResult::PassToDefaultBlockAction,
            }
        })
    }

    fn get_comparator_output<'a>(
        &'a self,
        _args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            // `CandleCakeBlock.getAnalogOutputSignal` and `hasAnalogOutputSignal`
            // (`CandleCakeBlock.java:137-144`) expose the full-cake signal, which is
            // `CakeBlock.FULL_CAKE_SIGNAL` (`CakeBlock.java:36`, `CakeBlock.java:144-145`).
            Some(14)
        })
    }

    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            Self::consume_and_drop_candle(args.block, args.player, args.position, args.world).await
        })
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !can_place_at(args.world.as_ref(), args.position) {
                args.world
                    .break_block(args.position, None, BlockFlags::empty())
                    .await;
            }
        })
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if !can_place_at(args.world, args.position) {
                args.world
                    .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
            }
            args.state_id
        })
    }
}

fn can_place_at(world: &dyn BlockAccessor, position: &BlockPos) -> bool {
    let state = world.get_block_state(&position.down());
    state.is_solid()
}

// `CandleCakeBlock.candleHit` (`CandleCakeBlock.java:101-103`) uses the block midpoint.
fn candle_hit(cursor_pos: &Vector3<f32>) -> bool {
    cursor_pos.y > 0.5
}

#[cfg(test)]
mod tests {
    use super::candle_hit;
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn candle_hit_requires_the_upper_half() {
        assert!(!candle_hit(&Vector3::new(0.5, 0.5, 0.5)));
        assert!(candle_hit(&Vector3::new(0.5, 0.5001, 0.5)));
    }
}
