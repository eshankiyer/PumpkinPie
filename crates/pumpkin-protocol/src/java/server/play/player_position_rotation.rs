use pumpkin_data::packet::serverbound::play::MOVE_PLAYER_POS_ROT;
use pumpkin_macros::java_packet;
use pumpkin_util::math::vector3::Vector3;

use crate::{
    ServerPacket,
    ser::{NetworkReadExt, ReadingError},
};
use pumpkin_util::version::JavaMinecraftVersion;

pub const FLAG_ON_GROUND: u8 = 0x01;
// `ServerboundMovePlayerPacket` packs these status bits (`ServerboundMovePlayerPacket.java:10-20,35-41`).
pub const FLAG_HORIZONTAL_COLLISION: u8 = 0x02;

#[java_packet(MOVE_PLAYER_POS_ROT)]
pub struct SPlayerPositionRotation {
    pub position: Vector3<f64>,
    pub yaw: f32,
    pub pitch: f32,
    /// bit 0: [`FLAG_ON_GROUND`], bit 1: [`FLAG_HORIZONTAL_COLLISION`]
    pub collision: u8,
}

impl<'a> ServerPacket<'a> for SPlayerPositionRotation {
    fn read(bytebuf: &mut &'a [u8], version: &JavaMinecraftVersion) -> Result<Self, ReadingError> {
        let x = bytebuf.get_f64_be()?;
        let y = bytebuf.get_f64_be()?;
        if *version <= JavaMinecraftVersion::V_1_7_6 {
            let _stance = bytebuf.get_f64_be()?;
        }
        let z = bytebuf.get_f64_be()?;
        let yaw = bytebuf.get_f32_be()?;
        let pitch = bytebuf.get_f32_be()?;
        // `PosRot.read` decodes the packed movement status bits (`ServerboundMovePlayerPacket.java:159-168`).
        let collision = bytebuf.get_u8()?;
        Ok(Self {
            position: Vector3::new(x, y, z),
            yaw,
            pitch,
            collision,
        })
    }
}

impl crate::ClientPacket for SPlayerPositionRotation {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        use crate::ser::NetworkWriteExt;
        write.write_f64_be(self.position.x)?;
        write.write_f64_be(self.position.y)?;
        if *version <= JavaMinecraftVersion::V_1_7_6 {
            write.write_f64_be(self.position.y + 1.62)?;
        }
        write.write_f64_be(self.position.z)?;
        write.write_f32_be(self.yaw)?;
        write.write_f32_be(self.pitch)?;
        // `PosRot.write` writes the packed movement status bits (`ServerboundMovePlayerPacket.java:171-177`).
        write.write_u8(self.collision)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ServerPacket;

    #[test]
    fn position_rotation_reads_horizontal_collision_flag() {
        // `PosRot.read` decodes the status byte after position and rotation
        // (`ServerboundMovePlayerPacket.java:159-168`).
        let mut input = &[
            0u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0, // x
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0, // y
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0, // z
            0,
            0,
            0,
            0, // yRot
            0,
            0,
            0,
            0, // xRot
            FLAG_ON_GROUND | FLAG_HORIZONTAL_COLLISION,
        ][..];
        let packet = SPlayerPositionRotation::read(
            &mut input,
            &pumpkin_util::version::JavaMinecraftVersion::V_26_2,
        )
        .expect("position/rotation packet should decode");

        assert_eq!(
            packet.collision & FLAG_HORIZONTAL_COLLISION,
            FLAG_HORIZONTAL_COLLISION
        );
        assert_eq!(packet.collision & FLAG_ON_GROUND, FLAG_ON_GROUND);
    }
}
