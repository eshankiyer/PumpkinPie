use crate::{ClientPacket, VarInt, WritingError, ser::NetworkWriteExt};
use pumpkin_data::packet::clientbound::play::MAP_ITEM_DATA;
use pumpkin_macros::java_packet;
use pumpkin_util::text::TextComponent;
use std::io::Write;

#[java_packet(MAP_ITEM_DATA)]
pub struct CMapItemData<'a> {
    pub map_id: VarInt,
    pub scale: i8,
    pub locked: bool,
    pub icons: Option<&'a [MapIcon]>,
    pub data: Option<MapPatch<'a>>,
}

pub struct MapIcon {
    pub icon_type: VarInt,
    pub x: i8,
    pub z: i8,
    pub direction: i8,
    pub display_name: Option<String>,
}

pub struct MapPatch<'a> {
    pub columns: u8,
    pub rows: u8,
    pub x: i8,
    pub z: i8,
    pub data: &'a [u8],
}

impl ClientPacket for CMapItemData<'_> {
    fn write_packet_data(
        &self,
        mut write: impl Write,
        version: &pumpkin_util::version::JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        write.write_var_int(&self.map_id)?;
        write.write_i8(self.scale)?;
        write.write_bool(self.locked)?;

        if let Some(icons) = self.icons {
            write.write_bool(true)?;
            write.write_var_int(&VarInt(icons.len() as i32))?;
            for icon in icons {
                write.write_var_int(&icon.icon_type)?;
                write.write_i8(icon.x)?;
                write.write_i8(icon.z)?;
                write.write_i8(icon.direction)?;
                if let Some(name) = &icon.display_name {
                    write.write_bool(true)?;
                    write.write_component(&TextComponent::text(name.clone()), version)?;
                } else {
                    write.write_bool(false)?;
                }
            }
        } else {
            write.write_bool(false)?;
        }

        if let Some(patch) = &self.data {
            write.write_u8(patch.columns)?;
            if patch.columns > 0 {
                write.write_u8(patch.rows)?;
                write.write_i8(patch.x)?;
                write.write_i8(patch.z)?;
                write.write_var_int(&VarInt(patch.data.len() as i32))?;
                write.write_all(patch.data).map_err(WritingError::IoError)?;
            }
        } else {
            write.write_u8(0)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CMapItemData, MapIcon, MapPatch};
    use crate::{ClientPacket, VarInt};
    use pumpkin_util::text::TextComponent;
    use pumpkin_util::version::JavaMinecraftVersion;

    fn serialize(packet: &CMapItemData<'_>) -> Vec<u8> {
        let mut out = Vec::new();
        packet
            .write_packet_data(&mut out, &JavaMinecraftVersion::V_26_2)
            .unwrap();
        out
    }

    /// `ClientboundMapItemDataPacket.STREAM_CODEC` is
    /// `MapId.STREAM_CODEC` (a `VarInt`), `ByteBufCodecs.BYTE`, `ByteBufCodecs.BOOL`, then the
    /// optional decoration list and `MapPatch.STREAM_CODEC`. Absent optionals are a single
    /// `false` byte, and an absent patch is a single zero width byte
    /// (`MapItemSavedData.MapPatch::write`).
    #[test]
    fn empty_packet_is_the_five_byte_minimum() {
        let bytes = serialize(&CMapItemData {
            map_id: VarInt(7),
            scale: 3,
            locked: true,
            icons: None,
            data: None,
        });
        assert_eq!(bytes, vec![7, 3, 1, 0, 0]);
    }

    /// `MapPatch::write` writes width, height, startX, startY, then a `VarInt`-prefixed byte
    /// array - in that order, all single bytes.
    #[test]
    fn patch_framing_is_width_height_x_z_then_prefixed_array() {
        let colors = [1u8, 2, 3, 4, 5, 6];
        let bytes = serialize(&CMapItemData {
            map_id: VarInt(0),
            scale: 0,
            locked: false,
            icons: Some(&[]),
            data: Some(MapPatch {
                columns: 3,
                rows: 2,
                x: 9,
                z: 11,
                data: &colors,
            }),
        });
        // map_id, scale, locked, icons present, icon count 0
        assert_eq!(&bytes[..5], &[0, 0, 0, 1, 0]);
        // width, height, startX, startY, array length, array
        assert_eq!(&bytes[5..10], &[3, 2, 9, 11, 6]);
        assert_eq!(&bytes[10..], &colors);
        assert_eq!(bytes.len(), 16);
    }

    /// `MapDecoration.STREAM_CODEC` is holder id (a plain `VarInt` registry id, since
    /// `ByteBufCodecs.holderRegistry` delegates to `registry(..)` which writes the raw id),
    /// x, y, rot as bytes, then `ComponentSerialization.OPTIONAL_STREAM_CODEC`.
    #[test]
    fn icon_fields_are_id_x_z_rot_then_optional_name() {
        let bytes = serialize(&CMapItemData {
            map_id: VarInt(0),
            scale: 0,
            locked: false,
            icons: Some(&[MapIcon {
                icon_type: VarInt(4),
                x: -2,
                z: 100,
                direction: 15,
                display_name: None,
            }]),
            data: None,
        });
        assert_eq!(bytes, vec![0, 0, 0, 1, 1, 4, 0xFE, 100, 15, 0, 0]);
    }

    /// Since 1.20.3 a `Component` on the wire is NBT, not a JSON string. Regressing to a
    /// length-prefixed JSON string makes the client read the length byte as an NBT tag id
    /// and drop the connection, so pin both the encoding and its leading tag byte.
    #[test]
    fn icon_display_name_is_nbt_not_a_json_string() {
        let bytes = serialize(&CMapItemData {
            map_id: VarInt(0),
            scale: 0,
            locked: false,
            icons: Some(&[MapIcon {
                icon_type: VarInt(0),
                x: 0,
                z: 0,
                direction: 0,
                display_name: Some("hi".to_string()),
            }]),
            data: None,
        });
        let name_start = 4 + 1 + 1 + 1 + 1 + 1 + 1;
        assert_eq!(&bytes[..name_start], &[0, 0, 0, 1, 1, 0, 0, 0, 0, 1]);

        let expected = TextComponent::text("hi").encode_for_version(&JavaMinecraftVersion::V_26_2);
        assert_eq!(&bytes[name_start..bytes.len() - 1], &*expected);
        // A bare NBT string is tag id 0x08; a JSON string would start with its length (2).
        assert_eq!(bytes[name_start], 0x08);
        // Trailing zero is the absent color patch.
        assert_eq!(bytes[bytes.len() - 1], 0);
    }
}
