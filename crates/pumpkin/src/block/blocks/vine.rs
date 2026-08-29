use crate::{block::{BlockBehaviour, CanPlaceAtArgs, CanUpdateAtArgs, GetStateForNeighborUpdateArgs, OnPlaceArgs, RandomTickArgs, UseWithItemArgs, registry::BlockActionResult}, entity::{EntityBase, player::Player}, world::World};
use pumpkin_data::{Block, BlockDirection, BlockState, BlockStateId, FacingExt, block_properties::{BlockProperties, VineLikeProperties}, block_rotation::{Mirror, Rotation}, game_rules::{GameRule, GameRuleValue}, item::Item, tag::{self, Taggable}};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::RngExt;

#[pumpkin_block("minecraft:vine")]
pub struct VineBlock;

/// `VineBlock.isAcceptableNeighbour` (`VineBlock.java:118-120`): solid faces and leaves support vines.
#[must_use]
pub fn is_acceptable_neighbour(block: &Block, state: &BlockState, direction: BlockDirection) -> bool {
    state.is_side_solid(direction.opposite()) || block.has_tag(&tag::Block::MINECRAFT_LEAVES)
}

/// `VineBlock.canSupportAtFace` (`VineBlock.java:99-116`).
#[must_use]
pub fn can_support_at_face(world: &dyn BlockAccessor, pos: &BlockPos, direction: BlockDirection) -> bool {
    if direction == BlockDirection::Down { return false; }
    let relative = pos.offset(direction.to_offset());
    let (block, state) = world.get_block_and_state(&relative);
    if is_acceptable_neighbour(block, state, direction) { return true; }
    if direction == BlockDirection::Up { return false; }
    let (above, above_state) = world.get_block_and_state(&pos.up());
    above == &Block::VINE && has_face_property(&VineLikeProperties::from_state_id(above_state.id, above), direction)
}

#[must_use]
pub fn get_updated_state(mut props: VineLikeProperties, world: &dyn BlockAccessor, pos: &BlockPos) -> VineLikeProperties {
    let above_pos = pos.up();
    if props.up {
        let (block, state) = world.get_block_and_state(&above_pos);
        props.up = is_acceptable_neighbour(block, state, BlockDirection::Up);
    }
    let mut above_props = None;
    for direction in [BlockDirection::North, BlockDirection::South, BlockDirection::West, BlockDirection::East] {
        if has_face_property(&props, direction) {
            let mut supported = can_support_at_face(world, pos, direction);
            if !supported {
                let p = above_props.get_or_insert_with(|| {
                    let (block, state) = world.get_block_and_state(&above_pos);
                    if block == &Block::VINE { VineLikeProperties::from_state_id(state.id, block) } else {
                        VineLikeProperties { north: false, south: false, east: false, west: false, up: false }
                    }
                });
                supported = has_face_property(p, direction);
            }
            set_face_property(&mut props, direction, supported);
        }
    }
    props
}

/// `VineBlock.getPropertyForFace` (`VineBlock.java:343`).
#[must_use]
pub const fn has_face_property(props: &VineLikeProperties, direction: BlockDirection) -> bool {
    match direction { BlockDirection::Up => props.up, BlockDirection::North => props.north, BlockDirection::South => props.south, BlockDirection::West => props.west, BlockDirection::East => props.east, BlockDirection::Down => false }
}

pub const fn set_face_property(props: &mut VineLikeProperties, direction: BlockDirection, value: bool) {
    match direction { BlockDirection::Up => props.up = value, BlockDirection::North => props.north = value, BlockDirection::South => props.south = value, BlockDirection::West => props.west = value, BlockDirection::East => props.east = value, BlockDirection::Down => {} }
}

#[must_use]
pub const fn count_faces(props: &VineLikeProperties) -> usize {
    let mut n = 0; if props.up { n += 1; } if props.north { n += 1; } if props.south { n += 1; } if props.west { n += 1; } if props.east { n += 1; } n
}

/// `VineBlock.hasHorizontalConnection` (`VineBlock.java:262-264`).
#[must_use]
pub const fn has_horizontal_connection(props: &VineLikeProperties) -> bool { props.north || props.east || props.south || props.west }

/// `VineBlock.canSpread` (`VineBlock.java:266-281`).
#[must_use]
pub fn can_spread(world: &World, pos: &BlockPos) -> bool {
    let mut count = 0;
    for x in pos.0.x - 4..=pos.0.x + 4 { for y in pos.0.y - 1..=pos.0.y + 1 { for z in pos.0.z - 4..=pos.0.z + 4 {
        if world.get_block(&BlockPos::new(x, y, z)) == &Block::VINE { count += 1; if count >= 5 { return false; } }
    }}}
    true
}

#[must_use]
pub fn get_nearest_looking_directions(player: &Player, replace_clicked: bool, clicked_face: BlockDirection) -> [BlockDirection; 6] {
    let fs = player.get_entity().get_entity_facing_order();
    let mut directions = [fs[0].to_block_direction(), fs[1].to_block_direction(), fs[2].to_block_direction(), fs[3].to_block_direction(), fs[4].to_block_direction(), fs[5].to_block_direction()];
    if !replace_clicked {
        let target = clicked_face.opposite(); let mut index = 0;
        while index < directions.len() && directions[index] != target { index += 1; }
        if index > 0 && index < directions.len() { directions.copy_within(0..index, 1); directions[0] = target; }
    }
    directions
}

impl BlockBehaviour for VineBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let (clicked, clicked_state) = args.world.get_block_and_state_id(args.position);
        let clicked_vine = clicked == &Block::VINE;
        let mut result = if clicked_vine { VineLikeProperties::from_state_id(clicked_state, args.block) } else { VineLikeProperties::default(args.block) };
        for direction in get_nearest_looking_directions(args.player, clicked_vine, args.direction) {
            if direction != BlockDirection::Down && !(clicked_vine && has_face_property(&result, direction)) && can_support_at_face(args.world, args.position, direction) {
                set_face_property(&mut result, direction, true); return result.to_state_id(args.block);
            }
        }
        if clicked_vine && count_faces(&result) > 0 { result.to_state_id(args.block) } else { Block::AIR.default_state.id }
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        let (clicked, state) = args.block_accessor.get_block_and_state(args.position);
        let clicked_vine = clicked == &Block::VINE;
        let result = if clicked_vine { VineLikeProperties::from_state_id(state.id, args.block) } else { VineLikeProperties::default(args.block) };
        if clicked_vine && count_faces(&result) >= 5 { return false; }
        let directions = args.player.map_or_else(|| args.direction.map_or([BlockDirection::North,BlockDirection::South,BlockDirection::West,BlockDirection::East,BlockDirection::Up,BlockDirection::Down], |d| [d.opposite(),BlockDirection::Up,BlockDirection::North,BlockDirection::South,BlockDirection::West,BlockDirection::East]), |p| get_nearest_looking_directions(p, clicked_vine, args.direction.unwrap_or(BlockDirection::Down)));
        directions.into_iter().any(|d| d != BlockDirection::Down && !(clicked_vine && has_face_property(&result, d)) && can_support_at_face(args.block_accessor, args.position, d)) || (clicked_vine && count_faces(&result) > 0)
    }

    fn get_state_for_neighbor_update(&self, args: GetStateForNeighborUpdateArgs<'_>) -> BlockStateId {
        if args.direction == BlockDirection::Down { return args.state_id; }
        let props = get_updated_state(VineLikeProperties::from_state_id(args.state_id, args.block), args.world, args.position);
        if count_faces(&props) == 0 { Block::AIR.default_state.id } else { props.to_state_id(args.block) }
    }

    fn can_update_at(&self, args: CanUpdateAtArgs<'_>) -> bool {
        let (block, state) = args.world.get_block_and_state(args.position);
        block == &Block::VINE && count_faces(&VineLikeProperties::from_state_id(state.id, block)) < 5
    }

    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        if args.item_stack.item.id != Item::VINE.id { return BlockActionResult::Pass; }
        let state = args.world.get_block_state(args.position); let mut props = VineLikeProperties::from_state_id(state.id, args.block);
        if count_faces(&props) >= 5 { return BlockActionResult::Pass; }
        for d in get_nearest_looking_directions(args.player, true, BlockDirection::Down) {
            if d != BlockDirection::Down && !has_face_property(&props, d) && can_support_at_face(&**args.world, args.position, d) {
                set_face_property(&mut props, d, true); args.world.set_block_state(args.position, props.to_state_id(args.block), BlockFlags::NOTIFY_ALL); return BlockActionResult::Consume;
            }
        }
        BlockActionResult::Pass
    }

    /// `VineBlock.randomTick` (`VineBlock.java:169-248`), whose spreading body is
    /// `VineBlock.randomTick` (`VineBlock.java:171-247`).
    // `VineBlock.java:249-260` supplies the random-face copy used by downward spread.
    #[expect(clippy::too_many_lines)]
    fn random_tick(&self, args: RandomTickArgs<'_>) {
        let do_spread = matches!(args.world.level_info.load().game_rules.get(&GameRule::SpreadVines), GameRuleValue::Bool(true));
        if !do_spread || rand::rng().random_range(0..4) != 0 { return; }
        let direction = BlockDirection::all()[rand::rng().random_range(0..6)];
        let above = args.position.up(); let state_id = args.world.get_block_state_id(args.position); let state = VineLikeProperties::from_state_id(state_id, args.block);
        if direction.is_horizontal() && !has_face_property(&state, direction) {
            if !can_spread(args.world, args.position) { return; }
            let target = args.position.offset(direction.to_offset()); let (target_block, target_state) = args.world.get_block_and_state(&target);
            if target_block.default_state.is_air() {
                let cw = direction.rotate_clockwise(); let ccw = direction.rotate_counter_clockwise(); let cw_pos = target.offset(cw.to_offset()); let ccw_pos = target.offset(ccw.to_offset());
                let (cw_b,cw_s) = args.world.get_block_and_state(&cw_pos); let (ccw_b,ccw_s) = args.world.get_block_and_state(&ccw_pos);
                if has_face_property(&state,cw) && is_acceptable_neighbour(cw_b,cw_s,cw) { let mut p=VineLikeProperties::default(args.block); set_face_property(&mut p,cw,true); args.world.set_block_state(&target,p.to_state_id(args.block),BlockFlags::NOTIFY_ALL); }
                else if has_face_property(&state,ccw) && is_acceptable_neighbour(ccw_b,ccw_s,ccw) { let mut p=VineLikeProperties::default(args.block); set_face_property(&mut p,ccw,true); args.world.set_block_state(&target,p.to_state_id(args.block),BlockFlags::NOTIFY_ALL); }
                else { let opposite=direction.opposite(); let (cw_sb,cw_ss)=args.world.get_block_and_state(&args.position.offset(cw.to_offset())); let (ccw_sb,ccw_ss)=args.world.get_block_and_state(&args.position.offset(ccw.to_offset()));
                    if has_face_property(&state,cw) && cw_b.default_state.is_air() && is_acceptable_neighbour(cw_sb,cw_ss,opposite) { let mut p=VineLikeProperties::default(args.block); set_face_property(&mut p,opposite,true); args.world.set_block_state(&cw_pos,p.to_state_id(args.block),BlockFlags::NOTIFY_ALL); }
                    else if has_face_property(&state,ccw) && ccw_b.default_state.is_air() && is_acceptable_neighbour(ccw_sb,ccw_ss,opposite) { let mut p=VineLikeProperties::default(args.block); set_face_property(&mut p,opposite,true); args.world.set_block_state(&ccw_pos,p.to_state_id(args.block),BlockFlags::NOTIFY_ALL); }
                    else if rand::rng().random_range(0.0..1.0f32)<0.05 && is_acceptable_neighbour(args.world.get_block(&target.up()),args.world.get_block_state(&target.up()),BlockDirection::Up) { let mut p=VineLikeProperties::default(args.block); p.up=true; args.world.set_block_state(&target,p.to_state_id(args.block),BlockFlags::NOTIFY_ALL); }
                }
            } else if is_acceptable_neighbour(target_block,target_state,direction) { let mut p=state; set_face_property(&mut p,direction,true); args.world.set_block_state(args.position,p.to_state_id(args.block),BlockFlags::NOTIFY_ALL); }
        } else if direction == BlockDirection::Up && args.position.0.y < args.world.dimension.min_y + args.world.dimension.height - 1 {
            if can_support_at_face(&**args.world,args.position,direction) { let mut p=state; p.up=true; args.world.set_block_state(args.position,p.to_state_id(args.block),BlockFlags::NOTIFY_ALL); return; }
            if args.world.get_block(&above).default_state.is_air() && can_spread(args.world,args.position) { let mut p=state; for d in [BlockDirection::North,BlockDirection::South,BlockDirection::West,BlockDirection::East] { let q=above.offset(d.to_offset()); let (b,s)=args.world.get_block_and_state(&q); if rand::rng().random_range(0..2)==0 || !is_acceptable_neighbour(b,s,d) { set_face_property(&mut p,d,false); }} if has_horizontal_connection(&p) { args.world.set_block_state(&above,p.to_state_id(args.block),BlockFlags::NOTIFY_ALL); } }
        } else if args.position.0.y > args.world.dimension.min_y {
            let below=args.position.down(); let (b,s)=args.world.get_block_and_state(&below); if b.default_state.is_air() || b==&Block::VINE { let before=if b.default_state.is_air(){VineLikeProperties::default(args.block)}else{VineLikeProperties::from_state_id(s.id,b)}; let mut after=before; for d in [BlockDirection::North,BlockDirection::South,BlockDirection::West,BlockDirection::East] { if rand::rng().random_range(0..2)==0 && has_face_property(&state,d){set_face_property(&mut after,d,true);} } if before!=after && has_horizontal_connection(&after){args.world.set_block_state(&below,after.to_state_id(args.block),BlockFlags::NOTIFY_ALL);} }
        }
    }

    fn rotate(&self, block: &Block, state_id: BlockStateId, rotation: Rotation) -> &'static BlockState {
        let p=VineLikeProperties::from_state_id(state_id,block); let mut r=p; match rotation { Rotation::Rotate180=>{r.north=p.south;r.east=p.west;r.south=p.north;r.west=p.east}, Rotation::CounterClockwise90=>{r.north=p.east;r.east=p.south;r.south=p.west;r.west=p.north}, Rotation::Clockwise90=>{r.north=p.west;r.east=p.north;r.south=p.east;r.west=p.south}, Rotation::None=>{} } BlockState::from_id(r.to_state_id(block))
    }
    fn mirror(&self, block: &Block, state_id: BlockStateId, mirror: Mirror) -> &'static BlockState {
        let p=VineLikeProperties::from_state_id(state_id,block); let mut r=p; match mirror { Mirror::LeftRight=>{r.north=p.south;r.south=p.north}, Mirror::FrontBack=>{r.east=p.west;r.west=p.east}, Mirror::None=>{} } BlockState::from_id(r.to_state_id(block))
    }
}
