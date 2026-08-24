use std::sync::Arc;

use pumpkin_data::{
    Block, BlockState, block_properties::BlockProperties, block_rotation::Rotation,
};
use pumpkin_util::{
    math::{block_box::BlockBox, position::BlockPos},
    random::{RandomGenerator, RandomImpl},
};

use crate::{
    ProtoChunk,
    generation::{
        positions::chunk_pos::{get_center_x, get_center_z},
        structure::{
            piece::StructurePieceType,
            structures::{
                StructureGenerator, StructureGeneratorContext, StructurePiece, StructurePieceBase,
                StructurePiecesCollector, StructurePosition, WorldPortalExt,
            },
            template::{StructureTemplate, get_template, place_template},
        },
    },
};
use pumpkin_data::block_properties::ChestLikeProperties;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};

const COLD_RUINS: &[&str] = &[
    "underwater_ruin/brick_1",
    "underwater_ruin/brick_2",
    "underwater_ruin/brick_3",
    "underwater_ruin/brick_4",
    "underwater_ruin/brick_5",
    "underwater_ruin/brick_6",
    "underwater_ruin/brick_7",
    "underwater_ruin/brick_8",
    "underwater_ruin/cracked_1",
    "underwater_ruin/cracked_2",
    "underwater_ruin/cracked_3",
    "underwater_ruin/mossy_1",
    "underwater_ruin/mossy_2",
    "underwater_ruin/mossy_3",
];

const WARM_RUINS: &[&str] = &[
    "underwater_ruin/warm_1",
    "underwater_ruin/warm_2",
    "underwater_ruin/warm_3",
    "underwater_ruin/warm_4",
    "underwater_ruin/warm_5",
    "underwater_ruin/warm_6",
    "underwater_ruin/warm_7",
    "underwater_ruin/warm_8",
    "underwater_ruin/big_warm_4",
    "underwater_ruin/big_warm_5",
];

pub struct OceanRuinGenerator {
    pub is_warm: bool,
}

impl StructureGenerator for OceanRuinGenerator {
    fn get_structure_position(
        &self,
        mut context: StructureGeneratorContext<'_>,
    ) -> Option<StructurePosition> {
        let chunk_center_x = get_center_x(context.chunk_x);
        let chunk_center_z = get_center_z(context.chunk_z);

        let rotation_idx = context.random.next_bounded_i32(4) as u8;
        let rotation = Rotation::from_index(rotation_idx);

        let pool = if self.is_warm { WARM_RUINS } else { COLD_RUINS };
        let template_idx = context.random.next_bounded_i32(pool.len() as i32) as usize;
        let template_name = pool[template_idx];
        let template = get_template(template_name)?;
        let is_large = template_name.starts_with("underwater_ruin/big_");

        let size = template.size;
        let bounding_box = BlockBox::new(
            chunk_center_x - size.x / 2,
            context.min_y,
            chunk_center_z - size.z / 2,
            chunk_center_x + size.x / 2,
            256,
            chunk_center_z + size.z / 2,
        );

        let mut collector = StructurePiecesCollector::default();
        collector.add_piece(Box::new(OceanRuinPiece {
            piece: StructurePiece::new(StructurePieceType::OceanTemple, bounding_box, 0),
            template,
            rotation,
            is_large,
        }));

        Some(StructurePosition {
            start_pos: BlockPos::new(chunk_center_x, 64, chunk_center_z),
            collector: Arc::new(collector.into()),
        })
    }
}

pub struct OceanRuinPiece {
    piece: StructurePiece,
    template: Arc<StructureTemplate>,
    rotation: Rotation,
    is_large: bool,
}

impl StructurePieceBase for OceanRuinPiece {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn get_structure_piece(&self) -> &StructurePiece {
        &self.piece
    }
    fn get_structure_piece_mut(&mut self) -> &mut StructurePiece {
        &mut self.piece
    }
    fn place(
        &mut self,
        chunk: &mut ProtoChunk,
        _block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        _seed: i64,
        chunk_box: &BlockBox,
    ) {
        let origin = self.piece.bounding_box.min;
        let sample_y = chunk.get_top_y(&pumpkin_util::HeightMap::OceanFloorWg, origin.x, origin.z);
        let pos = BlockPos::new(origin.x, sample_y, origin.z);
        let corner_local = self.template.size;
        let corner_local =
            pumpkin_util::math::vector3::Vector3::new(corner_local.x - 1, 0, corner_local.z - 1);
        let corner_local = self
            .rotation
            .transform_pos(corner_local, self.template.size);
        let corner = BlockPos::new(
            origin.x + corner_local.x,
            sample_y,
            origin.z + corner_local.z,
        );
        let target_y = Self::get_height(chunk, pos, corner);
        let final_origin = pumpkin_util::math::vector3::Vector3::new(origin.x, target_y, origin.z);

        place_template(
            chunk,
            &self.template,
            final_origin,
            (0, 0),
            self.rotation,
            true, // skip air
            true, // apply waterlogging
            &[],
            Some(chunk_box),
        );

        self.handle_data_markers(chunk, chunk_box, final_origin, random);
    }
}

impl OceanRuinPiece {
    /// Implements `OceanRuinPieces.OceanRuinPiece.handleDataMarker`.
    /// Vanilla: `OceanRuinPieces.java:318-340`.
    fn handle_data_markers(
        &self,
        chunk: &mut ProtoChunk,
        chunk_box: &BlockBox,
        origin: pumpkin_util::math::vector3::Vector3<i32>,
        random: &mut RandomGenerator,
    ) {
        for block in &self.template.blocks {
            let palette = &self.template.palette[block.state as usize];
            if palette.name != "minecraft:structure_block" {
                continue;
            }
            let Some(marker) = block
                .nbt
                .as_ref()
                .and_then(|nbt| nbt.get_string("metadata"))
            else {
                continue;
            };
            let local = self.rotation.transform_pos(block.pos, self.template.size);
            let position = pumpkin_util::math::vector3::Vector3::new(
                origin.x + local.x,
                origin.y + local.y,
                origin.z + local.z,
            );
            if !chunk_box.contains_pos(&position) {
                continue;
            }

            if marker == "chest" {
                let mut props = ChestLikeProperties::default(&Block::CHEST);
                props.waterlogged =
                    chunk.get_block_state(&position).to_block_id() == Block::WATER.id;
                chunk.set_block_state(
                    position.x,
                    position.y,
                    position.z,
                    BlockState::from_id(props.to_state_id(&Block::CHEST)),
                );
                let mut nbt = NbtCompound::new();
                nbt.put_string("id", "minecraft:chest".to_string());
                nbt.put_int("x", position.x);
                nbt.put_int("y", position.y);
                nbt.put_int("z", position.z);
                nbt.put_string(
                    "LootTable",
                    if self.is_large {
                        "minecraft:chests/underwater_ruin_big"
                    } else {
                        "minecraft:chests/underwater_ruin_small"
                    }
                    .to_string(),
                );
                nbt.put_long("LootTableSeed", random.next_i64());
                chunk.add_block_entity(nbt);
            } else if marker == "drowned" {
                let mut nbt = NbtCompound::new();
                nbt.put_string("id", "minecraft:drowned".to_string());
                nbt.put(
                    "Pos",
                    NbtTag::List(vec![
                        (f64::from(position.x) + 0.5).into(),
                        f64::from(position.y).into(),
                        (f64::from(position.z) + 0.5).into(),
                    ]),
                );
                nbt.put("Rotation", NbtTag::List(vec![0.0f32.into(), 0.0f32.into()]));
                nbt.put_bool("PersistenceRequired", true);
                chunk.add_structure_entity(nbt);
                chunk.set_block_state(
                    position.x,
                    position.y,
                    position.z,
                    if position.y > chunk.sea_level() {
                        Block::AIR.default_state
                    } else {
                        Block::WATER.default_state
                    },
                );
            }
        }
    }

    /// Implements `OceanRuinPieces.OceanRuinPiece.postProcess`'s buried-height adjustment.
    /// Vanilla: `OceanRuinPieces.java:343-363` and `getHeight` at lines 365-397.
    ///
    /// Not ported: vanilla's descent condition tests the FLUID state
    /// (`tempFluid.is(FluidTags.WATER)`), which is also true for a waterlogged non-air block
    /// (kelp, seagrass, ...); this checks the plain `WATER` block only, so a waterlogged plant
    /// in the scanned footprint stops the descent one block early instead of being treated as
    /// open water.
    fn get_height(chunk: &ProtoChunk, pos: BlockPos, corner: BlockPos) -> i32 {
        let mut min_y = 512;
        let top_y = pos.0.y - 1;
        let mut area = 0;
        for x in pos.0.x.min(corner.0.x)..=pos.0.x.max(corner.0.x) {
            for z in pos.0.z.min(corner.0.z)..=pos.0.z.max(corner.0.z) {
                let mut floor_y = pos.0.y - 1;
                loop {
                    let state = chunk
                        .get_block_state(&pumpkin_util::math::vector3::Vector3::new(x, floor_y, z));
                    let block = Block::from_state_id(state);
                    if block != &Block::AIR && block != &Block::WATER && block != &Block::ICE
                        || floor_y <= chunk.bottom_y() as i32 + 1
                    {
                        break;
                    }
                    floor_y -= 1;
                }
                min_y = min_y.min(floor_y);
                if floor_y < top_y - 2 {
                    area += 1;
                }
            }
        }
        let width = (pos.0.x - corner.0.x).abs();
        if top_y - min_y > 2 && area > width - 2 {
            min_y + 1
        } else {
            pos.0.y
        }
    }
}
