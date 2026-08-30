use pumpkin_data::block_properties::{BlockProperties, LightLikeProperties};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::BlockStateImpl;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::pumpkin_block;
use pumpkin_world::world::BlockFlags;

use crate::block::registry::BlockActionResult;
use crate::block::{BlockBehaviour, BlockFuture, GetCloneItemStackArgs, NormalUseArgs};

/// `LightBlock.MAX_LEVEL` (`net/minecraft/world/level/block/LightBlock.java:33`).
const MAX_LEVEL: u8 = 15;

/// `LightBlock` (`net/minecraft/world/level/block/LightBlock.java:31`).
///
/// The only server-side behaviour is the right-click that cycles `LEVEL`
/// (LightBlock.java:53-63). Everything else on the class is client-side or data-driven here:
/// `getShape`/`getRenderShape`/`getShadeBrightness` are rendering, `LIGHT_EMISSION` is the
/// generated per-state luminance, and `propagatesSkylightDown`/`getFluidState` follow from the
/// `WATERLOGGED` property.
#[pumpkin_block("minecraft:light")]
pub struct LightBlock;

/// `BlockState#cycle(LEVEL)` over `BlockStateProperties.LEVEL` (0..=15): increment, wrapping
/// past the maximum back to the minimum.
const fn cycle_level(level: u8) -> u8 {
    if level >= MAX_LEVEL { 0 } else { level + 1 }
}

impl BlockBehaviour for LightBlock {
    /// `LightBlock#useWithoutItem` (LightBlock.java:53-63).
    ///
    /// Vanilla gates on `Player#canUseGameMasterBlocks`
    /// (`net/minecraft/world/entity/player/Player.java:1863-1865`), which is creative mode plus
    /// the game-master permission; this codebase spells that as creative plus permission level
    /// two, the same pairing `jigsaw.rs` uses. A player who fails the gate gets `CONSUME`, not
    /// `PASS`, so the click is swallowed rather than falling through to a block placement.
    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            // `LightBlock.useWithoutItem` requires `Player.canUseGameMasterBlocks`
            // (`LightBlock.java:53-63`; `Player.java:1863-1865`).
            if !args.player.can_use_game_master_blocks() {
                return BlockActionResult::Consume;
            }

            let state_id = args.world.get_block_state_id(args.position);
            let mut props = LightLikeProperties::from_state_id(state_id, args.block);
            props.level = cycle_level(props.level);

            // Vanilla passes flag 2 (`NOTIFY_LISTENERS` only): no neighbour updates.
            args.world
                .set_block_state(
                    args.position,
                    props.to_state_id(args.block),
                    BlockFlags::NOTIFY_LISTENERS,
                )
                .await;
            BlockActionResult::SuccessServer
        })
    }

    /// `LightBlock.getCloneItemStack`/`setLightOnStack` (LightBlock.java:108-115): creative
    /// pick-block preserves the block's current light level on the returned item.
    fn get_clone_item_stack(&self, args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        let state = args.world.get_block_state(args.position);
        let props = LightLikeProperties::from_state_id(state.id, args.block);
        let mut stack = ItemStack::new(1, Item::from_id(args.block.item_id)?);
        stack.patch.push((
            DataComponent::BlockState,
            Some(Box::new(BlockStateImpl {
                properties: std::borrow::Cow::Owned(vec![(
                    std::borrow::Cow::Borrowed("level"),
                    std::borrow::Cow::Owned(props.level.to_string()),
                )]),
            })),
        ));
        Some(stack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::Block;

    #[test]
    fn cycle_wraps_at_max_level() {
        assert_eq!(cycle_level(0), 1);
        assert_eq!(cycle_level(14), 15);
        assert_eq!(cycle_level(MAX_LEVEL), 0);
    }

    /// `LightBlock` declares `LEVEL` (16 values) and `WATERLOGGED` (2), and defaults to
    /// `LEVEL=15, WATERLOGGED=false` (LightBlock.java:43-51).
    #[test]
    fn light_default_state_is_full_bright_and_dry() {
        assert_eq!(Block::LIGHT.states.len(), 32);
        let props =
            LightLikeProperties::from_state_id(Block::LIGHT.default_state.id, &Block::LIGHT);
        assert_eq!(props.level, MAX_LEVEL);
        assert!(!props.waterlogged);
    }
}
