use std::sync::Arc;

use crate::block::{
    BlockFuture, GetComparatorOutputArgs, OnPlaceArgs, OnSyncedBlockEventArgs, PlacedArgs,
};
use crate::block::{
    registry::BlockActionResult,
    {BlockBehaviour, NormalUseArgs},
};

use crate::block::entities::shulker_box::ShulkerBoxBlockEntity;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::BlockProperties;
use pumpkin_data::translation;
use pumpkin_data::{BlockDirection, FacingExt};
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    BoxFuture, InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_inventory::shulker_box_screen_handler::ShulkerBoxScreenHandler;
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::Inventory;
use tokio::sync::Mutex;

struct ShulkerBoxScreenFactory(Arc<dyn Inventory>);

/// `Shulker.getProgressDeltaAabb` (`Shulker.java:262-274`) for a closed-to-half-open lid,
/// used by `ShulkerBoxBlock.canOpen` (`ShulkerBoxBlock.java:90-97`).
fn lid_open_bounding_box(
    position: &pumpkin_util::math::position::BlockPos,
    facing: BlockDirection,
) -> BoundingBox {
    let block = BoundingBox::from_block(position);
    let offset = facing.to_offset();
    let mut min = block.min;
    let mut max = block.max;

    if offset.x < 0 {
        min.x += f64::from(offset.x) * 0.5;
        max.x += f64::from(offset.x);
    } else if offset.x > 0 {
        min.x += f64::from(offset.x);
        max.x += f64::from(offset.x) * 0.5;
    }
    if offset.y < 0 {
        min.y += f64::from(offset.y) * 0.5;
        max.y += f64::from(offset.y);
    } else if offset.y > 0 {
        min.y += f64::from(offset.y);
        max.y += f64::from(offset.y) * 0.5;
    }
    if offset.z < 0 {
        min.z += f64::from(offset.z) * 0.5;
        max.z += f64::from(offset.z);
    } else if offset.z > 0 {
        min.z += f64::from(offset.z);
        max.z += f64::from(offset.z) * 0.5;
    }

    BoundingBox::new(min, max).contract_all(1.0e-6)
}

impl ScreenHandlerFactory for ShulkerBoxScreenFactory {
    fn create_screen_handler<'a>(
        &'a self,
        sync_id: u8,
        player_inventory: &'a Arc<PlayerInventory>,
        _player: &'a dyn InventoryPlayer,
    ) -> BoxFuture<'a, Option<SharedScreenHandler>> {
        Box::pin(async move {
            // `ShulkerBoxMenu.java:17-32`: a shulker box is MenuType.SHULKER_BOX with
            // `ShulkerBoxSlot`s, not a generic 9x3 chest - the slots refuse items that
            // cannot nest inside container items (`BlockItem.java:193-196`).
            let handler =
                ShulkerBoxScreenHandler::new(sync_id, player_inventory, self.0.clone()).await;
            let screen_handler_arc = Arc::new(Mutex::new(handler));

            Some(screen_handler_arc as SharedScreenHandler)
        })
    }

    fn get_display_name(&self) -> TextComponent {
        pumpkin_macros::translate_cross!(
            translation::java::CONTAINER_SHULKERBOX,
            translation::bedrock::CONTAINER_SHULKERBOX
        )
    }
}

#[pumpkin_block_from_tag("minecraft:shulker_boxes")]
pub struct ShulkerBoxBlock;

type EndRodLikeProperties = pumpkin_data::block_properties::EndRodLikeProperties;

impl BlockBehaviour for ShulkerBoxBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = EndRodLikeProperties::default(args.block);
            props.facing = args.direction.to_facing().opposite();
            props.to_state_id(args.block)
        })
    }

    fn on_synced_block_event<'a>(
        &'a self,
        args: OnSyncedBlockEventArgs<'a>,
    ) -> BlockFuture<'a, bool> {
        Box::pin(async move {
            // On the server, we don't need the Animation steps for now, because the client is responsible for that.
            // TODO: Do not open the shulker box when it is currently closing
            args.r#type == Self::OPEN_ANIMATION_EVENT_TYPE
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let barrel_block_entity = ShulkerBoxBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(barrel_block_entity));
        })
    }

    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            if let Some(block_entity) = args.world.get_block_entity(args.position)
                && let Some(inventory) = block_entity.clone().get_inventory()
            {
                if let Some(shulker) = block_entity
                    .as_any()
                    .downcast_ref::<ShulkerBoxBlockEntity>()
                    && shulker.get_animation_status()
                        == crate::block::entities::shulker_box::AnimationStatus::Closed
                {
                    let state_id = args.world.get_block_state_id(args.position);
                    let facing = EndRodLikeProperties::from_state_id(state_id, args.block)
                        .facing
                        .to_block_direction();
                    let lid_box = lid_open_bounding_box(args.position, facing);
                    // `ShulkerBoxBlock.canOpen` (`ShulkerBoxBlock.java:90-97`) refuses to open
                    // when the closed-to-half-open lid volume collides with a block or entity.
                    if !args.world.is_space_empty(lid_box)
                        || !args.world.get_entities_at_box(&lid_box).is_empty()
                    {
                        return BlockActionResult::Success;
                    }
                }

                args.player
                    .increment_interaction_stat(
                        pumpkin_data::statistic::StatisticCategory::Custom,
                        pumpkin_data::statistic::CustomStatistic::OpenShulkerBox as i32,
                        1,
                    )
                    .await;
                args.player
                    .open_handled_screen(&ShulkerBoxScreenFactory(inventory), Some(*args.position))
                    .await;
            }

            BlockActionResult::Success
        })
    }

    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            if let Some(block_entity) = args.world.get_block_entity(args.position)
                && let Some(inventory) = block_entity.get_inventory()
            {
                Some(crate::block::calculate_comparator_output(inventory.as_ref()).await)
            } else {
                None
            }
        })
    }
}

impl ShulkerBoxBlock {
    pub const OPEN_ANIMATION_EVENT_TYPE: u8 = 1;
}

#[cfg(test)]
mod tests {
    use super::lid_open_bounding_box;
    use pumpkin_data::BlockDirection;
    use pumpkin_util::math::position::BlockPos;

    #[test]
    fn lid_open_box_extends_half_a_block_from_the_closed_box() {
        // `Shulker.getProgressDeltaAabb` (`Shulker.java:262-274`) uses the 0.0-to-0.5
        // animation interval for `ShulkerBoxBlock.canOpen` (`ShulkerBoxBlock.java:90-97`).
        let position = BlockPos::new(10, 64, -4);
        let east = lid_open_bounding_box(&position, BlockDirection::East);
        assert!((east.min.x - 11.000001).abs() < 1.0e-9);
        assert!((east.max.x - 11.499999).abs() < 1.0e-9);
        assert!((east.min.y - 64.000001).abs() < 1.0e-9);
        assert!((east.max.y - 64.999999).abs() < 1.0e-9);

        let down = lid_open_bounding_box(&position, BlockDirection::Down);
        assert!((down.min.y - 63.500001).abs() < 1.0e-9);
        assert!((down.max.y - 63.999999).abs() < 1.0e-9);
    }
}
