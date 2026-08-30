use crate::{
    ServerPacket,
    ser::{NetworkReadExt, ReadingError},
};
use pumpkin_data::packet::serverbound::play::MOVE_PLAYER_ROT;
use pumpkin_macros::java_packet;
use pumpkin_util::version::JavaMinecraftVersion;

use super::player_position_rotation::{FLAG_HORIZONTAL_COLLISION, FLAG_ON_GROUND};

#[java_packet(MOVE_PLAYER_ROT)]
pub struct SPlayerRotation {
    pub yaw: f32,
    pub pitch: f32,
    pub ground: bool,
    pub horizontal_collision: bool,
}

impl<'a> ServerPacket<'a> for SPlayerRotation {
    fn read(bytebuf: &mut &'a [u8], _version: &JavaMinecraftVersion) -> Result<Self, ReadingError> {
        let yaw = bytebuf.get_f32_be()?;
        let pitch = bytebuf.get_f32_be()?;
        // `Rot.read` unpacks both movement status bits (`ServerboundMovePlayerPacket.java:195-201`).
        let flags = bytebuf.get_u8()?;
        Ok(Self {
            yaw,
            pitch,
            ground: flags & FLAG_ON_GROUND != 0,
            horizontal_collision: flags & FLAG_HORIZONTAL_COLLISION != 0,
        })
    }
}

impl crate::ClientPacket for SPlayerRotation {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        use crate::ser::NetworkWriteExt;
        write.write_f32_be(self.yaw)?;
        write.write_f32_be(self.pitch)?;
        // `Rot.write` packs both movement status bits (`ServerboundMovePlayerPacket.java:204-208`).
        let flags = if self.ground { FLAG_ON_GROUND } else { 0 }
            | if self.horizontal_collision {
                FLAG_HORIZONTAL_COLLISION
            } else {
                0
            };
        write.write_u8(flags)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ServerPacket;

    #[test]
    fn rotation_reads_horizontal_collision_flag() {
        // `Rot.read` decodes the packed movement status byte
        // (`ServerboundMovePlayerPacket.java:195-201`).
        let mut input = &[0, 0, 0, 0, 0, 0, 0, 0, FLAG_HORIZONTAL_COLLISION][..];
        let packet = SPlayerRotation::read(&mut input, &JavaMinecraftVersion::V_26_2)
            .expect("rotation packet should decode");

        assert!(!packet.ground);
        assert!(packet.horizontal_collision);
    }
}
