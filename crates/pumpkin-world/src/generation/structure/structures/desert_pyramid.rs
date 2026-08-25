use std::sync::Arc;

use pumpkin_data::{
    Block, BlockState,
    block_properties::{BlockProperties, HorizontalFacing, OakStairsLikeProperties},
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::{
    BlockDirection, HeightMap,
    math::{block_box::BlockBox, position::BlockPos},
    random::{
        RandomDeriverImpl, RandomGenerator, RandomImpl, hash_block_pos, legacy_rand::LegacyRand,
        xoroshiro128::Xoroshiro,
    },
};

use crate::{
    ProtoChunk,
    generation::{
        positions::chunk_pos::{start_block_x, start_block_z},
        structure::{
            piece::StructurePieceType,
            structures::{
                StructureGenerator, StructureGeneratorContext, StructurePiece, StructurePieceBase,
                StructurePiecesCollector, StructurePosition, get_lowest_y,
            },
        },
    },
};

const WIDTH: i32 = 21;
const HEIGHT: i32 = 15;
const DEPTH: i32 = 21;

pub struct DesertPyramidGenerator;

impl StructureGenerator for DesertPyramidGenerator {
    fn get_structure_position(
        &self,
        mut context: StructureGeneratorContext<'_>,
    ) -> Option<StructurePosition> {
        let x = start_block_x(context.chunk_x);
        let z = start_block_z(context.chunk_z);

        // Port of `SinglePieceStructure.findGenerationPoint`
        // (`net/minecraft/world/level/levelgen/structure/SinglePieceStructure.java:24-29`):
        // the structure is rejected when the lowest corner surface height of its
        // 21x21 footprint (`getLowestY`, see [`get_lowest_y`]) is below sea level. Like
        // vanilla, this runs before any random values are consumed.
        if let Some(sampler) = context.height_sampler.as_deref_mut()
            && get_lowest_y(sampler, x, z, WIDTH, DEPTH) < context.sea_level
        {
            return None;
        }

        let facing = BlockDirection::get_random_horizontal_direction(&mut context.random);

        let mut piece = StructurePiece::new(
            StructurePieceType::DesertTemple,
            BlockBox::create_box(x, 64, z, facing.get_axis(), WIDTH, HEIGHT, DEPTH),
            0,
        );
        piece.set_facing(Some(facing));

        let mut collector = StructurePiecesCollector::default();
        collector.add_piece(Box::new(DesertPyramidPiece {
            piece,
            height_adjusted: false,
            has_placed_chest: [false; 4],
            potential_suspicious_sand_world_positions: Vec::new(),
            random_collapsed_roof_pos: BlockPos::ZERO,
        }));

        Some(StructurePosition {
            start_pos: BlockPos::new(x + (WIDTH / 2), 64, z + (DEPTH / 2)),
            collector: Arc::new(collector.into()),
        })
    }
}

pub struct DesertPyramidPiece {
    piece: StructurePiece,
    height_adjusted: bool,
    has_placed_chest: [bool; 4],
    potential_suspicious_sand_world_positions: Vec<BlockPos>,
    random_collapsed_roof_pos: BlockPos,
}

impl DesertPyramidPiece {
    fn sandstone_stairs(facing: HorizontalFacing) -> &'static BlockState {
        let mut props = OakStairsLikeProperties::default(&Block::SANDSTONE_STAIRS);
        props.facing = facing;
        BlockState::from_id(props.to_state_id(&Block::SANDSTONE_STAIRS))
    }

    fn adjust_height(&mut self, chunk: &ProtoChunk, random: &mut RandomGenerator) -> bool {
        if self.height_adjusted {
            return true;
        }

        let ground_offset = -(random.next_bounded_i32(3));
        let bb = self.piece.bounding_box;
        let mut lowest = i32::MAX;

        for z in bb.min.z..=bb.max.z {
            for x in bb.min.x..=bb.max.x {
                let y = chunk.get_top_y(&HeightMap::MotionBlockingNoLeaves, x, z);
                lowest = lowest.min(y);
            }
        }

        if lowest == i32::MAX {
            return false;
        }

        let shift_y = lowest - self.piece.bounding_box.min.y + ground_offset;
        self.piece.bounding_box.move_pos(0, shift_y, 0);
        self.height_adjusted = true;
        true
    }

    /// Vanilla `DesertPyramidPiece.placeSand` records every sand position for the structure's
    /// archaeology pass (`DesertPyramidPiece.java:390-393`).
    fn place_sand(&mut self, chunk: &mut ProtoChunk, bb: &BlockBox, x: i32, y: i32, z: i32) {
        self.potential_suspicious_sand_world_positions.push({
            let world_pos = self.piece.offset_pos(x, y, z);
            BlockPos::new(world_pos.x, world_pos.y, world_pos.z)
        });
        self.piece
            .add_block(chunk, Block::SAND.default_state, x, y, z, bb);
    }

    #[expect(clippy::too_many_arguments)]
    fn place_sand_box(
        &mut self,
        chunk: &mut ProtoChunk,
        bb: &BlockBox,
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        y1: i32,
        z1: i32,
    ) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    self.place_sand(chunk, bb, x, y, z);
                }
            }
        }
    }

    fn place_collapsed_roof_piece(
        &self,
        chunk: &mut ProtoChunk,
        bb: &BlockBox,
        random: &mut RandomGenerator,
        x: i32,
        y: i32,
        z: i32,
    ) {
        let state = if random.next_f32() < 0.33 {
            Block::SANDSTONE.default_state
        } else {
            Block::SAND.default_state
        };

        self.piece.add_block(chunk, state, x, y, z, bb);
    }

    #[expect(clippy::too_many_arguments)]
    fn place_collapsed_roof(
        &mut self,
        chunk: &mut ProtoChunk,
        bb: &BlockBox,
        random: &mut RandomGenerator,
        seed: i64,
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        z1: i32,
    ) {
        for x in x0..=x1 {
            for z in z0..=z1 {
                self.place_collapsed_roof_piece(chunk, bb, random, x, y0, z);
            }
        }
        let origin = self.piece.offset_pos(x0, y0, z0);
        let mut seed_random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(seed as u64));
        let positional_random = seed_random.next_splitter();
        let mut roof_random = positional_random.split_pos(origin.x, origin.y, origin.z);
        let roof_x = roof_random.next_inbetween_i32(x0, x1);
        let roof_z = roof_random.next_inbetween_i32(z0, z1);
        let roof_pos = self.piece.offset_pos(roof_x, y0, roof_z);
        self.random_collapsed_roof_pos = BlockPos::new(roof_pos.x, roof_pos.y, roof_pos.z);
    }

    fn try_place_chest(
        &mut self,
        chunk: &mut ProtoChunk,
        bb: &BlockBox,
        index: usize,
        x: i32,
        y: i32,
        z: i32,
    ) {
        if self.has_placed_chest[index] {
            return;
        }

        let world_pos = self.piece.offset_pos(x, y, z);
        if !bb.contains_pos(&world_pos) {
            return;
        }

        self.piece
            .add_block(chunk, Block::CHEST.default_state, x, y, z, bb);

        let mut nbt = NbtCompound::new();
        nbt.put_int("x", world_pos.x);
        nbt.put_int("y", world_pos.y);
        nbt.put_int("z", world_pos.z);
        nbt.put_string("id", "minecraft:chest".to_string());
        nbt.put_string("LootTable", "minecraft:chests/desert_pyramid".to_string());

        let mut random =
            LegacyRand::from_seed(hash_block_pos(world_pos.x, world_pos.y, world_pos.z) as u64);
        nbt.put_long("LootTableSeed", random.next_i64());

        chunk.add_block_entity(nbt);
        self.has_placed_chest[index] = true;
    }

    fn add_cellar(
        &mut self,
        chunk: &mut ProtoChunk,
        bb: &BlockBox,
        random: &mut RandomGenerator,
        seed: i64,
    ) {
        self.add_cellar_stairs(chunk, bb, random);
        self.add_cellar_room(chunk, bb, random, seed);
    }

    fn add_cellar_stairs(
        &self,
        chunk: &mut ProtoChunk,
        bb: &BlockBox,
        random: &mut RandomGenerator,
    ) {
        let west_stairs = Self::sandstone_stairs(HorizontalFacing::West);
        self.piece.add_block(chunk, west_stairs, 13, -1, 17, bb);
        self.piece.add_block(chunk, west_stairs, 14, -2, 17, bb);
        self.piece.add_block(chunk, west_stairs, 15, -3, 17, bb);

        let sand = Block::SAND.default_state;
        let sandstone = Block::SANDSTONE.default_state;
        let (x, y, z) = (16, -4, 13);
        let variant = random.next_bool();

        self.piece.add_block(chunk, sand, x - 4, y + 4, z + 4, bb);
        self.piece.add_block(chunk, sand, x - 3, y + 4, z + 4, bb);
        self.piece.add_block(chunk, sand, x - 2, y + 4, z + 4, bb);
        self.piece.add_block(chunk, sand, x - 1, y + 4, z + 4, bb);
        self.piece.add_block(chunk, sand, x, y + 4, z + 4, bb);
        self.piece.add_block(chunk, sand, x - 2, y + 3, z + 4, bb);
        self.piece.add_block(
            chunk,
            if variant { sand } else { sandstone },
            x - 1,
            y + 3,
            z + 4,
            bb,
        );
        self.piece.add_block(
            chunk,
            if variant { sandstone } else { sand },
            x,
            y + 3,
            z + 4,
            bb,
        );
        self.piece.add_block(chunk, sand, x - 1, y + 2, z + 4, bb);
        self.piece.add_block(chunk, sandstone, x, y + 2, z + 4, bb);
        self.piece.add_block(chunk, sand, x, y + 1, z + 4, bb);
    }

    #[expect(clippy::too_many_lines)]
    fn add_cellar_room(
        &mut self,
        chunk: &mut ProtoChunk,
        bb: &BlockBox,
        random: &mut RandomGenerator,
        seed: i64,
    ) {
        let (x, y, z) = (16, -4, 13);
        let cut = Block::CUT_SANDSTONE.default_state;
        let chiseled = Block::CHISELED_SANDSTONE.default_state;
        let orange = Block::ORANGE_TERRACOTTA.default_state;
        let blue = Block::BLUE_TERRACOTTA.default_state;
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x - 3,
            y + 1,
            z - 3,
            x - 3,
            y + 1,
            z + 2,
            cut,
            cut,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x + 3,
            y + 1,
            z - 3,
            x + 3,
            y + 1,
            z + 2,
            cut,
            cut,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x - 3,
            y + 1,
            z - 3,
            x + 3,
            y + 1,
            z - 2,
            cut,
            cut,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x - 3,
            y + 1,
            z + 3,
            x + 3,
            y + 1,
            z + 3,
            cut,
            cut,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x - 3,
            y + 2,
            z - 3,
            x - 3,
            y + 2,
            z + 2,
            chiseled,
            chiseled,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x + 3,
            y + 2,
            z - 3,
            x + 3,
            y + 2,
            z + 2,
            chiseled,
            chiseled,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x - 3,
            y + 2,
            z - 3,
            x + 3,
            y + 2,
            z - 2,
            chiseled,
            chiseled,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x - 3,
            y + 2,
            z + 3,
            x + 3,
            y + 2,
            z + 3,
            chiseled,
            chiseled,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x - 3,
            -1,
            z - 3,
            x - 3,
            -1,
            z + 2,
            cut,
            cut,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x + 3,
            -1,
            z - 3,
            x + 3,
            -1,
            z + 2,
            cut,
            cut,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x - 3,
            -1,
            z - 3,
            x + 3,
            -1,
            z - 2,
            cut,
            cut,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            true,
            x - 3,
            -1,
            z + 3,
            x + 3,
            -1,
            z + 3,
            cut,
            cut,
        );

        self.place_sand_box(chunk, bb, x - 2, y + 1, z - 2, x + 2, y + 3, z + 2);
        self.place_collapsed_roof(chunk, bb, random, seed, x - 2, y + 4, z - 2, x + 2, z + 2);
        self.piece.add_block(chunk, blue, x, y, z, bb);
        self.piece.add_block(chunk, orange, x + 1, y, z - 1, bb);
        self.piece.add_block(chunk, orange, x + 1, y, z + 1, bb);
        self.piece.add_block(chunk, orange, x - 1, y, z - 1, bb);
        self.piece.add_block(chunk, orange, x - 1, y, z + 1, bb);
        self.piece.add_block(chunk, orange, x + 2, y, z, bb);
        self.piece.add_block(chunk, orange, x - 2, y, z, bb);
        self.piece.add_block(chunk, orange, x, y, z + 2, bb);
        self.piece.add_block(chunk, orange, x, y, z - 2, bb);

        self.piece.add_block(chunk, orange, x + 3, y, z, bb);
        self.place_sand(chunk, bb, x + 3, y + 1, z);
        self.place_sand(chunk, bb, x + 3, y + 2, z);
        self.piece.add_block(chunk, cut, x + 4, y + 1, z, bb);
        self.piece.add_block(chunk, chiseled, x + 4, y + 2, z, bb);

        self.piece.add_block(chunk, orange, x - 3, y, z, bb);
        self.place_sand(chunk, bb, x - 3, y + 1, z);
        self.place_sand(chunk, bb, x - 3, y + 2, z);
        self.piece.add_block(chunk, cut, x - 4, y + 1, z, bb);
        self.piece.add_block(chunk, chiseled, x - 4, y + 2, z, bb);

        self.piece.add_block(chunk, orange, x, y, z + 3, bb);
        self.place_sand(chunk, bb, x, y + 1, z + 3);
        self.place_sand(chunk, bb, x, y + 2, z + 3);

        self.piece.add_block(chunk, orange, x, y, z - 3, bb);
        self.place_sand(chunk, bb, x, y + 1, z - 3);
        self.place_sand(chunk, bb, x, y + 2, z - 3);
        self.piece.add_block(chunk, cut, x, y + 1, z - 4, bb);
        self.piece.add_block(chunk, chiseled, x, -2, z - 4, bb);
    }

    /// Returns the sand positions collected for the structure archaeology pass. Not yet
    /// consumed: vanilla's structure-level `afterPlace` step samples this list (plus
    /// [`Self::get_random_collapsed_roof_pos`]) to convert a subset into `suspicious_sand`
    /// with archaeology loot; no caller here does that conversion yet, so desert pyramids
    /// currently generate with plain sand and no archaeology loot, same as before this data
    /// was collected.
    ///
    /// Vanilla `DesertPyramidPiece.getPotentialSuspiciousSandWorldPositions` returns this list
    /// (`DesertPyramidPiece.java:428-430`).
    #[must_use]
    pub fn get_potential_suspicious_sand_world_positions(&self) -> &[BlockPos] {
        &self.potential_suspicious_sand_world_positions
    }

    /// Returns the positional-random collapsed-roof sand position. See the not-yet-consumed
    /// note on [`Self::get_potential_suspicious_sand_world_positions`].
    ///
    /// Vanilla `DesertPyramidPiece.getRandomCollapsedRoofPos` returns this position
    /// (`DesertPyramidPiece.java:432-434`).
    #[must_use]
    pub const fn get_random_collapsed_roof_pos(&self) -> BlockPos {
        self.random_collapsed_roof_pos
    }
}

impl StructurePieceBase for DesertPyramidPiece {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn get_structure_piece(&self) -> &StructurePiece {
        &self.piece
    }

    fn get_structure_piece_mut(&mut self) -> &mut StructurePiece {
        &mut self.piece
    }

    #[expect(clippy::too_many_lines)]
    fn place(
        &mut self,
        chunk: &mut ProtoChunk,
        _block_registry: &dyn crate::world::WorldPortalExt,
        random: &mut RandomGenerator,
        seed: i64,
        chunk_box: &BlockBox,
    ) {
        if !self.adjust_height(chunk, random) {
            return;
        }

        let origin = self.piece.bounding_box.min;
        let mut level_random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(seed as u64));
        let splitter = level_random.next_splitter();
        let mut level_random = splitter.split_pos(origin.x, origin.y, origin.z);

        let bb = chunk_box;
        let ss = Block::SANDSTONE.default_state;
        let air = Block::AIR.default_state;
        let cut = Block::CUT_SANDSTONE.default_state;
        let chiseled = Block::CHISELED_SANDSTONE.default_state;
        let orange = Block::ORANGE_TERRACOTTA.default_state;
        let blue = Block::BLUE_TERRACOTTA.default_state;
        let slab = Block::SANDSTONE_SLAB.default_state;

        let north_stairs = Self::sandstone_stairs(HorizontalFacing::North);
        let south_stairs = Self::sandstone_stairs(HorizontalFacing::South);
        let east_stairs = Self::sandstone_stairs(HorizontalFacing::East);
        let west_stairs = Self::sandstone_stairs(HorizontalFacing::West);

        self.piece
            .fill_with_outline(chunk, bb, false, 0, -4, 0, WIDTH - 1, 0, DEPTH - 1, ss, ss);

        for pos in 1..=9 {
            self.piece.fill_with_outline(
                chunk,
                bb,
                false,
                pos,
                pos,
                pos,
                WIDTH - 1 - pos,
                pos,
                DEPTH - 1 - pos,
                ss,
                ss,
            );
            self.piece.fill_with_outline(
                chunk,
                bb,
                false,
                pos + 1,
                pos,
                pos + 1,
                WIDTH - 2 - pos,
                pos,
                DEPTH - 2 - pos,
                air,
                air,
            );
        }

        for x in 0..WIDTH {
            for z in 0..DEPTH {
                self.piece.fill_downwards(chunk, ss, x, -5, z, bb);
            }
        }

        self.piece
            .fill_with_outline(chunk, bb, false, 0, 0, 0, 4, 9, 4, ss, air);
        self.piece
            .fill_with_outline(chunk, bb, false, 1, 10, 1, 3, 10, 3, ss, ss);
        self.piece.add_block(chunk, north_stairs, 2, 10, 0, bb);
        self.piece.add_block(chunk, south_stairs, 2, 10, 4, bb);
        self.piece.add_block(chunk, east_stairs, 0, 10, 2, bb);
        self.piece.add_block(chunk, west_stairs, 4, 10, 2, bb);

        self.piece
            .fill_with_outline(chunk, bb, false, WIDTH - 5, 0, 0, WIDTH - 1, 9, 4, ss, air);
        self.piece
            .fill_with_outline(chunk, bb, false, WIDTH - 4, 10, 1, WIDTH - 2, 10, 3, ss, ss);
        self.piece
            .add_block(chunk, north_stairs, WIDTH - 3, 10, 0, bb);
        self.piece
            .add_block(chunk, south_stairs, WIDTH - 3, 10, 4, bb);
        self.piece
            .add_block(chunk, east_stairs, WIDTH - 5, 10, 2, bb);
        self.piece
            .add_block(chunk, west_stairs, WIDTH - 1, 10, 2, bb);

        self.piece
            .fill_with_outline(chunk, bb, false, 8, 0, 0, 12, 4, 4, ss, air);
        self.piece
            .fill_with_outline(chunk, bb, false, 9, 1, 0, 11, 3, 4, air, air);
        self.piece.add_block(chunk, cut, 9, 1, 1, bb);
        self.piece.add_block(chunk, cut, 9, 2, 1, bb);
        self.piece.add_block(chunk, cut, 9, 3, 1, bb);
        self.piece.add_block(chunk, cut, 10, 3, 1, bb);
        self.piece.add_block(chunk, cut, 11, 3, 1, bb);
        self.piece.add_block(chunk, cut, 11, 2, 1, bb);
        self.piece.add_block(chunk, cut, 11, 1, 1, bb);

        self.piece
            .fill_with_outline(chunk, bb, false, 4, 1, 1, 8, 3, 3, ss, air);
        self.piece
            .fill_with_outline(chunk, bb, false, 4, 1, 2, 8, 2, 2, air, air);
        self.piece
            .fill_with_outline(chunk, bb, false, 12, 1, 1, 16, 3, 3, ss, air);
        self.piece
            .fill_with_outline(chunk, bb, false, 12, 1, 2, 16, 2, 2, air, air);

        self.piece
            .fill_with_outline(chunk, bb, false, 5, 4, 5, WIDTH - 6, 4, DEPTH - 6, ss, ss);
        self.piece
            .fill_with_outline(chunk, bb, false, 9, 4, 9, 11, 4, 11, air, air);

        self.piece
            .fill_with_outline(chunk, bb, false, 8, 1, 8, 8, 3, 8, cut, cut);
        self.piece
            .fill_with_outline(chunk, bb, false, 12, 1, 8, 12, 3, 8, cut, cut);
        self.piece
            .fill_with_outline(chunk, bb, false, 8, 1, 12, 8, 3, 12, cut, cut);
        self.piece
            .fill_with_outline(chunk, bb, false, 12, 1, 12, 12, 3, 12, cut, cut);

        self.piece
            .fill_with_outline(chunk, bb, false, 1, 1, 5, 4, 4, 11, ss, ss);
        self.piece
            .fill_with_outline(chunk, bb, false, WIDTH - 5, 1, 5, WIDTH - 2, 4, 11, ss, ss);

        self.piece
            .fill_with_outline(chunk, bb, false, 6, 7, 9, 6, 7, 11, ss, ss);
        self.piece
            .fill_with_outline(chunk, bb, false, WIDTH - 7, 7, 9, WIDTH - 7, 7, 11, ss, ss);
        self.piece
            .fill_with_outline(chunk, bb, false, 5, 5, 9, 5, 7, 11, cut, cut);
        self.piece.fill_with_outline(
            chunk,
            bb,
            false,
            WIDTH - 6,
            5,
            9,
            WIDTH - 6,
            7,
            11,
            cut,
            cut,
        );

        self.piece.add_block(chunk, air, 5, 5, 10, bb);
        self.piece.add_block(chunk, air, 5, 6, 10, bb);
        self.piece.add_block(chunk, air, 6, 6, 10, bb);
        self.piece.add_block(chunk, air, WIDTH - 6, 5, 10, bb);
        self.piece.add_block(chunk, air, WIDTH - 6, 6, 10, bb);
        self.piece.add_block(chunk, air, WIDTH - 7, 6, 10, bb);

        self.piece
            .fill_with_outline(chunk, bb, false, 2, 4, 4, 2, 6, 4, air, air);
        self.piece
            .fill_with_outline(chunk, bb, false, WIDTH - 3, 4, 4, WIDTH - 3, 6, 4, air, air);
        self.piece.add_block(chunk, north_stairs, 2, 4, 5, bb);
        self.piece.add_block(chunk, north_stairs, 2, 3, 4, bb);
        self.piece
            .add_block(chunk, north_stairs, WIDTH - 3, 4, 5, bb);
        self.piece
            .add_block(chunk, north_stairs, WIDTH - 3, 3, 4, bb);

        self.piece
            .fill_with_outline(chunk, bb, false, 1, 1, 3, 2, 2, 3, ss, ss);
        self.piece
            .fill_with_outline(chunk, bb, false, WIDTH - 3, 1, 3, WIDTH - 2, 2, 3, ss, ss);
        self.piece.add_block(chunk, ss, 1, 1, 2, bb);
        self.piece.add_block(chunk, ss, WIDTH - 2, 1, 2, bb);
        self.piece.add_block(chunk, slab, 1, 2, 2, bb);
        self.piece.add_block(chunk, slab, WIDTH - 2, 2, 2, bb);
        self.piece.add_block(chunk, west_stairs, 2, 1, 2, bb);
        self.piece
            .add_block(chunk, east_stairs, WIDTH - 3, 1, 2, bb);

        self.piece
            .fill_with_outline(chunk, bb, false, 4, 3, 5, 4, 3, 17, ss, ss);
        self.piece
            .fill_with_outline(chunk, bb, false, WIDTH - 5, 3, 5, WIDTH - 5, 3, 17, ss, ss);
        self.piece
            .fill_with_outline(chunk, bb, false, 3, 1, 5, 4, 2, 16, air, air);
        self.piece.fill_with_outline(
            chunk,
            bb,
            false,
            WIDTH - 6,
            1,
            5,
            WIDTH - 5,
            2,
            16,
            air,
            air,
        );

        for z in (5..=17).step_by(2) {
            self.piece.add_block(chunk, cut, 4, 1, z, bb);
            self.piece.add_block(chunk, chiseled, 4, 2, z, bb);
            self.piece.add_block(chunk, cut, WIDTH - 5, 1, z, bb);
            self.piece.add_block(chunk, chiseled, WIDTH - 5, 2, z, bb);
        }

        self.piece.add_block(chunk, orange, 10, 0, 7, bb);
        self.piece.add_block(chunk, orange, 10, 0, 8, bb);
        self.piece.add_block(chunk, orange, 9, 0, 9, bb);
        self.piece.add_block(chunk, orange, 11, 0, 9, bb);
        self.piece.add_block(chunk, orange, 8, 0, 10, bb);
        self.piece.add_block(chunk, orange, 12, 0, 10, bb);
        self.piece.add_block(chunk, orange, 7, 0, 10, bb);
        self.piece.add_block(chunk, orange, 13, 0, 10, bb);
        self.piece.add_block(chunk, orange, 9, 0, 11, bb);
        self.piece.add_block(chunk, orange, 11, 0, 11, bb);
        self.piece.add_block(chunk, orange, 10, 0, 12, bb);
        self.piece.add_block(chunk, orange, 10, 0, 13, bb);
        self.piece.add_block(chunk, blue, 10, 0, 10, bb);

        for x in [0, WIDTH - 1] {
            self.piece.add_block(chunk, cut, x, 2, 1, bb);
            self.piece.add_block(chunk, orange, x, 2, 2, bb);
            self.piece.add_block(chunk, cut, x, 2, 3, bb);
            self.piece.add_block(chunk, cut, x, 3, 1, bb);
            self.piece.add_block(chunk, orange, x, 3, 2, bb);
            self.piece.add_block(chunk, cut, x, 3, 3, bb);
            self.piece.add_block(chunk, orange, x, 4, 1, bb);
            self.piece.add_block(chunk, chiseled, x, 4, 2, bb);
            self.piece.add_block(chunk, orange, x, 4, 3, bb);
            self.piece.add_block(chunk, cut, x, 5, 1, bb);
            self.piece.add_block(chunk, orange, x, 5, 2, bb);
            self.piece.add_block(chunk, cut, x, 5, 3, bb);
            self.piece.add_block(chunk, orange, x, 6, 1, bb);
            self.piece.add_block(chunk, chiseled, x, 6, 2, bb);
            self.piece.add_block(chunk, orange, x, 6, 3, bb);
            self.piece.add_block(chunk, orange, x, 7, 1, bb);
            self.piece.add_block(chunk, orange, x, 7, 2, bb);
            self.piece.add_block(chunk, orange, x, 7, 3, bb);
            self.piece.add_block(chunk, cut, x, 8, 1, bb);
            self.piece.add_block(chunk, cut, x, 8, 2, bb);
            self.piece.add_block(chunk, cut, x, 8, 3, bb);
        }

        for x in [2, WIDTH - 3] {
            self.piece.add_block(chunk, cut, x - 1, 2, 0, bb);
            self.piece.add_block(chunk, orange, x, 2, 0, bb);
            self.piece.add_block(chunk, cut, x + 1, 2, 0, bb);
            self.piece.add_block(chunk, cut, x - 1, 3, 0, bb);
            self.piece.add_block(chunk, orange, x, 3, 0, bb);
            self.piece.add_block(chunk, cut, x + 1, 3, 0, bb);
            self.piece.add_block(chunk, orange, x - 1, 4, 0, bb);
            self.piece.add_block(chunk, chiseled, x, 4, 0, bb);
            self.piece.add_block(chunk, orange, x + 1, 4, 0, bb);
            self.piece.add_block(chunk, cut, x - 1, 5, 0, bb);
            self.piece.add_block(chunk, orange, x, 5, 0, bb);
            self.piece.add_block(chunk, cut, x + 1, 5, 0, bb);
            self.piece.add_block(chunk, orange, x - 1, 6, 0, bb);
            self.piece.add_block(chunk, chiseled, x, 6, 0, bb);
            self.piece.add_block(chunk, orange, x + 1, 6, 0, bb);
            self.piece.add_block(chunk, orange, x - 1, 7, 0, bb);
            self.piece.add_block(chunk, orange, x, 7, 0, bb);
            self.piece.add_block(chunk, orange, x + 1, 7, 0, bb);
            self.piece.add_block(chunk, cut, x - 1, 8, 0, bb);
            self.piece.add_block(chunk, cut, x, 8, 0, bb);
            self.piece.add_block(chunk, cut, x + 1, 8, 0, bb);
        }

        self.piece
            .fill_with_outline(chunk, bb, false, 8, 4, 0, 12, 6, 0, cut, cut);
        self.piece.add_block(chunk, air, 8, 6, 0, bb);
        self.piece.add_block(chunk, air, 12, 6, 0, bb);
        self.piece.add_block(chunk, orange, 9, 5, 0, bb);
        self.piece.add_block(chunk, chiseled, 10, 5, 0, bb);
        self.piece.add_block(chunk, orange, 11, 5, 0, bb);

        self.piece
            .fill_with_outline(chunk, bb, false, 8, -14, 8, 12, -11, 12, cut, cut);
        self.piece
            .fill_with_outline(chunk, bb, false, 8, -10, 8, 12, -10, 12, chiseled, chiseled);
        self.piece
            .fill_with_outline(chunk, bb, false, 8, -9, 8, 12, -9, 12, cut, cut);
        self.piece
            .fill_with_outline(chunk, bb, false, 8, -8, 8, 12, -1, 12, ss, ss);
        self.piece
            .fill_with_outline(chunk, bb, false, 9, -11, 9, 11, -1, 11, air, air);
        self.piece.add_block(
            chunk,
            Block::STONE_PRESSURE_PLATE.default_state,
            10,
            -11,
            10,
            bb,
        );
        self.piece.fill_with_outline(
            chunk,
            bb,
            false,
            9,
            -13,
            9,
            11,
            -13,
            11,
            Block::TNT.default_state,
            air,
        );

        self.piece.add_block(chunk, air, 8, -11, 10, bb);
        self.piece.add_block(chunk, air, 8, -10, 10, bb);
        self.piece.add_block(chunk, chiseled, 7, -10, 10, bb);
        self.piece.add_block(chunk, cut, 7, -11, 10, bb);
        self.piece.add_block(chunk, air, 12, -11, 10, bb);
        self.piece.add_block(chunk, air, 12, -10, 10, bb);
        self.piece.add_block(chunk, chiseled, 13, -10, 10, bb);
        self.piece.add_block(chunk, cut, 13, -11, 10, bb);
        self.piece.add_block(chunk, air, 10, -11, 8, bb);
        self.piece.add_block(chunk, air, 10, -10, 8, bb);
        self.piece.add_block(chunk, chiseled, 10, -10, 7, bb);
        self.piece.add_block(chunk, cut, 10, -11, 7, bb);
        self.piece.add_block(chunk, air, 10, -11, 12, bb);
        self.piece.add_block(chunk, air, 10, -10, 12, bb);
        self.piece.add_block(chunk, chiseled, 10, -10, 13, bb);
        self.piece.add_block(chunk, cut, 10, -11, 13, bb);

        for (index, x, z) in [(0, 10, 12), (1, 8, 10), (2, 10, 8), (3, 12, 10)] {
            self.try_place_chest(chunk, bb, index, x, -11, z);
        }

        self.add_cellar(chunk, bb, &mut level_random, seed);
    }
}
