use pumpkin_data::packet::clientbound::play::PLAYER_ROTATION;
use pumpkin_macros::java_packet;

use crate::{ClientPacket, ser::NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

#[java_packet(PLAYER_ROTATION)]
pub struct CPlayerRotation {
    pub yaw: f32,
    pub pitch: f32,
}

impl CPlayerRotation {
    #[must_use]
    pub const fn new(yaw: f32, pitch: f32) -> Self {
        Self { yaw, pitch }
    }
}

impl ClientPacket for CPlayerRotation {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        // ClientboundPlayerRotationPacket.java:9-20 writes a relative flag after each rotation.
        write.write_f32_be(self.yaw)?;
        write.write_bool(false)?;
        write.write_f32_be(self.pitch)?;
        write.write_bool(false)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_vanilla_rotation_flags() {
        // ClientboundPlayerRotationPacket.java:9-20 orders yaw, relativeY, xRot, relativeX.
        let packet = CPlayerRotation::new(1.0, -2.0);
        let mut bytes = Vec::new();
        packet
            .write_packet_data(&mut bytes, &JavaMinecraftVersion::V_26_2)
            .expect("rotation packet should encode");

        assert_eq!(
            bytes,
            [0x3f, 0x80, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x00]
        );
    }
}
