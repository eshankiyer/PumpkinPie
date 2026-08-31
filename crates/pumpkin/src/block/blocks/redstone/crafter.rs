use rand::{RngExt, rng};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::entities::PropertyDelegate;
use crate::block::entities::crafter::CrafterBlockEntity;
use crate::block::entities::hopper::HopperBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockFuture, GetComparatorOutputArgs, NormalUseArgs, OnNeighborUpdateArgs,
    OnPlaceArgs, OnScheduledTickArgs, OnStateReplacedArgs, PlacedArgs,
};
use crate::entity::Entity;
use crate::entity::item::ItemEntity;
use crate::world::World;
use pumpkin_data::block_properties::{
    BlockProperties, CrafterLikeProperties, HorizontalFacing, Orientation,
};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipe_remainder::get_recipe_remainder_id;
use pumpkin_data::translation;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{BlockDirection, BlockStateId};
use pumpkin_inventory::crafter_screen_handler::CrafterScreenHandler;
use pumpkin_inventory::crafting::crafting_screen_handler::match_crafting_recipe;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    BoxFuture, InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::{Inventory, SimpleInventory};
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

struct CrafterScreenFactory {
    inventory: Arc<dyn Inventory>,
    properties: Arc<dyn PropertyDelegate>,
}

impl ScreenHandlerFactory for CrafterScreenFactory {
    fn create_screen_handler<'a>(
        &'a self,
        sync_id: u8,
        player_inventory: &'a Arc<PlayerInventory>,
        _player: &'a dyn InventoryPlayer,
    ) -> BoxFuture<'a, Option<SharedScreenHandler>> {
        Box::pin(async move {
            // `CrafterMenu.java:31-39`: the block-entity menu takes the crafter's
            // `CraftingContainer` plus its ten-entry `ContainerData`, and makes a fresh
            // `ResultContainer` for the non-interactive recipe preview
            // (`CrafterMenu.java:18`). `CrafterScreenHandler::refresh_recipe_result`
            // (`CrafterMenu.refreshRecipeResult`, `CrafterMenu.java:106-113`) populates that
            // preview slot from the same `match_crafting_recipe` this file's own
            // redstone-triggered crafting uses.
            let handler = CrafterScreenHandler::new(
                sync_id,
                player_inventory,
                self.inventory.clone(),
                Arc::new(SimpleInventory::new(1)),
                self.properties.clone(),
            )
            .await;
            let screen_handler_arc = Arc::new(Mutex::new(handler));

            Some(screen_handler_arc as SharedScreenHandler)
        })
    }

    fn get_display_name(&self) -> TextComponent {
        pumpkin_macros::translate_cross!(
            translation::java::CONTAINER_CRAFTER,
            translation::bedrock::CONTAINER_CRAFTER
        )
    }
}

#[pumpkin_block("minecraft:crafter")]
pub struct CrafterBlock;

impl CrafterBlock {
    /// `CrafterBlock.MAX_CRAFTING_TICKS` (`CrafterBlock.java:48`).
    const MAX_CRAFTING_TICKS: i32 = 6;
}

impl BlockBehaviour for CrafterBlock {
    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            if let Some(block_entity) = args.world.get_block_entity(args.position)
                && let Some(inventory) = block_entity.clone().get_inventory()
                && let Some(properties) = block_entity.to_property_delegate()
            {
                args.player
                    .open_handled_screen(
                        &CrafterScreenFactory {
                            inventory,
                            properties,
                        },
                        Some(*args.position),
                    )
                    .await;
            }
            BlockActionResult::Success
        })
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = CrafterLikeProperties::default(args.block);
            let facing = args.direction;
            let horizontal = args.player.living_entity.entity.get_horizontal_facing();
            props.orientation = match facing {
                BlockDirection::Down => match horizontal {
                    HorizontalFacing::North => Orientation::DownNorth,
                    HorizontalFacing::South => Orientation::DownSouth,
                    HorizontalFacing::East => Orientation::DownEast,
                    HorizontalFacing::West => Orientation::DownWest,
                },
                BlockDirection::Up => match horizontal {
                    HorizontalFacing::North => Orientation::UpNorth,
                    HorizontalFacing::South => Orientation::UpSouth,
                    HorizontalFacing::East => Orientation::UpEast,
                    HorizontalFacing::West => Orientation::UpWest,
                },
                BlockDirection::North => Orientation::NorthUp,
                BlockDirection::South => Orientation::SouthUp,
                BlockDirection::East => Orientation::EastUp,
                BlockDirection::West => Orientation::WestUp,
            };
            props.to_state_id(args.block)
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let crafter_block_entity = CrafterBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(crafter_block_entity));
        })
    }

    fn on_state_replaced<'a>(&'a self, args: OnStateReplacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // `CrafterBlock.affectNeighborsAfterRemoval` (`CrafterBlock.java:135-138`)
            // refreshes comparator inputs after the crafter is removed.
            args.world
                .update_comparators(args.position, args.block)
                .await;
        })
    }

    fn on_neighbor_update<'a>(&'a self, args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let powered = block_receives_redstone_power(args.world, args.position).await;
            let mut props = CrafterLikeProperties::from_state_id(
                args.world.get_block_state(args.position).id,
                args.block,
            );

            // `CrafterBlock.setBlockEntityTriggered` (`CrafterBlock.java:100-104`): the
            // block entity carries its own `triggered` flag, which is what feeds the menu's
            // `powered` property (`CrafterMenu.java:66-68`). Without this the open GUI never
            // sees a redstone change.
            let set_block_entity_triggered = |triggered: bool| {
                if let Some(block_entity) = args.world.get_block_entity(args.position)
                    && let Some(crafter) =
                        block_entity.as_any().downcast_ref::<CrafterBlockEntity>()
                {
                    crafter.set_triggered(triggered);
                }
            };

            if powered && !props.triggered {
                props.triggered = true;
                set_block_entity_triggered(true);
                args.world
                    .schedule_block_tick(args.block, *args.position, 4, TickPriority::Normal);
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_LISTENERS,
                    )
                    .await;
            } else if !powered && props.triggered {
                props.triggered = false;
                // `CrafterBlock.java:85` clears CRAFTING in the same setBlock.
                props.crafting = false;
                set_block_entity_triggered(false);
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_LISTENERS,
                    )
                    .await;
            }
        })
    }

    /// Vanilla `CrafterBlock.tick` -> `dispenseFrom` (`CrafterBlock.java:90-93`,
    /// `CrafterBlock.java:150-182`).
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return;
            };
            let Some(crafter) = block_entity.as_any().downcast_ref::<CrafterBlockEntity>() else {
                return;
            };

            let mut props = CrafterLikeProperties::from_state_id(
                args.world.get_block_state(args.position).id,
                args.block,
            );

            let provider = args
                .world
                .server
                .upgrade()
                .map(|server| server.recipe_manager.clone());
            let matched = match_crafting_recipe(
                crafter,
                provider.as_deref().map(|p| {
                    p as &dyn pumpkin_inventory::crafting::recipe_provider::RecipeProvider
                }),
            )
            .await;

            // `CrafterBlock.java:154-160`: no recipe, or a recipe that assembles to
            // nothing, both fall through to level event 1050 and no state change.
            let Some(matched) = matched else {
                args.world
                    .sync_world_event(WorldEvent::SoundCrafterFail, *args.position, 0);
                return;
            };
            let result = matched.to_item_stack();
            if result.is_empty() {
                args.world
                    .sync_world_event(WorldEvent::SoundCrafterFail, *args.position, 0);
                return;
            }

            // `CrafterBlock.dispenseFrom` invokes `ItemStack.onCraftedBySystem`
            // (`CrafterBlock.java:157-165`, `ItemStack.java:727-729`) before dispensing.
            let mut result = result;
            crate::world::map::process_crafted_map(&mut result, args.world).await;
            if result.is_empty() {
                args.world
                    .sync_world_event(WorldEvent::SoundCrafterFail, *args.position, 0);
                return;
            }

            // `CrafterBlock.java:162-163`.
            crafter.set_crafting_ticks_remaining(Self::MAX_CRAFTING_TICKS);
            props.crafting = true;
            args.world
                .set_block_state(
                    args.position,
                    props.to_state_id(args.block),
                    BlockFlags::NOTIFY_LISTENERS,
                )
                .await;

            let front = front_direction(props.orientation);

            // `CrafterBlock.java:167-171`: `Recipe.getRemainingItems` is the per-slot
            // crafting remainder table, overridden per recipe (e.g. book cloning).
            // Collected before the ingredients shrink, dispensed after the result.
            let mut remainders = Vec::new();
            for slot in 0..CrafterBlockEntity::INVENTORY_SIZE {
                let stack = crafter.get_stack(slot).await;
                if stack.is_empty() {
                    continue;
                }
                if let Some((_, item)) = matched
                    .remaining_items
                    .iter()
                    .find(|(index, item)| *index == slot && item.item == stack.item)
                {
                    remainders.push(item.clone());
                } else if let Some(item) = get_recipe_remainder_id(stack.item.id)
                    .and_then(pumpkin_data::item::Item::from_id)
                {
                    remainders.push(ItemStack::new(1, item));
                }
            }

            dispense_item(args.world, args.position, crafter, result, front).await;
            for remainder in remainders {
                dispense_item(args.world, args.position, crafter, remainder, front).await;
            }

            // `CrafterBlock.java:173-178`.
            for slot in 0..CrafterBlockEntity::INVENTORY_SIZE {
                let mut stack = crafter.get_stack(slot).await;
                if !stack.is_empty() {
                    stack.decrement(1);
                    crafter.set_stack(slot, stack).await;
                }
            }
            crafter.mark_dirty();
        })
    }

    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            if let Some(block_entity) = args.world.get_block_entity(args.position) {
                let crafter = block_entity.as_any().downcast_ref::<CrafterBlockEntity>()?;

                let mut occupied = 0u8;
                for i in 0..9 {
                    let stack = crafter.get_stack(i).await;
                    if !stack.is_empty() || crafter.is_slot_disabled(i) {
                        occupied += 1;
                    }
                }
                Some(occupied)
            } else {
                None
            }
        })
    }
}

/// `FrontAndTop.front()` for the crafter's `ORIENTATION` property.
const fn front_direction(orientation: Orientation) -> BlockDirection {
    match orientation {
        Orientation::DownEast
        | Orientation::DownNorth
        | Orientation::DownSouth
        | Orientation::DownWest => BlockDirection::Down,
        Orientation::UpEast | Orientation::UpNorth | Orientation::UpSouth | Orientation::UpWest => {
            BlockDirection::Up
        }
        Orientation::NorthUp => BlockDirection::North,
        Orientation::SouthUp => BlockDirection::South,
        Orientation::WestUp => BlockDirection::West,
        Orientation::EastUp => BlockDirection::East,
    }
}

/// `Direction.get3DDataValue`, the payload level event 2010 carries.
const fn direction_3d_data(direction: BlockDirection) -> i32 {
    match direction {
        BlockDirection::Down => 0,
        BlockDirection::Up => 1,
        BlockDirection::North => 2,
        BlockDirection::South => 3,
        BlockDirection::West => 4,
        BlockDirection::East => 5,
    }
}

/// Vanilla `CrafterBlock.dispenseItem` (`CrafterBlock.java:188-231`): push the stack into
/// the container the crafter faces, and throw whatever will not fit.
///
/// Vanilla resolves the destination with `HopperBlockEntity.getContainerAt`, which also
/// finds double chests and container *entities* (minecarts). This port reuses the same
/// block-entity-inventory lookup the hopper here uses, so those two cases are not covered.
/// Vanilla also inserts the whole stack at once for a non-crafter destination; inserting
/// one item at a time reaches the same end state.
async fn dispense_item(
    world: &Arc<World>,
    position: &BlockPos,
    crafter: &CrafterBlockEntity,
    result: ItemStack,
    front: BlockDirection,
) {
    let mut remaining = result;
    let target_position = position.offset(front.to_offset());
    let into = world
        .get_block_entity(&target_position)
        .and_then(crate::block::entities::BlockEntity::get_inventory);

    if let Some(into) = into {
        let target_face = front.opposite();
        let target_slots = into.slots_for_face(target_face);
        let mut insertable = Vec::new();
        for &slot in &target_slots {
            if into
                .can_insert_through_face(slot, &remaining, target_face)
                .await
            {
                insertable.push(slot);
            }
        }
        while !remaining.is_empty() {
            let mut copy = remaining.clone();
            let one = copy.split(1);
            if !HopperBlockEntity::add_one_item(crafter, into.as_ref(), one, &insertable).await {
                break;
            }
            remaining.decrement(1);
        }
    }

    if remaining.is_empty() {
        return;
    }

    // `DefaultDispenseItemBehavior.spawnItem` with accuracy 6.
    let offset = front.to_offset().to_f64() * 0.7;
    let mut spawn = position.to_centered_f64().add(&offset);
    spawn.y -= if matches!(front, BlockDirection::Up | BlockDirection::Down) {
        0.125
    } else {
        0.156_25
    };
    let step = front.to_offset();
    let power = rng().random::<f64>().mul_add(0.1, 0.2);
    let velocity = Vector3::new(
        triangle(f64::from(step.x) * power, 0.017_227_5 * 6.0),
        triangle(0.2, 0.017_227_5 * 6.0),
        triangle(f64::from(step.z) * power, 0.017_227_5 * 6.0),
    );
    let entity = Entity::new(world.clone(), spawn, &EntityType::ITEM);
    world
        .spawn_entity(Arc::new(ItemEntity::new_with_velocity(
            entity, remaining, velocity, 0,
        )))
        .await;

    // TODO: `CriteriaTriggers.CRAFTER_RECIPE_CRAFTED` for players within 17 blocks.
    world.sync_world_event(WorldEvent::SoundCrafterCraft, *position, 0);
    world.sync_world_event(
        WorldEvent::ParticlesShootWhiteSmoke,
        *position,
        direction_3d_data(front),
    );
}

fn triangle(min: f64, max: f64) -> f64 {
    let mut r = rng();
    (r.random::<f64>() - r.random::<f64>()).mul_add(max, min)
}

#[cfg(test)]
mod tests {
    use super::{direction_3d_data, front_direction};
    use pumpkin_data::BlockDirection;
    use pumpkin_data::block_properties::Orientation;

    /// `FrontAndTop` names front first, so every `Down*`/`Up*` variant faces that way and
    /// the four `*Up` variants face horizontally (`CrafterBlock.java:196`).
    #[test]
    fn orientation_front_is_the_first_half_of_the_name() {
        assert_eq!(
            front_direction(Orientation::DownNorth),
            BlockDirection::Down
        );
        assert_eq!(front_direction(Orientation::DownWest), BlockDirection::Down);
        assert_eq!(front_direction(Orientation::UpEast), BlockDirection::Up);
        assert_eq!(front_direction(Orientation::NorthUp), BlockDirection::North);
        assert_eq!(front_direction(Orientation::SouthUp), BlockDirection::South);
        assert_eq!(front_direction(Orientation::WestUp), BlockDirection::West);
        assert_eq!(front_direction(Orientation::EastUp), BlockDirection::East);
    }

    /// `Direction.get3DDataValue`, the payload of level event 2010.
    #[test]
    fn direction_data_matches_vanilla_ordering() {
        assert_eq!(direction_3d_data(BlockDirection::Down), 0);
        assert_eq!(direction_3d_data(BlockDirection::Up), 1);
        assert_eq!(direction_3d_data(BlockDirection::North), 2);
        assert_eq!(direction_3d_data(BlockDirection::South), 3);
        assert_eq!(direction_3d_data(BlockDirection::West), 4);
        assert_eq!(direction_3d_data(BlockDirection::East), 5);
    }
}
