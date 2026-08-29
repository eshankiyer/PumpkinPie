use std::sync::Arc;

use crate::{
    block::{BlockBehaviour, BlockDirection, GetComparatorOutputArgs, NormalUseArgs, OnScheduledTickArgs, UseWithItemArgs, registry::BlockActionResult},
    entity::{Entity, item::ItemEntity},
    world::World,
};
use pumpkin_data::{
    Block, BlockStateId,
    block_properties::{BlockProperties, ComposterLikeProperties},
    composter_increase_chance::get_composter_increase_chance_from_item_id,
    entity::EntityType,
    item::Item,
    item_stack::ItemStack,
    sound::{Sound, SoundCategory},
    world::WorldEvent,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::{tick::TickPriority, world::BlockFlags};
use rand::RngExt;

/// `ComposterBlock.READY` (`ComposterBlock.java:46`).
const COMPOSTER_READY: u8 = 8;

#[pumpkin_block("minecraft:composter")]
pub struct ComposterBlock;

impl BlockBehaviour for ComposterBlock {
    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        let state_id = args.world.get_block_state_id(args.position);
        let props = ComposterLikeProperties::from_state_id(state_id, args.block);
        if props.level == COMPOSTER_READY {
            self.clear_composter(args.world, args.position, state_id, args.block);
            // Vanilla `ComposterBlock.useWithoutItem` returns SUCCESS for a full composter
            // (`ComposterBlock.java:273-285`).
            return BlockActionResult::Success;
        }
        BlockActionResult::Pass
    }

    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        let state_id = args.world.get_block_state_id(args.position);
        let level = ComposterLikeProperties::from_state_id(state_id, args.block).level;
        let item_id = args.item_stack.item.id;
        let Some(chance) = get_composter_increase_chance_from_item_id(item_id) else {
            // Vanilla `ComposterBlock.useItemOn` delegates non-compostables to super
            // (`ComposterBlock.java:248-270`).
            return BlockActionResult::PassToDefaultBlockAction;
        };
        if level == COMPOSTER_READY {
            // Full extraction is handled by the empty-hand fallback
            // (`ComposterBlock.java:248-279`).
            return BlockActionResult::PassToDefaultBlockAction;
        }
        if level < 7 && !args.player.has_infinite_materials() {
            args.item_stack.decrement(1);
        }
        if level < 7 {
            let rose = level == 0 || rand::rng().random_bool(f64::from(chance));
            if rose {
                self.update_level_composter(args.world, args.position, state_id, args.block, level + 1);
            }
            args.world.sync_world_event(WorldEvent::ComposterFill, *args.position, i32::from(rose));
        }
        // Vanilla accepts every compostable item (`ComposterBlock.java:257-267`).
        BlockActionResult::Success
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let state_id = args.world.get_block_state_id(args.position);
        let level = ComposterLikeProperties::from_state_id(state_id, args.block).level;
        if level == 7 {
            self.update_level_composter(args.world, args.position, state_id, args.block, level + 1);
            args.world.play_sound(Sound::BlockComposterReady, SoundCategory::Blocks, &args.position.to_centered_f64());
        }
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        Some(ComposterLikeProperties::from_state_id(args.state.id, args.block).level)
    }
}

impl ComposterBlock {
    /// `ComposterBlock.insertItem` (`ComposterBlock.java:285-295`) consumes an accepted item
    /// even when its random fill roll does not raise the level.
    pub fn insert_item_from_villager(world: &Arc<World>, location: &BlockPos, item_stack: &mut ItemStack) -> bool {
        let (block, state_id) = world.get_block_and_state_id(location);
        if block != &Block::COMPOSTER { return false; }
        let level = ComposterLikeProperties::from_state_id(state_id, block).level;
        if level >= 7 { return false; }
        let Some(chance) = get_composter_increase_chance_from_item_id(item_stack.item.id) else { return false; };
        let rose = level == 0 || rand::rng().random_bool(f64::from(chance));
        if rose { ComposterBlock.update_level_composter(world, location, state_id, block, level + 1); }
        item_stack.decrement(1);
        true
    }

    pub fn update_level_composter(&self, world: &Arc<World>, location: &BlockPos, state_id: BlockStateId, block: &Block, level: u8) {
        let mut props = ComposterLikeProperties::from_state_id(state_id, block);
        props.level = level;
        world.set_block_state(location, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
        if level == 7 { world.schedule_block_tick(block, *location, 20, TickPriority::Normal); }
    }

    pub fn clear_composter(&self, world: &Arc<World>, location: &BlockPos, state_id: BlockStateId, block: &Block) {
        self.update_level_composter(world, location, state_id, block, 0);
        // `Vec3.atLowerCornerWithOffset(pos, 0.5, 1.01, 0.5).offsetRandomXZ` jitters X/Z only.
        let item_position = {
            let mut rng = rand::rng();
            location.to_centered_f64().add_raw(rng.random_range(-0.35..=0.35), 0.51, rng.random_range(-0.35..=0.35))
        };
        let item_entity = ItemEntity::new(Entity::new(world.clone(), item_position, &EntityType::ITEM), ItemStack::new(1, &Item::BONE_MEAL));
        world.spawn_entity(Arc::new(item_entity));
        world.play_sound(Sound::BlockComposterEmpty, SoundCategory::Blocks, &location.to_centered_f64());
    }
}

/// `ComposterBlock.InputContainer.setChanged` (`ComposterBlock.java:428-436`). The
    /// `WorldlyContainerHolder` path (`ComposterBlock.java:44`, `ComposterBlock.java:365-373`) accepts items only from
/// above and consumes them as `addItem` does (`ComposterBlock.java:317-334`).
pub fn hopper_insert_item(world: &Arc<World>, position: &BlockPos, face: BlockDirection, item_id: u16) -> bool {
    if face != BlockDirection::Up { return false; }
    let (block, state_id) = world.get_block_and_state_id(position);
    if block != &Block::COMPOSTER { return false; }
    let level = ComposterLikeProperties::from_state_id(state_id, block).level;
    if level >= 7 { return false; }
    let Some(chance) = get_composter_increase_chance_from_item_id(item_id) else { return false; };
    let rose = level == 0 || rand::rng().random_bool(f64::from(chance));
    if rose { ComposterBlock.update_level_composter(world, position, state_id, block, level + 1); }
    world.sync_world_event(WorldEvent::ComposterFill, *position, i32::from(rose));
    true
}

/// `ComposterBlock.OutputContainer.canTakeItemThroughFace` (`ComposterBlock.java:469-472`).
#[must_use]
pub fn hopper_output_ready(world: &Arc<World>, position: &BlockPos, face: BlockDirection) -> bool {
    if face != BlockDirection::Down { return false; }
    let (block, state_id) = world.get_block_and_state_id(position);
    block == &Block::COMPOSTER && ComposterLikeProperties::from_state_id(state_id, block).level == COMPOSTER_READY
}

/// `ComposterBlock.OutputContainer.setChanged` (`ComposterBlock.java:475-477`) empties the
/// composter without spawning an item entity or playing the hand-extraction sound
/// (`ComposterBlock.java:297-309`).
pub fn hopper_take_output(world: &Arc<World>, position: &BlockPos) {
    let (block, state_id) = world.get_block_and_state_id(position);
    if block == &Block::COMPOSTER { ComposterBlock.update_level_composter(world, position, state_id, block, 0); }
}
