use pumpkin_data::packet::clientbound::play::CHUNKS_BIOMES;
use pumpkin_macros::java_packet;

use crate::{ClientPacket, codec::var_int::VarInt, ser::NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

pub struct ChunkBiomeEntry<'a> {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub data: &'a [u8],
}

impl ChunkBiomeEntry<'_> {
    /// `ChunkPos.pack` (`ChunkPos.java:73-75`): x in the low 32 bits, z in the high 32.
    #[must_use]
    pub const fn packed_pos(&self) -> i64 {
        (self.chunk_x as i64 & 0xFFFF_FFFF) | ((self.chunk_z as i64 & 0xFFFF_FFFF) << 32)
    }
}

#[java_packet(CHUNKS_BIOMES)]
pub struct CChunksBiomes<'a> {
    pub chunks: &'a [ChunkBiomeEntry<'a>],
}

impl<'a> CChunksBiomes<'a> {
    #[must_use]
    pub const fn new(chunks: &'a [ChunkBiomeEntry<'a>]) -> Self {
        Self { chunks }
    }
}

impl ClientPacket for CChunksBiomes<'_> {
    /// `ClientboundChunksBiomesPacket.write` (`ClientboundChunksBiomesPacket.java:28-30`)
    /// then `ChunkBiomeData.write` (`:81-84`): a collection of entries, each a packed
    /// `ChunkPos` long followed by a VarInt-length-prefixed byte array.
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        write.write_var_int(&VarInt(self.chunks.len() as i32))?;
        for chunk in self.chunks {
            write.write_i64_be(chunk.packed_pos())?;
            write.write_var_int(&VarInt(chunk.data.len() as i32))?;
            write.write_slice(chunk.data)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CChunksBiomes, ChunkBiomeEntry};
    use crate::ClientPacket;
    use pumpkin_util::version::JavaMinecraftVersion;

    /// Byte-exact against `writeChunkPos` (`FriendlyByteBuf.java:406-409`, a big-endian
    /// `ChunkPos.pack`) plus `writeByteArray` (`FriendlyByteBuf.java:289-291`).
    #[test]
    fn entry_matches_the_vanilla_chunk_pos_and_byte_array_layout() {
        let data = [0xAAu8, 0xBB, 0xCC];
        let entries = [ChunkBiomeEntry {
            chunk_x: 1,
            chunk_z: -2,
            data: &data,
        }];
        let mut bytes = Vec::new();
        CChunksBiomes::new(&entries)
            .write_packet_data(&mut bytes, &JavaMinecraftVersion::V_26_2)
            .unwrap();

        assert_eq!(
            bytes,
            vec![
                1, // collection length
                // ChunkPos.pack(1, -2) = 0xFFFFFFFE_00000001, big-endian: z first.
                0xFF, 0xFF, 0xFF, 0xFE, 0x00, 0x00, 0x00, 0x01, //
                3,    // byte array length
                0xAA, 0xBB, 0xCC,
            ]
        );
    }
}
