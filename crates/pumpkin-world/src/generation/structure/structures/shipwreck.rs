use std::sync::Arc;

use pumpkin_data::block_rotation::Rotation;
use pumpkin_util::{
    math::{block_box::BlockBox, vector3::Vector3},
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
                on_top_of_chunk_center,
            },
            template::{StructureTemplate, get_template, place_template},
        },
    },
};

const TEMPLATES: &[&str] = &[
    "shipwreck/rightsideup_backhalf",
    "shipwreck/rightsideup_backhalf_degraded",
    "shipwreck/rightsideup_fronthalf",
    "shipwreck/rightsideup_fronthalf_degraded",
    "shipwreck/rightsideup_full",
    "shipwreck/rightsideup_full_degraded",
    "shipwreck/sideways_backhalf",
    "shipwreck/sideways_backhalf_degraded",
    "shipwreck/sideways_fronthalf",
    "shipwreck/sideways_fronthalf_degraded",
    "shipwreck/sideways_full",
    "shipwreck/sideways_full_degraded",
    "shipwreck/upsidedown_backhalf",
    "shipwreck/upsidedown_backhalf_degraded",
    "shipwreck/upsidedown_fronthalf",
    "shipwreck/upsidedown_fronthalf_degraded",
    "shipwreck/upsidedown_full",
    "shipwreck/upsidedown_full_degraded",
    "shipwreck/with_mast",
    "shipwreck/with_mast_degraded",
];

pub struct ShipwreckGenerator {
    pub is_beached: bool,
}

impl StructureGenerator for ShipwreckGenerator {
    fn get_structure_position(
        &self,
        mut context: StructureGeneratorContext<'_>,
    ) -> Option<StructurePosition> {
        let start_pos = on_top_of_chunk_center(&mut context, !self.is_beached, 64);
        let chunk_center_x = get_center_x(context.chunk_x);
        let chunk_center_z = get_center_z(context.chunk_z);

        // Deterministically select rotation and template
        let rotation_idx = context.random.next_bounded_i32(4) as u8;
        let rotation = Rotation::from_index(rotation_idx);

        let template_idx = context.random.next_bounded_i32(TEMPLATES.len() as i32) as usize;
        let template_name = TEMPLATES[template_idx];
        let template = get_template(template_name)?;

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
        collector.add_piece(Box::new(ShipwreckPiece {
            piece: StructurePiece::new(StructurePieceType::Shipwreck, bounding_box, 0),
            template,
            rotation,
            is_beached: self.is_beached,
        }));

        Some(StructurePosition {
            start_pos,
            collector: Arc::new(collector.into()),
        })
    }
}

pub struct ShipwreckPiece {
    piece: StructurePiece,
    template: Arc<StructureTemplate>,
    rotation: Rotation,
    is_beached: bool,
}

impl StructurePieceBase for ShipwreckPiece {
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
        let height_map_type = if self.is_beached {
            pumpkin_util::HeightMap::WorldSurfaceWg
        } else {
            pumpkin_util::HeightMap::OceanFloorWg
        };

        let sample_y = chunk.get_top_y(&height_map_type, origin.x, origin.z);
        let target_y = if self.is_beached {
            sample_y - 1
        } else {
            sample_y - 3
        };
        let mut final_origin = origin;
        final_origin.y = target_y;

        place_template(
            chunk,
            &self.template,
            final_origin,
            (0, 0),
            self.rotation,
            true,
            !self.is_beached,
            &[],
            Some(chunk_box),
        );

        self.apply_loot_markers(chunk, final_origin, random);
    }
}

/// `ShipwreckPieces.MARKERS_TO_LOOT` (`ShipwreckPieces.java:69-71`).
fn marker_loot_table(marker: &str) -> Option<&'static str> {
    match marker {
        "map_chest" => Some("minecraft:chests/shipwreck_map"),
        "treasure_chest" => Some("minecraft:chests/shipwreck_treasure"),
        "supply_chest" => Some("minecraft:chests/shipwreck_supply"),
        _ => None,
    }
}

impl ShipwreckPiece {
    /// Implements `ShipwreckPieces.ShipwreckPiece.handleDataMarker` for every `DATA` structure
    /// block, in template order (`TemplateStructurePiece.java:93-101`,
    /// `ShipwreckPieces.java:126-134`): the container below a mapped marker gets the loot
    /// table, and a seed is drawn only when that container exists
    /// (`RandomizableContainer.java:42-48`).
    fn apply_loot_markers(
        &self,
        chunk: &mut ProtoChunk,
        origin: Vector3<i32>,
        random: &mut RandomGenerator,
    ) {
        for block in &self.template.blocks {
            let Some(marker_nbt) = &block.nbt else {
                continue;
            };
            let Some(palette_entry) = self.template.palette.get(block.state as usize) else {
                continue;
            };
            if palette_entry.name != "minecraft:structure_block"
                || marker_nbt.get_string("mode") != Some("DATA")
            {
                continue;
            }
            let Some(loot_table) = marker_nbt
                .get_string("metadata")
                .and_then(marker_loot_table)
            else {
                continue;
            };

            // Same transform `place_template` applies, so this resolves to the placed chest.
            let marker_pos = origin + self.rotation.transform_pos(block.pos, self.template.size);
            let chest_pos = Vector3::new(marker_pos.x, marker_pos.y - 1, marker_pos.z);
            for mut nbt in chunk.take_pending_block_entities() {
                if nbt.get_string("id") == Some("minecraft:chest")
                    && nbt.get_int("x") == Some(chest_pos.x)
                    && nbt.get_int("y") == Some(chest_pos.y)
                    && nbt.get_int("z") == Some(chest_pos.z)
                {
                    nbt.put_string("LootTable", loot_table.to_string());
                    nbt.put_long("LootTableSeed", random.next_i64());
                }
                chunk.add_block_entity(nbt);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::{block_rotation::Rotation, dimension::Dimension};
    use pumpkin_util::{
        math::{block_box::BlockBox, vector3::Vector3},
        random::{RandomGenerator, legacy_rand::LegacyRand},
        world_seed::Seed,
    };

    use super::ShipwreckPiece;
    use crate::generation::{
        get_world_gen,
        proto_chunk::ProtoChunk,
        structure::{
            piece::StructurePieceType,
            structures::StructurePiece,
            template::{get_template, place_template},
        },
    };

    // `with_mast` has one supply, map and treasure marker, each above a chest
    // (`ShipwreckPieces.java:69-71,126-134`); a rotated wreck must still resolve all three.
    #[test]
    fn rotated_with_mast_chests_get_marker_loot_tables() {
        let generator = get_world_gen(
            Seed(0),
            Dimension::OVERWORLD,
            true,
            Vec::new(),
            String::new(),
        );
        let Some(template) = get_template("shipwreck/with_mast") else {
            panic!("with_mast template is missing");
        };
        let piece = ShipwreckPiece {
            piece: StructurePiece::new(
                StructurePieceType::Shipwreck,
                BlockBox::new(0, 0, 0, 0, 0, 0),
                0,
            ),
            template,
            rotation: Rotation::Clockwise90,
            is_beached: false,
        };
        let origin = Vector3::new(0, 64, 0);
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(0));
        let mut tables = Vec::new();
        for chunk_x in 0..2 {
            for chunk_z in 0..2 {
                let mut chunk = ProtoChunk::new(chunk_x, chunk_z, &generator);
                let chunk_box = BlockBox::new(
                    chunk_x * 16,
                    -64,
                    chunk_z * 16,
                    chunk_x * 16 + 15,
                    319,
                    chunk_z * 16 + 15,
                );
                place_template(
                    &mut chunk,
                    &piece.template,
                    origin,
                    (0, 0),
                    piece.rotation,
                    true,
                    true,
                    &[],
                    Some(&chunk_box),
                );
                piece.apply_loot_markers(&mut chunk, origin, &mut random);
                for nbt in chunk.take_pending_block_entities() {
                    if let Some(table) = nbt.get_string("LootTable") {
                        assert_eq!(nbt.get_string("id"), Some("minecraft:chest"));
                        assert!(nbt.get_long("LootTableSeed").is_some());
                        tables.push(table.to_string());
                    }
                }
            }
        }
        tables.sort();
        assert_eq!(
            tables,
            [
                "minecraft:chests/shipwreck_map",
                "minecraft:chests/shipwreck_supply",
                "minecraft:chests/shipwreck_treasure",
            ]
        );
    }
}
