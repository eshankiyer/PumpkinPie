use std::sync::Arc;

use crate::{
    block::{
        BlockBehaviour, BlockFuture, BrokenArgs, CanPlaceAtArgs, GetStateForNeighborUpdateArgs,
        OnPlaceArgs, OnScheduledTickArgs, PlacedArgs, RandomTickArgs,
        blocks::cauldron::can_receive_stalactite_drip,
    },
    entity::{falling::FallingEntity, player::Player},
    world::{
        World,
        game_event::{GameEventContext, emit_game_event},
    },
};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{
    Block, BlockDirection, BlockState, BlockStateId,
    block_properties::{
        BlockProperties, PointedDripstoneLikeProperties, SpeleothemThickness, VerticalDirection,
    },
    dimension::Dimension,
    fluid::Fluid,
    game_event::GameEvent,
    world::WorldEvent,
};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::{BlockAccessor, BlockFlags};

/// Ticks between a stalactite losing its support and starting to fall. This matches the delay
/// this repository already uses for every other falling block (`FallingBlock` and
/// `ConcretePowderBlock` both schedule 2 ticks); vanilla's own constant could not be verified
/// from an available source, and the wiki does not state one.
const DELAY_BEFORE_FALLING: u8 = 2;

/// Cap on the damage a falling stalactite can deal. minecraft.wiki, "Pointed Dripstone":
/// the damage is capped no matter how far the stalactite falls.
const STALACTITE_MAX_DAMAGE: i32 = 40;

/// Damage per block of fall distance for a falling stalactite.
///
/// minecraft.wiki, "Pointed Dripstone" (Java Edition): "the amount of damage is 1HP per pointed
/// dripstone falling (less than 6 will be counted as 6) per each block of falling distance".
#[must_use]
pub fn stalactite_damage_per_distance(block_count: usize) -> f32 {
    block_count.max(6) as f32
}

/// Any block in `#minecraft:speleothems`.
///
/// `SpeleothemBlock.isSpeleothemWithDirection` (SpeleothemBlock.java:182-184) tests
/// `blockState.is(BlockTags.SPELEOTHEMS)`, not a single block, so pointed dripstone and
/// sulfur spikes share every direction-based predicate below.
#[must_use]
pub fn is_speleothem(block: &Block) -> bool {
    block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_SPELEOTHEMS)
}

/// A downward-pointing speleothem, i.e. part of a stalactite.
///
/// `SpeleothemBlock.isStalactite` (SpeleothemBlock.java:231-233).
#[must_use]
pub fn is_stalactite(block: &Block, state_id: BlockStateId) -> bool {
    is_speleothem(block)
        && PointedDripstoneLikeProperties::from_state_id(state_id, block).vertical_direction
            == VerticalDirection::Down
}

/// The lowest block of a stalactite, including the merged-tip form.
const fn is_tip_thickness(thickness: SpeleothemThickness) -> bool {
    matches!(
        thickness,
        SpeleothemThickness::Tip | SpeleothemThickness::TipMerge
    )
}

/// Turns the unsupported stalactite starting at `position` into falling block entities.
///
/// minecraft.wiki, "Pointed Dripstone": "If the block supporting a stalactite or any block of
/// the stalactite is broken, all of the unsupported pointed dripstone below the broken block
/// drops, causing damage to any player and mobs standing beneath it, similar to a falling
/// anvil." Only the tip segment hurts entities; the damage scales with the number of blocks
/// in the falling stalactite (see [`stalactite_damage_per_distance`]).
async fn spawn_falling_stalactite(world: &Arc<World>, position: &BlockPos) {
    let mut segments = Vec::new();
    let mut current = *position;
    let mut has_tip = false;
    loop {
        let (block, state) = world.get_block_and_state(&current);
        if !is_stalactite(block, state.id) {
            break;
        }
        segments.push((current, state.id));
        let props = PointedDripstoneLikeProperties::from_state_id(state.id, block);
        if is_tip_thickness(props.thickness) {
            has_tip = true;
            break;
        }
        current = current.down();
    }

    let last_index = segments.len().saturating_sub(1);
    let damage_per_distance = stalactite_damage_per_distance(segments.len());
    for (index, (pos, state_id)) in segments.into_iter().enumerate() {
        let hurts = (has_tip && index == last_index)
            .then_some((damage_per_distance, STALACTITE_MAX_DAMAGE));
        FallingEntity::replace_spawn_hurting(world, pos, state_id, hurts).await;
    }
}

/// `PointedDripstoneBlock.MAX_SEARCH_LENGTH_WHEN_CHECKING_DRIP_TYPE` and
/// `MAX_SEARCH_LENGTH_BETWEEN_STALACTITE_TIP_AND_CAULDRON` (`PointedDripstoneBlock.java:38,41`).
const MAX_DRIP_SEARCH_LENGTH: i32 = 11;

/// `PointedDripstoneBlock.WATER_TRANSFER_PROBABILITY_PER_RANDOM_TICK` and
/// `LAVA_TRANSFER_PROBABILITY_PER_RANDOM_TICK` (`PointedDripstoneBlock.java:42-43`).
const WATER_TRANSFER_PROBABILITY: f32 = 0.175_781_25;
const LAVA_TRANSFER_PROBABILITY: f32 = 0.058_593_75;

/// The two source fluids a stalactite can carry into a cauldron.
///
/// `PointedDripstoneBlock.canFillCauldron` (`PointedDripstoneBlock.java:196-198`). Every other
/// fluid above a stalactite (flowing fluids, `EMPTY`) is represented as `None`; none of the
/// callers distinguish between them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DripFluid {
    Water,
    Lava,
}

/// `PointedDripstoneBlock.FluidInfo` (`PointedDripstoneBlock.java:236-237`).
struct FluidAboveStalactite {
    pos: BlockPos,
    fluid: Option<DripFluid>,
    source_block: &'static Block,
}

/// What a `findBlockVertical` probe decided about one position.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum VerticalProbe {
    Found,
    Continue,
    Stop,
}

/// The stepping of `SpeleothemBlock.findBlockVertical` (`SpeleothemBlock.java:340-362`):
/// `i` runs from 1 while `i < max_steps`, so at most `max_steps - 1` positions are probed.
fn find_block_vertical_by(
    start: BlockPos,
    up: bool,
    max_steps: i32,
    mut probe: impl FnMut(&BlockPos) -> VerticalProbe,
) -> Option<BlockPos> {
    let mut pos = start;
    for _ in 1..max_steps {
        pos = if up { pos.up() } else { pos.down() };
        match probe(&pos) {
            VerticalProbe::Found => return Some(pos),
            VerticalProbe::Stop => return None,
            VerticalProbe::Continue => {}
        }
    }
    None
}

/// `SpeleothemBlock.findBlockVertical`: the target is tested before the build-height and path
/// checks.
fn find_block_vertical(
    world: &World,
    start: &BlockPos,
    up: bool,
    path: impl Fn(&BlockPos, &Block, &BlockState) -> bool,
    target: impl Fn(&Block, &BlockState) -> bool,
    max_steps: i32,
) -> Option<BlockPos> {
    find_block_vertical_by(*start, up, max_steps, |pos| {
        let (block, state) = world.get_block_and_state(pos);
        if target(block, state) {
            VerticalProbe::Found
        } else if !world.is_in_height_limit(pos.0.y) || !path(pos, block, state) {
            VerticalProbe::Stop
        } else {
            VerticalProbe::Continue
        }
    })
}

/// `SpeleothemBlock.isTip` (`SpeleothemBlock.java:235-242`).
fn is_tip(block: &Block, state_id: BlockStateId, include_merged_tip: bool) -> bool {
    if !is_speleothem(block) {
        return false;
    }
    let thickness = PointedDripstoneLikeProperties::from_state_id(state_id, block).thickness;
    thickness == SpeleothemThickness::Tip
        || (include_merged_tip && thickness == SpeleothemThickness::TipMerge)
}

/// `SpeleothemBlock.isStalactiteStartPos` (`SpeleothemBlock.java:297-299`).
fn is_stalactite_start_pos(
    world: &World,
    block: &Block,
    state_id: BlockStateId,
    pos: &BlockPos,
) -> bool {
    is_stalactite(block, state_id) && world.get_block(&pos.up()) != block
}

/// `SpeleothemBlock.isFreeHangingStalactite` (`SpeleothemBlock.java:444-446`).
fn is_free_hanging_stalactite(block: &Block, state_id: BlockStateId) -> bool {
    if !is_stalactite(block, state_id) {
        return false;
    }
    let props = PointedDripstoneLikeProperties::from_state_id(state_id, block);
    props.thickness == SpeleothemThickness::Tip && !props.waterlogged
}

/// The path predicate shared by `findTip` and `findRootBlock`: the same block pointing the
/// same way.
fn same_speleothem_path(
    speleothem: &Block,
    direction: VerticalDirection,
) -> impl Fn(&BlockPos, &Block, &BlockState) -> bool {
    move |_, block, state| {
        block == speleothem
            && PointedDripstoneLikeProperties::from_state_id(state.id, block).vertical_direction
                == direction
    }
}

/// `SpeleothemBlock.findTip` (`SpeleothemBlock.java:324-338`).
fn find_tip(
    world: &World,
    block: &Block,
    state_id: BlockStateId,
    pos: &BlockPos,
    max_search_length: i32,
    include_merged_tip: bool,
) -> Option<BlockPos> {
    if is_tip(block, state_id, include_merged_tip) {
        return Some(*pos);
    }
    let direction = PointedDripstoneLikeProperties::from_state_id(state_id, block).vertical_direction;
    find_block_vertical(
        world,
        pos,
        direction == VerticalDirection::Up,
        same_speleothem_path(block, direction),
        |target, target_state| is_tip(target, target_state.id, include_merged_tip),
        max_search_length,
    )
}

/// `PointedDripstoneBlock.findRootBlock` (`PointedDripstoneBlock.java:151-158`): the first
/// block behind the speleothem that is not part of it.
fn find_root_block(
    world: &World,
    block: &Block,
    state_id: BlockStateId,
    pos: &BlockPos,
    max_search_length: i32,
) -> Option<BlockPos> {
    let direction = PointedDripstoneLikeProperties::from_state_id(state_id, block).vertical_direction;
    find_block_vertical(
        world,
        pos,
        direction == VerticalDirection::Down,
        same_speleothem_path(block, direction),
        |target, _| target != block,
        max_search_length,
    )
}

/// `PointedDripstoneBlock.getFluidAboveStalactite` (`PointedDripstoneBlock.java:179-194`).
fn fluid_above_stalactite(
    world: &World,
    block: &Block,
    state_id: BlockStateId,
    pos: &BlockPos,
) -> Option<FluidAboveStalactite> {
    if !is_stalactite(block, state_id) {
        return None;
    }
    let root = find_root_block(world, block, state_id, pos, MAX_DRIP_SEARCH_LENGTH)?;
    let above = root.up();
    let source_block = world.get_block(&above);
    // `WATER_EVAPORATES` is modelled as the nether, as elsewhere in this repository.
    let fluid = if source_block == &Block::MUD && world.dimension != Dimension::THE_NETHER {
        Some(DripFluid::Water)
    } else {
        // `getFluidState().getType()` is the source fluid only for a source; flowing fluids are
        // distinct types that `canFillCauldron` rejects.
        let (fluid, fluid_state) = world.get_fluid_and_fluid_state(&above);
        if !fluid_state.is_source {
            None
        } else if fluid.matches_type(&Fluid::WATER) {
            Some(DripFluid::Water)
        } else if fluid.matches_type(&Fluid::LAVA) {
            Some(DripFluid::Lava)
        } else {
            None
        }
    };
    Some(FluidAboveStalactite {
        pos: above,
        fluid,
        source_block,
    })
}

/// `PointedDripstoneBlock.REQUIRED_SPACE_TO_DRIP_THROUGH_NON_SOLID_BLOCK`, i.e.
/// `Block.column(4, 0, 16)` (`PointedDripstoneBlock.java:47`, `Block.java:177-185`).
fn drip_column() -> BoundingBox {
    BoundingBox::new(
        Vector3::new(6.0 / 16.0, 0.0, 6.0 / 16.0),
        Vector3::new(10.0 / 16.0, 1.0, 10.0 / 16.0),
    )
}

/// The collision half of `PointedDripstoneBlock.canDripThrough`: whether any collision box of
/// `state` overlaps the drip column.
fn collision_blocks_drip(state: &BlockState, pos: &BlockPos) -> bool {
    let column = drip_column();
    state
        .get_block_collision_shapes_at(pos)
        .any(|shape| shape.intersects(&column))
}

/// `PointedDripstoneBlock.canDripThrough` (`PointedDripstoneBlock.java:219-234`).
fn can_drip_through(world: &World, pos: &BlockPos, state: &BlockState) -> bool {
    if state.is_air() {
        return true;
    }
    if state.is_solid_render() {
        return false;
    }
    if !world.get_fluid_and_fluid_state(pos).1.is_empty {
        return false;
    }
    !collision_blocks_drip(state, pos)
}

/// `PointedDripstoneBlock.findStalactiteTipAboveCauldron` (`PointedDripstoneBlock.java:167-170`).
fn find_stalactite_tip_above_cauldron(world: &World, cauldron_pos: &BlockPos) -> Option<BlockPos> {
    find_block_vertical(
        world,
        cauldron_pos,
        true,
        |pos, _, state| can_drip_through(world, pos, state),
        |block, state| is_free_hanging_stalactite(block, state.id),
        MAX_DRIP_SEARCH_LENGTH,
    )
}

/// `PointedDripstoneBlock.getCauldronFillFluidType` (`PointedDripstoneBlock.java:172-177`);
/// `None` stands for `Fluids.EMPTY`.
fn cauldron_fill_fluid_type(world: &World, stalactite_pos: &BlockPos) -> Option<DripFluid> {
    let (block, state_id) = world.get_block_and_state_id(stalactite_pos);
    fluid_above_stalactite(world, block, state_id, stalactite_pos)?.fluid
}

/// The fluid a stalactite above `cauldron_pos` would drip into it, as read by
/// `AbstractCauldronBlock.tick` (`AbstractCauldronBlock.java:91-100`).
pub fn stalactite_drip_fluid_above_cauldron(
    world: &World,
    cauldron_pos: &BlockPos,
) -> Option<DripFluid> {
    let tip = find_stalactite_tip_above_cauldron(world, cauldron_pos)?;
    cauldron_fill_fluid_type(world, &tip)
}

/// `PointedDripstoneBlock.findFillableCauldronBelowStalactiteTip`
/// (`PointedDripstoneBlock.java:160-165`). Fullness is not checked here.
fn find_fillable_cauldron_below_stalactite_tip(
    world: &World,
    tip: &BlockPos,
    fluid: DripFluid,
) -> Option<BlockPos> {
    find_block_vertical(
        world,
        tip,
        false,
        |pos, _, state| can_drip_through(world, pos, state),
        |block, _| can_receive_stalactite_drip(block, fluid),
        MAX_DRIP_SEARCH_LENGTH,
    )
}

/// `PointedDripstoneBlock.maybeTransferFluid` (`PointedDripstoneBlock.java:90-132`).
///
/// The `Block.pushEntitiesUp` call of the mud-to-clay branch is not modelled.
async fn maybe_transfer_fluid(world: &Arc<World>, pos: &BlockPos, random_value: f32) {
    // `!(r > WATER) || !(r > LAVA)` reduces to `r <= WATER`.
    if random_value > WATER_TRANSFER_PROBABILITY {
        return;
    }
    let (block, state_id) = world.get_block_and_state_id(pos);
    if !is_stalactite_start_pos(world, block, state_id, pos) {
        return;
    }
    let Some(info) = fluid_above_stalactite(world, block, state_id, pos) else {
        return;
    };
    let (fluid, transfer_probability) = match info.fluid {
        Some(DripFluid::Water) => (DripFluid::Water, WATER_TRANSFER_PROBABILITY),
        Some(DripFluid::Lava) => (DripFluid::Lava, LAVA_TRANSFER_PROBABILITY),
        None => return,
    };
    if random_value >= transfer_probability {
        return;
    }
    let Some(tip) = find_tip(world, block, state_id, pos, MAX_DRIP_SEARCH_LENGTH, false) else {
        return;
    };

    if info.source_block == &Block::MUD && fluid == DripFluid::Water {
        let clay = Block::CLAY.default_state.id;
        world
            .set_block_state(&info.pos, clay, BlockFlags::NOTIFY_ALL)
            .await;
        emit_game_event(
            world,
            GameEvent::BlockChange,
            info.pos.to_centered_f64(),
            GameEventContext {
                source_entity: None,
                affected_block_state: Some(clay),
            },
        )
        .await;
        world.sync_world_event(WorldEvent::DripstoneDrip, tip, 0);
    } else if let Some(cauldron_pos) = find_fillable_cauldron_below_stalactite_tip(world, &tip, fluid)
    {
        world.sync_world_event(WorldEvent::DripstoneDrip, tip, 0);
        // At most `50 + 10`: the cauldron search probes ten blocks below the tip.
        let delay = 50 + (tip.0.y - cauldron_pos.0.y);
        let cauldron = world.get_block(&cauldron_pos);
        world.schedule_block_tick(
            cauldron,
            cauldron_pos,
            u8::try_from(delay).unwrap_or(u8::MAX),
            TickPriority::Normal,
        );
    }
}

/// Shared behaviour for every block in `#minecraft:speleothems`.
///
/// That tag holds `pointed_dripstone` and `sulfur_spike`. `Blocks.java:5342-5344` registers the
/// latter as a `SulfurSpikeBlock`, a `SpeleothemBlock` subclass whose only overrides are its
/// stalactite landing sound and max growth length (SulfurSpikeBlock.java:23-30) -- neither of
/// which this port implements, since speleothem growth is not implemented here.
#[pumpkin_block_from_tag("minecraft:speleothems")]
pub struct DripstoneBlock;

impl BlockBehaviour for DripstoneBlock {
    /// `PointedDripstoneBlock.randomTick` (`PointedDripstoneBlock.java:84-88`). Sulfur spikes do
    /// not override it, and the speleothem growth of `super.randomTick` is not implemented.
    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.block != &Block::POINTED_DRIPSTONE {
                return;
            }
            let random_value: f32 = rand::rng().random();
            maybe_transfer_fluid(args.world, args.position, random_value).await;
        })
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_place_at_pos(
            args.block_accessor,
            args.block,
            args.position,
            args.direction,
            args.player,
        )
    }
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut dripstone_props = PointedDripstoneLikeProperties::default(args.block);
            dripstone_props.waterlogged = args.replacing.water_source();
            let Some(support_block_ver_dir) = get_support_block_vertical_direction(
                args.world,
                args.block,
                args.position,
                Some(args.direction),
                Some(args.player),
            ) else {
                //this shouldn't happen
                return Block::AIR.default_state.id;
            };

            dripstone_props.vertical_direction = flip_dir(support_block_ver_dir);
            dripstone_props.to_state_id(args.block)
        })
    }
    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let (len, vertical_dir) = get_stalagmite_or_stalactice_len_and_dir_from_tip_pos(
                args.world,
                args.block,
                args.position,
                args.state_id,
            );
            match vertical_dir {
                VerticalDirection::Up => {
                    update_stalagmite(args.world, args.block, len, args.position).await;
                }
                VerticalDirection::Down => {
                    update_stalactite(args.world, args.block, len, args.position).await;
                }
            }
        })
    }
    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let broken_dripstone_props =
                PointedDripstoneLikeProperties::from_state_id(args.state.id, args.block);
            let new_tip_pos = match broken_dripstone_props.vertical_direction {
                VerticalDirection::Up => args.position.down(),
                VerticalDirection::Down => args.position.up(),
            };

            let (len, vertical_dir) = get_stalagmite_or_stalactice_len_and_dir_from_tip_pos(
                args.world,
                args.block,
                &new_tip_pos,
                args.state.id,
            );
            match vertical_dir {
                VerticalDirection::Up => {
                    update_stalagmite(args.world, args.block, len, &new_tip_pos).await;
                }
                VerticalDirection::Down => {
                    update_stalactite(args.world, args.block, len, &new_tip_pos).await;
                }
            }
        })
    }
    /// Fires `DELAY_BEFORE_FALLING` ticks after a stalactite lost its support. The guard is
    /// required: turning each segment into a falling entity replaces it with air and notifies
    /// its neighbors, which schedules further ticks for segments this call already consumed.
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let (block, state) = args.world.get_block_and_state(args.position);
            if !is_stalactite(block, state.id)
                || can_place_at_pos(&**args.world, block, args.position, None, None)
            {
                return;
            }
            spawn_falling_stalactite(args.world, args.position).await;
        })
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if !can_place_at_pos(args.world, args.block, args.position, None, None) {
                // An unsupported stalactite does not vanish: it falls (see
                // `spawn_falling_stalactite`). Stalagmites keep breaking immediately.
                if is_stalactite(args.block, args.state_id) {
                    args.world.schedule_block_tick(
                        args.block,
                        *args.position,
                        DELAY_BEFORE_FALLING,
                        TickPriority::Normal,
                    );
                    return args.state_id;
                }
                return Block::AIR.default_state.id;
            }
            let mut dripstone_props =
                PointedDripstoneLikeProperties::from_state_id(args.state_id, args.block);
            if dripstone_props.thickness != SpeleothemThickness::TipMerge {
                return args.state_id;
            }
            match dripstone_props.vertical_direction {
                VerticalDirection::Up => {
                    let block_above = args.world.get_block(&args.position.up());
                    if block_above != args.block {
                        dripstone_props.thickness = SpeleothemThickness::Tip;
                        return dripstone_props.to_state_id(args.block);
                    }
                }
                VerticalDirection::Down => {
                    let block_below = args.world.get_block(&args.position.down());
                    if block_below != args.block {
                        dripstone_props.thickness = SpeleothemThickness::Tip;
                        return dripstone_props.to_state_id(args.block);
                    }
                }
            }
            args.state_id
        })
    }
}
async fn update_stalagmite(
    world: &Arc<World>,
    speleothem: &Block,
    stalagmite_len: u8,
    tip_pos: &BlockPos,
) {
    let block_above = world.get_block(&tip_pos.up());
    if block_above == speleothem {
        modify_dripstone_thickness_to(world, speleothem, tip_pos, SpeleothemThickness::TipMerge)
            .await;
        modify_dripstone_thickness_to(
            world,
            speleothem,
            &tip_pos.up(),
            SpeleothemThickness::TipMerge,
        )
        .await;
    } else {
        modify_dripstone_thickness_to(world, speleothem, tip_pos, SpeleothemThickness::Tip).await;
    }
    match stalagmite_len {
        2 => {
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.down_height(1),
                SpeleothemThickness::Frustum,
            )
            .await;
        }
        3 => {
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.down_height(1),
                SpeleothemThickness::Frustum,
            )
            .await;
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.down_height(2),
                SpeleothemThickness::Base,
            )
            .await;
        }
        4 => {
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.down_height(1),
                SpeleothemThickness::Frustum,
            )
            .await;
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.down_height(2),
                SpeleothemThickness::Middle,
            )
            .await;
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.down_height(3),
                SpeleothemThickness::Base,
            )
            .await;
        }
        5 => {
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.down_height(1),
                SpeleothemThickness::Frustum,
            )
            .await;
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.down_height(2),
                SpeleothemThickness::Middle,
            )
            .await;
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.down_height(3),
                SpeleothemThickness::Middle,
            )
            .await;
        }
        _ => {}
    }
}

async fn update_stalactite(
    world: &Arc<World>,
    speleothem: &Block,
    stalagmite_len: u8,
    tip_pos: &BlockPos,
) {
    let block_below = world.get_block(&tip_pos.down());
    if block_below == speleothem {
        modify_dripstone_thickness_to(world, speleothem, tip_pos, SpeleothemThickness::TipMerge)
            .await;
        modify_dripstone_thickness_to(
            world,
            speleothem,
            &tip_pos.down(),
            SpeleothemThickness::TipMerge,
        )
        .await;
    } else {
        modify_dripstone_thickness_to(world, speleothem, tip_pos, SpeleothemThickness::Tip).await;
    }
    match stalagmite_len {
        2 => {
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.up_height(1),
                SpeleothemThickness::Frustum,
            )
            .await;
        }
        3 => {
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.up_height(1),
                SpeleothemThickness::Frustum,
            )
            .await;
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.up_height(2),
                SpeleothemThickness::Base,
            )
            .await;
        }
        4 => {
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.up_height(1),
                SpeleothemThickness::Frustum,
            )
            .await;
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.up_height(2),
                SpeleothemThickness::Middle,
            )
            .await;
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.up_height(3),
                SpeleothemThickness::Base,
            )
            .await;
        }
        5 => {
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.up_height(1),
                SpeleothemThickness::Frustum,
            )
            .await;
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.up_height(2),
                SpeleothemThickness::Middle,
            )
            .await;
            modify_dripstone_thickness_to(
                world,
                speleothem,
                &tip_pos.up_height(3),
                SpeleothemThickness::Middle,
            )
            .await;
        }
        _ => {}
    }
}
fn get_stalagmite_or_stalactice_len_and_dir_from_tip_pos(
    world: &Arc<World>,
    speleothem: &Block,
    position: &BlockPos,
    block_state_id: BlockStateId,
) -> (u8, VerticalDirection) {
    let props = PointedDripstoneLikeProperties::from_state_id(block_state_id, speleothem);

    let mut dripstone_len = 1;
    let mut next_dripstone_pos = offset_pos_by_vertical_dir(position, props.vertical_direction);
    //We dont care if it's longer than 5 blocks because of how thickness system works.
    while dripstone_len < 5 {
        if world.get_block(&next_dripstone_pos) != speleothem {
            break;
        }
        next_dripstone_pos =
            offset_pos_by_vertical_dir(&next_dripstone_pos, props.vertical_direction);
        dripstone_len += 1;
    }
    (dripstone_len, props.vertical_direction)
}
fn can_place_at_pos(
    block_accessor: &dyn BlockAccessor,
    speleothem: &Block,
    position: &BlockPos,
    placing_direction: Option<BlockDirection>,
    player_option: Option<&Player>,
) -> bool {
    // Determine support block
    let Some(support_block_vertical_direction) = get_support_block_vertical_direction(
        block_accessor,
        speleothem,
        position,
        placing_direction,
        player_option,
    ) else {
        return false;
    };
    let support_pos = match support_block_vertical_direction {
        VerticalDirection::Up => position.up(),
        VerticalDirection::Down => position.down(),
    };
    let support_block = block_accessor.get_block(&support_pos);
    if can_support_dripstone(support_block, speleothem) {
        return true;
    }
    false
}

fn get_support_block_vertical_direction(
    block_accessor: &dyn BlockAccessor,
    speleothem: &Block,
    position: &BlockPos,
    placing_direction_wrapper: Option<BlockDirection>,
    player_option: Option<&Player>,
) -> Option<VerticalDirection> {
    let Some(placing_direction) = placing_direction_wrapper else {
        //then this is basically called by a neighbor update check
        let (block, state) = block_accessor.get_block_and_state(position);
        if block != speleothem {
            return None;
        }
        let props = PointedDripstoneLikeProperties::from_state_id(state.id, block);
        return Some(flip_dir(props.vertical_direction));
    };
    match block_direction_to_vertical_direction(placing_direction) {
        Some(ver_dir) => match ver_dir {
            VerticalDirection::Up => {
                let block_above = block_accessor.get_block(&position.up());
                let block_below = block_accessor.get_block(&position.down());
                if can_support_dripstone(block_above, speleothem) {
                    return Some(VerticalDirection::Up);
                } else if can_support_dripstone(block_below, speleothem) {
                    return Some(VerticalDirection::Down);
                }
                None
            }
            VerticalDirection::Down => {
                let block_above = block_accessor.get_block(&position.up());
                let block_below = block_accessor.get_block(&position.down());
                if can_support_dripstone(block_below, speleothem) {
                    return Some(VerticalDirection::Down);
                } else if can_support_dripstone(block_above, speleothem) {
                    return Some(VerticalDirection::Up);
                }
                None
            }
        },
        None => player_option.map_or(Some(VerticalDirection::Up), |player| {
            let (_, pitch) = player.rotation();
            let (can_place_above, can_place_below) = {
                let block_above = block_accessor.get_block(&position.up());
                let block_below = block_accessor.get_block(&position.down());
                (
                    can_support_dripstone(block_above, speleothem),
                    can_support_dripstone(block_below, speleothem),
                )
            };
            match (can_place_above, can_place_below) {
                (true, true) => {
                    if pitch > 0.0 {
                        Some(VerticalDirection::Down)
                    } else {
                        Some(VerticalDirection::Up)
                    }
                }
                (false, false) => None,
                (true, false) => Some(VerticalDirection::Up),
                (false, true) => Some(VerticalDirection::Down),
            }
        }),
    }
}
/// `SpeleothemBlock.isValidSpeleothemPlacement` (SpeleothemBlock.java:174-180): a sturdy face
/// behind, or another speleothem of the *same* block (`behindState.is(this)`), so a sulfur
/// spike never hangs off a pointed dripstone.
fn can_support_dripstone(support_block: &Block, speleothem: &Block) -> bool {
    if support_block == speleothem {
        return true;
    }
    if support_block.default_state.is_full_cube() && support_block.default_state.is_solid_block() {
        return true;
    }
    false
}
async fn modify_dripstone_thickness_to(
    world: &Arc<World>,
    speleothem: &Block,
    pos: &BlockPos,
    new_thickness: SpeleothemThickness,
) {
    let (block, support_block_state_id) = world.get_block_and_state_id(pos);

    if block != speleothem {
        //this shouldn't happen
        return;
    }
    let mut support_props =
        PointedDripstoneLikeProperties::from_state_id(support_block_state_id, block);
    if support_props.thickness == new_thickness {
        return;
    }
    support_props.thickness = new_thickness;
    world
        .set_block_state(
            pos,
            support_props.to_state_id(speleothem),
            BlockFlags::empty(),
        )
        .await;
}
fn offset_pos_by_vertical_dir(pos: &BlockPos, ver_dir: VerticalDirection) -> BlockPos {
    match ver_dir {
        VerticalDirection::Up => pos.down(),
        VerticalDirection::Down => pos.up(),
    }
}
const fn block_direction_to_vertical_direction(dir: BlockDirection) -> Option<VerticalDirection> {
    match dir {
        BlockDirection::Up => Some(VerticalDirection::Up),
        BlockDirection::Down => Some(VerticalDirection::Down),
        _ => None,
    }
}
fn flip_dir(dir: VerticalDirection) -> VerticalDirection {
    if dir == VerticalDirection::Up {
        return VerticalDirection::Down;
    }
    VerticalDirection::Up
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalactite_damage_clamps_to_six() {
        assert!((stalactite_damage_per_distance(1) - 6.0).abs() < f32::EPSILON);
        assert!((stalactite_damage_per_distance(5) - 6.0).abs() < f32::EPSILON);
        assert!((stalactite_damage_per_distance(6) - 6.0).abs() < f32::EPSILON);
    }

    #[test]
    fn stalactite_damage_scales_past_six() {
        assert!((stalactite_damage_per_distance(7) - 7.0).abs() < f32::EPSILON);
        assert!((stalactite_damage_per_distance(20) - 20.0).abs() < f32::EPSILON);
    }

    #[test]
    fn stalactite_damage_feeds_the_shared_fall_damage_formula() {
        // A 6-block stalactite falling 10 blocks: 6 * 10 = 60, capped at 40.
        let per_distance = stalactite_damage_per_distance(6);
        let damage = crate::block::blocks::falling::fall_damage_amount(
            10,
            per_distance,
            STALACTITE_MAX_DAMAGE,
        );
        assert!((damage - 40.0).abs() < f32::EPSILON);
        // The same stalactite falling 3 blocks: 6 * 3 = 18, under the cap.
        let damage = crate::block::blocks::falling::fall_damage_amount(
            3,
            per_distance,
            STALACTITE_MAX_DAMAGE,
        );
        assert!((damage - 18.0).abs() < f32::EPSILON);
    }

    #[test]
    fn tip_thickness_covers_merged_tips() {
        assert!(is_tip_thickness(SpeleothemThickness::Tip));
        assert!(is_tip_thickness(SpeleothemThickness::TipMerge));
        assert!(!is_tip_thickness(SpeleothemThickness::Frustum));
        assert!(!is_tip_thickness(SpeleothemThickness::Middle));
        assert!(!is_tip_thickness(SpeleothemThickness::Base));
    }

    #[test]
    fn is_stalactite_only_matches_downward_dripstone() {
        let mut props = PointedDripstoneLikeProperties::default(&Block::POINTED_DRIPSTONE);
        props.vertical_direction = VerticalDirection::Down;
        let down = props.to_state_id(&Block::POINTED_DRIPSTONE);
        props.vertical_direction = VerticalDirection::Up;
        let up = props.to_state_id(&Block::POINTED_DRIPSTONE);

        assert!(is_stalactite(&Block::POINTED_DRIPSTONE, down));
        assert!(!is_stalactite(&Block::POINTED_DRIPSTONE, up));
        assert!(!is_stalactite(&Block::STONE, Block::STONE.default_state.id));
    }

    #[test]
    fn find_block_vertical_probes_at_most_ten_positions() {
        let start = BlockPos::new(0, 0, 0);
        let mut probed = 0;
        let found = find_block_vertical_by(start, false, MAX_DRIP_SEARCH_LENGTH, |_| {
            probed += 1;
            VerticalProbe::Continue
        });
        assert_eq!(found, None);
        assert_eq!(probed, 10);

        let found = find_block_vertical_by(start, true, MAX_DRIP_SEARCH_LENGTH, |pos| {
            if pos.0.y == 10 {
                VerticalProbe::Found
            } else {
                VerticalProbe::Continue
            }
        });
        assert_eq!(found, Some(BlockPos::new(0, 10, 0)));

        let found = find_block_vertical_by(start, true, MAX_DRIP_SEARCH_LENGTH, |pos| {
            if pos.0.y == 11 {
                VerticalProbe::Found
            } else {
                VerticalProbe::Continue
            }
        });
        assert_eq!(found, None);
    }

    #[test]
    fn drip_column_is_blocked_by_fence_posts_and_carpets() {
        let pos = BlockPos::new(0, 0, 0);
        assert!(collision_blocks_drip(Block::OAK_FENCE.default_state, &pos));
        assert!(collision_blocks_drip(Block::WHITE_CARPET.default_state, &pos));
        assert!(!collision_blocks_drip(Block::AIR.default_state, &pos));
    }

    /// `#minecraft:speleothems` holds `pointed_dripstone` and `sulfur_spike`, and
    /// `SpeleothemBlock.isSpeleothemWithDirection` (SpeleothemBlock.java:182-184) keys off that
    /// tag, so sulfur spikes must run the same stalactite logic.
    #[test]
    fn sulfur_spike_is_a_speleothem() {
        assert!(is_speleothem(&Block::POINTED_DRIPSTONE));
        assert!(is_speleothem(&Block::SULFUR_SPIKE));
        assert!(!is_speleothem(&Block::STONE));

        let mut props = PointedDripstoneLikeProperties::default(&Block::SULFUR_SPIKE);
        props.vertical_direction = VerticalDirection::Down;
        let down = props.to_state_id(&Block::SULFUR_SPIKE);
        props.vertical_direction = VerticalDirection::Up;
        let up = props.to_state_id(&Block::SULFUR_SPIKE);

        assert!(is_stalactite(&Block::SULFUR_SPIKE, down));
        assert!(!is_stalactite(&Block::SULFUR_SPIKE, up));
    }
}
