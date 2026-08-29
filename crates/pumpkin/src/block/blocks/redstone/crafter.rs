use rand::{RngExt, rng};
use std::sync::Arc;
use std::sync::Mutex;

use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::entities::PropertyDelegate;
use crate::block::entities::crafter::CrafterBlockEntity;
use crate::block::entities::hopper::HopperBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, GetComparatorOutputArgs, NormalUseArgs, OnNeighborUpdateArgs, OnPlaceArgs,
    OnScheduledTickArgs, PlacedArgs,
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
use pumpkin_data::{Block, BlockDirection, BlockId, BlockStateId};
use pumpkin_inventory::crafting::crafting_screen_handler::match_crafting_recipe;
use pumpkin_inventory::generic_container_screen_handler::create_crafter_3x3;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::Inventory;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

struct CrafterScreenFactory {
    inventory: Arc<dyn Inventory>,
    // `CrafterMenu.java:31-39`: the menu is backed by the crafter inventory and its
    // ten-entry data delegate; the synchronous upstream factory creates the same 3x3 menu.
    properties: Arc<dyn PropertyDelegate>,
}

impl ScreenHandlerFactory for CrafterScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        _player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        // `CrafterMenu.java:18` creates a fresh result container, while
        // `CrafterMenu.refreshRecipeResult` (`CrafterMenu.java:106-113`) refreshes its preview.
        let handler = create_crafter_3x3(sync_id, player_inventory, self.inventory.clone());
        let screen_handler_arc = Arc::new(Mutex::new(handler));
        let _ = &self.properties;
        Some(screen_handler_arc as SharedScreenHandler)
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
    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(inventory) = block_entity.get_inventory()
            && let Some(properties) = block_entity.to_property_delegate()
        {
            args.player.open_handled_screen(
                &CrafterScreenFactory {
                    inventory,
                    properties,
                },
                Some(*args.position),
            );
        }
        BlockActionResult::Success
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = CrafterLikeProperties::default(args.block);
        let horizontal = args.player.living_entity.entity.get_horizontal_facing();
        props.orientation = match args.direction {
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
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        args.world
            .add_block_entity(Arc::new(CrafterBlockEntity::new(*args.position)));
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        let powered = block_receives_redstone_power(args.world, args.position);
        let mut props = CrafterLikeProperties::from_state_id(
            args.world.get_block_state(args.position).id,
            args.block,
        );

        // `CrafterBlock.setBlockEntityTriggered` (`CrafterBlock.java:100-104`) keeps the
        // menu's powered property (`CrafterMenu.java:66-68`) synchronized with the block.
        let set_block_entity_triggered = |triggered: bool| {
            if let Some(block_entity) = args.world.get_block_entity(args.position)
                && let Some(crafter) = block_entity
                    .as_any()
                    .downcast_ref::<CrafterBlockEntity>()
            {
                crafter.set_triggered(triggered);
            }
        };

        if powered && !props.triggered {
            props.triggered = true;
            set_block_entity_triggered(true);
            args.world
                .schedule_block_tick(args.block, *args.position, 4, TickPriority::Normal);
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_LISTENERS,
            );
        } else if !powered && props.triggered {
            props.triggered = false;
            // `CrafterBlock.java:85` clears CRAFTING in the same setBlock.
            props.crafting = false;
            set_block_entity_triggered(false);
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_LISTENERS,
            );
        }
    }

    /// Vanilla `CrafterBlock.tick` -> `dispenseFrom` (`CrafterBlock.java:90-93`,
    /// `CrafterBlock.java:150-182`).
    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let world = args.world.clone();
        let pos = *args.position;
        let block = Block::from_id(args.block.id);
        tokio::spawn(async move { craft_crafter(world, pos, block).await });
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        let block_entity = args.world.get_block_entity(args.position)?;
        let crafter = block_entity.as_any().downcast_ref::<CrafterBlockEntity>()?;
        let items = crafter
            .items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut occupied = 0u8;
        for (index, stack) in items.iter().enumerate().take(9) {
            if !stack.is_empty() || crafter.is_slot_disabled(index) {
                occupied += 1;
            }
        }
        Some(occupied)
    }
}

async fn craft_crafter(world: Arc<World>, position: BlockPos, block: &'static Block) {
    let Some(block_entity) = world.get_block_entity(&position) else {
        return;
    };
    let Some(crafter) = block_entity.as_any().downcast_ref::<CrafterBlockEntity>() else {
        return;
    };
    let provider = world
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
    // `CrafterBlock.java:154-160`: no recipe, or a recipe assembling to nothing, produces
    // the fail event and leaves the crafter's inventory/state unchanged.
    let Some(matched) = matched else {
        world.sync_world_event(WorldEvent::SoundCrafterFail, position, 0);
        return;
    };
    let result = matched.to_item_stack();
    if result.is_empty() {
        world.sync_world_event(WorldEvent::SoundCrafterFail, position, 0);
        return;
    }

    // `CrafterBlock.java:162-163`.
    crafter.set_crafting_ticks_remaining(CrafterBlock::MAX_CRAFTING_TICKS);
    let state_id = world.get_block_state_id(&position);
    let mut props = pumpkin_data::block_properties::CrafterLikeProperties::from_state_id(
        state_id, block,
    );
    props.crafting = true;
    world.set_block_state(
        &position,
        props.to_state_id(block),
        BlockFlags::NOTIFY_LISTENERS,
    );
    let front = front_direction(props.orientation);

    // `CrafterBlock.java:167-171`: `Recipe.getRemainingItems` is collected before ingredients
    // shrink and dispensed after the result.
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

    dispense_item(&world, &position, crafter, result, front).await;
    for remainder in remainders {
        dispense_item(&world, &position, crafter, remainder, front).await;
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

/// Vanilla `CrafterBlock.dispenseItem` (`CrafterBlock.java:188-231`): push the stack into the
/// container the crafter faces, and throw whatever will not fit.
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
    world.sync_world_event(WorldEvent::SoundCrafterCraft, *position, 0);
    world.sync_world_event(
        WorldEvent::ParticlesShootWhiteSmoke,
        *position,
        direction_3d_data(front),
    );
}

fn triangle(min: f64, max: f64) -> f64 {
    let mut random = rng();
    (random.random::<f64>() - random.random::<f64>()).mul_add(max, min)
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
        assert_eq!(front_direction(Orientation::DownNorth), BlockDirection::Down);
        assert_eq!(front_direction(Orientation::DownWest), BlockDirection::Down);
        assert_eq!(front_direction(Orientation::UpEast), BlockDirection::Up);
        assert_eq!(front_direction(Orientation::NorthUp), BlockDirection::North);
        assert_eq!(front_direction(Orientation::SouthUp), BlockDirection::South);
        assert_eq!(front_direction(Orientation::WestUp), BlockDirection::West);
        assert_eq!(front_direction(Orientation::EastUp), BlockDirection::East);
    }

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
