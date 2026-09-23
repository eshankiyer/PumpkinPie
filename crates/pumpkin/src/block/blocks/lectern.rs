use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::block::entities::lectern::LecternBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BrokenArgs, EmitsRedstonePowerArgs, GetComparatorOutputArgs,
    GetRedstonePowerArgs, NormalUseArgs, OnPlaceArgs, OnScheduledTickArgs, OnStateReplacedArgs,
    PlacedArgs, UseWithItemArgs,
};
use crate::entity::Entity;
use crate::entity::EntityBase;
use crate::entity::item::ItemEntity;
use crate::world::World;
use crate::world::game_event::{GameEventContext, emit_game_event};
use pumpkin_data::block_properties::{BlockProperties, LecternLikeProperties};
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockDirection, BlockStateId, HorizontalFacingExt, tag, translation};
use pumpkin_inventory::lectern_screen_handler::{LecternController, LecternScreenHandler};
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::Inventory;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use std::sync::Mutex;

/// Mirrors the final non-book branch of `LecternBlock.useItemOn`: only an empty main hand
/// passes without invoking the empty-hand action (`LecternBlock.java:211-227`).
const fn non_book_item_result(item_is_empty: bool, main_hand: bool) -> BlockActionResult {
    if item_is_empty && main_hand {
        BlockActionResult::Pass
    } else {
        BlockActionResult::PassToDefaultBlockAction
    }
}

/// Bridges the screen handler back into the world: page changes emit the
/// vanilla redstone pulse and taking the book clears `has_book`.
struct LecternPageController {
    world: Arc<World>,
    position: BlockPos,
    inventory: Arc<dyn Inventory>,
}

impl LecternPageController {
    fn entity(&self) -> Option<&LecternBlockEntity> {
        self.inventory.as_any().downcast_ref::<LecternBlockEntity>()
    }
}

impl LecternController for LecternPageController {
    fn current_page(&self) -> i32 {
        self.entity()
            .map_or(0, |entity| entity.page.load(Ordering::Relaxed) as i32)
    }

    fn has_book(&self) -> bool {
        self.entity().is_some_and(LecternBlockEntity::has_book)
    }

    fn set_page(&self, page: i32) {
        let Some(entity) = self.entity() else {
            return;
        };
        let page_count = entity.page_count();
        let page = page.clamp(0, (page_count - 1).max(0));
        if page == entity.page.load(Ordering::Relaxed) as i32 {
            return;
        }
        entity.page.store(page as usize, Ordering::Relaxed);
        entity.mark_dirty();
        LecternBlock::pulse(&self.world, &self.position);
    }

    fn on_book_taken(&self) {
        if let Some(entity) = self.entity() {
            entity.page.store(0, Ordering::Relaxed);
        }
        LecternBlock::set_has_book(&self.world, &self.position, false, None);
    }
}

struct LecternScreenFactory {
    inventory: Arc<dyn Inventory>,
    controller: Arc<dyn LecternController>,
}

impl ScreenHandlerFactory for LecternScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        _player_inventory: &Arc<PlayerInventory>,
        _player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        let handler =
            LecternScreenHandler::new(sync_id, self.inventory.clone(), self.controller.clone());
        Some(Arc::new(Mutex::new(handler)) as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        pumpkin_macros::translate_cross!(
            translation::java::CONTAINER_LECTERN,
            translation::bedrock::TILE_LECTERN_NAME
        )
    }
}

#[pumpkin_block("minecraft:lectern")]
pub struct LecternBlock;

impl LecternBlock {
    /// Vanilla pulse length of a page-turn signal, in game ticks.
    const PAGE_TURN_PULSE_TICKS: u8 = 2;

    /// The lectern strongly powers the block below it, so its neighbors need
    /// updating whenever the power or book state changes.
    fn update_neighbors_below(world: &Arc<World>, position: &BlockPos) {
        world.update_neighbors(&position.down(), None);
    }

    /// Emits the vanilla page-turn redstone pulse: powered for two game ticks.
    pub(crate) fn pulse(world: &Arc<World>, position: &BlockPos) {
        let (block, state_id) = world.get_block_and_state_id(position);
        if block != &Block::LECTERN {
            return;
        }
        let mut props = LecternLikeProperties::from_state_id(state_id, block);
        props.powered = true;
        world.set_block_state(position, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
        Self::update_neighbors_below(world, position);
        world.schedule_block_tick(
            block,
            *position,
            Self::PAGE_TURN_PULSE_TICKS,
            TickPriority::Normal,
        );
        world.sync_world_event(WorldEvent::SoundPageTurn, *position, 0);
    }

    /// `LecternBlock.resetBookState` (`LecternBlock.java:154-159`): drops any pending pulse,
    /// sets `has_book`, and fires the `BLOCK_CHANGE` game event (sculk sensors/wardens key off
    /// this). `source_entity` is `None` for the book-emptied path
    /// (`LecternBlockEntity.onBookItemRemove`, `LecternBlockEntity.java:145-149`, which always
    /// passes `null`) and `Some(player)` for the place-book path (`LecternBlock.placeBook`,
    /// `LecternBlock.java:141-146`).
    pub(crate) fn set_has_book(
        world: &Arc<World>,
        position: &BlockPos,
        has_book: bool,
        source_entity: Option<Arc<dyn EntityBase>>,
    ) {
        let (block, state_id) = world.get_block_and_state_id(position);
        if block != &Block::LECTERN {
            return;
        }
        let mut props = LecternLikeProperties::from_state_id(state_id, block);
        props.powered = false;
        props.has_book = has_book;
        let new_state_id = props.to_state_id(block);
        world.set_block_state(position, new_state_id, BlockFlags::NOTIFY_ALL);

        let context = GameEventContext {
            source_entity,
            affected_block_state: Some(new_state_id),
        };
        emit_game_event(
            world,
            GameEvent::BlockChange,
            position.to_centered_f64(),
            context,
        );
        // `resetBookState` emits `BLOCK_CHANGE` before updating the block below
        // (`LecternBlock.java:148-153`).
        Self::update_neighbors_below(world, position);
    }
}

impl BlockBehaviour for LecternBlock {
    fn placed(&self, args: PlacedArgs<'_>) {
        let block_entity = LecternBlockEntity::new(*args.position);
        args.world.add_block_entity(Arc::new(block_entity));
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = LecternLikeProperties::default(args.block);
        props.facing = args
            .player
            .living_entity
            .entity
            .get_horizontal_facing()
            .opposite();
        props.to_state_id(args.block)
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        let props = LecternLikeProperties::from_state_id(
            args.world.get_block_state(args.position).id,
            args.block,
        );
        if !props.has_book {
            // `LecternBlock.useWithoutItem` consumes an empty lectern interaction
            // (`LecternBlock.java:230-241`).
            return BlockActionResult::Consume;
        }

        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            // `useWithoutItem` returns SUCCESS after `openScreen`, even when its
            // `instanceof LecternBlockEntity` check finds no entity
            // (`LecternBlock.java:233-241,249-253`).
            return BlockActionResult::Success;
        };
        let Some(inventory) = block_entity.get_inventory() else {
            // The outer `useWithoutItem` result remains SUCCESS when `openScreen` cannot
            // open a menu (`LecternBlock.java:233-241,249-253`).
            return BlockActionResult::Success;
        };

        args.player.increment_interaction_stat(
            pumpkin_data::statistic::StatisticCategory::Custom,
            pumpkin_data::statistic::CustomStatistic::InteractWithLectern as i32,
            1,
        );

        let controller = Arc::new(LecternPageController {
            world: args.world.clone(),
            position: *args.position,
            inventory: inventory.clone(),
        });
        args.player.open_handled_screen(
            &LecternScreenFactory {
                inventory,
                controller,
            },
            Some(*args.position),
        );

        BlockActionResult::Success
    }

    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        let item_stack = &mut *args.item_stack;
        let props = LecternLikeProperties::from_state_id(
            args.world.get_block_state(args.position).id,
            args.block,
        );
        if props.has_book {
            // Fall through so `normal_use` opens the reading screen.
            return BlockActionResult::PassToDefaultBlockAction;
        }

        if !item_stack.item.has_tag(&tag::Item::MINECRAFT_LECTERN_BOOKS) {
            // `useItemOn` passes an empty main hand, but tries the empty-hand action for
            // every other non-book item (`LecternBlock.java:211-227`).
            return non_book_item_result(
                item_stack.is_empty(),
                matches!(
                    args.equipment_slot,
                    &pumpkin_data::data_component_impl::EquipmentSlot::MainHand(_)
                ),
            );
        }

        let Some(lectern) = args.world.get_block_entity(args.position) else {
            // `tryPlaceBook` returns true from the state check even if `placeBook` finds no
            // block entity (`LecternBlock.java:126-145`).
            return BlockActionResult::Success;
        };
        let Some(lectern) = lectern.as_any().downcast_ref::<LecternBlockEntity>() else {
            // The same `tryPlaceBook` success result applies when `placeBook`'s type check
            // has no matching entity (`LecternBlock.java:126-145`).
            return BlockActionResult::Success;
        };

        let book = item_stack.split_unless_creative(args.player.gamemode.load(), 1);
        let _ = item_stack;
        lectern.set_stack(0, book);

        Self::set_has_book(
            args.world,
            args.position,
            true,
            Some(args.player.clone() as Arc<dyn EntityBase>),
        );
        args.world
            .play_block_sound(Sound::ItemBookPut, SoundCategory::Blocks, *args.position);

        BlockActionResult::Success
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let mut props = LecternLikeProperties::from_state_id(
            args.world.get_block_state(args.position).id,
            args.block,
        );
        props.powered = false;
        args.world.set_block_state(
            args.position,
            props.to_state_id(args.block),
            BlockFlags::NOTIFY_ALL,
        );
        Self::update_neighbors_below(args.world, args.position);
    }

    fn emits_redstone_power(&self, _args: EmitsRedstonePowerArgs<'_>) -> bool {
        true
    }

    fn get_weak_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8 {
        let props = LecternLikeProperties::from_state_id(args.state.id, args.block);
        if props.powered { 15 } else { 0 }
    }

    fn get_strong_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8 {
        let props = LecternLikeProperties::from_state_id(args.state.id, args.block);
        if props.powered && args.direction == BlockDirection::Up {
            15
        } else {
            0
        }
    }

    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>) {
        if !args.moved {
            let props = LecternLikeProperties::from_state_id(args.old_state_id, args.block);
            if props.powered {
                Self::update_neighbors_below(args.world, args.position);
            }
        }
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(lectern_entity) = block_entity.as_any().downcast_ref::<LecternBlockEntity>()
        {
            let book = lectern_entity.remove_stack(0);
            if !book.is_empty() {
                // `LecternBlockEntity.preRemoveSideEffects`
                // (`LecternBlockEntity.java:227-236`): the dropped book is offset a
                // quarter-block toward the lectern's facing direction and sits a full
                // block above the base, not at the block centre.
                let facing = LecternLikeProperties::from_state_id(args.state.id, args.block)
                    .facing
                    .to_block_direction();
                let offset = facing.to_offset();
                let entity = Entity::new(
                    args.world.clone(),
                    Vector3::new(
                        f64::from(args.position.0.x) + 0.5 + 0.25 * f64::from(offset.x),
                        f64::from(args.position.0.y) + 1.0,
                        f64::from(args.position.0.z) + 0.5 + 0.25 * f64::from(offset.z),
                    ),
                    &EntityType::ITEM,
                );
                let item_entity = ItemEntity::new(entity, book);
                args.world.spawn_entity(Arc::new(item_entity));
            }
        }
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        let props = LecternLikeProperties::from_state_id(args.state.id, args.block);
        // `getAnalogOutputSignal` is gated by `HAS_BOOK` before reading the block entity
        // (`LecternBlock.java:198-207`).
        if !props.has_book {
            return Some(0);
        }

        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(lectern_entity) = block_entity.as_any().downcast_ref::<LecternBlockEntity>()
        {
            Some(lectern_entity.comparator_output())
        } else {
            Some(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockActionResult, non_book_item_result};

    #[test]
    fn non_book_lectern_use_matches_vanilla_hand_fallback() {
        // `LecternBlock.useItemOn` passes only for an empty main hand and otherwise requests
        // the empty-hand action (`LecternBlock.java:211-227`).
        assert_eq!(non_book_item_result(true, true), BlockActionResult::Pass);
        assert_eq!(
            non_book_item_result(false, true),
            BlockActionResult::PassToDefaultBlockAction
        );
        assert_eq!(
            non_book_item_result(true, false),
            BlockActionResult::PassToDefaultBlockAction
        );
    }
}
