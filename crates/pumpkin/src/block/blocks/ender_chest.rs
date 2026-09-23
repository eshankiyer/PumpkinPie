use std::sync::Arc;

use crate::block::entities::ender_chest::EnderChestBlockEntity;
use crate::block::{
    BlockBehaviour, NormalUseArgs, OnPlaceArgs, OnScheduledTickArgs, OnSyncedBlockEventArgs,
    PlacedArgs, registry::BlockActionResult,
};
use crate::entity::mob::piglin_shared;
use crate::world::World;
use pumpkin_data::Block;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, LadderLikeProperties};
use pumpkin_data::translation;
use pumpkin_inventory::{
    generic_container_screen_handler::create_generic_9x3,
    player::ender_chest_inventory::EnderChestInventory,
    player::player_inventory::PlayerInventory,
    screen_handler::{InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler},
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;
use pumpkin_world::block::viewer::ViewerCountTracker;
use pumpkin_world::tick::TickPriority;
use std::sync::Mutex;

pub struct EnderChestScreenFactory {
    pub inventory: Arc<EnderChestInventory>,
    pub tracker: Option<Arc<ViewerCountTracker>>,
}

impl ScreenHandlerFactory for EnderChestScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        _player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        if let Some(tracker) = &self.tracker {
            self.inventory.set_tracker(tracker.clone());
        }
        let handler = create_generic_9x3(sync_id, player_inventory, self.inventory.clone());
        let concrete_arc = Arc::new(Mutex::new(handler));

        Some(concrete_arc as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        pumpkin_macros::translate_cross!(
            translation::java::CONTAINER_ENDERCHEST,
            translation::bedrock::CONTAINER_ENDERCHEST
        )
    }
}

#[pumpkin_block("minecraft:ender_chest")]
pub struct EnderChestBlock;

impl BlockBehaviour for EnderChestBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = LadderLikeProperties::default(args.block);
        props.facing = args
            .player
            .living_entity
            .entity
            .get_horizontal_facing()
            .opposite();
        props.waterlogged = args.replacing.water_source();
        props.to_state_id(args.block)
    }

    fn on_synced_block_event(&self, args: OnSyncedBlockEventArgs<'_>) -> bool {
        // On the server, we don't need to do more because the client is responsible for that.
        args.r#type == Self::LID_ANIMATION_EVENT_TYPE
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(ender_chest) = block_entity
                .as_any()
                .downcast_ref::<EnderChestBlockEntity>()
        {
            // `EnderChestBlock.tick` delegates scheduled server ticks to
            // `EnderChestBlockEntity.recheckOpen` (`EnderChestBlock.java:171-176`).
            ender_chest.recheck_open(args.world);
        }
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        if is_chest_blocked(args.world, args.position) {
            return BlockActionResult::Success;
        }

        let block_entity = if let Some(be) = args.world.get_block_entity(args.position) {
            be
        } else {
            let be = Arc::new(EnderChestBlockEntity::new(*args.position));
            args.world.add_block_entity(be.clone());
            be
        };

        if let Some(block_entity) = block_entity
            .as_any()
            .downcast_ref::<EnderChestBlockEntity>()
        {
            let inventory = args.player.ender_chest_inventory();
            args.player.increment_stat(
                pumpkin_data::statistic::StatisticCategory::Custom,
                pumpkin_data::statistic::CustomStatistic::OpenEnderchest as i32,
                1,
            );
            let was_empty = block_entity.get_tracker().get_viewer_count() == 0;
            let opened = args.player.open_handled_screen(
                &EnderChestScreenFactory {
                    inventory: inventory.clone(),
                    tracker: Some(block_entity.get_tracker()),
                },
                Some(*args.position),
            );
            if opened.is_some() && was_empty {
                // `ContainerOpenersCounter.incrementOpeners` schedules the opener recheck
                // when the first viewer arrives (`ContainerOpenersCounter.java:28-38, 100-102`).
                args.world.schedule_block_tick(
                    &Block::ENDER_CHEST,
                    *args.position,
                    5,
                    TickPriority::Normal,
                );
            }
            piglin_shared::anger_nearby_piglins(args.world, args.player);
        }

        BlockActionResult::Success
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        let block_entity = EnderChestBlockEntity::new(*args.position);
        args.world.add_block_entity(Arc::new(block_entity));
    }
}

fn is_chest_blocked(world: &World, block_pos: &BlockPos) -> bool {
    // TODO: Block opening when a cat is sitting on top.
    has_block_on_top(world, block_pos)
}
fn has_block_on_top(world: &World, block_pos: &BlockPos) -> bool {
    let above_pos = block_pos.up();
    let above_state = world.get_block_state(&above_pos);
    above_state.is_solid_block()
}
impl EnderChestBlock {
    pub const LID_ANIMATION_EVENT_TYPE: u8 = 1;
}
