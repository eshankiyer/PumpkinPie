use pumpkin_data::packet::clientbound::play::START_CONFIGURATION;
use pumpkin_macros::java_packet;

use crate::ClientPacket;
use pumpkin_util::version::JavaMinecraftVersion;

#[java_packet(START_CONFIGURATION)]
pub struct CStartConfiguration;

impl CStartConfiguration {
    /// Marks this packet as terminal for the current protocol phase.
    ///
    /// Mirrors `ClientboundStartConfigurationPacket.isTerminal`
    /// (`ClientboundStartConfigurationPacket.java:24-27`).
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        true
    }
}

impl ClientPacket for CStartConfiguration {
    fn write_packet_data(
        &self,
        _write: impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        Ok(())
    }
}
