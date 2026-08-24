use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockFuture, GetCloneItemStackArgs, RandomTickArgs, UseWithItemArgs,
};
use crate::world::game_event::{GameEventContext, emit_game_event};
use pumpkin_data::dimension::Dimension;
use pumpkin_data::flower_pot_transformations::get_potted_item;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{Block, BlockId};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::GameMode;
use pumpkin_world::world::BlockFlags;

#[pumpkin_block_from_tag("minecraft:flower_pots")]
pub struct FlowerPotBlock;

/// Vanilla `FlowerPotBlock#getPotted` (FlowerPotBlock.java:128-130): the content block a
/// filled pot holds (`None` for the empty `minecraft:flower_pot`). Pumpkin registers every
/// potted variant as its own block named after vanilla's registry name, so the content is
/// derived from that name. The two azalea pots are special-cased because vanilla registers
/// them as `potted_azalea_bush` / `potted_flowering_azalea_bush` while their contents are
/// plain `AZALEA` / `FLOWERING_AZALEA` (Blocks.java:5561-5563).
fn get_potted(block: &Block) -> Option<&'static Block> {
    let content_name = match block.name {
        "flower_pot" => return None,
        "potted_azalea_bush" => "azalea",
        "potted_flowering_azalea_bush" => "flowering_azalea",
        rest => rest.strip_prefix("potted_")?,
    };
    Block::from_name(content_name)
}

impl BlockBehaviour for FlowerPotBlock {
    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let item = args.item_stack.item;
            //Place the flower inside the pot
            let potted_block_id = get_potted_item(item.id);
            if args.block.eq(&Block::FLOWER_POT) {
                if potted_block_id == BlockId::AIR {
                    // Vanilla returns TRY_WITH_EMPTY_HAND here, which falls through to
                    // `useWithoutItem`; an empty pot has nothing to pick up, so that path
                    // consumes the interaction without acting
                    // (FlowerPotBlock.java:70-71,89-91).
                    return BlockActionResult::Consume;
                }
                args.world
                    .set_block_state(
                        args.position,
                        Block::from_id(potted_block_id).default_state.id,
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
                // Vanilla `level.gameEvent(player, GameEvent.BLOCK_CHANGE, pos)`
                // (FlowerPotBlock.java:78).
                emit_game_event(
                    args.world,
                    GameEvent::BlockChange,
                    args.position.to_centered_f64(),
                    GameEventContext::of_entity(args.player.clone()),
                )
                .await;
                // Vanilla `player.awardStat(Stats.POT_FLOWER)` (FlowerPotBlock.java:80,
                // Stats.java:73 registers `pot_flower` as a custom stat).
                args.player
                    .increment_stat(
                        pumpkin_data::statistic::StatisticCategory::Custom,
                        pumpkin_data::statistic::CustomStatistic::PotFlower as i32,
                        1,
                    )
                    .await;
                // Vanilla `itemStack.consume(1, player)` (FlowerPotBlock.java:81).
                if args.player.gamemode.load() != GameMode::Creative {
                    args.item_stack.decrement(1);
                }
                return BlockActionResult::Success;
            } else if potted_block_id != BlockId::AIR {
                //if the player have an item that can be potted in his hand, nothing happens
                return BlockActionResult::Consume;
            }

            //get the flower + empty the pot. Vanilla `useWithoutItem` also hands the plant
            // back to the player, dropping it if the inventory cannot hold it
            // (FlowerPotBlock.java:86-101, using `getPotted`, FlowerPotBlock.java:93,128).
            let Some(potted) = get_potted(args.block) else {
                return BlockActionResult::Consume;
            };
            let Some(potted_item) = Item::from_id(potted.item_id) else {
                return BlockActionResult::Consume;
            };
            let mut plant = ItemStack::new(1, potted_item);
            if !args
                .player
                .inventory
                .insert_stack_anywhere(&mut plant)
                .await
            {
                args.player.drop_item(plant).await;
            }

            args.world
                .set_block_state(
                    args.position,
                    Block::FLOWER_POT.default_state.id,
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
            // Vanilla `level.gameEvent(player, GameEvent.BLOCK_CHANGE, pos)`
            // (FlowerPotBlock.java:99).
            emit_game_event(
                args.world,
                GameEvent::BlockChange,
                args.position.to_centered_f64(),
                GameEventContext::of_entity(args.player.clone()),
            )
            .await;
            BlockActionResult::Success
        })
    }

    /// Vanilla `FlowerPotBlock#getCloneItemStack` (FlowerPotBlock.java:104-106):
    /// middle-clicking a filled pot yields its contents instead of the pot itself.
    fn get_clone_item_stack(&self, args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        let potted = get_potted(args.block)?;
        Some(ItemStack::new(1, Item::from_id(potted.item_id)?))
    }

    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if (args.world.dimension.eq(&Dimension::OVERWORLD)
                || args.world.dimension.eq(&Dimension::OVERWORLD_CAVES))
                && args.block.eq(&Block::POTTED_CLOSED_EYEBLOSSOM)
                && args.world.level_time.lock().await.time_of_day % 24000 > 14500
            {
                args.world
                    .set_block_state(
                        args.position,
                        Block::POTTED_OPEN_EYEBLOSSOM.default_state.id,
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
            }
            if args.block.eq(&Block::POTTED_OPEN_EYEBLOSSOM)
                && args.world.level_time.lock().await.time_of_day % 24000 <= 14500
            {
                args.world
                    .set_block_state(
                        args.position,
                        Block::POTTED_CLOSED_EYEBLOSSOM.default_state.id,
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
            }
        })
    }
}
