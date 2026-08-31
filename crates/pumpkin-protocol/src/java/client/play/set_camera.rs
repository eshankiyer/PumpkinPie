use pumpkin_data::packet::clientbound::play::SET_CAMERA;
use pumpkin_macros::java_packet;

use crate::{ClientPacket, codec::var_int::VarInt, ser::NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

#[java_packet(SET_CAMERA)]
pub struct CSetCamera {
    pub camera_id: VarInt,
}

impl CSetCamera {
    #[must_use]
    pub const fn new(camera_id: VarInt) -> Self {
        Self { camera_id }
    }
}

impl ClientPacket for CSetCamera {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        write.write_var_int(&self.camera_id)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::MultiVersionJavaPacket;

    #[test]
    fn set_camera_uses_vanilla_26_2_packet_type_id() {
        // ClientboundSetCameraPacket.java:29-31 returns the packet type represented here by the generated ID table.
        assert_eq!(CSetCamera::to_id(JavaMinecraftVersion::V_26_2), 93);
    }
}
