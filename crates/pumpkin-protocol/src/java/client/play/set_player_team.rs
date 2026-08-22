use std::io::Write;

use pumpkin_data::packet::clientbound::play::SET_PLAYER_TEAM;
use pumpkin_macros::java_packet;
use pumpkin_util::{text::TextComponent, translation::Locale, version::JavaMinecraftVersion};

use crate::{
    ClientPacket,
    codec::var_int::VarInt,
    ser::{NetworkWriteExt, WritingError},
};

#[repr(i8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamMethod {
    Create = 0,
    Remove = 1,
    Update = 2,
    AddPlayers = 3,
    RemovePlayers = 4,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeamParameters<'a> {
    pub display_name: &'a TextComponent,
    pub options: i8,
    pub nametag_visibility: &'a str,
    pub collision_rule: &'a str,
    pub color: i32,
    pub player_prefix: &'a TextComponent,
    pub player_suffix: &'a TextComponent,
}

#[java_packet(SET_PLAYER_TEAM)]
pub struct CSetPlayerTeam<'a> {
    pub team_name: String,
    pub method: TeamMethod,
    pub parameters: Option<TeamParameters<'a>>,
    pub players: Box<[String]>,
}

/// `Team.Visibility` ids (`Team.java:66-70`). Unknown names fall back to
/// `ALWAYS`, matching `ByIdMap.OutOfBoundsStrategy.ZERO` on decode.
fn nametag_visibility_to_id(s: &str) -> i32 {
    match s {
        "never" => 1,
        "hideForOtherTeams" => 2,
        "hideForOwnTeam" => 3,
        _ => 0,
    }
}

/// `Team.CollisionRule` ids (`Team.java:39-43`).
fn collision_rule_to_id(s: &str) -> i32 {
    match s {
        "never" => 1,
        "pushOtherTeams" => 2,
        "pushOwnTeam" => 3,
        _ => 0,
    }
}

impl ClientPacket for CSetPlayerTeam<'_> {
    fn write_packet_data(
        &self,
        mut write: impl Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        if *version >= JavaMinecraftVersion::V_1_18 {
            write.write_string(&self.team_name)?;
        } else {
            write.write_string_bounded(&self.team_name, 16)?;
        }

        write.write_i8(self.method as i8)?;

        if self.method == TeamMethod::Create || self.method == TeamMethod::Update {
            let Some(params) = &self.parameters else {
                return Err(WritingError::Message(
                    "Parameters missing for Create/Update".into(),
                ));
            };

            if *version >= JavaMinecraftVersion::V_26_2 {
                write.write_component(params.display_name, version)?;
                write.write_component(params.player_prefix, version)?;
                write.write_component(params.player_suffix, version)?;
                write
                    .write_var_int(&VarInt(nametag_visibility_to_id(params.nametag_visibility)))?;
                write.write_var_int(&VarInt(collision_rule_to_id(params.collision_rule)))?;
                // `TeamColor` ids run 0..=15 (`TeamColor.java:14-29`); anything outside
                // that range has no team colour, i.e. `Optional.empty()`.
                if (0..=15).contains(&params.color) {
                    write.write_bool(true)?;
                    write.write_var_int(&VarInt(params.color))?;
                } else {
                    write.write_bool(false)?;
                }
                write.write_i8(params.options)?;
            } else if *version >= JavaMinecraftVersion::V_1_13 {
                write.write_component(params.display_name, version)?;
                write.write_i8(params.options)?;
                if *version >= JavaMinecraftVersion::V_1_21_5 {
                    write.write_var_int(&VarInt(nametag_visibility_to_id(
                        params.nametag_visibility,
                    )))?;
                    write.write_var_int(&VarInt(collision_rule_to_id(params.collision_rule)))?;
                } else {
                    write.write_string_bounded(params.nametag_visibility, 40)?;
                    write.write_string_bounded(params.collision_rule, 40)?;
                }
                if *version >= JavaMinecraftVersion::V_1_17 {
                    let color_id = if params.color < 0 { 21 } else { params.color };
                    write.write_var_int(&VarInt(color_id))?;
                } else {
                    write.write_i8(params.color as i8)?;
                }
                write.write_component(params.player_prefix, version)?;
                write.write_component(params.player_suffix, version)?;
            } else {
                let display_name_legacy = params
                    .display_name
                    .to_legacy_string_for_version(version, Locale::EnUs);
                let player_prefix_legacy = params
                    .player_prefix
                    .to_legacy_string_for_version(version, Locale::EnUs);
                let player_suffix_legacy = params
                    .player_suffix
                    .to_legacy_string_for_version(version, Locale::EnUs);

                write.write_string_bounded(&display_name_legacy, 32)?;
                write.write_string_bounded(&player_prefix_legacy, 32)?;
                write.write_string_bounded(&player_suffix_legacy, 32)?;
                write.write_i8(params.options)?;

                if *version <= JavaMinecraftVersion::V_1_7_6 {
                    // 1.7.x has no nametag visibility, collision rule, or color in team info
                } else {
                    write.write_string_bounded(params.nametag_visibility, 32)?;
                    if *version >= JavaMinecraftVersion::V_1_9 {
                        write.write_string_bounded(params.collision_rule, 32)?;
                    }
                    write.write_i8(params.color as i8)?;
                }
            }
        }

        if self.method == TeamMethod::Create
            || self.method == TeamMethod::AddPlayers
            || self.method == TeamMethod::RemovePlayers
        {
            if *version <= JavaMinecraftVersion::V_1_7_6 {
                write.write_i16_be(self.players.len() as i16)?;
            } else {
                write.write_var_int(&VarInt(self.players.len() as i32))?;
            }
            for player in &self.players {
                write.write_string_bounded(player, 40)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CSetPlayerTeam, TeamMethod, TeamParameters};
    use crate::ClientPacket;
    use pumpkin_util::text::TextComponent;
    use pumpkin_util::version::JavaMinecraftVersion;

    fn encode(packet: &CSetPlayerTeam<'_>, version: JavaMinecraftVersion) -> Vec<u8> {
        let mut buf = Vec::new();
        packet.write_packet_data(&mut buf, &version).unwrap();
        buf
    }

    /// The 26.2 field order is display/prefix/suffix, visibility id, collision id,
    /// `Optional<TeamColor>`, flags
    /// (`ClientboundSetPlayerTeamPacket.java:151-167`, `Parameters.STREAM_CODEC`).
    #[test]
    fn create_uses_the_26_2_parameter_order() {
        let empty = TextComponent::text("");
        let packet = CSetPlayerTeam {
            team_name: "t".to_string(),
            method: TeamMethod::Create,
            parameters: Some(TeamParameters {
                display_name: &empty,
                options: 3,
                nametag_visibility: "hideForOwnTeam",
                collision_rule: "pushOtherTeams",
                color: 12,
                player_prefix: &empty,
                player_suffix: &empty,
            }),
            players: Box::new([]),
        };
        let bytes = encode(&packet, JavaMinecraftVersion::V_26_2);

        // team name, method, then the three components; assert the tail after them.
        let tail = &bytes[bytes.len() - 6..];
        assert_eq!(tail[0], 3); // Visibility HIDE_FOR_OWN_TEAM
        assert_eq!(tail[1], 2); // CollisionRule PUSH_OTHER_TEAMS
        assert_eq!(tail[2], 1); // Optional<TeamColor> present
        assert_eq!(tail[3], 12); // TeamColor RED
        assert_eq!(tail[4], 3); // option flags
        assert_eq!(tail[5], 0); // empty player list
        assert_eq!(bytes[0], 1); // string length of "t"
        assert_eq!(bytes[2], TeamMethod::Create as u8);
    }

    /// A colour outside `TeamColor`'s 0..=15 id range encodes as `Optional.empty()`.
    #[test]
    fn out_of_range_color_encodes_as_absent() {
        let empty = TextComponent::text("");
        let packet = CSetPlayerTeam {
            team_name: "t".to_string(),
            method: TeamMethod::Update,
            parameters: Some(TeamParameters {
                display_name: &empty,
                options: 0,
                nametag_visibility: "always",
                collision_rule: "always",
                color: 21,
                player_prefix: &empty,
                player_suffix: &empty,
            }),
            players: Box::new([]),
        };
        let bytes = encode(&packet, JavaMinecraftVersion::V_26_2);
        let tail = &bytes[bytes.len() - 4..];
        assert_eq!(tail[0], 0); // Visibility ALWAYS
        assert_eq!(tail[1], 0); // CollisionRule ALWAYS
        assert_eq!(tail[2], 0); // Optional<TeamColor> absent
        assert_eq!(tail[3], 0); // option flags
    }

    /// Pre-1.21.5 clients keep the layout whose flags byte comes second and whose
    /// visibility/collision rules are strings rather than ids.
    #[test]
    fn legacy_layout_still_puts_flags_after_the_display_name() {
        let empty = TextComponent::text("");
        let packet = CSetPlayerTeam {
            team_name: "t".to_string(),
            method: TeamMethod::Update,
            parameters: Some(TeamParameters {
                display_name: &empty,
                options: 3,
                nametag_visibility: "always",
                collision_rule: "always",
                color: 12,
                player_prefix: &empty,
                player_suffix: &empty,
            }),
            players: Box::new([]),
        };
        let bytes = encode(&packet, JavaMinecraftVersion::V_1_21_4);
        assert!(
            bytes.windows(2).any(|w| w == *b"al"),
            "legacy layout writes the rule names as strings"
        );
    }
}
