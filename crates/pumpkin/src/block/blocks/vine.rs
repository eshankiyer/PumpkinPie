use crate::{
    block::{
        BlockBehaviour, BlockFuture, CanPlaceAtArgs, CanUpdateAtArgs,
        GetStateForNeighborUpdateArgs, OnPlaceArgs, RandomTickArgs, UseWithItemArgs,
        blocks::abstract_multiface::can_attach_to, registry::BlockActionResult,
    },
    entity::{EntityBase, player::Player},
    world::World,
};
use pumpkin_data::{
    Block, BlockDirection, BlockState, BlockStateId, FacingExt, HorizontalFacingExt,
    block_properties::{BlockProperties, VineLikeProperties},
    block_rotation::{Mirror, Rotation},
    item::Item,
    tag::{self, Taggable},
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::RngExt;

#[pumpkin_block("minecraft:vine")]
pub struct VineBlock;

/// `VineBlock.PROPERTY_BY_DIRECTION.size()`: the five non-`DOWN` faces.
const MAX_FACES: usize = 5;

impl BlockBehaviour for VineBlock {
    /// `VineBlock.randomTick` (`VineBlock.java:169-248`).
    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !args.world.level_info.load().game_rules.spread_vines {
                return;
            }
            if rand::rng().random_range(0..4) != 0 {
                return;
            }
            spread(args.world, args.position).await;
        })
    }

    /// `VineBlock.getStateForPlacement` (`VineBlock.java:289-305`): the first non-`DOWN` face,
    /// in the player's nearest-looking order, that is free and can be supported.
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let (clicked_block, clicked_state_id) =
                args.world.get_block_and_state_id(args.position);
            let clicked_is_vine = clicked_block == &Block::VINE;
            let result = if clicked_is_vine {
                VineLikeProperties::from_state_id(clicked_state_id, args.block)
            } else {
                VineLikeProperties::default(args.block)
            };

            for direction in
                get_nearest_looking_directions(args.player, clicked_is_vine, args.direction)
            {
                if direction != BlockDirection::Down
                    && !(clicked_is_vine && face(result, direction))
                    && can_support_at_face(args.world, args.position, direction)
                {
                    return with_face(result, direction, true).to_state_id(args.block);
                }
            }

            if clicked_is_vine && count_faces(&result) > 0 {
                result.to_state_id(args.block)
            } else {
                Block::AIR.default_state.id
            }
        })
    }

    /// Mirrors `getStateForPlacement` returning non-null (`VineBlock.java:289-305`).
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        let (clicked_block, clicked_state) = args.block_accessor.get_block_and_state(args.position);
        let clicked_is_vine = clicked_block == &Block::VINE;
        let result = if clicked_is_vine {
            VineLikeProperties::from_state_id(clicked_state.id, args.block)
        } else {
            VineLikeProperties::default(args.block)
        };

        if clicked_is_vine && count_faces(&result) >= MAX_FACES {
            return false;
        }

        let nearest_directions = args.player.map_or_else(
            || {
                args.direction.map_or(
                    [
                        BlockDirection::North,
                        BlockDirection::South,
                        BlockDirection::West,
                        BlockDirection::East,
                        BlockDirection::Up,
                        BlockDirection::Down,
                    ],
                    |click_dir| {
                        [
                            click_dir.opposite(),
                            BlockDirection::Up,
                            BlockDirection::North,
                            BlockDirection::South,
                            BlockDirection::West,
                            BlockDirection::East,
                        ]
                    },
                )
            },
            |player| {
                let click_dir = args.direction.unwrap_or(BlockDirection::Down);
                get_nearest_looking_directions(player, clicked_is_vine, click_dir)
            },
        );

        for direction in nearest_directions {
            if direction != BlockDirection::Down
                && !(clicked_is_vine && face(result, direction))
                && can_support_at_face(args.block_accessor, args.position, direction)
            {
                return true;
            }
        }

        clicked_is_vine && count_faces(&result) > 0
    }

    /// `VineBlock.updateShape` (`VineBlock.java:147-163`).
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if args.direction == BlockDirection::Down {
                return args.state_id;
            }

            let updated = get_updated_state(
                VineLikeProperties::from_state_id(args.state_id, args.block),
                args.world,
                args.position,
            );
            if count_faces(&updated) == 0 {
                Block::AIR.default_state.id
            } else {
                updated.to_state_id(args.block)
            }
        })
    }

    /// `VineBlock.canBeReplaced` (`VineBlock.java:283-287`): a vine accepts another vine while
    /// it still has a free face.
    fn can_update_at(&self, args: CanUpdateAtArgs<'_>) -> bool {
        let (clicked_block, clicked_state) = args.world.get_block_and_state(args.position);
        if clicked_block != &Block::VINE {
            return false;
        }
        let props = VineLikeProperties::from_state_id(clicked_state.id, clicked_block);
        count_faces(&props) < MAX_FACES
    }

    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            if args.item_stack.item.id != Item::VINE.id {
                return BlockActionResult::Pass;
            }

            let state = args.world.get_block_state(args.position);
            let props = VineLikeProperties::from_state_id(state.id, args.block);
            if count_faces(&props) >= MAX_FACES {
                return BlockActionResult::Pass;
            }

            for direction in get_nearest_looking_directions(args.player, true, BlockDirection::Down)
            {
                if direction != BlockDirection::Down
                    && !face(props, direction)
                    && can_support_at_face(&**args.world, args.position, direction)
                {
                    args.world
                        .set_block_state(
                            args.position,
                            with_face(props, direction, true).to_state_id(args.block),
                            BlockFlags::NOTIFY_ALL,
                        )
                        .await;
                    return BlockActionResult::Consume;
                }
            }

            BlockActionResult::Pass
        })
    }

    /// `VineBlock.rotate` (`VineBlock.java:312-328`).
    fn rotate(
        &self,
        block: &Block,
        state_id: BlockStateId,
        rotation: Rotation,
    ) -> &'static BlockState {
        let props = VineLikeProperties::from_state_id(state_id, block);
        let mut rotated = props;
        match rotation {
            Rotation::Rotate180 => {
                rotated.north = props.south;
                rotated.east = props.west;
                rotated.south = props.north;
                rotated.west = props.east;
            }
            Rotation::CounterClockwise90 => {
                rotated.north = props.east;
                rotated.east = props.south;
                rotated.south = props.west;
                rotated.west = props.north;
            }
            Rotation::Clockwise90 => {
                rotated.north = props.west;
                rotated.east = props.north;
                rotated.south = props.east;
                rotated.west = props.south;
            }
            Rotation::None => {}
        }
        BlockState::from_id(rotated.to_state_id(block))
    }

    /// `VineBlock.mirror` (`VineBlock.java:330-340`).
    fn mirror(&self, block: &Block, state_id: BlockStateId, mirror: Mirror) -> &'static BlockState {
        let props = VineLikeProperties::from_state_id(state_id, block);
        let mut mirrored = props;
        match mirror {
            Mirror::LeftRight => {
                mirrored.north = props.south;
                mirrored.south = props.north;
            }
            Mirror::FrontBack => {
                mirrored.east = props.west;
                mirrored.west = props.east;
            }
            Mirror::None => {}
        }
        BlockState::from_id(mirrored.to_state_id(block))
    }
}

/// `BlockPlaceContext.getNearestLookingDirections`: the player's facing order, with the
/// clicked face's opposite moved to the front unless the clicked block itself is replaced.
#[must_use]
pub fn get_nearest_looking_directions(
    player: &Player,
    replace_clicked: bool,
    clicked_face: BlockDirection,
) -> [BlockDirection; 6] {
    let mut directions: [BlockDirection; 6] = {
        let fs = player.get_entity().get_entity_facing_order();
        [
            fs[0].to_block_direction(),
            fs[1].to_block_direction(),
            fs[2].to_block_direction(),
            fs[3].to_block_direction(),
            fs[4].to_block_direction(),
            fs[5].to_block_direction(),
        ]
    };

    if !replace_clicked {
        let target = clicked_face.opposite();
        let mut index = 0;
        while index < directions.len() && directions[index] != target {
            index += 1;
        }

        if index > 0 && index < directions.len() {
            directions.copy_within(0..index, 1);
            directions[0] = target;
        }
    }
    directions
}

/// `VineBlock.getPropertyForFace` read (`VineBlock.java:343`). `DOWN` has no property in
/// vanilla; the callers below never pass it.
const fn face(props: VineLikeProperties, direction: BlockDirection) -> bool {
    match direction {
        BlockDirection::Down => false,
        BlockDirection::Up => props.up,
        BlockDirection::North => props.north,
        BlockDirection::South => props.south,
        BlockDirection::West => props.west,
        BlockDirection::East => props.east,
    }
}

const fn with_face(
    mut props: VineLikeProperties,
    direction: BlockDirection,
    value: bool,
) -> VineLikeProperties {
    match direction {
        BlockDirection::Down => (),
        BlockDirection::Up => props.up = value,
        BlockDirection::North => props.north = value,
        BlockDirection::South => props.south = value,
        BlockDirection::West => props.west = value,
        BlockDirection::East => props.east = value,
    }
    props
}

#[must_use]
pub const fn has_face_property(props: &VineLikeProperties, direction: BlockDirection) -> bool {
    face(*props, direction)
}

pub const fn set_face_property(
    props: &mut VineLikeProperties,
    direction: BlockDirection,
    value: bool,
) {
    *props = with_face(*props, direction, value);
}

/// `VineBlock.countFaces` (`VineBlock.java:62-72`).
#[must_use]
pub const fn count_faces(props: &VineLikeProperties) -> usize {
    props.up as usize
        + props.north as usize
        + props.south as usize
        + props.west as usize
        + props.east as usize
}

/// `VineBlock.hasHorizontalConnection` (`VineBlock.java:262-264`).
const fn has_horizontal_connection(props: VineLikeProperties) -> bool {
    props.north || props.east || props.south || props.west
}

/// `VineBlock.isAcceptableNeighbour` (`VineBlock.java:118-120`), which forwards to
/// `MultifaceBlock.canAttachTo` on the state *at* `neighbour_pos`. Leaves have a full
/// collision shape in vanilla, so they always qualify.
#[must_use]
pub fn is_acceptable_neighbour(
    accessor: &dyn BlockAccessor,
    neighbour_pos: &BlockPos,
    direction_to_neighbour: BlockDirection,
) -> bool {
    let (block, state) = accessor.get_block_and_state(neighbour_pos);
    can_attach_to(state, direction_to_neighbour) || block.has_tag(&tag::Block::MINECRAFT_LEAVES)
}

/// `VineBlock.canSupportAtFace` (`VineBlock.java:99-116`).
#[must_use]
pub fn can_support_at_face(
    accessor: &dyn BlockAccessor,
    pos: &BlockPos,
    direction: BlockDirection,
) -> bool {
    if direction == BlockDirection::Down {
        return false;
    }
    if is_acceptable_neighbour(accessor, &pos.offset(direction.to_offset()), direction) {
        return true;
    }
    if direction == BlockDirection::Up {
        return false;
    }
    let above_pos = pos.up();
    let (above_block, above_state) = accessor.get_block_and_state(&above_pos);
    above_block == &Block::VINE
        && face(
            VineLikeProperties::from_state_id(above_state.id, above_block),
            direction,
        )
}

/// `VineBlock.getUpdatedState` (`VineBlock.java:122-145`).
#[must_use]
pub fn get_updated_state(
    mut props: VineLikeProperties,
    accessor: &dyn BlockAccessor,
    pos: &BlockPos,
) -> VineLikeProperties {
    let above_pos = pos.up();
    if props.up {
        // Vanilla passes `Direction.DOWN` here, not `UP` (`VineBlock.java:125`).
        props.up = is_acceptable_neighbour(accessor, &above_pos, BlockDirection::Down);
    }

    let mut above_props: Option<Option<VineLikeProperties>> = None;
    for direction in [
        BlockDirection::North,
        BlockDirection::East,
        BlockDirection::South,
        BlockDirection::West,
    ] {
        if face(props, direction) {
            let can_support = can_support_at_face(accessor, pos, direction)
                || above_props
                    .get_or_insert_with(|| {
                        let (above_block, above_state) = accessor.get_block_and_state(&above_pos);
                        (above_block == &Block::VINE)
                            .then(|| VineLikeProperties::from_state_id(above_state.id, above_block))
                    })
                    .is_some_and(|above| face(above, direction));
            props = with_face(props, direction, can_support);
        }
    }
    props
}

/// `VineBlock.canSpread` (`VineBlock.java:266-281`): at most four other vines in the
/// 9x3x9 box centred on `pos`.
fn can_spread(accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
    let mut remaining = 5;
    for x in -4..=4 {
        for y in -1..=1 {
            for z in -4..=4 {
                let probe = BlockPos::new(pos.0.x + x, pos.0.y + y, pos.0.z + z);
                if accessor.get_block(&probe) == &Block::VINE {
                    remaining -= 1;
                    if remaining <= 0 {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// `VineBlock.copyRandomFaces` (`VineBlock.java:249-260`).
fn copy_random_faces(from: VineLikeProperties, mut to: VineLikeProperties) -> VineLikeProperties {
    for direction in BlockDirection::horizontal_worldgen() {
        let direction = direction.to_block_direction();
        if rand::rng().random::<bool>() && face(from, direction) {
            to = with_face(to, direction, true);
        }
    }
    to
}

/// `VineBlock.randomTick`'s body (`VineBlock.java:171-247`), entered once the
/// `nextInt(4) == 0` gate and the `spreadVines` game rule have both passed.
#[expect(clippy::too_many_lines)]
async fn spread(world: &std::sync::Arc<World>, pos: &BlockPos) {
    let (block, state_id) = world.get_block_and_state_id(pos);
    if block != &Block::VINE {
        return;
    }
    let state = VineLikeProperties::from_state_id(state_id, block);
    let test_direction = BlockDirection::all()[rand::rng().random_range(0..6usize)];
    let above_pos = pos.up();

    if test_direction.is_horizontal() && !face(state, test_direction) {
        if !can_spread(world.as_ref(), pos) {
            return;
        }
        let test_pos = pos.offset(test_direction.to_offset());
        if world.get_block_state(&test_pos).is_air() {
            let cw = test_direction.rotate_clockwise();
            let ccw = test_direction.rotate_counter_clockwise();
            let cw_connected = face(state, cw);
            let ccw_connected = face(state, ccw);
            let cw_test_pos = test_pos.offset(cw.to_offset());
            let ccw_test_pos = test_pos.offset(ccw.to_offset());

            if cw_connected && is_acceptable_neighbour(world.as_ref(), &cw_test_pos, cw) {
                place_vine(world, &test_pos, cw).await;
            } else if ccw_connected && is_acceptable_neighbour(world.as_ref(), &ccw_test_pos, ccw) {
                place_vine(world, &test_pos, ccw).await;
            } else {
                let opposite = test_direction.opposite();
                if cw_connected
                    && world.get_block_state(&cw_test_pos).is_air()
                    && is_acceptable_neighbour(
                        world.as_ref(),
                        &pos.offset(cw.to_offset()),
                        opposite,
                    )
                {
                    place_vine(world, &cw_test_pos, opposite).await;
                } else if ccw_connected
                    && world.get_block_state(&ccw_test_pos).is_air()
                    && is_acceptable_neighbour(
                        world.as_ref(),
                        &pos.offset(ccw.to_offset()),
                        opposite,
                    )
                {
                    place_vine(world, &ccw_test_pos, opposite).await;
                } else if rand::rng().random::<f32>() < 0.05
                    && is_acceptable_neighbour(world.as_ref(), &test_pos.up(), BlockDirection::Up)
                {
                    place_vine(world, &test_pos, BlockDirection::Up).await;
                }
            }
        } else if is_acceptable_neighbour(world.as_ref(), &test_pos, test_direction) {
            let grown = with_face(state, test_direction, true);
            world
                .set_block_state(
                    pos,
                    grown.to_state_id(&Block::VINE),
                    BlockFlags::NOTIFY_LISTENERS,
                )
                .await;
        }
        return;
    }

    if test_direction == BlockDirection::Up && pos.0.y < world.get_top_y() {
        if can_support_at_face(world.as_ref(), pos, test_direction) {
            let grown = with_face(state, BlockDirection::Up, true);
            world
                .set_block_state(
                    pos,
                    grown.to_state_id(&Block::VINE),
                    BlockFlags::NOTIFY_LISTENERS,
                )
                .await;
            return;
        }

        if world.get_block_state(&above_pos).is_air() {
            if !can_spread(world.as_ref(), pos) {
                return;
            }
            let mut above_state = state;
            for direction in BlockDirection::horizontal_worldgen() {
                let direction = direction.to_block_direction();
                if rand::rng().random::<bool>()
                    || !is_acceptable_neighbour(
                        world.as_ref(),
                        &above_pos.offset(direction.to_offset()),
                        direction,
                    )
                {
                    above_state = with_face(above_state, direction, false);
                }
            }
            if has_horizontal_connection(above_state) {
                world
                    .set_block_state(
                        &above_pos,
                        above_state.to_state_id(&Block::VINE),
                        BlockFlags::NOTIFY_LISTENERS,
                    )
                    .await;
            }
            return;
        }
    }

    if pos.0.y > world.get_bottom_y() {
        let below_pos = pos.down();
        let (below_block, below_state) = world.get_block_and_state(&below_pos);
        if below_state.is_air() || below_block == &Block::VINE {
            let before = if below_state.is_air() {
                VineLikeProperties::default(&Block::VINE)
            } else {
                VineLikeProperties::from_state_id(below_state.id, below_block)
            };
            let after = copy_random_faces(state, before);
            if before != after && has_horizontal_connection(after) {
                world
                    .set_block_state(
                        &below_pos,
                        after.to_state_id(&Block::VINE),
                        BlockFlags::NOTIFY_LISTENERS,
                    )
                    .await;
            }
        }
    }
}

async fn place_vine(world: &std::sync::Arc<World>, pos: &BlockPos, direction: BlockDirection) {
    let props = with_face(VineLikeProperties::default(&Block::VINE), direction, true);
    world
        .set_block_state(
            pos,
            props.to_state_id(&Block::VINE),
            BlockFlags::NOTIFY_LISTENERS,
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::{count_faces, face, has_horizontal_connection, with_face};
    use pumpkin_data::{
        Block, BlockDirection,
        block_properties::{BlockProperties, VineLikeProperties},
    };

    #[test]
    fn down_has_no_face_property() {
        // VineBlock.java:343 - getPropertyForFace has no DOWN entry.
        let props = VineLikeProperties::default(&Block::VINE);
        assert!(!face(props, BlockDirection::Down));
        assert_eq!(with_face(props, BlockDirection::Down, true), props);
    }

    #[test]
    fn faces_round_trip() {
        let mut props = VineLikeProperties::default(&Block::VINE);
        for (count, direction) in [
            BlockDirection::Up,
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::West,
            BlockDirection::East,
        ]
        .into_iter()
        .enumerate()
        {
            props = with_face(props, direction, true);
            assert!(face(props, direction));
            assert_eq!(count_faces(&props), count + 1);
        }
    }

    #[test]
    fn horizontal_connection_ignores_up() {
        // VineBlock.java:262-264 checks NORTH/EAST/SOUTH/WEST only.
        let up_only = with_face(
            VineLikeProperties::default(&Block::VINE),
            BlockDirection::Up,
            true,
        );
        assert!(!has_horizontal_connection(up_only));
        assert!(has_horizontal_connection(with_face(
            up_only,
            BlockDirection::North,
            true
        )));
    }
}
