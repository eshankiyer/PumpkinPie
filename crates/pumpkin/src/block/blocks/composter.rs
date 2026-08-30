use std::sync::Arc;

use crate::{
    block::{
        BlockBehaviour, BlockFuture, GetComparatorOutputArgs, NormalUseArgs, OnScheduledTickArgs,
        UseWithItemArgs, registry::BlockActionResult,
    },
    entity::{Entity, item::ItemEntity},
    world::World,
};
use pumpkin_data::{
    Block, BlockDirection, BlockStateId,
    block_properties::{BlockProperties, ComposterLikeProperties},
    composter_increase_chance::get_composter_increase_chance_from_item_id,
    entity::EntityType,
    item::Item,
    item_stack::ItemStack,
    sound::{Sound, SoundCategory},
    world::WorldEvent,
};
use pumpkin_inventory::screen_handler::InventoryPlayer;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::{tick::TickPriority, world::BlockFlags};
use rand::RngExt;

/// `ComposterBlock.READY` (`ComposterBlock.java:46`).
const COMPOSTER_READY: u8 = 8;

#[pumpkin_block("minecraft:composter")]
pub struct ComposterBlock;

impl BlockBehaviour for ComposterBlock {
    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let props = ComposterLikeProperties::from_state_id(state_id, args.block);
            if props.level == 8 {
                self.clear_composter(args.world, args.position, state_id, args.block)
                    .await;
                // Vanilla `ComposterBlock.useWithoutItem` returns SUCCESS for a full
                // composter (`ComposterBlock.java:273-285`).
                return BlockActionResult::Success;
            }

            BlockActionResult::Pass
        })
    }

    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let props = ComposterLikeProperties::from_state_id(state_id, args.block);
            let level = props.level;

            let item_stack = &mut *args.item_stack;
            let item_id = item_stack.item.id;

            // Check if the item is consumable by the composter
            let Some(chance) = get_composter_increase_chance_from_item_id(item_id) else {
                // Vanilla `ComposterBlock.useItemOn` delegates non-compostables to
                // `super.useItemOn` (`ComposterBlock.java:248-270`).
                return BlockActionResult::PassToDefaultBlockAction;
            };

            if level == 8 {
                // Vanilla delegates full-composter extraction to the empty-hand fallback;
                // `useWithoutItem` performs it for the main hand (`ComposterBlock.java:248-279`).
                return BlockActionResult::PassToDefaultBlockAction;
            }

            // Consume one item from the stack (if in survival mode). Vanilla only
            // consumes below the "full" level (7); at 7 the interaction is a no-op
            // until the composter is emptied.
            if level < 7 && !args.player.has_infinite_materials() {
                item_stack.decrement(1);
            }

            // Determine if the composter level should increase
            if level < 7 {
                let rose = level == 0 || rand::rng().random_bool(f64::from(chance));
                if rose {
                    self.update_level_composter(
                        args.world,
                        args.position,
                        state_id,
                        args.block,
                        level + 1,
                    )
                    .await;
                }
                // levelEvent(1500, pos, state != newState ? 1 : 0): vanilla fires this
                // for every accepted item, using data 0 for the "did not rise" variant.
                args.world.sync_world_event(
                    WorldEvent::ComposterFill,
                    *args.position,
                    i32::from(rose),
                );
            }

            // Vanilla `useItemOn` returns SUCCESS unconditionally once the item is
            // accepted as compostable, not just when the level actually rises
            // (`ComposterBlock.java:257-267`).
            BlockActionResult::Success
        })
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let props = ComposterLikeProperties::from_state_id(state_id, args.block);
            let level = props.level;
            if level == 7 {
                self.update_level_composter(
                    args.world,
                    args.position,
                    state_id,
                    args.block,
                    level + 1,
                )
                .await;
                args.world.play_sound(
                    Sound::BlockComposterReady,
                    SoundCategory::Blocks,
                    &args.position.to_centered_f64(),
                );
            }
        })
    }

    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            let props = ComposterLikeProperties::from_state_id(args.state.id, args.block);
            Some(props.level)
        })
    }
}

impl ComposterBlock {
    /// `ComposterBlock.insertItem` (`ComposterBlock.java:285-295`), for a villager's
    /// workstation inventory. Accepted compostable items are consumed even when the random
    /// fill roll does not raise the level; full composters reject the item without consuming it.
    pub async fn insert_item_from_villager(
        world: &Arc<World>,
        location: &BlockPos,
        item_stack: &mut ItemStack,
    ) -> bool {
        let (block, state_id) = world.get_block_and_state_id(location);
        if block != &Block::COMPOSTER {
            return false;
        }

        let level = ComposterLikeProperties::from_state_id(state_id, block).level;
        if level >= 7 {
            return false;
        }

        let Some(chance) = get_composter_increase_chance_from_item_id(item_stack.item.id) else {
            return false;
        };

        let rose = level == 0 || rand::rng().random_bool(f64::from(chance));
        if rose {
            Self.update_level_composter(world, location, state_id, block, level + 1)
                .await;
        }
        item_stack.decrement(1);
        true
    }

    pub async fn update_level_composter(
        &self,
        world: &Arc<World>,
        location: &BlockPos,
        state_id: BlockStateId,
        block: &Block,
        level: u8,
    ) {
        let mut props = ComposterLikeProperties::from_state_id(state_id, block);
        props.level = level;
        world
            .set_block_state(location, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
            .await;
        if level == 7 {
            world.schedule_block_tick(block, *location, 20, TickPriority::Normal);
        }
    }

    pub async fn clear_composter(
        &self,
        world: &Arc<World>,
        location: &BlockPos,
        state_id: BlockStateId,
        block: &Block,
    ) {
        self.update_level_composter(world, location, state_id, block, 0)
            .await;

        // Vec3.atLowerCornerWithOffset(pos, 0.5, 1.01, 0.5).offsetRandomXZ(random, 0.7F)
        // jitters X and Z only; Y is exactly pos.y + 1.01.
        let item_position = {
            let mut rng = rand::rng();
            location.to_centered_f64().add_raw(
                rng.random_range(-0.35..=0.35),
                0.51,
                rng.random_range(-0.35..=0.35),
            )
        };

        let item_entity = ItemEntity::new(
            Entity::new(world.clone(), item_position, &EntityType::ITEM),
            ItemStack::new(1, &Item::BONE_MEAL),
        );

        world.spawn_entity(Arc::new(item_entity)).await;

        world.play_sound(
            Sound::BlockComposterEmpty,
            SoundCategory::Blocks,
            &location.to_centered_f64(),
        );
    }
}

/// `ComposterBlock.InputContainer.setChanged` (`ComposterBlock.java:428-436`).
///
/// `ComposterBlock` implements `WorldlyContainerHolder` (`ComposterBlock.java:44`), so hoppers
/// reach a composter through the throwaway containers `getContainer` hands out
/// (`ComposterBlock.java:365-373`). A composter has no block entity, so the hopper's normal
/// inventory lookup finds nothing; this and `hopper_take_output` stand in for those containers.
///
/// Only the UP face accepts
/// items, only below level 7, and only compostables. The item is consumed whether or not the
/// level rises, matching `addItem` (`ComposterBlock.java:317-334`), which also fires level event
/// 1500 with data 0 for the "did not rise" variant.
///
/// Returns `true` when the item was consumed.
pub async fn hopper_insert_item(
    world: &Arc<World>,
    position: &BlockPos,
    face: BlockDirection,
    item_id: u16,
) -> bool {
    if face != BlockDirection::Up {
        return false;
    }
    let (block, state_id) = world.get_block_and_state_id(position);
    if block != &Block::COMPOSTER {
        return false;
    }
    let level = ComposterLikeProperties::from_state_id(state_id, block).level;
    if level >= 7 {
        return false;
    }
    let Some(chance) = get_composter_increase_chance_from_item_id(item_id) else {
        return false;
    };
    let rose = level == 0 || rand::rng().random_bool(f64::from(chance));
    if rose {
        ComposterBlock
            .update_level_composter(world, position, state_id, block, level + 1)
            .await;
    }
    world.sync_world_event(WorldEvent::ComposterFill, *position, i32::from(rose));
    true
}

/// `ComposterBlock.OutputContainer.canTakeItemThroughFace` (`ComposterBlock.java:469-472`).
///
/// A ready composter hands its single bone meal out through the DOWN face only.
#[must_use]
pub fn hopper_output_ready(world: &Arc<World>, position: &BlockPos, face: BlockDirection) -> bool {
    if face != BlockDirection::Down {
        return false;
    }
    let (block, state_id) = world.get_block_and_state_id(position);
    block == &Block::COMPOSTER
        && ComposterLikeProperties::from_state_id(state_id, block).level == COMPOSTER_READY
}

/// `ComposterBlock.OutputContainer.setChanged` (`ComposterBlock.java:475-477`).
///
/// Taking the bone meal empties the composter. Unlike `extractProduce`
/// (`ComposterBlock.java:297-309`) this path spawns no item entity and plays no sound.
pub async fn hopper_take_output(world: &Arc<World>, position: &BlockPos) {
    let (block, state_id) = world.get_block_and_state_id(position);
    if block != &Block::COMPOSTER {
        return;
    }
    ComposterBlock
        .update_level_composter(world, position, state_id, block, 0)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::item::Item;

    #[test]
    fn composter_chance_matches_vanilla_bootstrap() {
        // Low tier (0.3): leaves, saplings, seeds, etc.
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::OAK_LEAVES.id),
            Some(0.3)
        );
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::WHEAT_SEEDS.id),
            Some(0.3)
        );

        // Mid tier (0.5): tall grass, dried kelp block, etc.
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::TALL_GRASS.id),
            Some(0.5)
        );
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::DRIED_KELP_BLOCK.id),
            Some(0.5)
        );

        // High tier (0.65): pumpkin, melon, apple, etc.
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::PUMPKIN.id),
            Some(0.65)
        );
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::APPLE.id),
            Some(0.65)
        );

        // Very high tier (0.85): hay block, bread, etc.
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::HAY_BLOCK.id),
            Some(0.85)
        );
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::BREAD.id),
            Some(0.85)
        );

        // Maximum tier (1.0): cake, pumpkin pie
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::CAKE.id),
            Some(1.0)
        );
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::PUMPKIN_PIE.id),
            Some(1.0)
        );

        // Non-compostable items return None
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::DIAMOND.id),
            None
        );
        assert_eq!(
            get_composter_increase_chance_from_item_id(Item::STONE.id),
            None
        );
    }
}
