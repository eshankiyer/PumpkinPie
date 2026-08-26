use pumpkin_data::packet::clientbound::play::PROJECTILE_POWER;
use pumpkin_macros::java_packet;

use crate::{ClientPacket, codec::var_int::VarInt, ser::NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

/// Port of vanilla `ClientboundProjectilePowerPacket`: a single
/// `accelerationPower` double, not per-axis components.
///
/// (`ClientboundProjectilePowerPacket.java:9-46`). Vanilla sends this bundled
/// with `ClientboundSetEntityMotionPacket` whenever an
/// `AbstractHurtingProjectile`'s velocity is resynced (`ServerEntity.sendChanges`,
/// `ServerEntity.java:176-190`).
#[java_packet(PROJECTILE_POWER)]
pub struct CProjectilePower {
    pub entity_id: VarInt,
    pub acceleration_power: f64,
}

impl CProjectilePower {
    #[must_use]
    pub const fn new(entity_id: VarInt, acceleration_power: f64) -> Self {
        Self {
            entity_id,
            acceleration_power,
        }
    }
}

impl ClientPacket for CProjectilePower {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        write.write_var_int(&self.entity_id)?;
        write.write_f64_be(self.acceleration_power)?;
        Ok(())
    }
}
