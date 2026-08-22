use super::util::write_compound_nbt;
use crate::VarInt;
use crate::WritingError;
use crate::codec::bit_set::BitSet;
use crate::java::client::play::light_update::{
    light_container_has_data, light_masks, write_light_container,
};
use crate::ser::NetworkWriteExt;
use pumpkin_data::block_state_remap::remap_block_state_for_version;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_util::math::position::get_local_cord;
use pumpkin_util::version::JavaMinecraftVersion;
use pumpkin_world::chunk::ChunkData;
use pumpkin_world::chunk::palette::NetworkPalette;
use std::io::Write;

/// Serializes chunk data for Minecraft 1.18+ through 26.2+.
#[expect(clippy::too_many_lines)]
pub fn write_chunk_data(
    chunk: &ChunkData,
    mut write: impl Write,
    version: &JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_i32_be(chunk.x)?;
    write.write_i32_be(chunk.z)?;

    let heightmaps = chunk
        .heightmap
        .lock()
        .map_err(|_| WritingError::Message("heightmap lock poisoned".into()))?;
    if version >= &JavaMinecraftVersion::V_1_21_5 {
        // Three heightmaps, by `Heightmap.Types` id: WORLD_SURFACE (1), MOTION_BLOCKING (4)
        // and MOTION_BLOCKING_NO_LEAVES (5) - the three marked `Usage.CLIENT`
        // (`Heightmap.java:146,149,153`). OCEAN_FLOOR (id 3) is `Usage.LIVE_WORLD`
        // (`Heightmap.java:148`): persisted to disk, never sent, so it is absent here.
        write.write_var_int(&VarInt(3))?; // Map size

        let mut write_heightmap = |index: i32, data: &[i64]| -> Result<(), WritingError> {
            write.write_var_int(&VarInt(index))?;
            write.write_var_int(&VarInt(data.len() as i32))?;
            for val in data {
                write.write_i64_be(*val)?;
            }
            Ok(())
        };

        write_heightmap(1, heightmaps.world_surface.as_deref().unwrap_or(&[0; 37]))?;
        write_heightmap(4, heightmaps.motion_blocking.as_deref().unwrap_or(&[0; 37]))?;
        write_heightmap(
            5,
            heightmaps
                .motion_blocking_no_leaves
                .as_deref()
                .unwrap_or(&[0; 37]),
        )?;
    } else {
        // Vanilla's `Heightmap.Types` marks OCEAN_FLOOR as `Usage.LIVE_WORLD`
        // (`Heightmap.java:148`), not `Usage.CLIENT` like the three below - it is never sent
        // to the client, only persisted to disk. It is deliberately absent from the compound
        // built here.
        let mut comp = pumpkin_nbt::compound::NbtCompound::new();
        if let Some(ref ws) = heightmaps.world_surface {
            comp.put(
                "WORLD_SURFACE",
                pumpkin_nbt::tag::NbtTag::LongArray(ws.to_vec()),
            );
        }
        if let Some(ref mb) = heightmaps.motion_blocking {
            comp.put(
                "MOTION_BLOCKING",
                pumpkin_nbt::tag::NbtTag::LongArray(mb.to_vec()),
            );
        }
        if let Some(ref mbnl) = heightmaps.motion_blocking_no_leaves {
            comp.put(
                "MOTION_BLOCKING_NO_LEAVES",
                pumpkin_nbt::tag::NbtTag::LongArray(mbnl.to_vec()),
            );
        }
        write_compound_nbt(&mut write, comp, *version)?;
    }
    drop(heightmaps);

    {
        let mut blocks_and_biomes_buf = Vec::new();
        let block_sections = chunk
            .section
            .block_sections
            .read()
            .map_err(|_| WritingError::Message("block_sections read lock poisoned".into()))?;
        let biome_sections = chunk
            .section
            .biome_sections
            .read()
            .map_err(|_| WritingError::Message("biome_sections read lock poisoned".into()))?;

        let mut zero_bytes_count = 0;

        for (block_palette, biome_palette) in block_sections.iter().zip(biome_sections.iter()) {
            let non_empty_block_count = block_palette.non_air_block_count() as i16;
            blocks_and_biomes_buf.write_i16_be(non_empty_block_count)?;
            if version >= &JavaMinecraftVersion::V_26_1 {
                // New in 26.1, fluid count
                let liquid_count = block_palette.liquid_block_count() as i16;
                blocks_and_biomes_buf.write_i16_be(liquid_count)?;
            }

            let mut block_network = block_palette.convert_network();
            if version < &CURRENT_MC_VERSION {
                match &mut block_network.palette {
                    NetworkPalette::Single(registry_id) => {
                        *registry_id = remap_block_state_for_version(*registry_id, *version);
                    }
                    NetworkPalette::Indirect(palette) => {
                        for registry_id in palette.iter_mut() {
                            *registry_id = remap_block_state_for_version(*registry_id, *version);
                        }
                    }
                    NetworkPalette::Direct => {
                        let bits_per_entry = usize::from(block_network.bits_per_entry);
                        let values_per_i64 = 64 / bits_per_entry;
                        let id_mask = (1u64 << bits_per_entry) - 1;

                        for packed_word in &mut block_network.packed_data {
                            let mut remapped_word = 0u64;
                            let packed_word_u64 = *packed_word as u64;
                            for index in 0..values_per_i64 {
                                let shift = index * bits_per_entry;
                                let state_id = ((packed_word_u64 >> shift) & id_mask) as u16;
                                let remapped_id = remap_block_state_for_version(state_id, *version);
                                remapped_word |= u64::from(remapped_id) << shift;
                            }
                            *packed_word = remapped_word as i64;
                        }
                    }
                }
            }
            blocks_and_biomes_buf.write_u8(block_network.bits_per_entry)?;

            match block_network.palette {
                NetworkPalette::Single(registry_id) => {
                    blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                }
                NetworkPalette::Indirect(palette) => {
                    blocks_and_biomes_buf.write_var_int(&palette.len().try_into().map_err(
                        |_| {
                            WritingError::Message(format!(
                                "{} is not representable as a VarInt!",
                                palette.len()
                            ))
                        },
                    )?)?;
                    for registry_id in palette {
                        blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                    }
                }
                NetworkPalette::Direct => {}
            }

            if version <= &JavaMinecraftVersion::V_1_21_4 {
                blocks_and_biomes_buf.write_list(&block_network.packed_data, |buf, &packed| {
                    buf.write_i64_be(packed)
                })?;
            } else {
                for packed in &block_network.packed_data {
                    blocks_and_biomes_buf.write_i64_be(*packed)?;
                }
            }

            let biome_network = biome_palette.convert_network();
            blocks_and_biomes_buf.write_u8(biome_network.bits_per_entry)?;

            match biome_network.palette {
                NetworkPalette::Single(registry_id) => {
                    blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                }
                NetworkPalette::Indirect(palette) => {
                    blocks_and_biomes_buf.write_var_int(&palette.len().try_into().map_err(
                        |_| {
                            WritingError::Message(format!(
                                "{} is not representable as a VarInt!",
                                palette.len()
                            ))
                        },
                    )?)?;
                    for registry_id in palette {
                        blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                    }
                }
                NetworkPalette::Direct => {}
            }

            if version <= &JavaMinecraftVersion::V_1_21_4 {
                blocks_and_biomes_buf.write_list(&biome_network.packed_data, |buf, &packed| {
                    buf.write_i64_be(packed)
                })?;
            } else {
                for packed in &biome_network.packed_data {
                    blocks_and_biomes_buf.write_i64_be(*packed)?;
                }
            }

            if version == &JavaMinecraftVersion::V_1_21_5 {
                let block_storage_len = block_network.packed_data.len() as i32;
                let biome_storage_len = biome_network.packed_data.len() as i32;
                zero_bytes_count += VarInt(block_storage_len).written_size()
                    + VarInt(biome_storage_len).written_size();
            }
        }

        if version == &JavaMinecraftVersion::V_1_21_5 && zero_bytes_count > 0 {
            blocks_and_biomes_buf.resize(blocks_and_biomes_buf.len() + zero_bytes_count, 0);
        }

        write.write_var_int(&blocks_and_biomes_buf.len().try_into().map_err(|_| {
            WritingError::Message(format!(
                "{} is not representable as a VarInt!",
                blocks_and_biomes_buf.len()
            ))
        })?)?;
        write.write_slice(&blocks_and_biomes_buf)?;
    };

    let block_entities = chunk
        .pending_block_entities
        .lock()
        .map_err(|_| WritingError::Message("block_entities lock poisoned".into()))?;
    write.write_var_int(&VarInt(block_entities.len() as i32))?;
    for (pos, nbt) in block_entities.iter() {
        let local_xz = ((get_local_cord(pos.0.x) & 0xF) << 4) | (get_local_cord(pos.0.z) & 0xF);

        write.write_u8(local_xz as u8)?;
        write.write_i16_be(pos.0.y as i16)?;

        let id = nbt.get_string("id").map_or(0, |id_str| {
            let name = id_str.split(':').next_back().unwrap_or(id_str);
            pumpkin_data::block_properties::BLOCK_ENTITY_TYPES
                .iter()
                .position(|&n| n == name)
                .unwrap_or(0)
        });
        let remapped_id =
            pumpkin_data::block_entity_type_id_remap::remap_block_entity_type_id_for_version(
                id as u32, *version,
            );

        write.write_var_int(&VarInt(remapped_id as i32))?;

        let mut client_nbt = nbt.clone();
        client_nbt.child_tags.remove("id");
        client_nbt.child_tags.remove("x");
        client_nbt.child_tags.remove("y");
        client_nbt.child_tags.remove("z");
        client_nbt.child_tags.remove("LootTable");
        client_nbt.child_tags.remove("LootTableSeed");
        client_nbt.child_tags.remove("PumpkinCustomData");
        client_nbt.child_tags.remove("BukkitValues");
        write_compound_nbt(&mut write, client_nbt, *version)?;
    }

    {
        // Light masks include sections from -1 (below world) to num_sections (above world)
        // This means we need to account for 2 extra sections in the bitset
        let light_engine = chunk
            .light_engine
            .lock()
            .map_err(|_| WritingError::Message("light_engine lock poisoned".into()))?;
        let num_sections = light_engine.sky_light.len();

        // Shared with `CLightUpdate`: a uniform `Empty(15)` layer is real full data on the
        // wire - only an implicit zero layer is `DataLayer.isEmpty()`
        // (`DataLayer.java:141-143`) - so it belongs in the data mask with a serialized 0xFF
        // array.
        let masks = light_masks(&light_engine);

        // Trust edges (1.18 - 1.19.4; removed in 1.20)
        if version < &JavaMinecraftVersion::V_1_20 {
            write.write_bool(true)?;
        }

        // Write Sky Light Mask
        write.write_bitset(&BitSet(Box::new([masks.sky as i64])))?;
        // Write Block Light Mask
        write.write_bitset(&BitSet(Box::new([masks.block as i64])))?;
        // Write Empty Sky Light Mask
        write.write_bitset(&BitSet(Box::new([masks.empty_sky as i64])))?;
        // Write Empty Block Light Mask
        write.write_bitset(&BitSet(Box::new([masks.empty_block as i64])))?;

        // Write Sky Light arrays
        write.write_var_int(&VarInt(masks.sky.count_ones() as i32))?;
        for section_index in 0..num_sections {
            if light_container_has_data(&light_engine.sky_light[section_index]) {
                write_light_container(&mut write, &light_engine.sky_light[section_index])?;
            }
        }

        // Write Block Light arrays
        write.write_var_int(&VarInt(masks.block.count_ones() as i32))?;
        for section_index in 0..num_sections {
            if light_container_has_data(&light_engine.block_light[section_index]) {
                write_light_container(&mut write, &light_engine.block_light[section_index])?;
            }
        }
    }

    Ok(())
}
