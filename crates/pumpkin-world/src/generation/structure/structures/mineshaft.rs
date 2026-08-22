//! Port of vanilla 26.2 `MineshaftPieces.java` and `MineshaftStructure.java`
//! (`net/minecraft/world/level/levelgen/structure/structures/`).
//!
//! Carried across: the four piece kinds (room, corridor, crossing, stairs), the depth-first
//! `generateAndAddPiece` graph with its RNG call order, cobwebs, cave-spider spawners, chest
//! minecarts, wall torches, rails, and the plank floor / pillar-down-or-chain-up support logic.
//! Not carried across: NBT serialization of pieces (`addAdditionalSaveData`), because this
//! codebase recomputes structure starts on resume rather than persisting them (see
//! `proto_chunk.rs::from_chunk_data`).

use std::sync::{Arc, Mutex};

use pumpkin_data::BlockDirection as DataDirection;
use pumpkin_data::block_properties::{
    BlockProperties, HorizontalFacing, OakFenceLikeProperties, RailLikeProperties, RailShape,
    WallTorchLikeProperties,
};
use pumpkin_data::{Block, BlockState};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::{
    BlockDirection, HeightMap,
    math::{block_box::BlockBox, position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};

use crate::{
    ProtoChunk,
    generation::{
        positions::chunk_pos::{start_block_x, start_block_z},
        structure::{
            piece::StructurePieceType,
            structures::{
                StructureGenerator, StructureGeneratorContext, StructurePiece, StructurePieceBase,
                StructurePiecesCollector, StructurePosition, WorldPortalExt,
            },
        },
    },
};

/// `MineshaftPieces.MAX_DEPTH` (`MineshaftPieces.java:44`).
const MAX_DEPTH: u32 = 8;
/// `MineshaftPieces.MAGIC_START_Y` (`MineshaftPieces.java:45`).
const MAGIC_START_Y: i32 = 50;
/// `MineshaftPieces.MAX_PILLAR_HEIGHT` (`MineshaftPieces.java:42`).
const MAX_PILLAR_HEIGHT: i32 = 20;
/// `MineshaftPieces.MAX_CHAIN_HEIGHT` (`MineshaftPieces.java:43`).
const MAX_CHAIN_HEIGHT: i32 = 50;

const ABANDONED_MINESHAFT_LOOT: &str = "minecraft:chests/abandoned_mineshaft";

/// `MineshaftStructure.Type` (`MineshaftStructure.java:70-73`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MineshaftType {
    is_mesa: bool,
}

impl MineshaftType {
    const fn wood(self) -> &'static Block {
        if self.is_mesa {
            &Block::DARK_OAK_LOG
        } else {
            &Block::OAK_LOG
        }
    }

    const fn planks(self) -> &'static Block {
        if self.is_mesa {
            &Block::DARK_OAK_PLANKS
        } else {
            &Block::OAK_PLANKS
        }
    }

    const fn fence(self) -> &'static Block {
        if self.is_mesa {
            &Block::DARK_OAK_FENCE
        } else {
            &Block::OAK_FENCE
        }
    }
}

// ---------------------------------------------------------------------------
// Local helpers.
//
// These mirror `StructurePiece` protected methods that the shared `StructurePiece` here does not
// carry, or carries with different semantics. They live in this file rather than on the shared
// type so that porting mineshafts cannot regress the other sixteen structures.
// ---------------------------------------------------------------------------

/// `StructurePiece.isInterior` (`StructurePiece.java:200-203`). Note the `y + 1`: the shared
/// `StructurePiece::is_under_sea_level` samples `y` instead, so it is not a drop-in substitute.
const fn is_interior(
    p: &StructurePiece,
    chunk: &ProtoChunk,
    x: i32,
    y: i32,
    z: i32,
    bb: &BlockBox,
) -> bool {
    let pos = p.offset_pos(x, y + 1, z);
    if !bb.contains_pos(&pos) {
        return false;
    }
    pos.y < chunk.get_top_y(&HeightMap::OceanFloorWg, pos.x, pos.z)
}

/// `StructurePiece.generateBox` (`StructurePiece.java:218-245`). Routed through
/// [`StructurePiece::place_block`] so the piece's mirror/rotation reach directional blocks
/// (fence connections, wall torches), which is what vanilla's `placeBlock` does.
#[allow(clippy::too_many_arguments)]
fn generate_box(
    p: &StructurePiece,
    chunk: &mut ProtoChunk,
    reg: &dyn WorldPortalExt,
    bb: &BlockBox,
    x0: i32,
    y0: i32,
    z0: i32,
    x1: i32,
    y1: i32,
    z1: i32,
    edge: &'static BlockState,
    fill: &'static BlockState,
    skip_air: bool,
) {
    for y in y0..=y1 {
        for x in x0..=x1 {
            for z in z0..=z1 {
                if skip_air && p.get_block_at(chunk, x, y, z, bb).is_air() {
                    continue;
                }
                let is_edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                let state = if is_edge { edge } else { fill };
                p.place_block(chunk, reg, state, x, y, z, bb);
            }
        }
    }
}

/// `StructurePiece.generateMaybeBox` (`StructurePiece.java:293-323`).
#[allow(clippy::too_many_arguments)]
fn generate_maybe_box(
    p: &StructurePiece,
    chunk: &mut ProtoChunk,
    reg: &dyn WorldPortalExt,
    bb: &BlockBox,
    random: &mut RandomGenerator,
    probability: f32,
    x0: i32,
    y0: i32,
    z0: i32,
    x1: i32,
    y1: i32,
    z1: i32,
    edge: &'static BlockState,
    fill: &'static BlockState,
    skip_air: bool,
    has_to_be_inside: bool,
) {
    for y in y0..=y1 {
        for x in x0..=x1 {
            for z in z0..=z1 {
                if random.next_f32() > probability {
                    continue;
                }
                if skip_air && p.get_block_at(chunk, x, y, z, bb).is_air() {
                    continue;
                }
                if has_to_be_inside && !is_interior(p, chunk, x, y, z, bb) {
                    continue;
                }
                let is_inner = y != y0 && y != y1 && x != x0 && x != x1 && z != z0 && z != z1;
                let state = if is_inner { fill } else { edge };
                p.place_block(chunk, reg, state, x, y, z, bb);
            }
        }
    }
}

/// `StructurePiece.maybeGenerateBlock` (`StructurePiece.java:326-339`).
#[allow(clippy::too_many_arguments)]
fn maybe_generate_block(
    p: &StructurePiece,
    chunk: &mut ProtoChunk,
    reg: &dyn WorldPortalExt,
    bb: &BlockBox,
    random: &mut RandomGenerator,
    probability: f32,
    x: i32,
    y: i32,
    z: i32,
    state: &'static BlockState,
) {
    if random.next_f32() < probability {
        p.place_block(chunk, reg, state, x, y, z, bb);
    }
}

/// `StructurePiece.generateUpperHalfSphere` (`StructurePiece.java:341-378`). Its only vanilla
/// caller is `MineShaftRoom.postProcess` (`MineshaftPieces.java:1249`).
#[allow(clippy::too_many_arguments, clippy::cast_precision_loss)]
fn generate_upper_half_sphere(
    p: &StructurePiece,
    chunk: &mut ProtoChunk,
    reg: &dyn WorldPortalExt,
    bb: &BlockBox,
    x0: i32,
    y0: i32,
    z0: i32,
    x1: i32,
    y1: i32,
    z1: i32,
    fill: &'static BlockState,
    skip_air: bool,
) {
    let diag_x = (x1 - x0 + 1) as f32;
    let diag_y = (y1 - y0 + 1) as f32;
    let diag_z = (z1 - z0 + 1) as f32;
    let cx = x0 as f32 + diag_x / 2.0;
    let cz = z0 as f32 + diag_z / 2.0;

    for y in y0..=y1 {
        let ny = (y - y0) as f32 / diag_y;
        for x in x0..=x1 {
            let nx = (x as f32 - cx) / (diag_x * 0.5);
            for z in z0..=z1 {
                let nz = (z as f32 - cz) / (diag_z * 0.5);
                if skip_air && p.get_block_at(chunk, x, y, z, bb).is_air() {
                    continue;
                }
                if nx.mul_add(nx, ny.mul_add(ny, nz * nz)) <= 1.05 {
                    p.place_block(chunk, reg, fill, x, y, z, bb);
                }
            }
        }
    }
}

/// `MineShaftPiece.isSupportingBox` (`MineshaftPieces.java:993-1001`).
fn is_supporting_box(
    p: &StructurePiece,
    chunk: &ProtoChunk,
    bb: &BlockBox,
    x0: i32,
    x1: i32,
    y1: i32,
    z0: i32,
) -> bool {
    (x0..=x1).all(|x| !p.get_block_at(chunk, x, y1 + 1, z0, bb).is_air())
}

/// `MineShaftPiece.isInInvalidLocation` (`MineshaftPieces.java:1003-1051`): refuses to carve
/// through a mineshaft-blocking biome, or where liquid touches the piece's shell.
fn is_in_invalid_location(p: &StructurePiece, chunk: &ProtoChunk, bb: &BlockBox) -> bool {
    let pb = &p.bounding_box;
    let x0 = (pb.min.x - 1).max(bb.min.x);
    let y0 = (pb.min.y - 1).max(bb.min.y);
    let z0 = (pb.min.z - 1).max(bb.min.z);
    let x1 = (pb.max.x + 1).min(bb.max.x);
    let y1 = (pb.max.y + 1).min(bb.max.y);
    let z1 = (pb.max.z + 1).min(bb.max.z);

    let blocking = pumpkin_data::tag::WorldgenBiome::MINECRAFT_MINESHAFT_BLOCKING.1;
    // Vanilla's `(a + b) / 2` truncates toward zero; `i32::midpoint` does not, and world
    // coordinates are far too small to overflow, so keep the vanilla arithmetic.
    #[allow(clippy::manual_midpoint)]
    let (mx, my, mz) = ((x0 + x1) / 2, (y0 + y1) / 2, (z0 + z1) / 2);
    // `LevelReader.getBiome` converts to quart coordinates internally (`QuartPos.fromBlock`);
    // `ProtoChunk::get_biome_id` does not, so convert here as `has_valid_biomes` does.
    let blocking_here = blocking.contains(&u16::from(chunk.get_biome_id(
        crate::generation::biome_coords::from_block(mx),
        crate::generation::biome_coords::from_block(my),
        crate::generation::biome_coords::from_block(mz),
    )));
    if blocking_here {
        return true;
    }

    let liquid_at = |x: i32, y: i32, z: i32| {
        chunk
            .get_block_state(&Vector3::new(x, y, z))
            .to_state()
            .is_liquid()
    };

    for x in x0..=x1 {
        for z in z0..=z1 {
            if liquid_at(x, y0, z) || liquid_at(x, y1, z) {
                return true;
            }
        }
    }
    for x in x0..=x1 {
        for y in y0..=y1 {
            if liquid_at(x, y, z0) || liquid_at(x, y, z1) {
                return true;
            }
        }
    }
    for z in z0..=z1 {
        for y in y0..=y1 {
            if liquid_at(x0, y, z) || liquid_at(x1, y, z) {
                return true;
            }
        }
    }
    false
}

/// `MineShaftPiece.setPlanksBlock` (`MineshaftPieces.java:1053-1060`).
fn set_planks_block(
    p: &StructurePiece,
    chunk: &mut ProtoChunk,
    bb: &BlockBox,
    planks: &'static BlockState,
    x: i32,
    y: i32,
    z: i32,
) {
    if !is_interior(p, chunk, x, y, z, bb) {
        return;
    }
    let pos = p.offset_pos(x, y, z);
    let existing = chunk.get_block_state(&pos).to_state();
    if !existing.is_side_solid(DataDirection::Up) {
        chunk.set_block_state(pos.x, pos.y, pos.z, planks);
    }
}

/// `StructurePiece.isReplaceableByStructures` (`StructurePiece.java:388-390`).
fn is_replaceable_by_structures(chunk: &ProtoChunk, pos: &Vector3<i32>) -> bool {
    let id = chunk.get_block_state(pos);
    let state = id.to_state();
    let block = id.to_block();
    state.is_air()
        || state.is_liquid()
        || block == &Block::GLOW_LICHEN
        || block == &Block::SEAGRASS
        || block == &Block::TALL_SEAGRASS
}

/// `MineShaftCorridor.canPlaceColumnOnTopOf` (`MineshaftPieces.java:537-539`).
fn can_place_column_on_top_of(chunk: &ProtoChunk, pos: &Vector3<i32>) -> bool {
    chunk
        .get_block_state(pos)
        .to_state()
        .is_side_solid(DataDirection::Up)
}

/// `MineShaftCorridor.canHangChainBelow` (`MineshaftPieces.java:541-543`). Vanilla excludes
/// `FallingBlock` instances; `pumpkin-data` carries no falling-block marker, so vanilla's
/// `FallingBlock` subclasses are listed by name here.
fn can_hang_chain_below(chunk: &ProtoChunk, pos: &Vector3<i32>) -> bool {
    let id = chunk.get_block_state(pos);
    if !id.to_state().is_side_solid(DataDirection::Down) {
        return false;
    }
    let name = id.to_block().name;
    !(name == "sand"
        || name == "red_sand"
        || name == "gravel"
        || name == "suspicious_sand"
        || name == "suspicious_gravel"
        || name == "dragon_egg"
        || name == "pointed_dripstone"
        || name == "scaffolding"
        || name.ends_with("_concrete_powder")
        || name.ends_with("anvil"))
}

/// `MineShaftCorridor.fillColumnBetween` (`MineshaftPieces.java:527-535`).
fn fill_column_between(
    chunk: &mut ProtoChunk,
    state: &'static BlockState,
    x: i32,
    z: i32,
    bottom_inclusive: i32,
    top_exclusive: i32,
) {
    for y in bottom_inclusive..top_exclusive {
        chunk.set_block_state(x, y, z, state);
    }
}

// ---------------------------------------------------------------------------
// Block states
// ---------------------------------------------------------------------------

fn rail_with_shape(shape: RailShape) -> &'static BlockState {
    let mut props = RailLikeProperties::default(&Block::RAIL);
    props.shape = shape;
    BlockState::from_id(props.to_state_id(&Block::RAIL))
}

fn fence_connected(fence: &'static Block, west: bool, east: bool) -> &'static BlockState {
    let mut props = OakFenceLikeProperties::default(fence);
    props.west = west;
    props.east = east;
    BlockState::from_id(props.to_state_id(fence))
}

fn wall_torch(facing: HorizontalFacing) -> &'static BlockState {
    let mut props = WallTorchLikeProperties::default(&Block::WALL_TORCH);
    props.facing = facing;
    BlockState::from_id(props.to_state_id(&Block::WALL_TORCH))
}

// ---------------------------------------------------------------------------
// Piece model
// ---------------------------------------------------------------------------

enum MineshaftKind {
    /// `MineshaftPieces.MineShaftRoom` (`MineshaftPieces.java:1065`).
    Room { entrances: Vec<BlockBox> },
    /// `MineshaftPieces.MineShaftCorridor` (`MineshaftPieces.java:107`).
    Corridor {
        has_rails: bool,
        spider_corridor: bool,
        has_placed_spider: bool,
        num_sections: i32,
    },
    /// `MineshaftPieces.MineShaftCrossing` (`MineshaftPieces.java:602`).
    Crossing {
        direction: BlockDirection,
        two_floored: bool,
    },
    /// `MineshaftPieces.MineShaftStairs` (`MineshaftPieces.java:1280`).
    Stairs,
}

pub struct MineshaftPiece {
    piece: StructurePiece,
    shaft_type: MineshaftType,
    kind: MineshaftKind,
}

/// Everything `addChildren` reads off the piece it was called on, snapshotted so the recursion
/// can continue after the piece has been handed to the collector. That reproduces vanilla's
/// `addPiece(newPiece); newPiece.addChildren(...)` ordering (`MineshaftPieces.java:99-101`),
/// under which the new piece is already visible to its children's collision checks.
struct ChildContext {
    bb: BlockBox,
    depth: u32,
    orientation: Option<BlockDirection>,
    kind: ChildKind,
    shaft_type: MineshaftType,
}

enum ChildKind {
    Corridor,
    Crossing {
        direction: BlockDirection,
        two_floored: bool,
    },
    Stairs,
}

// ---------------------------------------------------------------------------
// Piece graph
// ---------------------------------------------------------------------------

/// `StructurePieceAccessor.findCollisionPiece`. `extra` carries boxes not yet inside the
/// collector -- in practice the start room, which vanilla has already added to its builder.
fn find_collision(collector: &StructurePiecesCollector, extra: &[BlockBox], b: &BlockBox) -> bool {
    collector.get_intersecting(b).is_some() || extra.iter().any(|e| e.intersects(b))
}

/// `MineShaftCorridor.findCorridorSize` (`MineshaftPieces.java:143-169`).
fn find_corridor_size(
    collector: &StructurePiecesCollector,
    extra: &[BlockBox],
    random: &mut RandomGenerator,
    foot_x: i32,
    foot_y: i32,
    foot_z: i32,
    direction: BlockDirection,
) -> Option<BlockBox> {
    let mut corridor_length = random.next_bounded_i32(3) + 2;
    while corridor_length > 0 {
        let block_length = corridor_length * 5;
        let mut b = match direction {
            BlockDirection::South => BlockBox::new(0, 0, 0, 2, 2, block_length - 1),
            BlockDirection::West => BlockBox::new(-(block_length - 1), 0, 0, 0, 2, 2),
            BlockDirection::East => BlockBox::new(0, 0, 0, block_length - 1, 2, 2),
            _ => BlockBox::new(0, 0, -(block_length - 1), 2, 2, 0),
        };
        b.move_pos(foot_x, foot_y, foot_z);
        if !find_collision(collector, extra, &b) {
            return Some(b);
        }
        corridor_length -= 1;
    }
    None
}

/// `MineShaftCrossing.findCrossing` (`MineshaftPieces.java:624-647`).
fn find_crossing(
    collector: &StructurePiecesCollector,
    extra: &[BlockBox],
    random: &mut RandomGenerator,
    foot_x: i32,
    foot_y: i32,
    foot_z: i32,
    direction: BlockDirection,
) -> Option<BlockBox> {
    let y1 = if random.next_bounded_i32(4) == 0 {
        6
    } else {
        2
    };
    let mut b = match direction {
        BlockDirection::South => BlockBox::new(-1, 0, 0, 3, y1, 4),
        BlockDirection::West => BlockBox::new(-4, 0, -1, 0, y1, 3),
        BlockDirection::East => BlockBox::new(0, 0, -1, 4, y1, 3),
        _ => BlockBox::new(-1, 0, -4, 3, y1, 0),
    };
    b.move_pos(foot_x, foot_y, foot_z);
    if find_collision(collector, extra, &b) {
        None
    } else {
        Some(b)
    }
}

/// `MineShaftStairs.findStairs` (`MineshaftPieces.java:1289-1305`).
fn find_stairs(
    collector: &StructurePiecesCollector,
    extra: &[BlockBox],
    foot_x: i32,
    foot_y: i32,
    foot_z: i32,
    direction: BlockDirection,
) -> Option<BlockBox> {
    let mut b = match direction {
        BlockDirection::South => BlockBox::new(0, -5, 0, 2, 2, 8),
        BlockDirection::West => BlockBox::new(-8, -5, 0, 0, 2, 2),
        BlockDirection::East => BlockBox::new(0, -5, 0, 8, 2, 2),
        _ => BlockBox::new(0, -5, -8, 2, 2, 0),
    };
    b.move_pos(foot_x, foot_y, foot_z);
    if find_collision(collector, extra, &b) {
        None
    } else {
        Some(b)
    }
}

/// `MineshaftPieces.createRandomShaftPiece` (`MineshaftPieces.java:47-77`).
#[allow(clippy::too_many_arguments)]
fn create_random_shaft_piece(
    collector: &StructurePiecesCollector,
    extra: &[BlockBox],
    random: &mut RandomGenerator,
    foot_x: i32,
    foot_y: i32,
    foot_z: i32,
    direction: BlockDirection,
    gen_depth: u32,
    shaft_type: MineshaftType,
) -> Option<MineshaftPiece> {
    let selection = random.next_bounded_i32(100);
    if selection >= 80 {
        let b = find_crossing(collector, extra, random, foot_x, foot_y, foot_z, direction)?;
        // `MineShaftCrossing` never calls setOrientation -- it keeps `direction` in a field of
        // its own -- so its placement coordinates stay absolute (`MineshaftPieces.java:618-622`).
        let two_floored = b.get_block_count_y() > 3;
        Some(MineshaftPiece {
            piece: StructurePiece::new(StructurePieceType::MineshaftCrossing, b, gen_depth),
            shaft_type,
            kind: MineshaftKind::Crossing {
                direction,
                two_floored,
            },
        })
    } else if selection >= 70 {
        let b = find_stairs(collector, extra, foot_x, foot_y, foot_z, direction)?;
        let mut piece = StructurePiece::new(StructurePieceType::MineshaftStairs, b, gen_depth);
        piece.set_facing(Some(direction));
        Some(MineshaftPiece {
            piece,
            shaft_type,
            kind: MineshaftKind::Stairs,
        })
    } else {
        let b = find_corridor_size(collector, extra, random, foot_x, foot_y, foot_z, direction)?;
        let mut piece = StructurePiece::new(StructurePieceType::MineshaftCorridor, b, gen_depth);
        piece.set_facing(Some(direction));
        // `MineShaftCorridor` constructor (`MineshaftPieces.java:130-142`).
        let has_rails = random.next_bounded_i32(3) == 0;
        let spider_corridor = !has_rails && random.next_bounded_i32(23) == 0;
        let num_sections = if matches!(direction, BlockDirection::North | BlockDirection::South) {
            (b.max.z - b.min.z + 1) / 5
        } else {
            (b.max.x - b.min.x + 1) / 5
        };
        Some(MineshaftPiece {
            piece,
            shaft_type,
            kind: MineshaftKind::Corridor {
                has_rails,
                spider_corridor,
                has_placed_spider: false,
                num_sections,
            },
        })
    }
}

/// `MineshaftPieces.generateAndAddPiece` (`MineshaftPieces.java:79-105`). Returns the new
/// piece's bounding box, which `MineShaftRoom.addChildren` needs for its entrance boxes.
#[allow(clippy::too_many_arguments)]
fn generate_and_add_piece(
    start_bb: &BlockBox,
    collector: &mut StructurePiecesCollector,
    extra: &[BlockBox],
    random: &mut RandomGenerator,
    foot_x: i32,
    foot_y: i32,
    foot_z: i32,
    direction: BlockDirection,
    depth: u32,
    shaft_type: MineshaftType,
) -> Option<BlockBox> {
    if depth > MAX_DEPTH {
        return None;
    }
    if (foot_x - start_bb.min.x).abs() > 80 || (foot_z - start_bb.min.z).abs() > 80 {
        return None;
    }

    let piece = create_random_shaft_piece(
        collector,
        extra,
        random,
        foot_x,
        foot_y,
        foot_z,
        direction,
        depth + 1,
        shaft_type,
    )?;

    let ctx = ChildContext {
        bb: piece.piece.bounding_box,
        depth: piece.piece.chain_length,
        orientation: piece.piece.facing,
        kind: match &piece.kind {
            MineshaftKind::Corridor { .. } => ChildKind::Corridor,
            MineshaftKind::Crossing {
                direction,
                two_floored,
            } => ChildKind::Crossing {
                direction: *direction,
                two_floored: *two_floored,
            },
            MineshaftKind::Stairs | MineshaftKind::Room { .. } => ChildKind::Stairs,
        },
        shaft_type,
    };
    let bb = ctx.bb;

    collector.add_piece(Box::new(piece));
    add_children(start_bb, collector, extra, random, &ctx);

    Some(bb)
}

fn add_children(
    start_bb: &BlockBox,
    collector: &mut StructurePiecesCollector,
    extra: &[BlockBox],
    random: &mut RandomGenerator,
    ctx: &ChildContext,
) {
    match ctx.kind {
        ChildKind::Corridor => add_children_corridor(start_bb, collector, extra, random, ctx),
        ChildKind::Crossing {
            direction,
            two_floored,
        } => add_children_crossing(
            start_bb,
            collector,
            extra,
            random,
            ctx,
            direction,
            two_floored,
        ),
        ChildKind::Stairs => add_children_stairs(start_bb, collector, extra, random, ctx),
    }
}

/// `MineShaftCorridor.addChildren` (`MineshaftPieces.java:170-341`).
#[allow(clippy::too_many_lines)]
fn add_children_corridor(
    start_bb: &BlockBox,
    collector: &mut StructurePiecesCollector,
    extra: &[BlockBox],
    random: &mut RandomGenerator,
    ctx: &ChildContext,
) {
    let depth = ctx.depth;
    let end_selection = random.next_bounded_i32(4);
    let Some(orientation) = ctx.orientation else {
        return;
    };
    let bb = ctx.bb;
    let t = ctx.shaft_type;

    // Vanilla evaluates `minY - 1 + random.nextInt(3)` inline in each branch, so exactly one
    // `nextInt(3)` is drawn regardless of which branch is taken.
    let spawn = |random: &mut RandomGenerator,
                 collector: &mut StructurePiecesCollector,
                 x: i32,
                 z: i32,
                 dir: BlockDirection| {
        let y = bb.min.y - 1 + random.next_bounded_i32(3);
        generate_and_add_piece(start_bb, collector, extra, random, x, y, z, dir, depth, t);
    };

    match orientation {
        BlockDirection::South => {
            if end_selection <= 1 {
                spawn(random, collector, bb.min.x, bb.max.z + 1, orientation);
            } else if end_selection == 2 {
                spawn(
                    random,
                    collector,
                    bb.min.x - 1,
                    bb.max.z - 3,
                    BlockDirection::West,
                );
            } else {
                spawn(
                    random,
                    collector,
                    bb.max.x + 1,
                    bb.max.z - 3,
                    BlockDirection::East,
                );
            }
        }
        BlockDirection::West => {
            if end_selection <= 1 {
                spawn(random, collector, bb.min.x - 1, bb.min.z, orientation);
            } else if end_selection == 2 {
                spawn(
                    random,
                    collector,
                    bb.min.x,
                    bb.min.z - 1,
                    BlockDirection::North,
                );
            } else {
                spawn(
                    random,
                    collector,
                    bb.min.x,
                    bb.max.z + 1,
                    BlockDirection::South,
                );
            }
        }
        BlockDirection::East => {
            if end_selection <= 1 {
                spawn(random, collector, bb.max.x + 1, bb.min.z, orientation);
            } else if end_selection == 2 {
                spawn(
                    random,
                    collector,
                    bb.max.x - 3,
                    bb.min.z - 1,
                    BlockDirection::North,
                );
            } else {
                spawn(
                    random,
                    collector,
                    bb.max.x - 3,
                    bb.max.z + 1,
                    BlockDirection::South,
                );
            }
        }
        // NORTH, and vanilla's `default` arm.
        _ => {
            if end_selection <= 1 {
                spawn(random, collector, bb.min.x, bb.min.z - 1, orientation);
            } else if end_selection == 2 {
                spawn(
                    random,
                    collector,
                    bb.min.x - 1,
                    bb.min.z,
                    BlockDirection::West,
                );
            } else {
                spawn(
                    random,
                    collector,
                    bb.max.x + 1,
                    bb.min.z,
                    BlockDirection::East,
                );
            }
        }
    }

    // Side branches (`MineshaftPieces.java:311-340`).
    if depth >= MAX_DEPTH {
        return;
    }
    if matches!(orientation, BlockDirection::North | BlockDirection::South) {
        let mut z = bb.min.z + 3;
        while z + 3 <= bb.max.z {
            let selection = random.next_bounded_i32(5);
            if selection == 0 {
                generate_and_add_piece(
                    start_bb,
                    collector,
                    extra,
                    random,
                    bb.min.x - 1,
                    bb.min.y,
                    z,
                    BlockDirection::West,
                    depth + 1,
                    t,
                );
            } else if selection == 1 {
                generate_and_add_piece(
                    start_bb,
                    collector,
                    extra,
                    random,
                    bb.max.x + 1,
                    bb.min.y,
                    z,
                    BlockDirection::East,
                    depth + 1,
                    t,
                );
            }
            z += 5;
        }
    } else {
        let mut x = bb.min.x + 3;
        while x + 3 <= bb.max.x {
            let selection = random.next_bounded_i32(5);
            if selection == 0 {
                generate_and_add_piece(
                    start_bb,
                    collector,
                    extra,
                    random,
                    x,
                    bb.min.y,
                    bb.min.z - 1,
                    BlockDirection::North,
                    depth + 1,
                    t,
                );
            } else if selection == 1 {
                generate_and_add_piece(
                    start_bb,
                    collector,
                    extra,
                    random,
                    x,
                    bb.min.y,
                    bb.max.z + 1,
                    BlockDirection::South,
                    depth + 1,
                    t,
                );
            }
            x += 5;
        }
    }
}

/// `MineShaftCrossing.addChildren` (`MineshaftPieces.java:648-830`).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn add_children_crossing(
    start_bb: &BlockBox,
    collector: &mut StructurePiecesCollector,
    extra: &[BlockBox],
    random: &mut RandomGenerator,
    ctx: &ChildContext,
    direction: BlockDirection,
    two_floored: bool,
) {
    let depth = ctx.depth;
    let bb = ctx.bb;
    let t = ctx.shaft_type;
    let go = |random: &mut RandomGenerator,
              collector: &mut StructurePiecesCollector,
              x: i32,
              y: i32,
              z: i32,
              dir: BlockDirection| {
        generate_and_add_piece(start_bb, collector, extra, random, x, y, z, dir, depth, t);
    };

    match direction {
        BlockDirection::South => {
            go(
                random,
                collector,
                bb.min.x + 1,
                bb.min.y,
                bb.max.z + 1,
                BlockDirection::South,
            );
            go(
                random,
                collector,
                bb.min.x - 1,
                bb.min.y,
                bb.min.z + 1,
                BlockDirection::West,
            );
            go(
                random,
                collector,
                bb.max.x + 1,
                bb.min.y,
                bb.min.z + 1,
                BlockDirection::East,
            );
        }
        BlockDirection::West => {
            go(
                random,
                collector,
                bb.min.x + 1,
                bb.min.y,
                bb.min.z - 1,
                BlockDirection::North,
            );
            go(
                random,
                collector,
                bb.min.x + 1,
                bb.min.y,
                bb.max.z + 1,
                BlockDirection::South,
            );
            go(
                random,
                collector,
                bb.min.x - 1,
                bb.min.y,
                bb.min.z + 1,
                BlockDirection::West,
            );
        }
        BlockDirection::East => {
            go(
                random,
                collector,
                bb.min.x + 1,
                bb.min.y,
                bb.min.z - 1,
                BlockDirection::North,
            );
            go(
                random,
                collector,
                bb.min.x + 1,
                bb.min.y,
                bb.max.z + 1,
                BlockDirection::South,
            );
            go(
                random,
                collector,
                bb.max.x + 1,
                bb.min.y,
                bb.min.z + 1,
                BlockDirection::East,
            );
        }
        // NORTH, and vanilla's `default` arm.
        _ => {
            go(
                random,
                collector,
                bb.min.x + 1,
                bb.min.y,
                bb.min.z - 1,
                BlockDirection::North,
            );
            go(
                random,
                collector,
                bb.min.x - 1,
                bb.min.y,
                bb.min.z + 1,
                BlockDirection::West,
            );
            go(
                random,
                collector,
                bb.max.x + 1,
                bb.min.y,
                bb.min.z + 1,
                BlockDirection::East,
            );
        }
    }

    if !two_floored {
        return;
    }
    let upper = bb.min.y + 3 + 1;
    if random.next_bool() {
        go(
            random,
            collector,
            bb.min.x + 1,
            upper,
            bb.min.z - 1,
            BlockDirection::North,
        );
    }
    if random.next_bool() {
        go(
            random,
            collector,
            bb.min.x - 1,
            upper,
            bb.min.z + 1,
            BlockDirection::West,
        );
    }
    if random.next_bool() {
        go(
            random,
            collector,
            bb.max.x + 1,
            upper,
            bb.min.z + 1,
            BlockDirection::East,
        );
    }
    if random.next_bool() {
        go(
            random,
            collector,
            bb.min.x + 1,
            upper,
            bb.max.z + 1,
            BlockDirection::South,
        );
    }
}

/// `MineShaftStairs.addChildren` (`MineshaftPieces.java:1307-1362`).
fn add_children_stairs(
    start_bb: &BlockBox,
    collector: &mut StructurePiecesCollector,
    extra: &[BlockBox],
    random: &mut RandomGenerator,
    ctx: &ChildContext,
) {
    let Some(orientation) = ctx.orientation else {
        return;
    };
    let bb = ctx.bb;
    let (x, y, z, dir) = match orientation {
        BlockDirection::South => (bb.min.x, bb.min.y, bb.max.z + 1, BlockDirection::South),
        BlockDirection::West => (bb.min.x - 1, bb.min.y, bb.min.z, BlockDirection::West),
        BlockDirection::East => (bb.max.x + 1, bb.min.y, bb.min.z, BlockDirection::East),
        _ => (bb.min.x, bb.min.y, bb.min.z - 1, BlockDirection::North),
    };
    generate_and_add_piece(
        start_bb,
        collector,
        extra,
        random,
        x,
        y,
        z,
        dir,
        ctx.depth,
        ctx.shaft_type,
    );
}

/// `MineShaftRoom.addChildren` (`MineshaftPieces.java:1080-1235`): four passes (north, south,
/// west, east walls), each stepping a random cursor along the room's span.
#[allow(clippy::too_many_lines)]
fn add_children_room(
    room_bb: BlockBox,
    collector: &mut StructurePiecesCollector,
    random: &mut RandomGenerator,
    depth: u32,
    shaft_type: MineshaftType,
) -> Vec<BlockBox> {
    let mut entrances = Vec::new();
    let extra = [room_bb];
    let x_span = room_bb.max.x - room_bb.min.x + 1;
    let z_span = room_bb.max.z - room_bb.min.z + 1;
    let mut height_space = room_bb.get_block_count_y() - 3 - 1;
    if height_space <= 0 {
        height_space = 1;
    }

    // North wall.
    let mut pos = 0;
    while pos < x_span {
        pos += random.next_bounded_i32(x_span);
        if pos + 3 > x_span {
            break;
        }
        let y = room_bb.min.y + random.next_bounded_i32(height_space) + 1;
        if let Some(child) = generate_and_add_piece(
            &room_bb,
            collector,
            &extra,
            random,
            room_bb.min.x + pos,
            y,
            room_bb.min.z - 1,
            BlockDirection::North,
            depth,
            shaft_type,
        ) {
            entrances.push(BlockBox::new(
                child.min.x,
                child.min.y,
                room_bb.min.z,
                child.max.x,
                child.max.y,
                room_bb.min.z + 1,
            ));
        }
        pos += 4;
    }

    // South wall.
    pos = 0;
    while pos < x_span {
        pos += random.next_bounded_i32(x_span);
        if pos + 3 > x_span {
            break;
        }
        let y = room_bb.min.y + random.next_bounded_i32(height_space) + 1;
        if let Some(child) = generate_and_add_piece(
            &room_bb,
            collector,
            &extra,
            random,
            room_bb.min.x + pos,
            y,
            room_bb.max.z + 1,
            BlockDirection::South,
            depth,
            shaft_type,
        ) {
            entrances.push(BlockBox::new(
                child.min.x,
                child.min.y,
                room_bb.max.z - 1,
                child.max.x,
                child.max.y,
                room_bb.max.z,
            ));
        }
        pos += 4;
    }

    // West wall.
    pos = 0;
    while pos < z_span {
        pos += random.next_bounded_i32(z_span);
        if pos + 3 > z_span {
            break;
        }
        let y = room_bb.min.y + random.next_bounded_i32(height_space) + 1;
        if let Some(child) = generate_and_add_piece(
            &room_bb,
            collector,
            &extra,
            random,
            room_bb.min.x - 1,
            y,
            room_bb.min.z + pos,
            BlockDirection::West,
            depth,
            shaft_type,
        ) {
            entrances.push(BlockBox::new(
                room_bb.min.x,
                child.min.y,
                child.min.z,
                room_bb.min.x + 1,
                child.max.y,
                child.max.z,
            ));
        }
        pos += 4;
    }

    // East wall.
    pos = 0;
    while pos < z_span {
        pos += random.next_bounded_i32(z_span);
        if pos + 3 > z_span {
            break;
        }
        let y = room_bb.min.y + random.next_bounded_i32(height_space) + 1;
        if let Some(child) = generate_and_add_piece(
            &room_bb,
            collector,
            &extra,
            random,
            room_bb.max.x + 1,
            y,
            room_bb.min.z + pos,
            BlockDirection::East,
            depth,
            shaft_type,
        ) {
            entrances.push(BlockBox::new(
                room_bb.max.x - 1,
                child.min.y,
                child.min.z,
                room_bb.max.x,
                child.max.y,
                child.max.z,
            ));
        }
        pos += 4;
    }

    entrances
}

// ---------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------

impl MineshaftPiece {
    /// `MineShaftRoom.postProcess` (`MineshaftPieces.java:1211-1259`).
    fn place_room(
        &self,
        entrances: &[BlockBox],
        chunk: &mut ProtoChunk,
        reg: &dyn WorldPortalExt,
        bb: &BlockBox,
    ) {
        let p = &self.piece;
        if is_in_invalid_location(p, chunk, bb) {
            return;
        }
        let air = Block::CAVE_AIR.default_state;
        let pb = p.bounding_box;

        generate_box(
            p,
            chunk,
            reg,
            bb,
            pb.min.x,
            pb.min.y + 1,
            pb.min.z,
            pb.max.x,
            (pb.min.y + 3).min(pb.max.y),
            pb.max.z,
            air,
            air,
            false,
        );

        for e in entrances {
            generate_box(
                p,
                chunk,
                reg,
                bb,
                e.min.x,
                e.max.y - 2,
                e.min.z,
                e.max.x,
                e.max.y,
                e.max.z,
                air,
                air,
                false,
            );
        }

        generate_upper_half_sphere(
            p,
            chunk,
            reg,
            bb,
            pb.min.x,
            pb.min.y + 4,
            pb.min.z,
            pb.max.x,
            pb.max.y,
            pb.max.z,
            air,
            false,
        );
    }

    /// `MineShaftCorridor.postProcess` (`MineshaftPieces.java:377-479`).
    #[allow(clippy::too_many_lines)]
    fn place_corridor(
        &mut self,
        chunk: &mut ProtoChunk,
        reg: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        bb: &BlockBox,
    ) {
        let MineshaftKind::Corridor {
            has_rails,
            spider_corridor,
            ref mut has_placed_spider,
            num_sections,
        } = self.kind
        else {
            return;
        };
        let p = &self.piece;
        let t = self.shaft_type;
        if is_in_invalid_location(p, chunk, bb) {
            return;
        }

        let air = Block::CAVE_AIR.default_state;
        let planks = t.planks().default_state;
        let length = num_sections * 5 - 1;

        generate_box(p, chunk, reg, bb, 0, 0, 0, 2, 1, length, air, air, false);
        generate_maybe_box(
            p, chunk, reg, bb, random, 0.8, 0, 2, 0, 2, 2, length, air, air, false, false,
        );
        if spider_corridor {
            generate_maybe_box(
                p,
                chunk,
                reg,
                bb,
                random,
                0.6,
                0,
                0,
                0,
                2,
                1,
                length,
                Block::COBWEB.default_state,
                air,
                false,
                true,
            );
        }

        for section in 0..num_sections {
            let z = 2 + section * 5;
            place_support(p, t, chunk, reg, bb, 0, 0, z, 2, 2, random);
            maybe_place_cobweb(p, chunk, reg, bb, random, 0.1, 0, 2, z - 1);
            maybe_place_cobweb(p, chunk, reg, bb, random, 0.1, 2, 2, z - 1);
            maybe_place_cobweb(p, chunk, reg, bb, random, 0.1, 0, 2, z + 1);
            maybe_place_cobweb(p, chunk, reg, bb, random, 0.1, 2, 2, z + 1);
            maybe_place_cobweb(p, chunk, reg, bb, random, 0.05, 0, 2, z - 2);
            maybe_place_cobweb(p, chunk, reg, bb, random, 0.05, 2, 2, z - 2);
            maybe_place_cobweb(p, chunk, reg, bb, random, 0.05, 0, 2, z + 2);
            maybe_place_cobweb(p, chunk, reg, bb, random, 0.05, 2, 2, z + 2);

            if random.next_bounded_i32(100) == 0 {
                create_chest_minecart(p, chunk, reg, bb, random, 2, 0, z - 1);
            }
            if random.next_bounded_i32(100) == 0 {
                create_chest_minecart(p, chunk, reg, bb, random, 0, 0, z + 1);
            }

            if spider_corridor && !*has_placed_spider {
                let new_z = z - 1 + random.next_bounded_i32(3);
                let pos = p.offset_pos(1, 0, new_z);
                if bb.contains_pos(&pos) && is_interior(p, chunk, 1, 0, new_z, bb) {
                    *has_placed_spider = true;
                    chunk.set_block_state(pos.x, pos.y, pos.z, Block::SPAWNER.default_state);
                    let mut nbt = NbtCompound::new();
                    nbt.put_string("id", "minecraft:mob_spawner".to_string());
                    nbt.put_int("x", pos.x);
                    nbt.put_int("y", pos.y);
                    nbt.put_int("z", pos.z);
                    let mut spawn_entry = NbtCompound::new();
                    let mut inner = NbtCompound::new();
                    inner.put_string("id", "minecraft:cave_spider".to_string());
                    spawn_entry.put_compound("entity", inner);
                    nbt.put_compound("SpawnData", spawn_entry);
                    chunk.add_block_entity(nbt);
                }
            }
        }

        for x in 0..=2 {
            for z in 0..=length {
                set_planks_block(p, chunk, bb, planks, x, -1, z);
            }
        }

        place_double_lower_or_upper_support(p, t, chunk, bb, 0, -1, 2);
        if num_sections > 1 {
            place_double_lower_or_upper_support(p, t, chunk, bb, 0, -1, length - 2);
        }

        if has_rails {
            let rail = rail_with_shape(RailShape::NorthSouth);
            for z in 0..=length {
                let floor = p.get_block_at(chunk, 1, -1, z, bb);
                if !floor.is_air() && floor.is_solid_render() {
                    let probability = if is_interior(p, chunk, 1, 0, z, bb) {
                        0.7
                    } else {
                        0.9
                    };
                    maybe_generate_block(p, chunk, reg, bb, random, probability, 1, 0, z, rail);
                }
            }
        }
    }

    /// `MineShaftCrossing.postProcess` (`MineshaftPieces.java:831-955`).
    #[allow(clippy::too_many_lines)]
    fn place_crossing(
        &self,
        two_floored: bool,
        chunk: &mut ProtoChunk,
        reg: &dyn WorldPortalExt,
        bb: &BlockBox,
    ) {
        let p = &self.piece;
        if is_in_invalid_location(p, chunk, bb) {
            return;
        }
        let air = Block::CAVE_AIR.default_state;
        let planks = self.shaft_type.planks().default_state;
        let pb = p.bounding_box;

        if two_floored {
            generate_box(
                p,
                chunk,
                reg,
                bb,
                pb.min.x + 1,
                pb.min.y,
                pb.min.z,
                pb.max.x - 1,
                pb.min.y + 2,
                pb.max.z,
                air,
                air,
                false,
            );
            generate_box(
                p,
                chunk,
                reg,
                bb,
                pb.min.x,
                pb.min.y,
                pb.min.z + 1,
                pb.max.x,
                pb.min.y + 2,
                pb.max.z - 1,
                air,
                air,
                false,
            );
            generate_box(
                p,
                chunk,
                reg,
                bb,
                pb.min.x + 1,
                pb.max.y - 2,
                pb.min.z,
                pb.max.x - 1,
                pb.max.y,
                pb.max.z,
                air,
                air,
                false,
            );
            generate_box(
                p,
                chunk,
                reg,
                bb,
                pb.min.x,
                pb.max.y - 2,
                pb.min.z + 1,
                pb.max.x,
                pb.max.y,
                pb.max.z - 1,
                air,
                air,
                false,
            );
            generate_box(
                p,
                chunk,
                reg,
                bb,
                pb.min.x + 1,
                pb.min.y + 3,
                pb.min.z + 1,
                pb.max.x - 1,
                pb.min.y + 3,
                pb.max.z - 1,
                air,
                air,
                false,
            );
        } else {
            generate_box(
                p,
                chunk,
                reg,
                bb,
                pb.min.x + 1,
                pb.min.y,
                pb.min.z,
                pb.max.x - 1,
                pb.max.y,
                pb.max.z,
                air,
                air,
                false,
            );
            generate_box(
                p,
                chunk,
                reg,
                bb,
                pb.min.x,
                pb.min.y,
                pb.min.z + 1,
                pb.max.x,
                pb.max.y,
                pb.max.z - 1,
                air,
                air,
                false,
            );
        }

        self.place_support_pillar(
            chunk,
            reg,
            bb,
            pb.min.x + 1,
            pb.min.y,
            pb.min.z + 1,
            pb.max.y,
        );
        self.place_support_pillar(
            chunk,
            reg,
            bb,
            pb.min.x + 1,
            pb.min.y,
            pb.max.z - 1,
            pb.max.y,
        );
        self.place_support_pillar(
            chunk,
            reg,
            bb,
            pb.max.x - 1,
            pb.min.y,
            pb.min.z + 1,
            pb.max.y,
        );
        self.place_support_pillar(
            chunk,
            reg,
            bb,
            pb.max.x - 1,
            pb.min.y,
            pb.max.z - 1,
            pb.max.y,
        );

        let y = pb.min.y - 1;
        for x in pb.min.x..=pb.max.x {
            for z in pb.min.z..=pb.max.z {
                set_planks_block(p, chunk, bb, planks, x, y, z);
            }
        }
    }

    /// `MineShaftCrossing.placeSupportPillar` (`MineshaftPieces.java:957-963`).
    #[allow(clippy::too_many_arguments)]
    fn place_support_pillar(
        &self,
        chunk: &mut ProtoChunk,
        reg: &dyn WorldPortalExt,
        bb: &BlockBox,
        x: i32,
        y0: i32,
        z: i32,
        y1: i32,
    ) {
        let p = &self.piece;
        if p.get_block_at(chunk, x, y1 + 1, z, bb).is_air() {
            return;
        }
        let planks = self.shaft_type.planks().default_state;
        generate_box(
            p,
            chunk,
            reg,
            bb,
            x,
            y0,
            z,
            x,
            y1,
            z,
            planks,
            Block::CAVE_AIR.default_state,
            false,
        );
    }

    /// `MineShaftStairs.postProcess` (`MineshaftPieces.java:1364-1383`).
    fn place_stairs(&self, chunk: &mut ProtoChunk, reg: &dyn WorldPortalExt, bb: &BlockBox) {
        let p = &self.piece;
        if is_in_invalid_location(p, chunk, bb) {
            return;
        }
        let air = Block::CAVE_AIR.default_state;
        generate_box(p, chunk, reg, bb, 0, 5, 0, 2, 7, 1, air, air, false);
        generate_box(p, chunk, reg, bb, 0, 0, 7, 2, 2, 8, air, air, false);
        for i in 0..5 {
            let y0 = 5 - i - i32::from(i < 4);
            generate_box(
                p,
                chunk,
                reg,
                bb,
                0,
                y0,
                2 + i,
                2,
                7 - i,
                2 + i,
                air,
                air,
                false,
            );
        }
    }
}

/// `MineShaftCorridor.placeSupport` (`MineshaftPieces.java:545-570`).
#[allow(clippy::too_many_arguments)]
fn place_support(
    p: &StructurePiece,
    t: MineshaftType,
    chunk: &mut ProtoChunk,
    reg: &dyn WorldPortalExt,
    bb: &BlockBox,
    x0: i32,
    y0: i32,
    z: i32,
    y1: i32,
    x1: i32,
    random: &mut RandomGenerator,
) {
    if !is_supporting_box(p, chunk, bb, x0, x1, y1, z) {
        return;
    }
    let air = Block::CAVE_AIR.default_state;
    let planks = t.planks().default_state;
    let fence = t.fence();
    let fence_west = fence_connected(fence, true, false);
    let fence_east = fence_connected(fence, false, true);

    generate_box(
        p,
        chunk,
        reg,
        bb,
        x0,
        y0,
        z,
        x0,
        y1 - 1,
        z,
        fence_west,
        air,
        false,
    );
    generate_box(
        p,
        chunk,
        reg,
        bb,
        x1,
        y0,
        z,
        x1,
        y1 - 1,
        z,
        fence_east,
        air,
        false,
    );

    if random.next_bounded_i32(4) == 0 {
        generate_box(p, chunk, reg, bb, x0, y1, z, x0, y1, z, planks, air, false);
        generate_box(p, chunk, reg, bb, x1, y1, z, x1, y1, z, planks, air, false);
    } else {
        generate_box(p, chunk, reg, bb, x0, y1, z, x1, y1, z, planks, air, false);
        maybe_generate_block(
            p,
            chunk,
            reg,
            bb,
            random,
            0.05,
            x0 + 1,
            y1,
            z - 1,
            wall_torch(HorizontalFacing::South),
        );
        maybe_generate_block(
            p,
            chunk,
            reg,
            bb,
            random,
            0.05,
            x0 + 1,
            y1,
            z + 1,
            wall_torch(HorizontalFacing::North),
        );
    }
}

/// `MineShaftCorridor.maybePlaceCobWeb` (`MineshaftPieces.java:572-580`).
#[allow(clippy::too_many_arguments)]
fn maybe_place_cobweb(
    p: &StructurePiece,
    chunk: &mut ProtoChunk,
    reg: &dyn WorldPortalExt,
    bb: &BlockBox,
    random: &mut RandomGenerator,
    probability: f32,
    x: i32,
    y: i32,
    z: i32,
) {
    if is_interior(p, chunk, x, y, z, bb)
        && random.next_f32() < probability
        && has_sturdy_neighbours(p, chunk, bb, x, y, z, 2)
    {
        p.place_block(chunk, reg, Block::COBWEB.default_state, x, y, z, bb);
    }
}

/// `MineShaftCorridor.hasSturdyNeighbours` (`MineshaftPieces.java:582-597`).
#[allow(clippy::too_many_arguments)]
fn has_sturdy_neighbours(
    p: &StructurePiece,
    chunk: &ProtoChunk,
    bb: &BlockBox,
    x: i32,
    y: i32,
    z: i32,
    count: u32,
) -> bool {
    let base = p.offset_pos(x, y, z);
    let mut sturdy = 0;
    // `Direction.values()` order, with the face each neighbour must present back toward `base`.
    for (offset, opposite) in [
        ((0, -1, 0), DataDirection::Up),
        ((0, 1, 0), DataDirection::Down),
        ((0, 0, -1), DataDirection::South),
        ((0, 0, 1), DataDirection::North),
        ((-1, 0, 0), DataDirection::East),
        ((1, 0, 0), DataDirection::West),
    ] {
        let pos = Vector3::new(base.x + offset.0, base.y + offset.1, base.z + offset.2);
        if bb.contains_pos(&pos)
            && chunk
                .get_block_state(&pos)
                .to_state()
                .is_side_solid(opposite)
        {
            sturdy += 1;
            if sturdy >= count {
                return true;
            }
        }
    }
    false
}

/// `MineShaftCorridor.placeDoubleLowerOrUpperSupport` (`MineshaftPieces.java:481-491`).
fn place_double_lower_or_upper_support(
    p: &StructurePiece,
    t: MineshaftType,
    chunk: &mut ProtoChunk,
    bb: &BlockBox,
    x: i32,
    y: i32,
    z: i32,
) {
    let planks_id = t.planks().default_state.id;
    if p.get_block_at(chunk, x, y, z, bb).id == planks_id {
        fill_pillar_down_or_chain_up(p, t, chunk, bb, x, y, z);
    }
    if p.get_block_at(chunk, x + 2, y, z, bb).id == planks_id {
        fill_pillar_down_or_chain_up(p, t, chunk, bb, x + 2, y, z);
    }
}

/// `MineShaftCorridor.fillPillarDownOrChainUp` (`MineshaftPieces.java:493-525`): searches
/// downward for a surface to rest a wood pillar on and upward for a ceiling to hang an iron
/// chain from, taking whichever it reaches first.
fn fill_pillar_down_or_chain_up(
    p: &StructurePiece,
    t: MineshaftType,
    chunk: &mut ProtoChunk,
    bb: &BlockBox,
    x: i32,
    y: i32,
    z: i32,
) {
    let base = p.offset_pos(x, y, z);
    if !bb.contains_pos(&base) {
        return;
    }
    let world_y = base.y;
    let min_y = i32::from(chunk.bottom_y());
    let max_y = min_y + i32::from(chunk.height()) - 1;

    let mut distance = 1;
    let mut check_below = true;
    let mut check_above = true;

    while check_below || check_above {
        if check_below {
            let pos = Vector3::new(base.x, world_y - distance, base.z);
            let empty_below = is_replaceable_by_structures(chunk, &pos)
                && chunk.get_block_state(&pos).to_block() != &Block::LAVA;
            if !empty_below && can_place_column_on_top_of(chunk, &pos) {
                fill_column_between(
                    chunk,
                    t.wood().default_state,
                    base.x,
                    base.z,
                    world_y - distance + 1,
                    world_y,
                );
                return;
            }
            check_below = distance <= MAX_PILLAR_HEIGHT && empty_below && pos.y > min_y + 1;
        }

        if check_above {
            let pos = Vector3::new(base.x, world_y + distance, base.z);
            let empty_above = is_replaceable_by_structures(chunk, &pos);
            if !empty_above && can_hang_chain_below(chunk, &pos) {
                chunk.set_block_state(base.x, world_y + 1, base.z, t.fence().default_state);
                fill_column_between(
                    chunk,
                    Block::IRON_CHAIN.default_state,
                    base.x,
                    base.z,
                    world_y + 2,
                    world_y + distance,
                );
                return;
            }
            check_above = distance <= MAX_CHAIN_HEIGHT && empty_above && pos.y < max_y;
        }

        distance += 1;
    }
}

/// `MineShaftCorridor.createChest` (`MineshaftPieces.java:343-375`): a rail plus a chest
/// minecart entity -- not a chest block, which is why the shared `add_chest` is not used here.
#[allow(clippy::too_many_arguments)]
fn create_chest_minecart(
    p: &StructurePiece,
    chunk: &mut ProtoChunk,
    reg: &dyn WorldPortalExt,
    bb: &BlockBox,
    random: &mut RandomGenerator,
    x: i32,
    y: i32,
    z: i32,
) -> bool {
    let pos = p.offset_pos(x, y, z);
    if !bb.contains_pos(&pos) {
        return false;
    }
    let here_is_air = chunk.get_block_state(&pos).to_state().is_air();
    let below_is_air = chunk
        .get_block_state(&Vector3::new(pos.x, pos.y - 1, pos.z))
        .to_state()
        .is_air();
    if !here_is_air || below_is_air {
        return false;
    }

    let shape = if random.next_bool() {
        RailShape::NorthSouth
    } else {
        RailShape::EastWest
    };
    p.place_block(chunk, reg, rail_with_shape(shape), x, y, z, bb);

    let mut nbt = NbtCompound::new();
    nbt.put_string("id", "minecraft:chest_minecart".to_string());
    nbt.put(
        "Pos",
        NbtTag::List(vec![
            (f64::from(pos.x) + 0.5).into(),
            (f64::from(pos.y) + 0.5).into(),
            (f64::from(pos.z) + 0.5).into(),
        ]),
    );
    nbt.put(
        "Motion",
        NbtTag::List(vec![0.0.into(), 0.0.into(), 0.0.into()]),
    );
    nbt.put("Rotation", NbtTag::List(vec![0.0f32.into(), 0.0f32.into()]));
    nbt.put_string("LootTable", ABANDONED_MINESHAFT_LOOT.to_string());
    nbt.put_long("LootTableSeed", random.next_i64());
    chunk.add_structure_entity(nbt);

    true
}

impl StructurePieceBase for MineshaftPiece {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_structure_piece(&self) -> &StructurePiece {
        &self.piece
    }

    fn get_structure_piece_mut(&mut self) -> &mut StructurePiece {
        &mut self.piece
    }

    fn translate(&mut self, x: i32, y: i32, z: i32) {
        self.piece.bounding_box.move_pos(x, y, z);
        // `MineShaftRoom.move` also shifts the recorded entrances
        // (`MineshaftPieces.java:1261-1268`).
        if let MineshaftKind::Room { entrances } = &mut self.kind {
            for e in entrances {
                e.move_pos(x, y, z);
            }
        }
    }

    fn place(
        &mut self,
        chunk: &mut ProtoChunk,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        _seed: i64,
        chunk_box: &BlockBox,
    ) {
        match &self.kind {
            MineshaftKind::Room { entrances } => {
                let entrances = entrances.clone();
                self.place_room(&entrances, chunk, block_registry, chunk_box);
            }
            MineshaftKind::Corridor { .. } => {
                self.place_corridor(chunk, block_registry, random, chunk_box);
            }
            MineshaftKind::Crossing { two_floored, .. } => {
                let two_floored = *two_floored;
                self.place_crossing(two_floored, chunk, block_registry, chunk_box);
            }
            MineshaftKind::Stairs => self.place_stairs(chunk, block_registry, chunk_box),
        }
    }
}

// ---------------------------------------------------------------------------
// Structure entry point
// ---------------------------------------------------------------------------

pub struct MineshaftGenerator {
    pub is_mesa: bool,
}

impl StructureGenerator for MineshaftGenerator {
    /// `MineshaftStructure.findGenerationPoint` and `generatePiecesAndAdjust`
    /// (`MineshaftStructure.java:35-65`).
    fn get_structure_position(
        &self,
        mut context: StructureGeneratorContext<'_>,
    ) -> Option<StructurePosition> {
        let shaft_type = MineshaftType {
            is_mesa: self.is_mesa,
        };
        let mut random = context.random;

        // `MineshaftStructure.java:37`: a discarded nextDouble, kept so the stream lines up.
        random.next_f64();

        let west = start_block_x(context.chunk_x) + 2;
        let north = start_block_z(context.chunk_z) + 2;

        // `MineShaftRoom` constructor (`MineshaftPieces.java:1068-1077`). The room is never
        // given an orientation, so its placement coordinates stay absolute.
        let room_bb = BlockBox::new(
            west,
            MAGIC_START_Y,
            north,
            west + 7 + random.next_bounded_i32(6),
            54 + random.next_bounded_i32(6),
            north + 7 + random.next_bounded_i32(6),
        );

        let mut collector = StructurePiecesCollector::default();
        let entrances = add_children_room(room_bb, &mut collector, &mut random, 0, shaft_type);

        // Vanilla adds the room to the builder before generating its children; inserting it at
        // the front here reproduces that placement order without the recursion having to borrow
        // it, and `find_collision` already saw it through the `extra` list.
        collector.pieces.insert(
            0,
            Box::new(MineshaftPiece {
                piece: StructurePiece::new(StructurePieceType::MineshaftRoom, room_bb, 0),
                shaft_type,
                kind: MineshaftKind::Room { entrances },
            }),
        );

        let sea_level = context.sea_level;
        let y_offset = if self.is_mesa {
            // `MineshaftStructure.java:53-60`: mesa shafts are lifted toward the surface.
            let bounds = collector.get_bounding_box();
            // `BoundingBox.getCenter` (`BoundingBox.java:237-239`) is
            // `min + (max - min + 1) / 2`, not the midpoint of the two bounds.
            let center_x = bounds.min.x + (bounds.max.x - bounds.min.x + 1) / 2;
            let center_z = bounds.min.z + (bounds.max.z - bounds.min.z + 1) / 2;
            let center_y = bounds.min.y + (bounds.max.y - bounds.min.y + 1) / 2;
            let surface = context
                .height_sampler
                .as_mut()
                .map_or(sea_level, |s| s.estimate_height(center_x, center_z));
            let target = if surface <= sea_level {
                sea_level
            } else {
                random.next_inbetween_i32(sea_level, surface)
            };
            let dy = target - center_y;
            collector.shift(dy);
            dy
        } else {
            // `MineshaftStructure.java:62`: moveBelowSeaLevel(seaLevel, minY, random, 10).
            collector.shift_into(sea_level, context.min_y, &mut random, 10)
        };

        Some(StructurePosition {
            // `MineshaftStructure.java:38-41`: middle block X, min block Z, y 50 + offset.
            start_pos: BlockPos::new(
                start_block_x(context.chunk_x) + 8,
                MAGIC_START_Y + y_offset,
                start_block_z(context.chunk_z),
            ),
            collector: Arc::new(Mutex::new(collector)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk_system::{Chunk, StagedChunkEnum, generate_single_chunk};
    use crate::generation::get_world_gen;
    use pumpkin_data::BlockStateId;
    use pumpkin_data::dimension::Dimension;
    use pumpkin_util::world_seed::Seed;

    struct TestRegistry;
    impl WorldPortalExt for TestRegistry {
        fn can_place_at(
            &self,
            _block: &Block,
            _state: &BlockState,
            _accessor: &dyn crate::world::BlockAccessor,
            _pos: &BlockPos,
        ) -> bool {
            true
        }
        fn mirror(
            &self,
            block: &Block,
            state_id: BlockStateId,
            mirror: pumpkin_data::Mirror,
        ) -> &'static BlockState {
            block.mirror(state_id, mirror)
        }
        fn rotate(
            &self,
            block: &Block,
            state_id: BlockStateId,
            rotation: pumpkin_data::Rotation,
        ) -> &'static BlockState {
            block.rotate(state_id, rotation)
        }
        fn spawn_mobs_for_chunk_generation(
            &self,
            _cache: &mut dyn crate::generation::proto_chunk::GenerationCache,
            _biome: &'static pumpkin_data::chunk::Biome,
            _chunk_x: i32,
            _chunk_z: i32,
        ) {
        }
    }

    /// Before the `MineshaftPieces` port this file placed a hardcoded plus-shaped corridor at a
    /// fixed y with no cobwebs, torches, plank floor or curved rails. Chunk (-23, -5) on seed 42
    /// carries a mineshaft start; assert the vanilla furniture actually reaches the world.
    #[test]
    fn mineshaft_places_vanilla_corridor_furniture() {
        let dim = Dimension::OVERWORLD;
        let world_gen = get_world_gen(Seed(42), dim.clone(), false, Vec::new(), String::new());
        let registry = TestRegistry;

        let mut counts = std::collections::BTreeMap::new();
        for (cx, cz) in [(-23, -5), (-23, -4), (-22, -5)] {
            let chunk = generate_single_chunk(
                &dim,
                0,
                &world_gen,
                &registry,
                cx,
                cz,
                StagedChunkEnum::Full,
            );
            let Chunk::Level(data) = chunk else {
                panic!("expected a level chunk")
            };
            let sections = &data.section;
            let section_count = i32::try_from(
                sections
                    .block_sections
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .len(),
            )
            .unwrap_or(0);
            for y in sections.min_y..(sections.min_y + section_count * 16) {
                for z in 0..16 {
                    for x in 0..16 {
                        if let Some(id) = sections.get_block_absolute_y(x, y, z) {
                            *counts
                                .entry(Block::from_state_id(id).name)
                                .or_insert(0usize) += 1;
                        }
                    }
                }
            }
        }

        for expected in ["rail", "oak_planks", "oak_fence", "cobweb"] {
            assert!(
                counts.get(expected).copied().unwrap_or(0) > 0,
                "mineshaft placed no {expected}; counts were {counts:?}"
            );
        }
    }
}
