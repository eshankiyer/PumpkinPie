use pumpkin_data::packet::clientbound::play::STORE_COOKIE;
use pumpkin_macros::java_packet;
use pumpkin_util::resource_location::ResourceLocation;

use crate::ClientPacket;
use crate::ser::NetworkWriteExt;
use pumpkin_util::version::JavaMinecraftVersion;

// `ClientboundStoreCookiePacket.PAYLOAD_STREAM_CODEC` limits payloads to 5120 bytes
// (`ClientboundStoreCookiePacket.java:15-16`).
const MAX_PAYLOAD_SIZE: usize = 5120;

/// Stores some arbitrary data on the client, which persists between server transfers.
/// The Notchian client only accepts cookies of up to 5 kiB in size.
#[java_packet(STORE_COOKIE)]
pub struct CStoreCookie<'a> {
    pub key: &'a ResourceLocation,
    pub payload: &'a [u8], // 5120,
}

impl<'a> CStoreCookie<'a> {
    #[must_use]
    pub const fn new(key: &'a ResourceLocation, payload: &'a [u8]) -> Self {
        Self { key, payload }
    }
}

impl ClientPacket for CStoreCookie<'_> {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        if self.payload.len() > MAX_PAYLOAD_SIZE {
            return Err(crate::ser::WritingError::Message(format!(
                "cookie payload exceeds {MAX_PAYLOAD_SIZE} bytes"
            )));
        }
        write.write_string(self.key)?;
        write.write_var_int(&crate::VarInt(self.payload.len() as i32))?;
        write
            .write_all(self.payload)
            .map_err(|_| crate::ser::WritingError::Message("IO Error".into()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Vanilla accepts the inclusive 5120-byte boundary and rejects larger payloads
    // (`ClientboundStoreCookiePacket.java:15-24`).
    use super::{CStoreCookie, MAX_PAYLOAD_SIZE};
    use crate::{ClientPacket, ser::WritingError};
    use pumpkin_util::version::JavaMinecraftVersion;

    #[test]
    fn cookie_payload_matches_vanilla_limit() {
        let key = "minecraft:test".to_string();
        let accepted = vec![0; MAX_PAYLOAD_SIZE];
        let rejected = vec![0; MAX_PAYLOAD_SIZE + 1];

        assert!(
            CStoreCookie::new(&key, &accepted)
                .write_packet_data(&mut Vec::new(), &JavaMinecraftVersion::V_26_2)
                .is_ok()
        );
        assert!(matches!(
            CStoreCookie::new(&key, &rejected)
                .write_packet_data(&mut Vec::new(), &JavaMinecraftVersion::V_26_2),
            Err(WritingError::Message(_))
        ));
    }
}
