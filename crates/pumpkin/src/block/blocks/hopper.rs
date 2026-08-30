use std::sync::Arc;

use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::{
    BlockFuture, GetComparatorOutputArgs, OnEntityCollisionArgs, OnNeighborUpdateArgs, OnPlaceArgs,
    PlacedArgs,
};
use crate::block::{
    registry::BlockActionResult,
    {BlockBehaviour, NormalUseArgs},
};
use crate::world::World;

use crate::block::entities::hopper::HopperBlockEntity;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, FacingHopper};
use pumpkin_data::{Block, BlockDirection, translation};
use pumpkin_inventory::generic_container_screen_handler::create_hopper;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    BoxFuture, InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::Inventory;
use pumpkin_world::world::BlockFlags;
use tokio::sync::Mutex;

struct HopperBlockScreenFactory(Arc<dyn Inventory>);

impl ScreenHandlerFactory for HopperBlockScreenFactory {
    fn create_screen_handler<'a>(
        &'a self,
        sync_id: u8,
        player_inventory: &'a Arc<PlayerInventory>,
        _player: &'a dyn InventoryPlayer,
    ) -> BoxFuture<'a, Option<SharedScreenHandler>> {
        Box::pin(async move {
            let concrete_handler = create_hopper(sync_id, player_inventory, self.0.clone()).await;

            let concrete_arc = Arc::new(Mutex::new(concrete_handler));

            Some(concrete_arc as SharedScreenHandler)
        })
    }

    fn get_display_name(&self) -> TextComponent {
        pumpkin_macros::translate_cross!(
            translation::java::CONTAINER_HOPPER,
            translation::bedrock::CONTAINER_HOPPER
        )
    }
}

#[pumpkin_block("minecraft:hopper")]
pub struct HopperBlock;

type HopperLikeProperties = pumpkin_data::block_properties::HopperLikeProperties;

/// Returns the server-side interaction boxes used by `HopperBlock.getInteractionShape`.
pub(crate) fn interaction_shapes_at(
    state_id: BlockStateId,
    position: &BlockPos,
) -> Vec<BoundingBox> {
    let facing = HopperLikeProperties::from_state_id(state_id, &Block::HOPPER).facing;
    let inside = BoundingBox::new_array(
        [2.0 / 16.0, 11.0 / 16.0, 2.0 / 16.0],
        [14.0 / 16.0, 1.0, 14.0 / 16.0],
    );
    let spout = match facing {
        FacingHopper::North => Some(BoundingBox::new_array(
            [6.0 / 16.0, 8.0 / 16.0, 0.0],
            [10.0 / 16.0, 10.0 / 16.0, 4.0 / 16.0],
        )),
        FacingHopper::East => Some(BoundingBox::new_array(
            [12.0 / 16.0, 8.0 / 16.0, 6.0 / 16.0],
            [1.0, 10.0 / 16.0, 10.0 / 16.0],
        )),
        FacingHopper::South => Some(BoundingBox::new_array(
            [6.0 / 16.0, 8.0 / 16.0, 12.0 / 16.0],
            [10.0 / 16.0, 10.0 / 16.0, 1.0],
        )),
        FacingHopper::West => Some(BoundingBox::new_array(
            [0.0, 8.0 / 16.0, 6.0 / 16.0],
            [4.0 / 16.0, 10.0 / 16.0, 10.0 / 16.0],
        )),
        FacingHopper::Down => None,
    };

    // `HopperBlock` builds `inside` and the horizontally rotated spout at
    // (`HopperBlock.java:56-61`), then selects only `inside` for DOWN
    // (`HopperBlock.java:79-80`).
    std::iter::once(inside)
        .chain(spout)
        .map(|shape| shape.shift(position.0.to_f64()))
        .collect()
}

impl BlockBehaviour for HopperBlock {
    /// `HopperBlock.entityInside` (`HopperBlock.java:163-169`) delegates item pickup to the
    /// block entity's `entityInside` transfer path.
    fn on_entity_collision<'a>(&'a self, args: OnEntityCollisionArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            HopperBlockEntity::entity_inside(args.world, args.position, args.entity).await;
        })
    }

    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            if let Some(block_entity) = args.world.get_block_entity(args.position)
                && let Some(inventory) = block_entity.get_inventory()
            {
                args.player
                    .increment_interaction_stat(
                        pumpkin_data::statistic::StatisticCategory::Custom,
                        pumpkin_data::statistic::CustomStatistic::InspectHopper as i32,
                        1,
                    )
                    .await;
                args.player
                    .open_handled_screen(&HopperBlockScreenFactory(inventory), Some(*args.position))
                    .await;
            }

            BlockActionResult::Success
        })
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = HopperLikeProperties::default(args.block);
            props.facing = match args.direction {
                BlockDirection::North => FacingHopper::North,
                BlockDirection::East => FacingHopper::East,
                BlockDirection::South => FacingHopper::South,
                BlockDirection::West => FacingHopper::West,
                BlockDirection::Up | BlockDirection::Down => FacingHopper::Down,
            };
            props.enabled = true;
            props.to_state_id(args.block)
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let props = HopperLikeProperties::from_state_id(args.state_id, args.block);
            let hopper_block_entity = HopperBlockEntity::new(*args.position, props.facing);
            args.world.add_block_entity(Arc::new(hopper_block_entity));
            if Block::from_state_id(args.old_state_id) != Block::from_state_id(args.state_id) {
                check_powered_state(args.world, args.position, args.state_id, args.block).await;
            }
        })
    }

    fn on_neighbor_update<'a>(&'a self, args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            check_powered_state(
                args.world,
                args.position,
                args.world.get_block_state_id(args.position),
                args.block,
            )
            .await;
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

async fn check_powered_state(
    world: &Arc<World>,
    pos: &BlockPos,
    state_id: BlockStateId,
    block: &Block,
) {
    let signal = !block_receives_redstone_power(world, pos).await;
    let mut state = HopperLikeProperties::from_state_id(state_id, block);
    if signal != state.enabled {
        state.enabled = signal;
        world
            .set_block_state(pos, state.to_state_id(block), BlockFlags::NOTIFY_LISTENERS)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interaction_shape_has_only_the_inside_for_down_facing_hoppers() {
        // `HopperBlock.java:56-61,79-80` uses the inside shape alone for DOWN and adds one
        // horizontally rotated spout for each horizontal facing.
        let position = BlockPos::new(3, 64, -2);
        let down = HopperLikeProperties {
            enabled: true,
            facing: FacingHopper::Down,
        }
        .to_state_id(&Block::HOPPER);
        let north = HopperLikeProperties {
            enabled: true,
            facing: FacingHopper::North,
        }
        .to_state_id(&Block::HOPPER);

        assert_eq!(interaction_shapes_at(down, &position).len(), 1);
        let north_shapes = interaction_shapes_at(north, &position);
        assert_eq!(north_shapes.len(), 2);
        assert_eq!(north_shapes[1].min.z, -2.0);
        assert_eq!(north_shapes[1].max.z, -1.75);
    }
}
