use pumpkin_data::BlockId;
use pumpkin_data::block_properties::{
    BlockProperties, EnumVariants, TestBlockLikeProperties, TestBlockMode,
};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::BlockStateImpl;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::{GameMode, PermissionLvl};

use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::entities::test_block::TestBlockBlockEntity;
use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, CanPlaceAtArgs, EmitsRedstonePowerArgs,
    GetCloneItemStackArgs, GetRedstonePowerArgs, NormalUseArgs, OnNeighborUpdateArgs,
    OnScheduledTickArgs, registry::BlockActionResult,
};

/// `net.minecraft.world.level.block.TestBlock`.
pub struct TestBlock;

impl BlockMetadata for TestBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::TEST_BLOCK].into()
    }
}

impl BlockBehaviour for TestBlock {
    /// Vanilla `GameMasterBlockItem.getPlacementState` (GameMasterBlockItem.java:15-18): the
    /// test block item is a `GameMasterBlockItem`, so a player who cannot use game-master
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

    /// `TestBlock.tick`: the scheduled tick resets the block entity.
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if let Some(entity) = args.world.get_block_entity(args.position)
                && let Some(test_block) = entity.as_any().downcast_ref::<TestBlockBlockEntity>()
            {
                test_block.reset(args.world).await;
            }
        })
    }

    /// `TestBlock.neighborChanged`: a rising redstone edge triggers non-START blocks; a
    /// falling edge only clears `powered`. START-mode blocks are driven by `trigger()`
    /// from the test framework instead, so they ignore neighbour signal entirely.
    fn on_neighbor_update<'a>(&'a self, args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let Some(entity) = args.world.get_block_entity(args.position) else {
                return;
            };
            let Some(test_block) = entity.as_any().downcast_ref::<TestBlockBlockEntity>() else {
                return;
            };
            if test_block.get_mode().await == TestBlockMode::Start {
                return;
            }
            let should_trigger = block_receives_redstone_power(args.world, args.position).await;
            let is_powered = test_block.is_powered();
            if should_trigger && !is_powered {
                test_block.set_powered(true);
                test_block.trigger(args.world).await;
            } else if !should_trigger && is_powered {
                test_block.set_powered(false);
            }
        })
    }

    fn emits_redstone_power<'a>(
        &'a self,
        _args: EmitsRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, bool> {
        Box::pin(async move { true })
    }

    /// `TestBlock.ownSignal`: only a powered START block emits, and it emits full strength.
    fn get_weak_redstone_power<'a>(
        &'a self,
        args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(async move {
            let props = TestBlockLikeProperties::from_state_id(args.state.id, args.block);
            if props.mode != TestBlockMode::Start {
                return 0;
            }
            let powered = args
                .world
                .get_block_entity(args.position)
                .is_some_and(|entity| {
                    entity
                        .as_any()
                        .downcast_ref::<TestBlockBlockEntity>()
                        .is_some_and(TestBlockBlockEntity::is_powered)
                });
            if powered { 15 } else { 0 }
        })
    }

    /// `TestBlock.useWithoutItem` (TestBlock.java:61-75): only a game master may open the test
    /// block; the server validates permissions and reports success while the client opens its
    /// own screen. `Player.canUseGameMasterBlocks` (Player.java:1863-1865) requires instabuild
    /// and permission level 2.
    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            if args.world.get_block_entity(args.position).is_none() {
                return BlockActionResult::Pass;
            }
            let instabuild = args.player.abilities.lock().await.creative;
            if !instabuild || args.player.permission_lvl.load() < PermissionLvl::Two {
                return BlockActionResult::Pass;
            }
            BlockActionResult::SuccessServer
        })
    }

    /// `TestBlock.getCloneItemStack` + `setModeOnStack` (TestBlock.java:122-130): pick-block
    /// carries the block's current mode on the item's `block_state` component so a re-placed
    /// copy restores it.
    fn get_clone_item_stack(&self, args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        let state = args.world.get_block_state(args.position);
        let props = TestBlockLikeProperties::from_state_id(state.id, args.block);
        let mut stack = ItemStack::new(1, Item::from_id(args.block.item_id)?);
        stack.patch.push((
            DataComponent::BlockState,
            Some(Box::new(BlockStateImpl {
                properties: std::borrow::Cow::Owned(vec![(
                    std::borrow::Cow::Borrowed("mode"),
                    std::borrow::Cow::Borrowed(props.mode.to_value()),
                )]),
            })),
        ));
        Some(stack)
    }
}
