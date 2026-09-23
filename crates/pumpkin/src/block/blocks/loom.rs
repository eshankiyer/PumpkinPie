use std::sync::Arc;

use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, WallTorchLikeProperties};
use pumpkin_data::translation;
use pumpkin_inventory::loom_screen_handler::LoomScreenHandler;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::text::TextComponent;
use std::sync::Mutex;

use crate::block::registry::BlockActionResult;
use crate::block::{BlockBehaviour, NormalUseArgs, OnPlaceArgs};

#[pumpkin_block("minecraft:loom")]
pub struct LoomBlock;

impl BlockBehaviour for LoomBlock {
    // getStateForPlacement (LoomBlock.java:52-55): faces the placing player, same as
    // stonecutter.rs's on_place.
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = WallTorchLikeProperties::default(args.block);
        props.facing = args
            .player
            .living_entity
            .entity
            .get_horizontal_facing()
            .opposite();
        props.to_state_id(args.block)
    }

    // useWithoutItem (LoomBlock.java:33-43): opens the banner-pattern menu and awards the
    // interaction stat.
    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        args.player.increment_interaction_stat(
            pumpkin_data::statistic::StatisticCategory::Custom,
            pumpkin_data::statistic::CustomStatistic::InteractWithLoom as i32,
            1,
        );
        args.player
            .open_handled_screen(&LoomScreenFactory, Some(*args.position));

        BlockActionResult::Success
    }
}

struct LoomScreenFactory;

impl ScreenHandlerFactory for LoomScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        _player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        let handler: SharedScreenHandler = Arc::new(Mutex::new(LoomScreenHandler::new(
            sync_id,
            player_inventory,
        )));
        Some(handler)
    }

    fn get_display_name(&self) -> TextComponent {
        TextComponent::translate_cross(
            translation::java::CONTAINER_LOOM,
            translation::bedrock::CONTAINER_LOOM,
            &[],
        )
    }
}
