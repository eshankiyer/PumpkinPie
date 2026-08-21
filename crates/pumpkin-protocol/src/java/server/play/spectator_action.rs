use crate::{
    ServerPacket,
    ser::{NetworkReadExt, ReadingError},
};
use pumpkin_data::packet::serverbound::play::SPECTATOR_ACTION;
use pumpkin_macros::java_packet;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::VarInt;

/// 26.2's `ServerboundSpectatorActionPacket`, which attaches a spectator's camera to an entity.
///
/// Distinct from [`super::SSpectateEntity`], the older UUID-keyed packet that teleports a
/// spectator to a player: that one is `SPECTATE_ENTITY`, whose 26.2 id is -1.
#[java_packet(SPECTATOR_ACTION)]
pub struct SSpectatorAction {
    pub entity_id: Option<VarInt>,
}

impl<'a> ServerPacket<'a> for SSpectatorAction {
    fn read(bytebuf: &mut &'a [u8], _version: &JavaMinecraftVersion) -> Result<Self, ReadingError> {
        Ok(Self {
            entity_id: bytebuf.get_option(NetworkReadExt::get_var_int)?,
        })
    }
}
