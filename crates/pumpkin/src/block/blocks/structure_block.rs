use std::sync::Arc;

use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, StructureBlockLikeProperties};
use pumpkin_macros::pumpkin_block;

use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::entities::structure_block::StructureBlockBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, CanPlaceAtArgs, NormalUseArgs, OnNeighborUpdateArgs, OnPlaceArgs, PlacedArgs,
    PlayerPlacedArgs,
};

/// `net.minecraft.world.level.block.StructureBlock`.
///
/// An operator-only block whose interesting behavior lives on `StructureBlockBlockEntity`.
#[pumpkin_block("minecraft:structure_block")]
pub struct StructureBlock;

impl BlockBehaviour for StructureBlock {
    /// Vanilla `GameMasterBlockItem.getPlacementState` (GameMasterBlockItem.java:15-18): the
    /// structure block item is a `GameMasterBlockItem`, so a player who cannot use game-master
    /// blocks gets no placement state and the block is never placed.
    /// `Player.canUseGameMasterBlocks` (Player.java:1863-1865) requires instabuild plus
    /// permission level 2; Pumpkin models instabuild through creative mode (the same mapping
    /// `CommandBlock::can_place_at` uses). A `None` player context (no player involved in the
    /// placement) passes, matching vanilla's null-player branch of the same check.
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        let Some(player) = args.player else {
            return true;
        };
        player.can_use_game_master_blocks()
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let props = StructureBlockLikeProperties::default(args.block);
        props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        args.world
            .add_block_entity(Arc::new(StructureBlockBlockEntity::new(*args.position)));
    }

    /// `StructureBlockEntity.setPlacedBy`: records the placing player's name as the author.
    fn player_placed(&self, args: PlayerPlacedArgs<'_>) {
        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return;
        };
        let Some(structure_block) = block_entity
            .as_any()
            .downcast_ref::<StructureBlockBlockEntity>()
        else {
            return;
        };
        *structure_block
            .author
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            args.player.gameprofile.name.clone();
    }

    /// `StructureBlock.useWithoutItem`/`StructureBlockEntity.usedBy`: opens the editor GUI for
    /// operators. `StructureBlock` is a `GameMasterBlock` gated on permission level 2, following
    /// the same shape as `jigsaw.rs`'s `normal_use` (the jigsaw port's additional creative-mode
    /// check has no vanilla citation and is deliberately not copied here, per the design doc).
    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        // `StructureBlockEntity.usedBy` requires `Player.canUseGameMasterBlocks`
        // (`StructureBlockEntity.java:149-151`; `Player.java:1863-1865`).
        if !args.player.can_use_game_master_blocks() {
            return BlockActionResult::Pass;
        }
        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return BlockActionResult::Pass;
        };
        args.world.update_block_entity(&block_entity);
        BlockActionResult::SuccessServer
    }

    /// `StructureBlock.neighborChanged` (`StructureBlock.java:68-81`) triggers only on a rising
    /// redstone edge; the entity's powered field retains that edge state between updates.
    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return;
        };
        let Some(structure_block) = block_entity
            .as_any()
            .downcast_ref::<StructureBlockBlockEntity>()
        else {
            return;
        };
        let should_trigger = block_receives_redstone_power(args.world, args.position);
        let is_powered = *structure_block
            .powered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if should_trigger && !is_powered {
            *structure_block
                .powered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            let mode = structure_block
                .mode
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            match mode.as_str() {
                "LOAD" => {
                    structure_block.place_structure(args.world);
                }
                "SAVE" => {
                    structure_block.save_structure(args.world, false);
                }
                "CORNER" => {
                    structure_block.unload_structure();
                }
                _ => {}
            }
        } else if !should_trigger && is_powered {
            *structure_block
                .powered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        }
    }
}
