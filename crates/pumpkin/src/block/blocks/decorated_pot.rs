use std::sync::Arc;

use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, DecoratedPotLikeProperties};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{DataComponentImpl, PotDecorationsImpl};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Enchantment, tag};
use pumpkin_macros::pumpkin_block;

use crate::block::entities::decorated_pot::{DecoratedPotBlockEntity, WobbleStyle};
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockFuture, BrokenArgs, GetCloneItemStackArgs, GetComparatorOutputArgs,
    NormalUseArgs, OnPlaceArgs, OnSyncedBlockEventArgs, PlacedArgs, PlayerWillDestroyArgs,
    UseWithItemArgs,
};
use pumpkin_world::world::BlockFlags;

#[pumpkin_block("minecraft:decorated_pot")]
pub struct DecoratedPotBlock;

impl BlockBehaviour for DecoratedPotBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props =
            DecoratedPotLikeProperties::from_state_id(args.block.default_state.id, args.block);
        props.facing = args
            .player
            .living_entity
            .entity
            .get_horizontal_facing()
            .opposite();
        props.waterlogged = args.replacing.water_source();
        props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        let entity = DecoratedPotBlockEntity::new(*args.position);
        args.world.add_block_entity(Arc::new(entity));
    }

    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        if args.item_stack.item_count == 0 {
            return self.normal_use(NormalUseArgs {
                server: args.server,
                world: args.world,
                block: args.block,
                position: args.position,
                player: args.player,
                hit: args.hit,
            });
        }

        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(pot_entity) = block_entity
                .as_any()
                .downcast_ref::<DecoratedPotBlockEntity>()
        {
            if pot_entity.try_insert_item(args.item_stack, 1) {
                // `DecoratedPotBlock.useItemOn` wobbles positively on a successful insert
                // (`DecoratedPotBlock.java:104`).
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(pot_entity.wobble(args.world, WobbleStyle::Positive));
                });
                args.world.play_sound(
                    Sound::BlockDecoratedPotInsert,
                    SoundCategory::Blocks,
                    &args.position.to_f64(),
                );
            } else {
                args.world.play_sound(
                    Sound::BlockDecoratedPotInsertFail,
                    SoundCategory::Blocks,
                    &args.position.to_f64(),
                );
            }
            return BlockActionResult::Success;
        }

        BlockActionResult::Pass
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        args.world.play_sound(
            Sound::BlockDecoratedPotInsertFail,
            SoundCategory::Blocks,
            &args.position.to_f64(),
        );
        // `DecoratedPotBlock.useWithoutItem` wobbles negatively on a failed interaction
        // (`DecoratedPotBlock.java:140`).
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(pot_entity) = block_entity.as_any().downcast_ref::<DecoratedPotBlockEntity>()
        {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(pot_entity.wobble(args.world, WobbleStyle::Negative));
            });
        }
        BlockActionResult::Success
    }

    /// `DecoratedPotBlockEntity.triggerEvent` (`DecoratedPotBlockEntity.java:167-175`): accept
    /// only the pot-wobble event so the world broadcasts it to clients.
    fn on_synced_block_event(&self, args: OnSyncedBlockEventArgs<'_>) -> bool {
        args.r#type == DecoratedPotBlockEntity::EVENT_POT_WOBBLES && args.data < 2
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(pot_entity) = block_entity
                .as_any()
                .downcast_ref::<DecoratedPotBlockEntity>()
        {
            if let Some(contained) = pot_entity.take_item() {
                args.world.drop_stack(args.position, contained);
            }
            if let Some(decorations) = pot_entity.decorations() {
                for decoration in decorations {
                    if let Some(item) = Item::from_registry_key(
                        decoration.strip_prefix("minecraft:").unwrap_or(&decoration),
                    ) {
                        args.world.drop_stack(args.position, ItemStack::new(1, item));
                    }
                }
            }
        }

        args.world.play_sound(
            Sound::BlockDecoratedPotShatter,
            SoundCategory::Blocks,
            &args.position.to_f64(),
        );
        args.world
            .drop_stack(args.position, ItemStack::new(4, &Item::BRICK));
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(pot_entity) = block_entity
                .as_any()
                .downcast_ref::<DecoratedPotBlockEntity>()
        {
            Some(pot_entity.get_comparator_output())
        } else {
            Some(0)
        }
    }

    /// `DecoratedPotBlock.playerWillDestroy` (`DecoratedPotBlock.java:195-203`): tools in
    /// `#minecraft:breaks_decorated_pots` crack the state unless they have an enchantment in
    /// `#minecraft:prevents_decorated_pot_shattering` (currently Silk Touch).
    fn player_will_destroy<'a>(&'a self, args: PlayerWillDestroyArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let tool = args.player.inventory().held_item().await;
            if tool
                .item
                .has_tag(&tag::Item::MINECRAFT_BREAKS_DECORATED_POTS)
                && tool.get_enchantment_level(&Enchantment::SILK_TOUCH) == 0
            {
                let mut props =
                    DecoratedPotLikeProperties::from_state_id(args.state.id, args.block);
                props.cracked = true;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::empty(),
                    )
                    .await;
            }
        })
    }

    /// `DecoratedPotBlock.getCloneItemStack` (`DecoratedPotBlock.java:226-233`): creative
    /// pick-block preserves the pot-decoration component from the block entity.
    fn get_clone_item_stack(&self, args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        let entity = args.world.get_block_entity(args.position)?;
        let pot = entity.as_any().downcast_ref::<DecoratedPotBlockEntity>()?;
        let decorations = pot.decorations()?;
        Some(ItemStack::new_with_component(
            1,
            &Item::DECORATED_POT,
            vec![(
                DataComponent::PotDecorations,
                Some(Box::new(PotDecorationsImpl { decorations }).to_dyn()),
            )],
        ))
    }
}
