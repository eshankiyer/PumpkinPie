use std::io::Write;

use pumpkin_data::packet::clientbound::play::RECIPE_BOOK_REMOVE;
use pumpkin_macros::java_packet;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::{ClientPacket, VarInt, WritingError, ser::NetworkWriteExt};

/// `ClientboundRecipeBookRemovePacket`
/// (`ClientboundRecipeBookRemovePacket.java:11-14`): a single field, a list of
/// `RecipeDisplayId`.
///
/// Wire format, from `RecipeDisplayId.STREAM_CODEC.apply(ByteBufCodecs.list())`:
/// `VarInt` element count, then one `VarInt` per id
/// (`RecipeDisplayId.java:8-10` - the record is a bare `int` behind
/// `ByteBufCodecs.VAR_INT`).
#[java_packet(RECIPE_BOOK_REMOVE)]
pub struct CRecipeBookRemove<'a> {
    pub recipes: &'a [i32],
}

impl<'a> CRecipeBookRemove<'a> {
    #[must_use]
    pub const fn new(recipes: &'a [i32]) -> Self {
        Self { recipes }
    }
}

impl ClientPacket for CRecipeBookRemove<'_> {
    fn write_packet_data(
        &self,
        write: impl Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let mut write = write;
        let count = i32::try_from(self.recipes.len())
            .map_err(|_| WritingError::Message("too many recipe display ids".into()))?;
        write.write_var_int(&VarInt(count))?;
        for id in self.recipes {
            write.write_var_int(&VarInt(*id))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(recipes: &[i32]) -> Vec<u8> {
        let mut buf = Vec::new();
        CRecipeBookRemove::new(recipes)
            .write_packet_data(&mut buf, &JavaMinecraftVersion::V_26_2)
            .expect("write");
        buf
    }

    #[test]
    fn empty_list_is_a_single_zero() {
        assert_eq!(encode(&[]), vec![0x00]);
    }

    #[test]
    fn ids_are_var_ints_after_a_var_int_count() {
        // count 3, then 0, 1, 300 (0xAC 0x02 as a VarInt).
        assert_eq!(encode(&[0, 1, 300]), vec![0x03, 0x00, 0x01, 0xAC, 0x02]);
    }
}
