use crate::block::entities::test_instance_block::TestInstanceBlockBlockEntity;
use crate::block::{BlockActionResult, BlockBehaviour, NormalUseArgs};
use pumpkin_macros::pumpkin_block;

#[pumpkin_block("minecraft:test_instance_block")]
pub struct TestInstanceBlock;

impl BlockBehaviour for TestInstanceBlock {
    /// Vanilla `TestInstanceBlock.useWithoutItem` (`TestInstanceBlock.java:28-45`) only
    /// succeeds for its block entity and a player allowed to use game-master blocks.
    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return BlockActionResult::Pass;
        };
        if block_entity
            .as_any()
            .downcast_ref::<TestInstanceBlockBlockEntity>()
            .is_none()
        {
            return BlockActionResult::Pass;
        }
        if !args.player.can_use_game_master_blocks() {
            return BlockActionResult::Pass;
        }
        BlockActionResult::SuccessServer
    }
}
