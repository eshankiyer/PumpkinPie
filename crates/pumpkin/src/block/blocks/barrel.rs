use std::sync::Arc;
use std::sync::Mutex;

use crate::block::{GetComparatorOutputArgs, OnPlaceArgs, OnScheduledTickArgs, PlacedArgs};
use crate::block::{
    registry::BlockActionResult,
    {BlockBehaviour, NormalUseArgs},
};

use crate::block::entities::barrel::BarrelBlockEntity;
use crate::entity::EntityBase;
use crate::entity::mob::piglin_shared;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BarrelLikeProperties, BlockProperties};
use pumpkin_data::translation;
use pumpkin_inventory::generic_container_screen_handler::create_generic_9x3;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::Inventory;
use pumpkin_world::tick::TickPriority;

struct BarrelScreenFactory(Arc<dyn Inventory>);

impl ScreenHandlerFactory for BarrelScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        _player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        let handler = create_generic_9x3(sync_id, player_inventory, self.0.clone());
        let concrete_arc = Arc::new(Mutex::new(handler));

        Some(concrete_arc as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        pumpkin_macros::translate_cross!(
            translation::java::CONTAINER_BARREL,
            translation::bedrock::CONTAINER_BARREL
        )
    }
}

#[pumpkin_block("minecraft:barrel")]
pub struct BarrelBlock;

impl BlockBehaviour for BarrelBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = BarrelLikeProperties::default(args.block);
        props.facing = args.player.get_entity().get_facing().opposite();
        props.to_state_id(args.block)
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(inventory) = block_entity.clone().get_inventory()
        {
            // `BarrelBlockEntity.startOpen` schedules the first recheck only on the
            // zero-to-one transition (`BarrelBlockEntity.java:102-109`).
            let was_empty = block_entity
                .as_any()
                .downcast_ref::<BarrelBlockEntity>()
                .is_some_and(|barrel| barrel.viewer_count() == 0);
            args.player.increment_interaction_stat(
                pumpkin_data::statistic::StatisticCategory::Custom,
                pumpkin_data::statistic::CustomStatistic::OpenBarrel as i32,
                1,
            );
            let opened = args
                .player
                .open_handled_screen(&BarrelScreenFactory(inventory), Some(*args.position));
            if opened.is_some() && was_empty {
                // `ContainerOpenersCounter.incrementOpeners` schedules the opener recheck
                // when the first viewer arrives (`ContainerOpenersCounter.java:28-38,100-102`).
                args.world.schedule_block_tick(
                    &pumpkin_data::Block::BARREL,
                    *args.position,
                    5,
                    TickPriority::Normal,
                );
            }
            // Vanilla `BarrelBlock.useWithoutItem` (`BarrelBlock.java:43-52`) angers
            // nearby piglins after opening the barrel.
            piglin_shared::anger_nearby_piglins(args.world, args.player);
        }

        BlockActionResult::Success
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(barrel) = block_entity.as_any().downcast_ref::<BarrelBlockEntity>()
        {
            // `BarrelBlock.tick` delegates scheduled ticks to
            // `BarrelBlockEntity.recheckOpen` (`BarrelBlock.java:61-65`).
            barrel.recheck_open(args.world);
        }
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        let barrel_block_entity = BarrelBlockEntity::new(*args.position);
        args.world.add_block_entity(Arc::new(barrel_block_entity));
    }

    fn on_state_replaced(&self, args: crate::block::OnStateReplacedArgs<'_>) {
        // Vanilla `BarrelBlock.affectNeighborsAfterRemoval` (`BarrelBlock.java:55-58`)
        // refreshes adjacent comparator inputs using the removed barrel state.
        args.world.update_comparators(args.position, args.block);
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(inventory) = block_entity.get_inventory()
        {
            Some(crate::block::calculate_comparator_output(
                inventory.as_ref(),
            ))
        } else {
            None
        }
    }
}
