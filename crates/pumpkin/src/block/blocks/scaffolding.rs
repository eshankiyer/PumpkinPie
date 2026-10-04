use pumpkin_data::fluid::Fluid;
use pumpkin_data::{
    Block, BlockDirection, BlockStateId, HorizontalFacingExt,
    block_properties::{BlockProperties, ScaffoldingLikeProperties},
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockAccessor;
use pumpkin_world::world::BlockFlags;

use crate::block::{
    BlockBehaviour, BlockFuture, CanPlaceAtArgs, GetStateForNeighborUpdateArgs, OnPlaceArgs,
    OnScheduledTickArgs, PlacedArgs,
};
use crate::entity::falling::FallingEntity;
use crate::entity::player::Player;
use crate::world::World;

/// `net.minecraft.world.level.block.ScaffoldingBlock`: distance-from-support propagation with a
/// tick-driven collapse into a falling-block entity when unsupported.
#[pumpkin_block("minecraft:scaffolding")]
pub struct ScaffoldingBlock;

const MAX_DISTANCE: u8 = 7;

/// Outcome of the pure part of `ScaffoldingBlockItem.updatePlacementContext`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlacementRedirect {
    /// Place at this position; the direction is the new context's clicked face.
    Place(BlockPos, BlockDirection),
    /// `updatePlacementContext` returned null without leaving the world.
    Refused,
    /// The redirect walked out of world bounds at this position (vanilla then returns null,
    /// sending the build-limit message when it left through the top).
    OutOfBounds(BlockPos),
}

impl ScaffoldingBlock {
    /// `ScaffoldingBlock.getInteractionShape` (`ScaffoldingBlock.java:67-69`) always returns the
    /// full block, even when the context-dependent collision shape is empty.
    pub(crate) fn interaction_shape_at(position: &BlockPos) -> BoundingBox {
        BoundingBox::full_block().shift(position.0.to_f64())
    }

    /// Returns the context-dependent collision pieces from
    /// `ScaffoldingBlock.getCollisionShape` (`ScaffoldingBlock.java:137-145`). The generated
    /// stable pieces already contain `SHAPE_STABLE`; the lower slab is the additional piece in
    /// `SHAPE_UNSTABLE_BOTTOM`.
    pub(crate) fn collision_shapes_for_context(
        state_id: BlockStateId,
        above_block: bool,
        above_below_block: bool,
        descending: bool,
    ) -> Vec<BoundingBox> {
        let state = pumpkin_data::BlockState::from_id(state_id);
        let props = ScaffoldingLikeProperties::from_state_id(state_id, &Block::SCAFFOLDING);

        if above_block && !descending {
            return state.get_block_collision_shapes().collect();
        }

        if props.r#distance != 0 && props.r#bottom && above_below_block {
            return vec![BoundingBox::new_array([0.0, 0.0, 0.0], [1.0, 0.125, 1.0])];
        }

        Vec::new()
    }

    /// `ScaffoldingBlock.getDistance` (`ScaffoldingBlock.java:158-179`).
    ///
    /// Faithful to a vanilla mutable-cursor quirk: the vertical check reads `pos.below()`, but
    /// the horizontal scan afterward is centered on `pos` itself (the cursor is reused via
    /// `setWithOffset(pos, direction)`, not advanced from `pos.below()`).
    pub(crate) fn get_distance(accessor: &dyn BlockAccessor, pos: &BlockPos) -> u8 {
        let below_pos = pos.down();
        let (below_block, below_state) = accessor.get_block_and_state(&below_pos);

        let mut distance = MAX_DISTANCE;
        if below_block == &Block::SCAFFOLDING {
            distance =
                ScaffoldingLikeProperties::from_state_id(below_state.id, below_block).r#distance;
        } else if below_state.is_side_solid(BlockDirection::Up) {
            return 0;
        }

        for direction in [
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::West,
            BlockDirection::East,
        ] {
            let neighbor_pos = pos.offset(direction.to_offset());
            let (neighbor_block, neighbor_state) = accessor.get_block_and_state(&neighbor_pos);
            if neighbor_block == &Block::SCAFFOLDING {
                let neighbor_distance =
                    ScaffoldingLikeProperties::from_state_id(neighbor_state.id, neighbor_block)
                        .r#distance;
                distance = distance.min(neighbor_distance.saturating_add(1));
                if distance == 1 {
                    break;
                }
            }
        }

        distance
    }

    /// `ScaffoldingBlockItem.updatePlacementContext` (`ScaffoldingBlockItem.java:19-64`) for the
    /// server's block-item path. `pos` is `BlockPlaceContext.getClickedPos`. Returns the final
    /// placement position and the redirected clicked face, or `None` when vanilla returns null
    /// (nothing is placed and nothing is consumed).
    pub(crate) async fn update_placement_context(
        world: &World,
        player: &Player,
        pos: BlockPos,
        clicked_face: BlockDirection,
        inside_block: bool,
    ) -> Option<(BlockPos, BlockDirection)> {
        let entity = &player.living_entity.entity;
        let redirect = Self::resolve_placement(
            world,
            pos,
            clicked_face,
            entity.is_sneaking(),
            inside_block,
            entity.get_horizontal_facing().to_block_direction(),
            |p| world.is_in_build_limit(p),
        );
        match redirect {
            PlacementRedirect::Place(target, direction) => Some((target, direction)),
            PlacementRedirect::Refused => None,
            PlacementRedirect::OutOfBounds(out) => {
                let max_y = world.get_top_y();
                if out.0.y > max_y {
                    // `ServerPlayer.sendBuildLimitMessage(true, maxY)` (`ServerPlayer.java:1808-1810`).
                    player
                        .send_overlay_message(
                            &pumpkin_util::text::TextComponent::translate_cross(
                                pumpkin_data::translation::java::BUILD_TOOHIGH,
                                pumpkin_data::translation::bedrock::BUILD_TOOHIGH,
                                vec![pumpkin_util::text::TextComponent::text(max_y.to_string())],
                            )
                            .color_named(pumpkin_util::text::color::NamedColor::Red),
                        )
                        .await;
                }
                None
            }
        }
    }

    /// World-independent body of [`Self::update_placement_context`]. `horizontal_direction` is
    /// `UseOnContext.getHorizontalDirection` and `in_bounds` is `Level.isInWorldBounds`.
    fn resolve_placement(
        accessor: &dyn BlockAccessor,
        pos: BlockPos,
        clicked_face: BlockDirection,
        sneaking: bool,
        inside_block: bool,
        horizontal_direction: BlockDirection,
        in_bounds: impl Fn(BlockPos) -> bool,
    ) -> PlacementRedirect {
        if accessor.get_block(&pos) != &Block::SCAFFOLDING {
            return if Self::get_distance(accessor, &pos) == MAX_DISTANCE {
                PlacementRedirect::Refused
            } else {
                PlacementRedirect::Place(pos, clicked_face)
            };
        }

        let direction = if sneaking {
            if inside_block {
                clicked_face.opposite()
            } else {
                clicked_face
            }
        } else if clicked_face == BlockDirection::Up {
            horizontal_direction
        } else {
            BlockDirection::Up
        };

        // Only horizontal steps count, so an upward redirect climbs the whole column.
        let mut horizontal_distance = 0;
        let mut placement_pos = pos.offset(direction.to_offset());
        while horizontal_distance < MAX_DISTANCE {
            if !in_bounds(placement_pos) {
                return PlacementRedirect::OutOfBounds(placement_pos);
            }

            let (block, state) = accessor.get_block_and_state(&placement_pos);
            if block != &Block::SCAFFOLDING {
                if state.replaceable() {
                    return PlacementRedirect::Place(placement_pos, direction);
                }
                break;
            }

            placement_pos = placement_pos.offset(direction.to_offset());
            if direction.is_horizontal() {
                horizontal_distance += 1;
            }
        }

        PlacementRedirect::Refused
    }

    /// `ScaffoldingBlock.isBottom` (`ScaffoldingBlock.java:154-156`): true when the scaffold isn't
    /// resting directly on another scaffold, but also isn't fully supported (`distance > 0`).
    fn is_bottom(accessor: &dyn BlockAccessor, pos: &BlockPos, distance: u8) -> bool {
        distance > 0 && accessor.get_block(&pos.down()) != &Block::SCAFFOLDING
    }
}

impl BlockBehaviour for ScaffoldingBlock {
    /// `ScaffoldingBlock.getStateForPlacement`.
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = ScaffoldingLikeProperties::default(args.block);
            props.r#waterlogged = args.replacing.water_source();
            props.r#distance = Self::get_distance(args.world, args.position);
            props.r#bottom = Self::is_bottom(args.world, args.position, props.r#distance);
            props.to_state_id(args.block)
        })
    }

    /// The block-item path (`use_item_on` set) skips survival because
    /// `ScaffoldingBlockItem.mustSurvive` is false (`ScaffoldingBlockItem.java:66-69`; a
    /// distance-7 cell reached by a redirect then falls on its scheduled tick). Every other
    /// caller, such as falling-block landing (`FallingBlockEntity.java:185-189`), gets
    /// `ScaffoldingBlock.canSurvive`: `getDistance(level, pos) < 7` (`ScaffoldingBlock.java:131-134`).
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        args.use_item_on.is_some()
            || Self::get_distance(args.block_accessor, args.position) < MAX_DISTANCE
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
        })
    }

    /// `ScaffoldingBlock.updateShape`: schedules the same tick-delay-1 on every neighbor update,
    /// plus a water tick if waterlogged. Does not itself recompute distance/bottom - that only
    /// happens in the scheduled tick.
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let props = ScaffoldingLikeProperties::from_state_id(args.state_id, args.block);
            if props.r#waterlogged {
                args.world.schedule_fluid_tick(
                    &Fluid::WATER,
                    *args.position,
                    Fluid::WATER.flow_speed as u8,
                    TickPriority::Normal,
                );
            }
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
            args.state_id
        })
    }

    /// `ScaffoldingBlock.tick` (`ScaffoldingBlock.java:116-129`). See the doc's gap review for
    /// why this three-way branch must be transcribed literally rather than re-derived: the
    /// destroy-no-entity and fall-as-entity paths have different drop semantics.
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let old_state_id = args.world.get_block_state_id(args.position);
            let old_props = ScaffoldingLikeProperties::from_state_id(old_state_id, args.block);

            let new_distance = Self::get_distance(args.world.as_ref(), args.position);
            let mut new_props = old_props;
            new_props.r#distance = new_distance;
            new_props.r#bottom = Self::is_bottom(args.world.as_ref(), args.position, new_distance);
            let new_state_id = new_props.to_state_id(args.block);

            if new_distance == MAX_DISTANCE {
                if old_props.r#distance == MAX_DISTANCE {
                    FallingEntity::replace_spawn(args.world, *args.position, new_state_id).await;
                } else {
                    args.world
                        .break_block(args.position, None, BlockFlags::empty())
                        .await;
                }
            } else if new_state_id != old_state_id {
                args.world
                    .set_block_state(args.position, new_state_id, BlockFlags::NOTIFY_ALL)
                    .await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::BlockState;
    use pumpkin_data::fluid::Fluid;
    use std::collections::HashMap;

    /// Minimal `BlockAccessor` mock for unit-testing `get_distance`/`is_bottom` without a live
    /// `World`. Unlisted positions default to air.
    #[derive(Default)]
    struct MockAccessor {
        blocks: HashMap<BlockPos, (&'static Block, BlockStateId)>,
    }

    impl MockAccessor {
        fn set(&mut self, pos: BlockPos, block: &'static Block, state_id: BlockStateId) {
            self.blocks.insert(pos, (block, state_id));
        }
    }

    impl BlockAccessor for MockAccessor {
        fn get_block(&self, position: &BlockPos) -> &'static Block {
            self.blocks
                .get(position)
                .map_or(&Block::AIR, |(block, _)| *block)
        }

        fn get_block_state(&self, position: &BlockPos) -> &'static BlockState {
            self.blocks
                .get(position)
                .map_or(Block::AIR.default_state, |(_, state_id)| {
                    BlockState::from_id(*state_id)
                })
        }

        fn get_block_state_id(&self, position: &BlockPos) -> BlockStateId {
            self.blocks
                .get(position)
                .map_or(Block::AIR.default_state.id, |(_, state_id)| *state_id)
        }

        fn get_block_and_state(
            &self,
            position: &BlockPos,
        ) -> (&'static Block, &'static BlockState) {
            (self.get_block(position), self.get_block_state(position))
        }

        fn get_fluid(&self, _position: &BlockPos) -> Fluid {
            Fluid::EMPTY
        }
    }

    fn accessor_with(blocks: &[(BlockPos, &'static Block, BlockStateId)]) -> MockAccessor {
        let mut accessor = MockAccessor::default();
        for (pos, block, state_id) in blocks {
            accessor.set(*pos, block, *state_id);
        }
        accessor
    }

    fn scaffolding_state(distance: u8) -> BlockStateId {
        let mut props = ScaffoldingLikeProperties::default(&Block::SCAFFOLDING);
        props.r#distance = distance;
        props.to_state_id(&Block::SCAFFOLDING)
    }

    #[test]
    fn distance_zero_when_directly_supported() {
        let pos = BlockPos::new(0, 1, 0);
        let accessor = accessor_with(&[(pos.down(), &Block::STONE, Block::STONE.default_state.id)]);
        assert_eq!(ScaffoldingBlock::get_distance(&accessor, &pos), 0);
    }

    #[test]
    fn distance_seven_when_unsupported() {
        let pos = BlockPos::new(0, 1, 0);
        let accessor = accessor_with(&[(pos.down(), &Block::AIR, Block::AIR.default_state.id)]);
        assert_eq!(
            ScaffoldingBlock::get_distance(&accessor, &pos),
            MAX_DISTANCE
        );
    }

    #[test]
    fn distance_propagates_vertically_through_scaffolding() {
        let pos = BlockPos::new(0, 1, 0);
        let accessor = accessor_with(&[(pos.down(), &Block::SCAFFOLDING, scaffolding_state(3))]);
        assert_eq!(ScaffoldingBlock::get_distance(&accessor, &pos), 3);
    }

    #[test]
    fn distance_propagates_horizontally_plus_one() {
        let pos = BlockPos::new(0, 1, 0);
        let accessor = accessor_with(&[
            (pos.down(), &Block::AIR, Block::AIR.default_state.id),
            (
                pos.offset(BlockDirection::North.to_offset()),
                &Block::SCAFFOLDING,
                scaffolding_state(2),
            ),
        ]);
        assert_eq!(ScaffoldingBlock::get_distance(&accessor, &pos), 3);
    }

    #[test]
    fn distance_prefers_vertical_over_horizontal() {
        let pos = BlockPos::new(0, 1, 0);
        let accessor = accessor_with(&[
            (pos.down(), &Block::SCAFFOLDING, scaffolding_state(1)),
            (
                pos.offset(BlockDirection::North.to_offset()),
                &Block::SCAFFOLDING,
                scaffolding_state(0),
            ),
        ]);
        // Vertical gives 1 directly; horizontal would give 0+1=1 too, min is still 1.
        assert_eq!(ScaffoldingBlock::get_distance(&accessor, &pos), 1);
    }

    #[test]
    fn is_bottom_false_when_resting_on_scaffolding() {
        let pos = BlockPos::new(0, 1, 0);
        let accessor = accessor_with(&[(pos.down(), &Block::SCAFFOLDING, scaffolding_state(0))]);
        assert!(!ScaffoldingBlock::is_bottom(&accessor, &pos, 1));
    }

    #[test]
    fn is_bottom_true_when_floating_and_not_fully_supported() {
        let pos = BlockPos::new(0, 1, 0);
        let accessor = accessor_with(&[(pos.down(), &Block::AIR, Block::AIR.default_state.id)]);
        assert!(ScaffoldingBlock::is_bottom(&accessor, &pos, 3));
    }

    #[test]
    fn is_bottom_false_when_fully_supported() {
        let pos = BlockPos::new(0, 1, 0);
        let accessor = accessor_with(&[(pos.down(), &Block::AIR, Block::AIR.default_state.id)]);
        assert!(!ScaffoldingBlock::is_bottom(&accessor, &pos, 0));
    }

    #[test]
    fn interaction_shape_is_the_full_block() {
        // `ScaffoldingBlock.getInteractionShape` (`ScaffoldingBlock.java:67-69`) returns
        // `Shapes.block()` for every scaffolding state.
        let shape = ScaffoldingBlock::interaction_shape_at(&BlockPos::new(3, 64, -2));
        assert_eq!(shape.min.x, 3.0);
        assert_eq!(shape.min.y, 64.0);
        assert_eq!(shape.min.z, -2.0);
        assert_eq!(shape.max.x, 4.0);
        assert_eq!(shape.max.y, 65.0);
        assert_eq!(shape.max.z, -1.0);
    }

    #[test]
    fn collision_shape_uses_the_unstable_bottom_for_descending_entities() {
        // `ScaffoldingBlock.getCollisionShape` (`ScaffoldingBlock.java:137-145`) returns the
        // lower two-pixel slab for a bottom scaffold when the entity is descending.
        let mut props = ScaffoldingLikeProperties::default(&Block::SCAFFOLDING);
        props.r#distance = 3;
        props.r#bottom = true;
        let state_id = props.to_state_id(&Block::SCAFFOLDING);

        let shapes = ScaffoldingBlock::collision_shapes_for_context(state_id, false, true, true);
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].max.y, 0.125);
    }

    fn in_bounds_below(top_y: i32) -> impl Fn(BlockPos) -> bool {
        move |p: BlockPos| p.0.y <= top_y
    }

    #[test]
    fn placement_on_a_non_scaffold_is_refused_at_distance_seven() {
        // `ScaffoldingBlockItem.java:25-27`: no support below or beside means null.
        let pos = BlockPos::new(0, 1, 0);
        let accessor = MockAccessor::default();
        assert_eq!(
            ScaffoldingBlock::resolve_placement(
                &accessor,
                pos,
                BlockDirection::Up,
                false,
                false,
                BlockDirection::North,
                in_bounds_below(320),
            ),
            PlacementRedirect::Refused
        );

        let supported =
            accessor_with(&[(pos.down(), &Block::STONE, Block::STONE.default_state.id)]);
        assert_eq!(
            ScaffoldingBlock::resolve_placement(
                &supported,
                pos,
                BlockDirection::Up,
                false,
                false,
                BlockDirection::North,
                in_bounds_below(320),
            ),
            PlacementRedirect::Place(pos, BlockDirection::Up)
        );
    }

    #[test]
    fn side_click_climbs_the_whole_column() {
        // Vertical steps never count toward the seven-step limit.
        let mut accessor = MockAccessor::default();
        accessor.set(
            BlockPos::new(0, 0, 0),
            &Block::STONE,
            Block::STONE.default_state.id,
        );
        for y in 1..=3 {
            accessor.set(
                BlockPos::new(0, y, 0),
                &Block::SCAFFOLDING,
                scaffolding_state(0),
            );
        }
        assert_eq!(
            ScaffoldingBlock::resolve_placement(
                &accessor,
                BlockPos::new(0, 2, 0),
                BlockDirection::North,
                false,
                false,
                BlockDirection::East,
                in_bounds_below(320),
            ),
            PlacementRedirect::Place(BlockPos::new(0, 4, 0), BlockDirection::Up)
        );
    }

    #[test]
    fn top_click_extends_horizontally_up_to_seven_steps() {
        let mut accessor = MockAccessor::default();
        for x in 0..=6 {
            accessor.set(
                BlockPos::new(x, 1, 0),
                &Block::SCAFFOLDING,
                scaffolding_state(0),
            );
        }
        let place = |accessor: &MockAccessor| {
            ScaffoldingBlock::resolve_placement(
                accessor,
                BlockPos::new(0, 1, 0),
                BlockDirection::Up,
                false,
                false,
                BlockDirection::East,
                in_bounds_below(320),
            )
        };
        assert_eq!(
            place(&accessor),
            PlacementRedirect::Place(BlockPos::new(7, 1, 0), BlockDirection::East)
        );

        // An eighth scaffold uses up all seven horizontal steps.
        accessor.set(
            BlockPos::new(7, 1, 0),
            &Block::SCAFFOLDING,
            scaffolding_state(0),
        );
        assert_eq!(place(&accessor), PlacementRedirect::Refused);
    }

    #[test]
    fn top_click_at_the_build_limit_still_redirects_horizontally() {
        let top_y = 319;
        let accessor = accessor_with(&[(
            BlockPos::new(0, top_y, 0),
            &Block::SCAFFOLDING,
            scaffolding_state(0),
        )]);
        assert_eq!(
            ScaffoldingBlock::resolve_placement(
                &accessor,
                BlockPos::new(0, top_y, 0),
                BlockDirection::Up,
                false,
                false,
                BlockDirection::South,
                in_bounds_below(top_y),
            ),
            PlacementRedirect::Place(BlockPos::new(0, top_y, 1), BlockDirection::South)
        );
        assert_eq!(
            ScaffoldingBlock::resolve_placement(
                &accessor,
                BlockPos::new(0, top_y, 0),
                BlockDirection::West,
                false,
                false,
                BlockDirection::South,
                in_bounds_below(top_y),
            ),
            PlacementRedirect::OutOfBounds(BlockPos::new(0, top_y + 1, 0))
        );
    }

    #[test]
    fn sneaking_follows_the_clicked_face() {
        let accessor = accessor_with(&[(
            BlockPos::new(0, 1, 0),
            &Block::SCAFFOLDING,
            scaffolding_state(0),
        )]);
        let place = |inside| {
            ScaffoldingBlock::resolve_placement(
                &accessor,
                BlockPos::new(0, 1, 0),
                BlockDirection::West,
                true,
                inside,
                BlockDirection::North,
                in_bounds_below(320),
            )
        };
        assert_eq!(
            place(false),
            PlacementRedirect::Place(BlockPos::new(-1, 1, 0), BlockDirection::West)
        );
        assert_eq!(
            place(true),
            PlacementRedirect::Place(BlockPos::new(1, 1, 0), BlockDirection::East)
        );
    }
}
