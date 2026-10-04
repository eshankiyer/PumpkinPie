use std::sync::{Arc, atomic::Ordering};

use pumpkin_macros::pumpkin_block;

use crate::block::blocks::shelf::selectable_hit_slot;
use crate::block::entities::chiseled_bookshelf::ChiseledBookshelfBlockEntity;
use crate::{
    block::{
        BlockBehaviour, BlockFuture, BlockHitResult, GetCloneItemStackArgs,
        GetComparatorOutputArgs, NormalUseArgs, OnPlaceArgs, OnStateReplacedArgs, PlacedArgs,
        UseWithItemArgs, registry::BlockActionResult,
    },
    entity::{EntityBase, player::Player},
    world::World,
};
use pumpkin_data::{
    BlockStateId,
    block_properties::{BlockProperties, ChiseledBookshelfLikeProperties, HorizontalFacing},
    data_component::DataComponent,
    data_component_impl::{ContainerImpl, DataComponentImpl},
    item::Item,
    item_stack::ItemStack,
    sound::{Sound, SoundCategory},
    tag,
    tag::Taggable,
};
use pumpkin_inventory::screen_handler::InventoryPlayer;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::inventory::Inventory;

#[pumpkin_block("minecraft:chiseled_bookshelf")]
pub struct ChiseledBookshelfBlock;

// Vanilla `ItemContainerContents.fromItems` retains each occupied slot
// (`ChiseledBookShelfBlockEntity.java:129-132`).
fn occupied_items(items: &[ItemStack]) -> Vec<(u8, ItemStack)> {
    items
        .iter()
        .enumerate()
        .filter(|(_, stack)| !stack.is_empty())
        .map(|(slot, stack)| (slot as u8, stack.clone()))
        .collect()
}

impl BlockBehaviour for ChiseledBookshelfBlock {
    /// Vanilla `ChiseledBookShelfBlockEntity.collectImplicitComponents` stores the occupied
    /// slots in the picked item (`ChiseledBookShelfBlockEntity.java:122-132`); the live pick-item
    /// caller is `JavaClient::handle_pick_item_from_block` through `BlockRegistry::get_clone_item_stack`.
    fn get_clone_item_stack(&self, args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        let item = Item::from_id(args.block.item_id)?;
        let block_entity = args.world.get_block_entity(args.position)?;
        let bookshelf = block_entity
            .as_any()
            .downcast_ref::<ChiseledBookshelfBlockEntity>()?;
        let items = futures::executor::block_on(bookshelf.items.read());
        let items = occupied_items(items.as_slice());

        Some(if items.is_empty() {
            ItemStack::new(1, item)
        } else {
            ItemStack::new_with_component(
                1,
                item,
                vec![(
                    DataComponent::Container,
                    Some(Box::new(ContainerImpl { items }).to_dyn()),
                )],
            )
        })
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut properties = ChiseledBookshelfLikeProperties::default(args.block);

            // Face in the opposite direction the player is facing
            properties.facing = args.player.get_entity().get_horizontal_facing().opposite();

            properties.to_state_id(args.block)
        })
    }

    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let properties = ChiseledBookshelfLikeProperties::from_state_id(state.id, args.block);

            if let Some(slot) = Self::get_slot_for_hit(args.hit, properties.facing) {
                if Self::is_slot_used(properties, slot) {
                    if let Some(block_entity) = args.world.get_block_entity(args.position)
                        && let Some(block_entity) = block_entity
                            .as_any()
                            .downcast_ref::<ChiseledBookshelfBlockEntity>()
                    {
                        Self::try_remove_book(
                            args.world,
                            args.player,
                            args.position,
                            block_entity,
                            slot,
                        )
                        .await;
                        return BlockActionResult::Success;
                    }
                } else {
                    return BlockActionResult::Consume;
                }
            }
            BlockActionResult::Pass
        })
    }

    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let properties = ChiseledBookshelfLikeProperties::from_state_id(state.id, args.block);

            if !args
                .item_stack
                .get_item()
                .has_tag(&tag::Item::MINECRAFT_BOOKSHELF_BOOKS)
            {
                return BlockActionResult::PassToDefaultBlockAction;
            }
            if let Some(slot) = Self::get_slot_for_hit(args.hit, properties.facing) {
                if Self::is_slot_used(properties, slot) {
                    return BlockActionResult::PassToDefaultBlockAction;
                } else if let Some(block_entity) = args.world.get_block_entity(args.position)
                    && let Some(block_entity) = block_entity
                        .as_any()
                        .downcast_ref::<ChiseledBookshelfBlockEntity>()
                {
                    Self::try_add_book(
                        args.world,
                        args.player,
                        args.position,
                        block_entity,
                        slot,
                        args.item_stack,
                    )
                    .await;
                    return BlockActionResult::Success;
                }
            }

            BlockActionResult::Pass
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let block_entity = ChiseledBookshelfBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(block_entity));
        })
    }

    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            if let Some(block_entity) = args.world.get_block_entity(args.position)
                && let Some(block_entity) = block_entity
                    .as_any()
                    .downcast_ref::<ChiseledBookshelfBlockEntity>()
            {
                return Some((block_entity.last_interacted_slot.load(Ordering::Relaxed) + 1) as u8);
            }
            None
        })
    }

    fn on_state_replaced<'a>(&'a self, args: OnStateReplacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // Vanilla ChiseledBookShelfBlock.affectNeighborsAfterRemoval
            // (ChiseledBookShelfBlock.java:165-168) notifies comparator outputs with the old block.
            args.world
                .update_comparators(args.position, args.block)
                .await;
        })
    }
}

impl ChiseledBookshelfBlock {
    /// Runs the `updateState` vanilla's `removeItem`/`setItem` perform after a hopper changed a
    /// slot through the raw container (`ChiseledBookShelfBlockEntity.java:78-97`).
    pub(crate) async fn refresh_after_inventory_transfer(world: &Arc<World>, position: &BlockPos) {
        let Some(block_entity) = world.get_block_entity(position) else {
            return;
        };
        if let Some(bookshelf) = block_entity
            .as_any()
            .downcast_ref::<ChiseledBookshelfBlockEntity>()
        {
            bookshelf.refresh_pending(world).await;
        }
    }

    async fn try_add_book(
        world: &Arc<World>,
        player: &Player,
        position: &BlockPos,
        entity: &ChiseledBookshelfBlockEntity,
        slot: i8,
        item: &mut ItemStack,
    ) {
        player
            .increment_stat(
                pumpkin_data::statistic::StatisticCategory::Used,
                item.item.id as i32,
                1,
            )
            .await;
        let sound = if item.get_item() == &Item::ENCHANTED_BOOK {
            Sound::BlockChiseledBookshelfPickupEnchanted
        } else {
            Sound::BlockChiseledBookshelfPickup
        };

        entity
            .set_stack(
                slot as usize,
                item.split_unless_creative(player.gamemode.load(), 1),
            )
            .await;
        entity.update_state(world, slot as usize).await;

        world.play_sound(sound, SoundCategory::Blocks, &position.to_centered_f64());
    }

    async fn try_remove_book(
        world: &Arc<World>,
        player: &Player,
        position: &BlockPos,
        entity: &ChiseledBookshelfBlockEntity,
        slot: i8,
    ) {
        let mut stack = entity.remove_stack_specific(slot as usize, 1).await;

        let sound = if stack.get_item() == &Item::ENCHANTED_BOOK {
            Sound::BlockChiseledBookshelfPickupEnchanted
        } else {
            Sound::BlockChiseledBookshelfPickup
        };

        if !player
            .get_inventory()
            .insert_stack_anywhere(&mut stack)
            .await
        {
            // Drop the item on the ground if the player cannot hold it because of a full inventory
            player.drop_item(stack).await;
        }
        entity.update_state(world, slot as usize).await;

        world.play_sound(sound, SoundCategory::Blocks, &position.to_centered_f64());
    }

    /// `ChiseledBookShelfBlock` is a `SelectableSlotContainer` with `getRows() = 2` and
    /// `getColumns() = 3` (`ChiseledBookShelfBlock.java:54-62`).
    fn get_slot_for_hit(hit: &BlockHitResult<'_>, facing: HorizontalFacing) -> Option<i8> {
        selectable_hit_slot(hit, facing, Self::ROWS, Self::COLUMNS).map(|slot| slot as i8)
    }

    const ROWS: i32 = 2;
    const COLUMNS: i32 = 3;

    const fn is_slot_used(properties: ChiseledBookshelfLikeProperties, slot: i8) -> bool {
        match slot {
            0 => properties.slot_0_occupied,
            1 => properties.slot_1_occupied,
            2 => properties.slot_2_occupied,
            3 => properties.slot_3_occupied,
            4 => properties.slot_4_occupied,
            5 => properties.slot_5_occupied,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occupied_items_preserves_bookshelf_slots() {
        let mut items: [ItemStack; ChiseledBookshelfBlockEntity::INVENTORY_SIZE] =
            std::array::from_fn(|_| ItemStack::EMPTY.clone());
        items[4] = ItemStack::new(1, &Item::BOOK);

        let occupied = occupied_items(&items);

        assert_eq!(occupied.len(), 1);
        assert_eq!(occupied[0].0, 4);
        assert_eq!(occupied[0].1.get_item().id, Item::BOOK.id);
    }

    #[test]
    fn hit_slot_uses_even_selectable_slot_sections() {
        use crate::block::blocks::shelf::hit_slot_from_coordinates;
        let slot = |x, y| {
            hit_slot_from_coordinates(
                x,
                y,
                ChiseledBookshelfBlock::ROWS,
                ChiseledBookshelfBlock::COLUMNS,
            )
        };
        assert_eq!(slot(0.34, 0.9), 1);
        assert_eq!(slot(0.67, 0.9), 2);
        assert_eq!(slot(0.1, 0.5), 3);
        assert_eq!(slot(0.99, 0.01), 5);
    }
}
