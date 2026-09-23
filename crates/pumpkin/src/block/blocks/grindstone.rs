use std::sync::Arc;

use pumpkin_data::{
    Block, BlockDirection, BlockStateId, HorizontalFacingExt,
    block_properties::{AttachFace, BlockProperties, GrindstoneLikeProperties},
};
use pumpkin_inventory::grindstone_screen_handler::GrindstoneScreenHandler;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;
use pumpkin_world::world::BlockAccessor;
use std::sync::Mutex;

use crate::block::CanPlaceAtArgs;
use crate::block::registry::BlockActionResult;
use crate::block::{BlockBehaviour, NormalUseArgs};
use crate::block::{GetStateForNeighborUpdateArgs, OnPlaceArgs};

use super::abstract_wall_mounting::WallMountedBlock;

#[pumpkin_block("minecraft:grindstone")]
pub struct GrindstoneBlock;

impl BlockBehaviour for GrindstoneBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props =
            GrindstoneLikeProperties::from_state_id(args.block.default_state.id, args.block);
        (props.face, props.facing) =
            WallMountedBlock::get_placement_face(self, args.player, args.direction);

        props.to_state_id(args.block)
    }

    // useWithoutItem (GrindstoneBlock.java:73-83): opens the repair/disenchant menu and awards
    // the interaction stat.
    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        args.player.increment_interaction_stat(
            pumpkin_data::statistic::StatisticCategory::Custom,
            pumpkin_data::statistic::CustomStatistic::InteractWithGrindstone as i32,
            1,
        );
        args.player
            .open_handled_screen(&GrindstoneScreenFactory, Some(*args.position));

        BlockActionResult::Success
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        // Use the provided direction, or fallback to the current state's direction if missing
        let direction = args
            .direction
            .unwrap_or_else(|| self.get_direction(args.state.id, args.block));

        WallMountedBlock::can_place_at(self, args.block_accessor, args.position, direction)
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        WallMountedBlock::get_state_for_neighbor_update(self, args)
    }
}

impl WallMountedBlock for GrindstoneBlock {
    fn can_place_at(
        &self,
        _world: &dyn BlockAccessor,
        _pos: &BlockPos,
        _direction: BlockDirection,
    ) -> bool {
        true
    }

    fn get_direction(&self, state_id: BlockStateId, block: &Block) -> BlockDirection {
        let props = GrindstoneLikeProperties::from_state_id(state_id, block);
        match props.face {
            AttachFace::Floor => BlockDirection::Up,
            AttachFace::Ceiling => BlockDirection::Down,
            AttachFace::Wall => props.facing.to_block_direction(),
        }
    }
}

struct GrindstoneScreenFactory;

impl ScreenHandlerFactory for GrindstoneScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        _player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        let handler: SharedScreenHandler = Arc::new(Mutex::new(GrindstoneScreenHandler::new(
            sync_id,
            player_inventory,
        )));
        Some(handler)
    }

    fn get_display_name(&self) -> TextComponent {
        // No Bedrock-side `container.grindstone_title` key exists; unlike Loom/Stonecutter,
        // Bedrock has no direct equivalent to translate against.
        TextComponent::translate(
            pumpkin_data::translation::java::CONTAINER_GRINDSTONE_TITLE,
            &[],
        )
    }
}
