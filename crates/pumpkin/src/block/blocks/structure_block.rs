use std::sync::Arc;

use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, StructureBlockLikeProperties};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::{GameMode, PermissionLvl};

use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::entities::structure_block::StructureBlockBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, CanPlaceAtArgs, NormalUseArgs, OnNeighborUpdateArgs, OnPlaceArgs, PlacedArgs,
    PlayerPlacedArgs,
};

/// `net.minecraft.world.level.block.StructureBlock`.
///
/// An operator-only block whose interesting behavior lives on `StructureBlockBlockEntity`. Only
/// the LOAD half is implemented here (see `StructureBlockBlockEntity::place_structure`); SAVE
/// needs filesystem-backed template storage that doesn't exist in this codebase yet and is left
/// as a documented follow-up.
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
        player.gamemode.load() == GameMode::Creative
            && player.permission_lvl.load() >= PermissionLvl::Two
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
        let author = args.player.gameprofile.name.clone();
        tokio::task::block_in_place(|| {
            *tokio::runtime::Handle::current().block_on(structure_block.author.lock()) = author;
        });
    }

    /// `StructureBlock.useWithoutItem`/`StructureBlockEntity.usedBy`: opens the editor GUI for
    /// operators. `StructureBlock` is a `GameMasterBlock` gated on permission level 2, following
    /// the same shape as `jigsaw.rs`'s `normal_use` (the jigsaw port's additional creative-mode
    /// check has no vanilla citation and is deliberately not copied here, per the design doc).
    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
            if args.player.permission_lvl.load() < PermissionLvl::Two {
                return BlockActionResult::Pass;
            }
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return BlockActionResult::Pass;
            };
            args.world.update_block_entity(&block_entity);
            BlockActionResult::Success
    }

    /// `StructureBlock.neighborChanged` (`StructureBlock.java:67-97`): on a redstone signal,
    /// LOAD mode always places the named template (no size-match gating - that's
    /// `placeStructureIfSameSize`, only reachable from the GUI's load button, which requires the
    /// C2S structure-block packet this codebase doesn't have yet). SAVE captures the selected area
    /// in memory, CORNER evicts the named template, and DATA is a no-op here. SAVE has no
    /// filesystem-backed template writer to save to on this redstone path.
    ///
    /// Like `TNTBlock::on_neighbor_update` in this codebase, this checks the current redstone
    /// state rather than tracking a false-to-true edge, so a template can be re-placed on any
    /// neighbor update while already powered, not only on the rising edge.
    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        if !block_receives_redstone_power(args.world, args.position) {
                return;
        }
        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return;
        };
        let Some(structure_block) = block_entity
            .as_any()
            .downcast_ref::<StructureBlockBlockEntity>()
        else {
            return;
        };
        let mode = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(structure_block.mode.lock()).clone()
        });
        match mode.as_str() {
            "LOAD" => tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(structure_block.place_structure(args.world));
            }),
            "SAVE" => tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(structure_block.save_structure(args.world, false));
            }),
            "CORNER" => tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(structure_block.unload_structure());
            }),
            _ => {}
        }
    }
}
