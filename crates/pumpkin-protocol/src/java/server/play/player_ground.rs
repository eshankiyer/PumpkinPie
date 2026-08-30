use crate::{
    ServerPacket,
    ser::{NetworkReadExt, ReadingError},
};
use pumpkin_data::packet::serverbound::play::MOVE_PLAYER_STATUS_ONLY;
use pumpkin_macros::java_packet;
use pumpkin_util::version::JavaMinecraftVersion;

use super::player_position_rotation::{FLAG_HORIZONTAL_COLLISION, FLAG_ON_GROUND};

#[java_packet(MOVE_PLAYER_STATUS_ONLY)]
pub struct SSetPlayerGround {
    pub on_ground: bool,
    pub horizontal_collision: bool,
}

impl<'a> ServerPacket<'a> for SSetPlayerGround {
    fn read(bytebuf: &mut &'a [u8], _version: &JavaMinecraftVersion) -> Result<Self, ReadingError> {
        // `StatusOnly.read` unpacks both movement status bits (`ServerboundMovePlayerPacket.java:225-229`).
        let flags = bytebuf.get_u8()?;
        Ok(Self {
            on_ground: flags & FLAG_ON_GROUND != 0,
            horizontal_collision: flags & FLAG_HORIZONTAL_COLLISION != 0,
        })
    }
}

impl crate::ClientPacket for SSetPlayerGround {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        use crate::ser::NetworkWriteExt;
        // `StatusOnly.write` packs both movement status bits (`ServerboundMovePlayerPacket.java:232-234`).
        let flags = if self.on_ground { FLAG_ON_GROUND } else { 0 }
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
    fn status_only_reads_horizontal_collision_flag() {
        // `StatusOnly.read` decodes the packed movement status byte
        // (`ServerboundMovePlayerPacket.java:225-229`).
        let mut input = &[FLAG_ON_GROUND | FLAG_HORIZONTAL_COLLISION][..];
        let packet = SSetPlayerGround::read(&mut input, &JavaMinecraftVersion::V_26_2)
            .expect("status-only packet should decode");

        assert!(packet.on_ground);
        assert!(packet.horizontal_collision);
    }
}
