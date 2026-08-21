use std::collections::HashSet;

use crate::{
    block::{
        BlockBehaviour, BlockFuture, BlockIsReplacing, CanPlaceAtArgs, CanUpdateAtArgs,
        GetStateForNeighborUpdateArgs, OnPlaceArgs, RandomTickArgs, UseWithItemArgs,
        blocks::abstract_multiface::can_attach_to, registry::BlockActionResult,
    },
    entity::{EntityBase, player::Player},
    world::World,
};
use pumpkin_data::{
    Block, BlockDirection, BlockStateId, FacingExt, HorizontalFacingExt,
    block_properties::{BlockProperties, VineLikeProperties},
    item::Item,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::RngExt;

#[pumpkin_block("minecraft:vine")]
pub struct VineBlock;

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

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if let BlockIsReplacing::Itself(state_id) = args.replacing {
                let (Some(direction), _) = get_accurate_direction(
                    args.world,
                    args.position,
                    Some(args.player),
                    args.direction,
                    true,
                ) else {
                    return Block::AIR.default_state.id;
                };
                let mut props = VineLikeProperties::from_state_id(state_id, args.block);
                vine_direction_mapper(direction, &mut props);
                return props.to_state_id(args.block);
            }
            let (Some(direction), _) = get_accurate_direction(
                args.world,
                args.position,
                Some(args.player),
                args.direction,
                false,
            ) else {
                return Block::AIR.default_state.id;
            };
            let mut props = VineLikeProperties::default(args.block);
            vine_direction_mapper(direction, &mut props);
            props.to_state_id(args.block)
        })
    }
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_place_vine_at(
            args.block_accessor,
            args.position,
            args.direction,
            args.player,
            false,
        )
    }
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let old_props = VineLikeProperties::from_state_id(args.state_id, args.block);
            let old_directions = get_vine_block_directions(old_props);
            let mut new_directions = old_directions.clone();
            for old_dir in old_directions {
                let support_block = args
                    .world
                    .get_block(&args.position.offset(old_dir.to_offset()));
                if !supports_vine(support_block)
                    && !is_top_block_full_vine(args.world, args.position)
                {
                    new_directions.remove(&old_dir);
                }
            }
            if new_directions.is_empty() {
                return Block::AIR.default_state.id;
            }
            let mut new_props = VineLikeProperties::default(args.block);

            for new_dir in new_directions {
                vine_direction_mapper(new_dir, &mut new_props);
            }

            new_props.to_state_id(args.block)
        })
    }
    fn can_update_at(&self, args: CanUpdateAtArgs<'_>) -> bool {
        get_accurate_direction(
            args.world,
            args.position,
            Some(args.player),
            args.direction,
            true,
        )
        .0
        .is_some()
    }
    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let mut props = VineLikeProperties::from_state_id(state.id, args.block);

            let item = args.item_stack.item;

            if item.id != Item::VINE.id {
                return BlockActionResult::Pass;
            }
            let (Some(accurate_dir), _) = get_accurate_direction(
                args.world.as_ref(),
                args.position,
                Some(args.player),
                BlockDirection::Down,
                true,
            ) else {
                return BlockActionResult::Fail;
            };
            vine_direction_mapper(accurate_dir, &mut props);

            args.world
                .set_block_state(
                    args.position,
                    props.to_state_id(args.block),
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
            BlockActionResult::Consume
        })
    }
}
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

        if index > 0 {
            directions.copy_within(0..index, 1);
            directions[0] = target;
        }
    }
    directions
}
fn can_place_vine_at(
    block_accessor: &dyn BlockAccessor,
    block_pos: &BlockPos,
    click_direction_wrapper: Option<BlockDirection>,
    player_wrapper: Option<&Player>,
    replacing: bool,
) -> bool {
    let Some(click_direction) = click_direction_wrapper else {
        return false;
    };
    let (direction, _) = get_accurate_direction(
        block_accessor,
        block_pos,
        player_wrapper,
        click_direction,
        replacing,
    );
    let Some(direction) = direction else {
        return false;
    };

    let support_pos = block_pos.offset(direction.to_offset());
    let (support_block, _support_block_state) = block_accessor.get_block_and_state(&support_pos);
    if !supports_vine(support_block) && !is_top_block_full_vine(block_accessor, block_pos) {
        return false;
    }
    true
}
const fn supports_vine(support_block: &Block) -> bool {
    if support_block.default_state.is_full_cube() {
        return true;
    }
    false
}
//returns (accurate direction, boolean)
// true if this direction is for hanging vine
// false if it is not
fn get_accurate_direction(
    block_accessor: &dyn BlockAccessor,
    block_pos: &BlockPos,
    player_wrapper: Option<&Player>,
    click_direction: BlockDirection,
    replacing: bool,
) -> (Option<BlockDirection>, bool) {
    let clicked_block = block_accessor.get_block(&block_pos.offset(click_direction.to_offset()));
    if !replacing && clicked_block == &Block::VINE && click_direction != BlockDirection::Up {
        return (None, false);
    }

    if click_direction != BlockDirection::Down && supports_vine(clicked_block) {
        return (Some(click_direction), false);
    }
    let (replacing_block, replacing_block_state) = block_accessor.get_block_and_state(block_pos);
    let already_active_directions = if replacing_block == &Block::VINE {
        let props = VineLikeProperties::from_state_id(replacing_block_state.id, replacing_block);
        get_vine_block_directions(props)
    } else {
        HashSet::new()
    };
    if let Some(player) = player_wrapper {
        let mut up = false;
        for dir in get_nearest_looking_directions(player, replacing, click_direction) {
            if dir != BlockDirection::Down && !already_active_directions.contains(&dir) {
                let support_pos = block_pos.offset(dir.to_offset());
                let (support_block, _support_block_state) =
                    block_accessor.get_block_and_state(&support_pos);
                if !supports_vine(support_block) {
                    //handler for hanging vine
                    if is_top_block_full_vine(block_accessor, block_pos) {
                        if dir == BlockDirection::Up {
                            continue;
                        }
                        return (Some(dir), true);
                    }
                    continue;
                }
                if dir == BlockDirection::Up && !replacing {
                    up = true;
                    continue;
                }

                return (Some(dir), false);
            }
        }
        if up {
            return (Some(BlockDirection::Up), false);
        }
    }
    (None, false)
}
fn is_top_block_full_vine(block_accessor: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
    let (top_block, top_block_state) = block_accessor.get_block_and_state(&block_pos.up());
    if top_block != &Block::VINE {
        return false;
    }
    let props = VineLikeProperties::from_state_id(top_block_state.id, top_block);
    props.up && props.west && props.east && props.north && props.south
}
fn get_vine_block_directions(props: VineLikeProperties) -> HashSet<BlockDirection> {
    let mut set = HashSet::new();
    if props.north {
        set.insert(BlockDirection::North);
    }
    if props.south {
        set.insert(BlockDirection::South);
    }
    if props.east {
        set.insert(BlockDirection::East);
    }
    if props.west {
        set.insert(BlockDirection::West);
    }
    if props.up {
        set.insert(BlockDirection::Up);
    }
    set
}
const fn vine_direction_mapper(direction: BlockDirection, props: &mut VineLikeProperties) {
    match direction {
        BlockDirection::Down => (),
        BlockDirection::Up => props.up = true,
        BlockDirection::North => props.north = true,
        BlockDirection::South => props.south = true,
        BlockDirection::West => props.west = true,
        BlockDirection::East => props.east = true,
    }
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

/// `VineBlock.hasHorizontalConnection` (`VineBlock.java:262-264`).
const fn has_horizontal_connection(props: VineLikeProperties) -> bool {
    props.north || props.east || props.south || props.west
}

/// `VineBlock.isAcceptableNeighbour` (`VineBlock.java:118-120`), which forwards to
/// `MultifaceBlock.canAttachTo` on the state *at* `neighbour_pos`.
fn is_acceptable_neighbour(
    accessor: &dyn BlockAccessor,
    neighbour_pos: &BlockPos,
    direction_to_neighbour: BlockDirection,
) -> bool {
    can_attach_to(
        accessor.get_block_state(neighbour_pos),
        direction_to_neighbour,
    )
}

/// `VineBlock.canSupportAtFace` (`VineBlock.java:99-116`).
fn can_support_at_face(
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
    use super::{face, has_horizontal_connection, with_face};
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
        for direction in [
            BlockDirection::Up,
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::West,
            BlockDirection::East,
        ] {
            props = with_face(props, direction, true);
            assert!(face(props, direction));
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
