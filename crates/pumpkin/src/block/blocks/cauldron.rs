use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::world::World;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, GetComparatorOutputArgs, HandlePrecipitationArgs,
    OnEntityCollisionArgs, Precipitation, UseWithItemArgs,
};
use crate::world::game_event::{GameEventContext, emit_game_event};
use pumpkin_data::Block;
use pumpkin_data::BlockId;
use pumpkin_data::block_properties::{BlockProperties, WaterCauldronLikeProperties};
use pumpkin_data::damage::DamageType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::{RngExt, rng};

pub struct CauldronBlock;

impl BlockMetadata for CauldronBlock {
    fn ids() -> Box<[BlockId]> {
        [
            BlockId::CAULDRON,
            BlockId::WATER_CAULDRON,
            BlockId::LAVA_CAULDRON,
            BlockId::POWDER_SNOW_CAULDRON,
        ]
        .into()
    }
}

impl BlockBehaviour for CauldronBlock {
    fn on_entity_collision<'a>(&'a self, args: OnEntityCollisionArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = args.entity.get_entity();
            match args.block.id {
                BlockId::LAVA_CAULDRON => {
                    if !entity.entity_type.fire_immune
                        && !entity.fire_immune.load(Ordering::Relaxed)
                    {
                        args.entity.set_on_fire_for(15.0);
                        args.entity.damage(args.entity, 4.0, DamageType::LAVA).await;
                    }
                }
                BlockId::WATER_CAULDRON | BlockId::POWDER_SNOW_CAULDRON => {
                    if entity.fire_ticks.load(Ordering::Relaxed) > 0 {
                        lower_fill_level(args.world, args.position, args.block).await;
                    }
                    entity.extinguish();
                }
                _ => {}
            }
        })
    }

    #[allow(clippy::too_many_lines)]
    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let item_id = args.item_stack.item.id;
            let block_id = args.block.id;
            let gamemode = args.player.gamemode.load();

            // Filling empty cauldron with buckets
            if block_id == BlockId::CAULDRON {
                if item_id == Item::WATER_BUCKET.id {
                    let state_id = Block::WATER_CAULDRON
                        .from_properties(&[("level", "3")])
                        .to_state_id(&Block::WATER_CAULDRON);
                    args.world
                        .set_block_state(args.position, state_id, BlockFlags::NOTIFY_ALL)
                        .await;
                    args.world.play_sound(
                        Sound::ItemBucketEmpty,
                        SoundCategory::Blocks,
                        &args.position.to_f64(),
                    );
                    args.item_stack.decrement_unless_creative(gamemode, 1);
                    args.player
                        .inventory
                        .offer_or_drop_stack(ItemStack::new(1, &Item::BUCKET), args.player.as_ref())
                        .await;
                    return BlockActionResult::Success;
                } else if item_id == Item::LAVA_BUCKET.id {
                    args.world
                        .set_block_state(
                            args.position,
                            Block::LAVA_CAULDRON.default_state.id,
                            BlockFlags::NOTIFY_ALL,
                        )
                        .await;
                    args.world.play_sound(
                        Sound::ItemBucketEmptyLava,
                        SoundCategory::Blocks,
                        &args.position.to_f64(),
                    );
                    args.item_stack.decrement_unless_creative(gamemode, 1);
                    args.player
                        .inventory
                        .offer_or_drop_stack(ItemStack::new(1, &Item::BUCKET), args.player.as_ref())
                        .await;
                    return BlockActionResult::Success;
                } else if item_id == Item::POWDER_SNOW_BUCKET.id {
                    let state_id = Block::POWDER_SNOW_CAULDRON
                        .from_properties(&[("level", "3")])
                        .to_state_id(&Block::POWDER_SNOW_CAULDRON);
                    args.world
                        .set_block_state(args.position, state_id, BlockFlags::NOTIFY_ALL)
                        .await;
                    args.world.play_sound(
                        Sound::ItemBucketEmptyPowderSnow,
                        SoundCategory::Blocks,
                        &args.position.to_f64(),
                    );
                    args.item_stack.decrement_unless_creative(gamemode, 1);
                    args.player
                        .inventory
                        .offer_or_drop_stack(ItemStack::new(1, &Item::BUCKET), args.player.as_ref())
                        .await;
                    return BlockActionResult::Success;
                } else if item_id == Item::POTION.id {
                    let state_id = Block::WATER_CAULDRON
                        .from_properties(&[("level", "1")])
                        .to_state_id(&Block::WATER_CAULDRON);
                    args.world
                        .set_block_state(args.position, state_id, BlockFlags::NOTIFY_ALL)
                        .await;
                    args.world.play_sound(
                        Sound::ItemBottleEmpty,
                        SoundCategory::Blocks,
                        &args.position.to_f64(),
                    );
                    args.item_stack.decrement_unless_creative(gamemode, 1);
                    args.player
                        .inventory
                        .offer_or_drop_stack(
                            ItemStack::new(1, &Item::GLASS_BOTTLE),
                            args.player.as_ref(),
                        )
                        .await;
                    return BlockActionResult::Success;
                }
            }

            // Collecting fluid from full cauldrons into empty bucket
            if item_id == Item::BUCKET.id {
                let state_id = args.world.get_block_state_id(args.position);
                let (filled_item, sound) = if block_id == BlockId::WATER_CAULDRON {
                    let props = WaterCauldronLikeProperties::from_state_id(state_id, args.block);
                    if props.level == 3 {
                        (Some(&Item::WATER_BUCKET), Sound::ItemBucketFill)
                    } else {
                        (None, Sound::ItemBucketFill)
                    }
                } else if block_id == BlockId::LAVA_CAULDRON {
                    (Some(&Item::LAVA_BUCKET), Sound::ItemBucketFillLava)
                } else if block_id == BlockId::POWDER_SNOW_CAULDRON {
                    let props = WaterCauldronLikeProperties::from_state_id(state_id, args.block);
                    if props.level == 3 {
                        (
                            Some(&Item::POWDER_SNOW_BUCKET),
                            Sound::ItemBucketFillPowderSnow,
                        )
                    } else {
                        (None, Sound::ItemBucketFillPowderSnow)
                    }
                } else {
                    (None, Sound::ItemBucketFill)
                };

                if let Some(result_item) = filled_item {
                    args.world
                        .set_block_state(
                            args.position,
                            Block::CAULDRON.default_state.id,
                            BlockFlags::NOTIFY_ALL,
                        )
                        .await;
                    args.world
                        .play_sound(sound, SoundCategory::Blocks, &args.position.to_f64());
                    args.item_stack.decrement_unless_creative(gamemode, 1);
                    args.player
                        .inventory
                        .offer_or_drop_stack(ItemStack::new(1, result_item), args.player.as_ref())
                        .await;
                    return BlockActionResult::Success;
                }
            }

            // Adding water bottle to non-full water cauldron
            if block_id == BlockId::WATER_CAULDRON && item_id == Item::POTION.id {
                let state_id = args.world.get_block_state_id(args.position);
                let props = WaterCauldronLikeProperties::from_state_id(state_id, args.block);
                if props.level < 3 {
                    let next_level_str = match props.level {
                        1 => "2",
                        _ => "3",
                    };
                    let new_state_id = Block::WATER_CAULDRON
                        .from_properties(&[("level", next_level_str)])
                        .to_state_id(&Block::WATER_CAULDRON);
                    args.world
                        .set_block_state(args.position, new_state_id, BlockFlags::NOTIFY_ALL)
                        .await;
                    args.world.play_sound(
                        Sound::ItemBottleEmpty,
                        SoundCategory::Blocks,
                        &args.position.to_f64(),
                    );
                    args.item_stack.decrement_unless_creative(gamemode, 1);
                    args.player
                        .inventory
                        .offer_or_drop_stack(
                            ItemStack::new(1, &Item::GLASS_BOTTLE),
                            args.player.as_ref(),
                        )
                        .await;
                    return BlockActionResult::Success;
                }
            }

            BlockActionResult::PassToDefaultBlockAction
        })
    }

    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            match args.block.id {
                BlockId::WATER_CAULDRON | BlockId::POWDER_SNOW_CAULDRON => {
                    let state_id = args.world.get_block_state_id(args.position);
                    let props = WaterCauldronLikeProperties::from_state_id(state_id, args.block);
                    Some(props.level)
                }
                BlockId::LAVA_CAULDRON => Some(3),
                _ => Some(0),
            }
        })
    }

    fn handle_precipitation<'a>(
        &'a self,
        args: HandlePrecipitationArgs<'a>,
    ) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !should_handle_precipitation(args.precipitation) {
                return;
            }

            let new_state_id = match (args.block.id, args.precipitation) {
                (BlockId::CAULDRON, Precipitation::Rain) => Block::WATER_CAULDRON.default_state.id,
                (BlockId::CAULDRON, Precipitation::Snow) => {
                    Block::POWDER_SNOW_CAULDRON.default_state.id
                }
                (BlockId::WATER_CAULDRON, Precipitation::Rain)
                | (BlockId::POWDER_SNOW_CAULDRON, Precipitation::Snow) => {
                    let mut props =
                        WaterCauldronLikeProperties::from_state_id(args.state_id, args.block);
                    if props.level >= 3 {
                        return;
                    }
                    props.level += 1;
                    props.to_state_id(args.block)
                }
                _ => return,
            };

            args.world
                .set_block_state(args.position, new_state_id, BlockFlags::NOTIFY_ALL)
                .await;
            emit_game_event(
                args.world,
                GameEvent::BlockChange,
                args.position.to_centered_f64(),
                GameEventContext::none(),
            )
            .await;
        })
    }
}

fn should_handle_precipitation(precipitation: Precipitation) -> bool {
    match precipitation {
        Precipitation::Rain => rng().random::<f32>() < 0.05,
        Precipitation::Snow => rng().random::<f32>() < 0.1,
    }
}

async fn lower_fill_level(world: &Arc<World>, position: &BlockPos, block: &Block) {
    let state_id = world.get_block_state_id(position);
    let level = WaterCauldronLikeProperties::from_state_id(state_id, block).level;

    let new_state_id = if level <= 1 {
        Block::CAULDRON.default_state.id
    } else {
        let mut props = WaterCauldronLikeProperties::default(&Block::WATER_CAULDRON);
        props.level = level - 1;
        props.to_state_id(&Block::WATER_CAULDRON)
    };

    world
        .set_block_state(position, new_state_id, BlockFlags::NOTIFY_ALL)
        .await;
    emit_game_event(
        world,
        GameEvent::BlockChange,
        position.to_centered_f64(),
        GameEventContext::none(),
    )
    .await;
}
