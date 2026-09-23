use std::sync::Arc;
use std::sync::Mutex;

use crate::block::entities::BlockEntity;
use pumpkin_data::translation;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::Inventory;

use crate::block::registry::BlockActionResult;
use crate::block::{BlockBehaviour, NormalUseArgs, PlacedArgs};

// Create the factory just like ChestScreenFactory
struct BeaconScreenFactory(
    Arc<dyn Inventory>,
    Arc<dyn crate::block::entities::PropertyDelegate>,
);

impl ScreenHandlerFactory for BeaconScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        _player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        use pumpkin_inventory::beacon_screen_handler::create_beacon_handler;

        let concrete_handler =
            create_beacon_handler(sync_id, player_inventory, self.0.clone(), self.1.clone());
        let concrete_arc = Arc::new(Mutex::new(concrete_handler));

        Some(concrete_arc as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        pumpkin_macros::translate_cross!(
            translation::java::CONTAINER_BEACON,
            translation::bedrock::CONTAINER_BEACON
        )
    }
}

#[pumpkin_block("minecraft:beacon")]
pub struct BeaconBlock;

impl BlockBehaviour for BeaconBlock {
    /// `BeaconBlock.newBlockEntity` (`BeaconBlock.java:37-40`) supplies the block entity when a
    /// beacon is placed; the placement hook is the live Rust block-entity creation path.
    fn placed(&self, args: PlacedArgs<'_>) {
        args.world.add_block_entity(Arc::new(
            crate::block::entities::beacon::BeaconBlockEntity::new(*args.position),
        ));
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        let block_entity = args.world.get_block_entity(args.position);

        // Extract the inventory and property delegate from the entity
        let Some(inventory) = block_entity.clone().and_then(BlockEntity::get_inventory) else {
            return BlockActionResult::Fail;
        };
        let Some(property_delegate) = block_entity.and_then(BlockEntity::to_property_delegate)
        else {
            return BlockActionResult::Fail;
        };

        args.player.increment_interaction_stat(
            pumpkin_data::statistic::StatisticCategory::Custom,
            pumpkin_data::statistic::CustomStatistic::InteractWithBeacon as i32,
            1,
        );

        // Open the screen using the factory
        args.player.open_handled_screen(
            &BeaconScreenFactory(inventory, property_delegate),
            Some(*args.position),
        );

        BlockActionResult::Success
    }
}
