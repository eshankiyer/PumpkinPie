use std::io::Write;

use crate::codec::bit_set::BitSet;
use crate::codec::var_int::VarInt;
use crate::ser::{NetworkReadExt, NetworkWriteExt, ReadingError, WritingError};
use crate::{ClientPacket, ServerPacket};
use pumpkin_data::packet::clientbound::play::LIGHT_UPDATE;
use pumpkin_macros::java_packet;
use pumpkin_util::version::JavaMinecraftVersion;
use pumpkin_world::chunk::format::LightContainer;
use pumpkin_world::chunk::{ChunkData, ChunkLight};

/// Sent by the server to update light levels (block light and sky light) for a chunk.
///
/// This packet updates lighting data for a specific chunk without sending the full chunk data.
/// It was introduced in Minecraft 1.14 (protocol version 477).
#[derive(Debug, PartialEq, Eq, Clone)]
#[java_packet(LIGHT_UPDATE)]
pub struct CLightUpdate {
    pub chunk_x: VarInt,
    pub chunk_z: VarInt,
    pub light_data: LightData,
}

pub type CUpdateLight = CLightUpdate;

#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct LightData {
    pub trust_edges: bool,
    pub sky_light_mask: BitSet,
    pub block_light_mask: BitSet,
    pub empty_sky_light_mask: BitSet,
    pub empty_block_light_mask: BitSet,
    pub sky_light_arrays: Vec<Vec<u8>>,
    pub block_light_arrays: Vec<Vec<u8>>,
}

/// The four masks shared by initial chunk data (`CChunkData`, 1.18+) and incremental light
/// updates.
///
/// Minecraft numbers light sections from the padding section below the world, so physical chunk
/// section zero is bit one and the final above-world padding section is also explicitly empty.
pub(super) struct LightMasks {
    pub sky: u64,
    pub block: u64,
    pub empty_sky: u64,
    pub empty_block: u64,
}

/// Java's `DataLayer.isEmpty()` is `data == null && defaultValue == 0`
/// (`DataLayer.java:141-143`), so it is true only for an implicit zero-filled layer.
/// `Empty(15)` is a uniform layer in Pumpkin's storage representation, but it is a real
/// full-data layer in the wire protocol and must be included in the data mask with a
/// serialized 0xFF array.
pub(super) const fn light_container_has_data(container: &LightContainer) -> bool {
    !matches!(container, LightContainer::Empty(0))
}

/// The 2048-byte nibble array a light container serializes to.
pub(super) fn light_container_bytes(container: &LightContainer) -> Vec<u8> {
    match container {
        LightContainer::Full(data) => data.to_vec(),
        LightContainer::Empty(default) => {
            vec![default << 4 | default; LightContainer::ARRAY_SIZE]
        }
    }
}

/// Writes one light container as a `VarInt`-prefixed 2048-byte array.
pub(super) fn write_light_container(
    write: &mut impl Write,
    container: &LightContainer,
) -> Result<(), WritingError> {
    let light_data_size = VarInt(LightContainer::ARRAY_SIZE as i32);
    write.write_var_int(&light_data_size)?;
    match container {
        LightContainer::Full(data) => write.write_slice(data.as_ref())?,
        LightContainer::Empty(default) => {
            let byte = default << 4 | default;
            write.write_slice(&[byte; LightContainer::ARRAY_SIZE])?;
        }
    }
    Ok(())
}

pub(super) fn light_masks(light_engine: &ChunkLight) -> LightMasks {
    light_masks_for_sections(light_engine, None)
}

/// When `changed_sections` is `Some`, only those physical sections appear in any mask and the
/// below/above-world padding bits are deliberately omitted: an incremental update must not
/// claim to describe sections it does not carry arrays for.
pub(super) fn light_masks_for_sections(
    light_engine: &ChunkLight,
    changed_sections: Option<&[usize]>,
) -> LightMasks {
    let num_sections = light_engine.sky_light.len();
    let include_padding = changed_sections.is_none();
    let mut masks = LightMasks {
        sky: 0,
        block: 0,
        empty_sky: u64::from(include_padding),
        empty_block: u64::from(include_padding),
    };

    for section_index in 0..num_sections {
        if let Some(changed_sections) = changed_sections
            && !changed_sections.contains(&section_index)
        {
            continue;
        }

        let bit_index = section_index + 1;
        if light_container_has_data(&light_engine.sky_light[section_index]) {
            masks.sky |= 1 << bit_index;
        } else {
            masks.empty_sky |= 1 << bit_index;
        }

        if light_container_has_data(&light_engine.block_light[section_index]) {
            masks.block |= 1 << bit_index;
        } else {
            masks.empty_block |= 1 << bit_index;
        }
    }

    if include_padding {
        let above_world_bit = num_sections + 1;
        masks.empty_sky |= 1 << above_world_bit;
        masks.empty_block |= 1 << above_world_bit;
    }
    masks
}

impl CLightUpdate {
    #[must_use]
    pub const fn new(chunk_x: VarInt, chunk_z: VarInt, light_data: LightData) -> Self {
        Self {
            chunk_x,
            chunk_z,
            light_data,
        }
    }

    pub fn from_chunk(
        chunk: &ChunkData,
        version: JavaMinecraftVersion,
    ) -> Result<Self, WritingError> {
        Self::sections_inner(chunk, version, None)
    }

    /// An incremental update carrying only the sections the light engine marked dirty, as
    /// vanilla's `ServerChunkCache` does when propagation touches a subset of a chunk.
    pub fn sections(
        chunk: &ChunkData,
        sections: &[usize],
        version: JavaMinecraftVersion,
    ) -> Result<Self, WritingError> {
        Self::sections_inner(chunk, version, Some(sections))
    }

    fn sections_inner(
        chunk: &ChunkData,
        version: JavaMinecraftVersion,
        sections: Option<&[usize]>,
    ) -> Result<Self, WritingError> {
        let light_data = LightData::from_chunk_sections(chunk, version, sections)?;
        Ok(Self {
            chunk_x: VarInt(chunk.x),
            chunk_z: VarInt(chunk.z),
            light_data,
        })
    }
}

impl LightData {
    #[must_use]
    pub const fn new(
        trust_edges: bool,
        sky_light_mask: BitSet,
        block_light_mask: BitSet,
        empty_sky_light_mask: BitSet,
        empty_block_light_mask: BitSet,
        sky_light_arrays: Vec<Vec<u8>>,
        block_light_arrays: Vec<Vec<u8>>,
    ) -> Self {
        Self {
            trust_edges,
            sky_light_mask,
            block_light_mask,
            empty_sky_light_mask,
            empty_block_light_mask,
            sky_light_arrays,
            block_light_arrays,
        }
    }

    pub fn from_chunk(
        chunk: &ChunkData,
        version: JavaMinecraftVersion,
    ) -> Result<Self, WritingError> {
        Self::from_chunk_sections(chunk, version, None)
    }

    /// `changed_sections` is honoured only on the 1.18+ path; pre-1.18 clients have no
    /// incremental section addressing here and always receive the full column.
    #[expect(clippy::too_many_lines)]
    pub fn from_chunk_sections(
        chunk: &ChunkData,
        version: JavaMinecraftVersion,
        changed_sections: Option<&[usize]>,
    ) -> Result<Self, WritingError> {
        let light_engine = chunk
            .light_engine
            .lock()
            .map_err(|_| WritingError::Message("light_engine lock poisoned".into()))?;

        if version < JavaMinecraftVersion::V_1_18 {
            let base_section = (0 - chunk.section.min_y).max(0) as usize / 16;
            let mut sky_light_mask = 0u64;
            let mut block_light_mask = 0u64;
            let mut sky_light_empty_mask = 0u64;
            let mut block_light_empty_mask = 0u64;
            let mut sky_light_arrays = Vec::new();
            let mut block_light_arrays = Vec::new();

            // Bit 0: Y = -1 (below world section 0)
            if base_section > 0 && base_section - 1 < light_engine.sky_light.len() {
                match &light_engine.sky_light[base_section - 1] {
                    LightContainer::Full(data) => {
                        sky_light_mask |= 1 << 0;
                        sky_light_arrays.push(data.to_vec());
                    }
                    LightContainer::Empty(val) if *val > 0 => {
                        sky_light_mask |= 1 << 0;
                        sky_light_arrays.push(vec![*val << 4 | *val; 2048]);
                    }
                    LightContainer::Empty(_) => {
                        sky_light_empty_mask |= 1 << 0;
                    }
                }
            } else {
                sky_light_empty_mask |= 1 << 0;
            }

            if base_section > 0 && base_section - 1 < light_engine.block_light.len() {
                match &light_engine.block_light[base_section - 1] {
                    LightContainer::Full(data) => {
                        block_light_mask |= 1 << 0;
                        block_light_arrays.push(data.to_vec());
                    }
                    LightContainer::Empty(val) if *val > 0 => {
                        block_light_mask |= 1 << 0;
                        block_light_arrays.push(vec![*val << 4 | *val; 2048]);
                    }
                    LightContainer::Empty(_) => {
                        block_light_empty_mask |= 1 << 0;
                    }
                }
            } else {
                block_light_empty_mask |= 1 << 0;
            }

            // Bits 1..=16: world sections (Y = 0..15)
            for i in 0..16 {
                let bit_index = i + 1;
                let sec_idx = base_section + i;

                if sec_idx < light_engine.sky_light.len() {
                    match &light_engine.sky_light[sec_idx] {
                        LightContainer::Full(data) => {
                            sky_light_mask |= 1 << bit_index;
                            sky_light_arrays.push(data.to_vec());
                        }
                        LightContainer::Empty(val) if *val > 0 => {
                            sky_light_mask |= 1 << bit_index;
                            sky_light_arrays.push(vec![*val << 4 | *val; 2048]);
                        }
                        LightContainer::Empty(_) => {
                            sky_light_empty_mask |= 1 << bit_index;
                        }
                    }
                } else {
                    sky_light_empty_mask |= 1 << bit_index;
                }

                if sec_idx < light_engine.block_light.len() {
                    match &light_engine.block_light[sec_idx] {
                        LightContainer::Full(data) => {
                            block_light_mask |= 1 << bit_index;
                            block_light_arrays.push(data.to_vec());
                        }
                        LightContainer::Empty(val) if *val > 0 => {
                            block_light_mask |= 1 << bit_index;
                            block_light_arrays.push(vec![*val << 4 | *val; 2048]);
                        }
                        LightContainer::Empty(_) => {
                            block_light_empty_mask |= 1 << bit_index;
                        }
                    }
                } else {
                    block_light_empty_mask |= 1 << bit_index;
                }
            }

            // Bit 17: Y = 16 (above world section 15)
            let top_sec = base_section + 16;
            if top_sec < light_engine.sky_light.len() {
                match &light_engine.sky_light[top_sec] {
                    LightContainer::Full(data) => {
                        sky_light_mask |= 1 << 17;
                        sky_light_arrays.push(data.to_vec());
                    }
                    LightContainer::Empty(val) if *val > 0 => {
                        sky_light_mask |= 1 << 17;
                        sky_light_arrays.push(vec![*val << 4 | *val; 2048]);
                    }
                    LightContainer::Empty(_) => {
                        sky_light_empty_mask |= 1 << 17;
                    }
                }
            } else {
                sky_light_empty_mask |= 1 << 17;
            }

            if top_sec < light_engine.block_light.len() {
                match &light_engine.block_light[top_sec] {
                    LightContainer::Full(data) => {
                        block_light_mask |= 1 << 17;
                        block_light_arrays.push(data.to_vec());
                    }
                    LightContainer::Empty(val) if *val > 0 => {
                        block_light_mask |= 1 << 17;
                        block_light_arrays.push(vec![*val << 4 | *val; 2048]);
                    }
                    LightContainer::Empty(_) => {
                        block_light_empty_mask |= 1 << 17;
                    }
                }
            } else {
                block_light_empty_mask |= 1 << 17;
            }

            Ok(Self {
                trust_edges: true,
                sky_light_mask: BitSet::from_u64(sky_light_mask),
                block_light_mask: BitSet::from_u64(block_light_mask),
                empty_sky_light_mask: BitSet::from_u64(sky_light_empty_mask),
                empty_block_light_mask: BitSet::from_u64(block_light_empty_mask),
                sky_light_arrays,
                block_light_arrays,
            })
        } else {
            let num_sections = light_engine.sky_light.len();
            // A uniform `Empty(15)` layer is real full data on the wire - only an implicit
            // zero layer is `DataLayer.isEmpty()` (`DataLayer.java:141-143`) - so it belongs
            // in the data mask with a serialized 0xFF array.
            let masks = light_masks_for_sections(&light_engine, changed_sections);

            let mut sky_light_arrays = Vec::new();
            let mut block_light_arrays = Vec::new();

            for section_index in 0..num_sections {
                if changed_sections.is_some_and(|sections| !sections.contains(&section_index)) {
                    continue;
                }

                if light_container_has_data(&light_engine.sky_light[section_index]) {
                    sky_light_arrays.push(light_container_bytes(
                        &light_engine.sky_light[section_index],
                    ));
                }
                if light_container_has_data(&light_engine.block_light[section_index]) {
                    block_light_arrays.push(light_container_bytes(
                        &light_engine.block_light[section_index],
                    ));
                }
            }

            Ok(Self {
                trust_edges: true,
                sky_light_mask: BitSet::from_u64(masks.sky),
                block_light_mask: BitSet::from_u64(masks.block),
                empty_sky_light_mask: BitSet::from_u64(masks.empty_sky),
                empty_block_light_mask: BitSet::from_u64(masks.empty_block),
                sky_light_arrays,
                block_light_arrays,
            })
        }
    }

    pub fn write(
        &self,
        mut write: impl Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        // Trust edges (1.16 - 1.19.4; added in 1.16, removed in 1.20)
        if *version >= JavaMinecraftVersion::V_1_16 && *version <= JavaMinecraftVersion::V_1_19_4 {
            write.write_bool(self.trust_edges)?;
        }

        // Chunk bitmasks
        if *version >= JavaMinecraftVersion::V_1_17 {
            write.write_bitset(&self.sky_light_mask)?;
            write.write_bitset(&self.block_light_mask)?;
            write.write_bitset(&self.empty_sky_light_mask)?;
            write.write_bitset(&self.empty_block_light_mask)?;
        } else {
            write.write_var_int(&VarInt(self.sky_light_mask.as_u64() as i32))?;
            write.write_var_int(&VarInt(self.block_light_mask.as_u64() as i32))?;
            write.write_var_int(&VarInt(self.empty_sky_light_mask.as_u64() as i32))?;
            write.write_var_int(&VarInt(self.empty_block_light_mask.as_u64() as i32))?;
        }

        // Sky light arrays
        if *version >= JavaMinecraftVersion::V_1_17 {
            write.write_var_int(&VarInt(self.sky_light_arrays.len() as i32))?;
            for array in &self.sky_light_arrays {
                write.write_var_int(&VarInt(array.len() as i32))?;
                write.write_slice(array)?;
            }
        } else {
            let mut array_idx = 0;
            for i in 0..18 {
                if self.sky_light_mask.get_bit(i)
                    && let Some(array) = self.sky_light_arrays.get(array_idx)
                {
                    write.write_var_int(&VarInt(array.len() as i32))?;
                    write.write_slice(array)?;
                    array_idx += 1;
                }
            }
        }

        // Block light arrays
        if *version >= JavaMinecraftVersion::V_1_17 {
            write.write_var_int(&VarInt(self.block_light_arrays.len() as i32))?;
            for array in &self.block_light_arrays {
                write.write_var_int(&VarInt(array.len() as i32))?;
                write.write_slice(array)?;
            }
        } else {
            let mut array_idx = 0;
            for i in 0..18 {
                if self.block_light_mask.get_bit(i)
                    && let Some(array) = self.block_light_arrays.get(array_idx)
                {
                    write.write_var_int(&VarInt(array.len() as i32))?;
                    write.write_slice(array)?;
                    array_idx += 1;
                }
            }
        }

        Ok(())
    }

    pub fn read(bytebuf: &mut &[u8], version: &JavaMinecraftVersion) -> Result<Self, ReadingError> {
        let trust_edges = if *version >= JavaMinecraftVersion::V_1_16
            && *version <= JavaMinecraftVersion::V_1_19_4
        {
            bytebuf.get_bool()?
        } else {
            false
        };

        let (sky_light_mask, block_light_mask, empty_sky_light_mask, empty_block_light_mask) =
            if *version >= JavaMinecraftVersion::V_1_17 {
                (
                    BitSet::decode(bytebuf)?,
                    BitSet::decode(bytebuf)?,
                    BitSet::decode(bytebuf)?,
                    BitSet::decode(bytebuf)?,
                )
            } else {
                (
                    BitSet::from_u64(bytebuf.get_var_int()?.0 as u64),
                    BitSet::from_u64(bytebuf.get_var_int()?.0 as u64),
                    BitSet::from_u64(bytebuf.get_var_int()?.0 as u64),
                    BitSet::from_u64(bytebuf.get_var_int()?.0 as u64),
                )
            };

        let sky_light_arrays = if *version >= JavaMinecraftVersion::V_1_17 {
            let count = bytebuf.get_var_int()?.0 as usize;
            let mut arrays = Vec::with_capacity(count);
            for _ in 0..count {
                let len = bytebuf.get_var_int()?.0 as usize;
                let mut buf = vec![0u8; len];
                bytebuf.read_bytes_to_buf(&mut buf)?;
                arrays.push(buf);
            }
            arrays
        } else {
            let mut arrays = Vec::new();
            for i in 0..18 {
                if sky_light_mask.get_bit(i) {
                    let len = bytebuf.get_var_int()?.0 as usize;
                    let mut buf = vec![0u8; len];
                    bytebuf.read_bytes_to_buf(&mut buf)?;
                    arrays.push(buf);
                }
            }
            arrays
        };

        let block_light_arrays = if *version >= JavaMinecraftVersion::V_1_17 {
            let count = bytebuf.get_var_int()?.0 as usize;
            let mut arrays = Vec::with_capacity(count);
            for _ in 0..count {
                let len = bytebuf.get_var_int()?.0 as usize;
                let mut buf = vec![0u8; len];
                bytebuf.read_bytes_to_buf(&mut buf)?;
                arrays.push(buf);
            }
            arrays
        } else {
            let mut arrays = Vec::new();
            for i in 0..18 {
                if block_light_mask.get_bit(i) {
                    let len = bytebuf.get_var_int()?.0 as usize;
                    let mut buf = vec![0u8; len];
                    bytebuf.read_bytes_to_buf(&mut buf)?;
                    arrays.push(buf);
                }
            }
            arrays
        };

        Ok(Self {
            trust_edges,
            sky_light_mask,
            block_light_mask,
            empty_sky_light_mask,
            empty_block_light_mask,
            sky_light_arrays,
            block_light_arrays,
        })
    }
}

impl ClientPacket for CLightUpdate {
    fn write_packet_data(
        &self,
        mut write: impl Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        write.write_var_int(&self.chunk_x)?;
        write.write_var_int(&self.chunk_z)?;
        self.light_data.write(&mut write, version)
    }
}

impl<'a> ServerPacket<'a> for CLightUpdate {
    fn read(bytebuf: &mut &'a [u8], version: &JavaMinecraftVersion) -> Result<Self, ReadingError> {
        let chunk_x = bytebuf.get_var_int()?;
        let chunk_z = bytebuf.get_var_int()?;
        let light_data = LightData::read(bytebuf, version)?;
        Ok(Self {
            chunk_x,
            chunk_z,
            light_data,
        })
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_world::chunk::format::LightContainer;

    use super::{CLightUpdate, light_masks};
    use crate::ClientPacket;
    use pumpkin_data::chunk::ChunkStatus;
    use pumpkin_util::version::JavaMinecraftVersion;
    use pumpkin_world::chunk::{ChunkData, ChunkLight, ChunkSections};
    use pumpkin_world::tick::scheduler::ChunkTickScheduler;

    /// Builds a `ChunkData` whose light arrays are exactly `sky`/`block`.
    fn chunk_with_light(sky: Vec<LightContainer>, block: Vec<LightContainer>) -> ChunkData {
        let sections = sky.len();
        assert_eq!(sections, block.len());
        ChunkData {
            section: ChunkSections::new(sections, 0),
            heightmap: std::sync::Mutex::default(),
            x: 0,
            z: 0,
            block_ticks: ChunkTickScheduler::default(),
            fluid_ticks: ChunkTickScheduler::default(),
            pending_block_entities: std::sync::Mutex::default(),
            light_engine: std::sync::Mutex::new(ChunkLight {
                sky_light: sky.into_boxed_slice(),
                block_light: block.into_boxed_slice(),
            }),
            light_populated: std::sync::atomic::AtomicBool::new(true),
            status: ChunkStatus::Full,
            blending_data: None,
            unknown_nbt: pumpkin_nbt::compound::NbtCompound::new(),
            dirty: std::sync::atomic::AtomicBool::new(false),
            inhabited_time: std::sync::atomic::AtomicU64::new(0),
            custom_data: std::sync::Mutex::new(pumpkin_nbt::compound::NbtCompound::new()),
        }
    }

    fn serialize(chunk: &ChunkData) -> Vec<u8> {
        let mut out = Vec::new();
        CLightUpdate::from_chunk(chunk, JavaMinecraftVersion::V_26_2)
            .unwrap()
            .write_packet_data(&mut out, &JavaMinecraftVersion::V_26_2)
            .unwrap();
        out
    }

    fn serialize_sections(chunk: &ChunkData, sections: &[usize]) -> Vec<u8> {
        let mut out = Vec::new();
        CLightUpdate::sections(chunk, sections, JavaMinecraftVersion::V_26_2)
            .unwrap()
            .write_packet_data(&mut out, &JavaMinecraftVersion::V_26_2)
            .unwrap();
        out
    }

    /// The client's nibble layout, per `DataLayer.java`:
    ///
    /// ```java
    /// getIndex(x, y, z) { return y << 8 | z << 4 | x; }
    /// getByteIndex(i)   { return i >> 1; }
    /// getNibbleIndex(i) { return i & 1; }
    /// get(i) -> data[getByteIndex(i)] >> 4 * getNibbleIndex(i) & 15
    /// ```
    ///
    /// So `y` is slowest-varying and `x` fastest, index parity 0 is the LOW nibble, and each
    /// section is `4096 / 2 = 2048` bytes (`DataLayer.java:11`, `SIZE`). The packet framing is
    /// from <https://minecraft.wiki/w/Java_Edition_protocol>: four `BitSet`s (sky, block,
    /// empty sky, empty block), then a prefixed array of sky light arrays - "There is 1 array
    /// for each bit set to true in the sky light mask, starting with the lowest value" - then
    /// the same for block light, each inner array length 2048.
    ///
    /// This asserts the byte at the exact protocol offset rather than reading the code. Point
    /// chosen with x != z != y so that BOTH a transposed index formula and a swapped nibble
    /// parity move the answer:
    ///   correct:    idx = 2*256 + 3*16 + 1 = 561 -> byte 280, HIGH nibble  -> 0xA0
    ///   transposed: idx = 1*256 + 3*16 + 2 = 306 -> byte 153, LOW nibble
    #[test]
    fn sky_light_nibble_lands_at_the_offset_the_client_reads() {
        let mut lit = LightContainer::new_filled(0);
        lit.set(1, 2, 3, 0xA);
        let chunk = chunk_with_light(vec![lit], vec![LightContainer::new_empty(0)]);
        // chunk_x (1) + chunk_z (1) + 4 BitSets of one long each (4 * (1 + 8)) = 38
        // + sky array count VarInt (1) + inner array length VarInt(2048) (2) = 41
        let sky_array_start: usize = 1 + 1 + 4 * (1 + 8) + 1 + 2;
        let bytes = serialize(&chunk);

        // VarInt(2048) is 0x80 0x10; assert the framing before trusting the offset.
        assert_eq!(
            &bytes[sky_array_start - 3..sky_array_start],
            &[1, 0x80, 0x10]
        );

        let index = 2 * 256 + 3 * 16 + 1;
        assert_eq!(index, 561);
        assert_eq!(
            bytes[sky_array_start + (index >> 1)],
            0xA0,
            "sky light nibble for (1,2,3) must be the high nibble of byte {}",
            index >> 1
        );
        // Every other byte of the section is 0: nothing leaked to a transposed offset.
        assert!(
            bytes[sky_array_start..sky_array_start + 2048]
                .iter()
                .enumerate()
                .all(|(i, &b)| (i == index >> 1) == (b != 0))
        );
    }

    /// A mask whose popcount disagrees with the number of arrays actually written
    /// desynchronises every following array, so pin the exact serialized length and the
    /// exact mask bytes for a mixed chunk.
    #[test]
    fn array_count_matches_mask_popcount_and_total_length() {
        let chunk = chunk_with_light(
            vec![
                LightContainer::new_filled(15),
                LightContainer::new_empty(0),
                LightContainer::new_filled(7),
            ],
            vec![
                LightContainer::new_empty(0),
                LightContainer::new_filled(1),
                LightContainer::new_empty(0),
            ],
        );
        // The mask bit math assumes the light arrays are exactly as long as the block
        // sections; if that invariant breaks, `section_index + 1` is wrong.
        assert_eq!(
            chunk.light_engine.lock().unwrap().sky_light.len(),
            chunk.section.section_count()
        );
        let bytes = serialize(&chunk);

        // Physical section i is bit i+1; bit 0 and bit 4 are the below/above-world padding.
        let sky_mask: u64 = (1 << 1) | (1 << 3);
        let block_mask: u64 = 1 << 2;
        assert_eq!(&bytes[2..3], &[1]);
        assert_eq!(&bytes[3..11], &sky_mask.to_be_bytes());
        assert_eq!(&bytes[12..20], &block_mask.to_be_bytes());
        let empty_sky_mask: u64 = 1 | (1 << 2) | (1 << 4);
        let empty_block_mask: u64 = 1 | (1 << 1) | (1 << 3) | (1 << 4);
        assert_eq!(&bytes[21..29], &empty_sky_mask.to_be_bytes());
        assert_eq!(&bytes[30..38], &empty_block_mask.to_be_bytes());

        assert_eq!(bytes[38], sky_mask.count_ones() as u8);
        assert_eq!(bytes[38], 2);
        let block_count_at = 38 + 1 + 2 * (2 + 2048);
        assert_eq!(bytes[block_count_at], block_mask.count_ones() as u8);
        assert_eq!(bytes.len(), block_count_at + 1 + (2 + 2048));

        // Second sky array is the 7-filled section, at the offset the client would seek to.
        let second_sky = 38 + 1 + (2 + 2048) + 2;
        assert_eq!(bytes[second_sky], 0x77);
    }

    #[test]
    fn masks_include_padding_and_offset_physical_sections() {
        let light = ChunkLight {
            sky_light: [LightContainer::new_empty(15), LightContainer::new_empty(0)].into(),
            block_light: [LightContainer::new_empty(0), LightContainer::new_filled(1)].into(),
        };

        let masks = light_masks(&light);

        assert_eq!(masks.sky, 1 << 1);
        assert_eq!(masks.block, 1 << 2);
        assert_eq!(masks.empty_sky, (1 << 0) | (1 << 2) | (1 << 3));
        assert_eq!(masks.empty_block, (1 << 0) | (1 << 1) | (1 << 3));
    }

    #[test]
    fn uniform_full_sky_layer_is_serialized_as_data() {
        let chunk = chunk_with_light(
            vec![LightContainer::new_empty(15)],
            vec![LightContainer::new_empty(0)],
        );
        let bytes = serialize(&chunk);

        // Coordinates (1, 1) and four one-long bitsets precede the arrays.
        assert_eq!(&bytes[3..11], &(1u64 << 1).to_be_bytes());
        assert_eq!(&bytes[21..29], &(1u64 | (1 << 2)).to_be_bytes());
        let array_start = 1 + 1 + 4 * (1 + 8) + 1 + 2;
        assert_eq!(bytes[array_start..array_start + 2048], [0xFF; 2048]);
    }

    #[test]
    fn incremental_update_contains_only_changed_sections() {
        let chunk = chunk_with_light(
            vec![
                LightContainer::new_filled(1),
                LightContainer::new_empty(15),
                LightContainer::new_filled(3),
            ],
            vec![
                LightContainer::new_filled(4),
                LightContainer::new_empty(0),
                LightContainer::new_filled(5),
            ],
        );
        let bytes = serialize_sections(&chunk, &[1]);

        // Only physical section 1 (wire bit 2) is present. Incremental masks
        // intentionally omit the below/above-world padding sections.
        let changed_bit = (1u64 << 2).to_be_bytes();
        assert_eq!(&bytes[3..11], &changed_bit); // sky data mask
        assert_eq!(&bytes[12..20], &[0; 8]); // block section is empty
        assert_eq!(&bytes[21..29], &[0; 8]); // no empty sky section
        assert_eq!(&bytes[30..38], &changed_bit); // empty block mask

        let sky_count_at = 38;
        assert_eq!(bytes[sky_count_at], 1);
        let block_count_at = sky_count_at + 1 + 2 + 2048;
        assert_eq!(bytes[block_count_at], 0);
    }
}
