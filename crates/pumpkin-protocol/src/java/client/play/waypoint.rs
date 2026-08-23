use std::io::Write;

use crate::ser::NetworkWriteExt;
use crate::{ClientPacket, VarInt, WritingError};
use pumpkin_data::packet::clientbound::play::WAYPOINT;
use pumpkin_macros::java_packet;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::version::JavaMinecraftVersion;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum WaypointOperation {
    Track = 0,
    Untrack = 1,
    Update = 2,
}

#[derive(Clone, Debug)]
pub enum WaypointTarget {
    Position(BlockPos),
    Chunk { x: i32, z: i32 },
    Azimuth(f32),
    Empty,
}

/// Vanilla `Waypoint.Icon` (`Waypoint.java:34-60`). `style` defaults to
/// `minecraft:default` (`WaypointStyleAssets.java:9`) and is always written;
/// `color` is an `Optional<RGB>`.
#[derive(Clone, Debug)]
pub struct WaypointIcon<'a> {
    pub style: Option<&'a str>,
    pub color: Option<i32>,
}

/// Vanilla `WaypointStyleAssets.DEFAULT` (`WaypointStyleAssets.java:9`).
pub const DEFAULT_WAYPOINT_STYLE: &str = "minecraft:default";

/// Vanilla `TrackedWaypoint` (`TrackedWaypoint.java`). `icon: None` encodes
/// `Waypoint.Icon.NULL`, which the wire format still writes in full.
#[derive(Clone, Debug)]
pub struct TrackedWaypoint<'a> {
    pub identifier: Uuid,
    pub icon: Option<WaypointIcon<'a>>,
    pub target: WaypointTarget,
}

impl TrackedWaypoint<'_> {
    #[must_use]
    pub const fn empty(identifier: Uuid) -> Self {
        Self {
            identifier,
            icon: None,
            target: WaypointTarget::Empty,
        }
    }

    #[must_use]
    pub const fn set_position(
        identifier: Uuid,
        icon: Option<WaypointIcon<'_>>,
        position: BlockPos,
    ) -> TrackedWaypoint<'_> {
        TrackedWaypoint {
            identifier,
            icon,
            target: WaypointTarget::Position(position),
        }
    }

    /// Vanilla `TrackedWaypoint` constructed for
    /// `ClientboundTrackedWaypointPacket.addWaypointChunk` /
    /// `updateWaypointChunk` (`ClientboundTrackedWaypointPacket.java:42-48`).
    #[must_use]
    pub const fn set_chunk(
        identifier: Uuid,
        icon: Option<WaypointIcon<'_>>,
        x: i32,
        z: i32,
    ) -> TrackedWaypoint<'_> {
        TrackedWaypoint {
            identifier,
            icon,
            target: WaypointTarget::Chunk { x, z },
        }
    }

    /// Vanilla `TrackedWaypoint` constructed for
    /// `ClientboundTrackedWaypointPacket.addWaypointAzimuth` /
    /// `updateWaypointAzimuth` (`ClientboundTrackedWaypointPacket.java:50-56`).
    #[must_use]
    pub const fn set_azimuth(
        identifier: Uuid,
        icon: Option<WaypointIcon<'_>>,
        angle: f32,
    ) -> TrackedWaypoint<'_> {
        TrackedWaypoint {
            identifier,
            icon,
            target: WaypointTarget::Azimuth(angle),
        }
    }
}

/// Syncs tracked waypoints (`ClientboundTrackedWaypointPacket`) to client.
#[java_packet(WAYPOINT)]
pub struct CWaypoint<'a> {
    pub operation: WaypointOperation,
    pub waypoint: TrackedWaypoint<'a>,
}

impl<'a> CWaypoint<'a> {
    #[must_use]
    pub const fn new(operation: WaypointOperation, waypoint: TrackedWaypoint<'a>) -> Self {
        Self {
            operation,
            waypoint,
        }
    }

    #[must_use]
    pub const fn remove(identifier: Uuid) -> Self {
        Self {
            operation: WaypointOperation::Untrack,
            waypoint: TrackedWaypoint::empty(identifier),
        }
    }

    #[must_use]
    pub const fn add_position(
        identifier: Uuid,
        icon: Option<WaypointIcon<'a>>,
        position: BlockPos,
    ) -> Self {
        Self {
            operation: WaypointOperation::Track,
            waypoint: TrackedWaypoint::set_position(identifier, icon, position),
        }
    }

    #[must_use]
    pub const fn update_position(
        identifier: Uuid,
        icon: Option<WaypointIcon<'a>>,
        position: BlockPos,
    ) -> Self {
        Self {
            operation: WaypointOperation::Update,
            waypoint: TrackedWaypoint::set_position(identifier, icon, position),
        }
    }

    /// Vanilla `ClientboundTrackedWaypointPacket.addWaypointChunk`
    /// (`ClientboundTrackedWaypointPacket.java:42-44`).
    #[must_use]
    pub const fn add_chunk(
        identifier: Uuid,
        icon: Option<WaypointIcon<'a>>,
        x: i32,
        z: i32,
    ) -> Self {
        Self {
            operation: WaypointOperation::Track,
            waypoint: TrackedWaypoint::set_chunk(identifier, icon, x, z),
        }
    }

    /// Vanilla `ClientboundTrackedWaypointPacket.updateWaypointChunk`
    /// (`ClientboundTrackedWaypointPacket.java:46-48`).
    #[must_use]
    pub const fn update_chunk(
        identifier: Uuid,
        icon: Option<WaypointIcon<'a>>,
        x: i32,
        z: i32,
    ) -> Self {
        Self {
            operation: WaypointOperation::Update,
            waypoint: TrackedWaypoint::set_chunk(identifier, icon, x, z),
        }
    }

    /// Vanilla `ClientboundTrackedWaypointPacket.addWaypointAzimuth`
    /// (`ClientboundTrackedWaypointPacket.java:50-52`).
    #[must_use]
    pub const fn add_azimuth(identifier: Uuid, icon: Option<WaypointIcon<'a>>, angle: f32) -> Self {
        Self {
            operation: WaypointOperation::Track,
            waypoint: TrackedWaypoint::set_azimuth(identifier, icon, angle),
        }
    }

    /// Vanilla `ClientboundTrackedWaypointPacket.updateWaypointAzimuth`
    /// (`ClientboundTrackedWaypointPacket.java:54-56`).
    #[must_use]
    pub const fn update_azimuth(
        identifier: Uuid,
        icon: Option<WaypointIcon<'a>>,
        angle: f32,
    ) -> Self {
        Self {
            operation: WaypointOperation::Update,
            waypoint: TrackedWaypoint::set_azimuth(identifier, icon, angle),
        }
    }
}

impl ClientPacket for CWaypoint<'_> {
    /// Vanilla `ClientboundTrackedWaypointPacket.STREAM_CODEC`
    /// (`ClientboundTrackedWaypointPacket.java:22-28`) then
    /// `TrackedWaypoint.write` (`TrackedWaypoint.java:38-44`).
    fn write_packet_data(
        &self,
        mut write: impl Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        // Operation ordinal (TRACK = 0, UNTRACK = 1, UPDATE = 2).
        write.write_var_int(&VarInt(self.operation as i32))?;

        // `byteBuf.writeEither(identifier, UUIDUtil.STREAM_CODEC, writeUtf)`:
        // `writeEither` prefixes a boolean, `true` for the left (UUID) side
        // (`FriendlyByteBuf.java:237-247`). The server only ever sends UUIDs.
        write.write_bool(true)?;
        write.write_uuid(&self.waypoint.identifier)?;

        // `Waypoint.Icon.STREAM_CODEC`: a non-optional style ResourceKey followed
        // by an `Optional<RGB_COLOR>` (`Waypoint.java:42-49`).
        let (style, color) = self
            .waypoint
            .icon
            .as_ref()
            .map_or((DEFAULT_WAYPOINT_STYLE, None), |icon| {
                (icon.style.unwrap_or(DEFAULT_WAYPOINT_STYLE), icon.color)
            });
        if style.contains(':') {
            write.write_string(style)?;
        } else {
            write.write_string(&format!("minecraft:{style}"))?;
        }
        if let Some(color) = color {
            write.write_bool(true)?;
            // `ByteBufCodecs.RGB_COLOR` is three raw bytes, not an int
            // (`ByteBufCodecs.java:239-249`).
            write.write_u8(((color >> 16) & 0xFF) as u8)?;
            write.write_u8(((color >> 8) & 0xFF) as u8)?;
            write.write_u8((color & 0xFF) as u8)?;
        } else {
            write.write_bool(false)?;
        }

        // `TrackedWaypoint.Type` ordinal: EMPTY, VEC3I, CHUNK, AZIMUTH
        // (`TrackedWaypoint.java:246-250`), then `writeContents`.
        match &self.waypoint.target {
            WaypointTarget::Empty => {
                write.write_var_int(&VarInt(0))?;
            }
            WaypointTarget::Position(pos) => {
                write.write_var_int(&VarInt(1))?;
                // `Vec3iWaypoint.writeContents` writes three VarInts, not a
                // packed BlockPos long.
                write.write_var_int(&VarInt(pos.0.x))?;
                write.write_var_int(&VarInt(pos.0.y))?;
                write.write_var_int(&VarInt(pos.0.z))?;
            }
            WaypointTarget::Chunk { x, z } => {
                write.write_var_int(&VarInt(2))?;
                // `ChunkWaypoint.writeContents`: two VarInts.
                write.write_var_int(&VarInt(*x))?;
                write.write_var_int(&VarInt(*z))?;
            }
            WaypointTarget::Azimuth(angle) => {
                write.write_var_int(&VarInt(3))?;
                write.write_f32_be(*angle)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CWaypoint, TrackedWaypoint, WaypointIcon, WaypointOperation, WaypointTarget};
    use crate::ClientPacket;
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_util::version::JavaMinecraftVersion;
    use uuid::Uuid;

    fn encode(packet: &CWaypoint<'_>) -> Vec<u8> {
        let mut buf = Vec::new();
        packet
            .write_packet_data(&mut buf, &JavaMinecraftVersion::V_26_2)
            .unwrap();
        buf
    }

    const ID: Uuid = Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);

    /// `TrackedWaypoint.write` prefixes the identifier with the `writeEither`
    /// boolean and always writes a full `Waypoint.Icon`.
    #[test]
    fn untrack_writes_either_flag_and_the_null_icon() {
        let bytes = encode(&CWaypoint::remove(ID));
        let mut expected = vec![1u8]; // operation UNTRACK
        expected.push(1); // writeEither -> left
        expected.extend_from_slice(ID.as_bytes());
        expected.push(17); // string length of "minecraft:default"
        expected.extend_from_slice(b"minecraft:default");
        expected.push(0); // Optional<RGB> absent
        expected.push(0); // Type EMPTY
        assert_eq!(bytes, expected);
    }

    /// `Vec3iWaypoint.writeContents` writes three `VarInt`s, and
    /// `ByteBufCodecs.RGB_COLOR` writes three raw bytes.
    #[test]
    fn position_writes_three_var_ints_and_a_three_byte_color() {
        let packet = CWaypoint::add_position(
            ID,
            Some(WaypointIcon {
                style: Some("bowtie"),
                color: Some(0x00FF_8000),
            }),
            BlockPos::new(1, 2, 3),
        );
        let bytes = encode(&packet);
        let mut expected = vec![0u8]; // operation TRACK
        expected.push(1);
        expected.extend_from_slice(ID.as_bytes());
        expected.push(16);
        expected.extend_from_slice(b"minecraft:bowtie");
        expected.push(1); // Optional<RGB> present
        expected.extend_from_slice(&[0xFF, 0x80, 0x00]);
        expected.push(1); // Type VEC3I
        expected.extend_from_slice(&[1, 2, 3]);
        assert_eq!(bytes, expected);
    }

    /// `ChunkWaypoint.writeContents` writes two `VarInt`s, and CHUNK is ordinal 2.
    #[test]
    fn chunk_writes_two_var_ints_under_ordinal_two() {
        let packet = CWaypoint::new(
            WaypointOperation::Update,
            TrackedWaypoint {
                identifier: ID,
                icon: None,
                target: WaypointTarget::Chunk { x: 5, z: -1 },
            },
        );
        let bytes = encode(&packet);
        let tail = &bytes[bytes.len() - 7..];
        assert_eq!(tail[0], 2); // Type CHUNK
        assert_eq!(tail[1], 5); // VarInt 5
        assert_eq!(&tail[2..7], &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]); // VarInt -1
        assert_eq!(bytes[0], 2); // operation UPDATE
    }

    /// AZIMUTH is ordinal 3 and its payload is a big-endian float.
    #[test]
    fn azimuth_writes_a_big_endian_float_under_ordinal_three() {
        let packet = CWaypoint::new(
            WaypointOperation::Track,
            TrackedWaypoint {
                identifier: ID,
                icon: None,
                target: WaypointTarget::Azimuth(1.5),
            },
        );
        let bytes = encode(&packet);
        let tail = &bytes[bytes.len() - 5..];
        assert_eq!(tail[0], 3);
        assert_eq!(&tail[1..], &1.5f32.to_be_bytes());
    }
}
