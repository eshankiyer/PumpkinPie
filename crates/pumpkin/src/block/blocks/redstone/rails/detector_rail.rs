use pumpkin_data::BlockDirection;
use pumpkin_data::BlockStateId;
use pumpkin_data::entity::EntityType;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;

use crate::block::BlockBehaviour;
use crate::block::BlockFuture;
use crate::block::CanPlaceAtArgs;
use crate::block::EmitsRedstonePowerArgs;
use crate::block::GetComparatorOutputArgs;
use crate::block::GetRedstonePowerArgs;
use crate::block::OnEntityCollisionArgs;
use crate::block::OnNeighborUpdateArgs;
use crate::block::OnPlaceArgs;
use crate::block::OnScheduledTickArgs;
use crate::block::OnStateReplacedArgs;
use crate::block::PlacedArgs;
use crate::entity::EntityBase;
use crate::entity::vehicle::minecart::MinecartEntity;
use crate::world::World;
use pumpkin_data::Block;

use super::common::{
    can_place_rail_at, compute_placed_rail_shape, rail_placement_is_valid,
    update_flanking_rails_shape,
};
use super::{Rail, RailProperties, should_update_ascending_neighbor};

/// `DetectorRailBlock.PRESSED_CHECK_PERIOD` (DetectorRailBlock.java:33).
const PRESSED_CHECK_PERIOD: u8 = 20;

/// `DetectorRailBlock.getSearchBB` (DetectorRailBlock.java:167-170): the cart is looked for in a
/// box inset by 0.2 horizontally and capped at 0.8 vertically, not in the full block cube.
const SEARCH_BOX: BoundingBox = BoundingBox::new_array([0.2, 0.0, 0.2], [0.8, 0.8, 0.8]);

/// Vanilla looks for `AbstractMinecart`, which is exactly these seven entity types.
const fn is_minecart(entity_type: &'static EntityType) -> bool {
    let id = entity_type.id;
    id == EntityType::MINECART.id
        || id == EntityType::CHEST_MINECART.id
        || id == EntityType::FURNACE_MINECART.id
        || id == EntityType::TNT_MINECART.id
        || id == EntityType::HOPPER_MINECART.id
        || id == EntityType::COMMAND_BLOCK_MINECART.id
        || id == EntityType::SPAWNER_MINECART.id
}

/// `DetectorRailBlock.ownSignal` (DetectorRailBlock.java:51-53).
const fn own_signal(powered: bool) -> u8 {
    if powered { 15 } else { 0 }
}

/// `DetectorRailBlock.getDirectSignal` (DetectorRailBlock.java:74-80): only the block below a
/// pressed detector rail is strongly powered.
const fn direct_signal(powered: bool, direction: BlockDirection) -> u8 {
    if powered && matches!(direction, BlockDirection::Up) {
        15
    } else {
        0
    }
}

#[pumpkin_block("minecraft:detector_rail")]
pub struct DetectorRailBlock;

impl BlockBehaviour for DetectorRailBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut rail_props = RailProperties::default(args.block);
            let player_facing = args.player.get_entity().get_horizontal_facing();

            rail_props.set_waterlogged(args.replacing.water_source());
            rail_props.set_straight_shape(
                compute_placed_rail_shape(args.world, args.position, player_facing).await,
            );

            rail_props.to_state_id(args.block)
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            update_flanking_rails_shape(args.world, args.block, args.state_id, args.position).await;

            // `DetectorRailBlock.onPlace` (DetectorRailBlock.java:127-132) runs a pressure check
            // once the shape has settled, so a rail placed under a standing cart powers up.
            if Block::from_state_id(args.old_state_id) != args.block {
                Self::check_pressed(args.world, args.block, args.position).await;
            }
        })
    }

    fn on_entity_collision<'a>(&'a self, args: OnEntityCollisionArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // `DetectorRailBlock.entityInside` (DetectorRailBlock.java:56-64).
            if RailProperties::new(args.state.id, args.block).is_powered() {
                return;
            }
            Self::check_pressed(args.world, args.block, args.position).await;
        })
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // `DetectorRailBlock.tick` (DetectorRailBlock.java:67-71).
            let state_id = args.world.get_block_state_id(args.position);
            if !RailProperties::new(state_id, args.block).is_powered() {
                return;
            }
            Self::check_pressed(args.world, args.block, args.position).await;
        })
    }

    fn on_state_replaced<'a>(&'a self, args: OnStateReplacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // `BaseRailBlock.affectNeighborsAfterRemoval` (`BaseRailBlock.java:121-132`)
            // updates the block above a removed ascending detector rail. DetectorRailBlock is
            // not straight.
            let rail_props = RailProperties::new(args.old_state_id, args.block);
            if should_update_ascending_neighbor(args.moved, rail_props.shape()) {
                args.world
                    .update_neighbor(&args.position.up(), args.block)
                    .await;
            }
        })
    }

    fn emits_redstone_power<'a>(
        &'a self,
        _args: EmitsRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, bool> {
        Box::pin(async move { true })
    }

    fn get_weak_redstone_power<'a>(
        &'a self,
        args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(
            async move { own_signal(RailProperties::new(args.state.id, args.block).is_powered()) },
        )
    }

    fn get_strong_redstone_power<'a>(
        &'a self,
        args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(async move {
            direct_signal(
                RailProperties::new(args.state.id, args.block).is_powered(),
                args.direction,
            )
        })
    }

    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        // `DetectorRailBlock.hasAnalogOutputSignal` and `getAnalogOutputSignal`
        // (DetectorRailBlock.java:140-159) read the first command cart, then the first container
        // cart, and return zero for every other cart.
        Box::pin(async move {
            let carts = args
                .world
                .get_entities_at_box(&SEARCH_BOX.at_pos(*args.position));

            for entity in &carts {
                if entity.get_entity().entity_type.id != EntityType::COMMAND_BLOCK_MINECART.id {
                    continue;
                }
                if let Some(minecart) = entity.cast_any().downcast_ref::<MinecartEntity>() {
                    return Some(minecart.comparator_output().await);
                }
            }

            for entity in carts {
                let entity_type = entity.get_entity().entity_type.id;
                if entity_type != EntityType::CHEST_MINECART.id
                    && entity_type != EntityType::HOPPER_MINECART.id
                {
                    continue;
                }
                if let Some(minecart) = entity.cast_any().downcast_ref::<MinecartEntity>() {
                    return Some(minecart.comparator_output().await);
                }
            }

            Some(0)
        })
    }

    fn on_neighbor_update<'a>(&'a self, args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !rail_placement_is_valid(args.world, args.block, args.position).await {
                args.world
                    .break_block(args.position, None, BlockFlags::NOTIFY_ALL)
                    .await;
            }
        })
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_place_rail_at(args.block_accessor, args.position)
    }
}

impl DetectorRailBlock {
    /// `DetectorRailBlock.checkPressed` (DetectorRailBlock.java:82-115).
    async fn check_pressed(world: &Arc<World>, block: &Block, pos: &BlockPos) {
        if !rail_placement_is_valid(world, block, pos).await {
            return;
        }

        let state_id = world.get_block_state_id(pos);
        let mut props = RailProperties::new(state_id, block);
        let was_pressed = props.is_powered();
        let should_be_pressed = world
            .get_entities_at_box(&SEARCH_BOX.at_pos(*pos))
            .into_iter()
            .any(|entity| is_minecart(entity.get_entity().entity_type));

        if should_be_pressed != was_pressed {
            props.set_powered(should_be_pressed);
            world
                .set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
                .await;
            Self::update_power_to_connected(world, block, pos, &props).await;
            world.update_neighbors(pos, None).await;
            world.update_neighbors(&pos.down(), None).await;
        }

        if should_be_pressed {
            world.schedule_block_tick(block, *pos, PRESSED_CHECK_PERIOD, TickPriority::Normal);
        }
    }

    /// `DetectorRailBlock.updatePowerToConnected` (DetectorRailBlock.java:117-124): the rails this
    /// one connects to get a neighbour update, so a powered rail chain reacts on the same tick.
    async fn update_power_to_connected(
        world: &Arc<World>,
        block: &Block,
        pos: &BlockPos,
        props: &RailProperties,
    ) {
        for direction in props.directions() {
            if let Some(rail) = Rail::find_with_elevation(world, pos.offset(direction.to_offset()))
            {
                world.update_neighbor(&rail.position, block).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PRESSED_CHECK_PERIOD, SEARCH_BOX, direct_signal, is_minecart, own_signal};
    use pumpkin_data::BlockDirection;
    use pumpkin_data::entity::EntityType;

    #[test]
    fn pressed_detector_rail_emits_exactly_fifteen() {
        assert_eq!(own_signal(true), 15);
        assert_eq!(own_signal(false), 0);
    }

    #[test]
    fn only_the_block_below_is_strongly_powered() {
        assert_eq!(direct_signal(true, BlockDirection::Up), 15);
        for direction in [
            BlockDirection::Down,
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::East,
            BlockDirection::West,
        ] {
            assert_eq!(direct_signal(true, direction), 0);
        }
        assert_eq!(direct_signal(false, BlockDirection::Up), 0);
    }

    #[test]
    fn recheck_period_is_twenty_ticks() {
        assert_eq!(PRESSED_CHECK_PERIOD, 20);
    }

    #[test]
    fn search_box_is_inset_like_vanilla() {
        assert!((SEARCH_BOX.min.x - 0.2).abs() < f64::EPSILON);
        assert!((SEARCH_BOX.max.x - 0.8).abs() < f64::EPSILON);
        assert!((SEARCH_BOX.min.y - 0.0).abs() < f64::EPSILON);
        assert!((SEARCH_BOX.max.y - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn every_minecart_variant_presses_the_rail() {
        for entity_type in [
            &EntityType::MINECART,
            &EntityType::CHEST_MINECART,
            &EntityType::FURNACE_MINECART,
            &EntityType::TNT_MINECART,
            &EntityType::HOPPER_MINECART,
            &EntityType::COMMAND_BLOCK_MINECART,
            &EntityType::SPAWNER_MINECART,
        ] {
            assert!(is_minecart(entity_type));
        }
        assert!(!is_minecart(&EntityType::PIG));
        assert!(!is_minecart(&EntityType::ITEM));
        assert!(!is_minecart(&EntityType::OAK_BOAT));
    }
}
