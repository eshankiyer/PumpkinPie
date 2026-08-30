use pumpkin_protocol::bedrock::server::text::SText;
use pumpkin_util::{Hand, PermissionLvl};
use rsa::pkcs1v15::{Signature as RsaPkcs1v15Signature, VerifyingKey};
use rsa::signature::Verifier;
use sha1::Sha1;
use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::{Level, debug, error, info, trace, warn};

use crate::block::BlockHitResult;
use crate::block::registry::BlockActionResult;
use crate::block::{self};
use crate::entity::EntityBase;
use crate::entity::player::statistics::StatisticCategory;
use crate::entity::player::{ChatMode, ChatSession, Player};
use crate::error::PumpkinError;
use crate::log_at_level;
use crate::net::PlayerConfig;
use crate::net::java::JavaClient;
use crate::plugin::player::changed_main_hand::PlayerChangedMainHandEvent;
use crate::plugin::player::fish::{PlayerFishEvent, PlayerFishState};
use crate::plugin::player::item_held::PlayerItemHeldEvent;
use crate::plugin::player::player_chat::PlayerChatEvent;
use crate::plugin::player::player_command_send::PlayerCommandSendEvent;
use crate::plugin::player::player_interact_entity_event::PlayerInteractEntityEvent;
use crate::plugin::player::player_interact_event::{InteractAction, PlayerInteractEvent};
use crate::plugin::player::player_interact_unknown_entity_event::PlayerInteractUnknownEntityEvent;
use crate::plugin::player::player_move::PlayerMoveEvent;
use crate::plugin::player::player_toggle_flight_event::PlayerToggleFlightEvent;
use crate::plugin::player::player_toggle_sneak_event::PlayerToggleSneakEvent;

use crate::block::entities::command_block::CommandBlockEntity;
use crate::block::entities::jigsaw_block::JigsawBlockEntity;
use crate::plugin::player::player_toggle_sprint_event::PlayerToggleSprintEvent;
use crate::server::{Server, seasonal_events};
use crate::world::{BlockBreakingProgress, World, chunker};
use pumpkin_data::block_properties::{BlockProperties, CommandBlockLikeProperties};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{
    BlocksAttacksImpl, ConsumableImpl, DataComponentImpl, EquipmentSlot, EquippableImpl, FoodImpl,
    WritableBookContentImpl, WrittenBookContentImpl,
};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Advancement, Block, BlockDirection, translation};
use pumpkin_inventory::InventoryError;
use pumpkin_inventory::merchant::merchant_screen_handler::MerchantScreenHandler;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{InventoryPlayer, ScreenHandler};
use pumpkin_protocol::bedrock::client::CMovePlayer;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::codec::var_ulong::VarULong;
use pumpkin_protocol::java::client::play::{
    CBlockUpdate, CCommandSuggestions, CEntityPositionSync, CHeadRot, COpenSignEditor,
    CPingResponse, CPlayerInfoUpdate, CPlayerPosition, CSetCamera, CSetSelectedSlot,
    CSystemChatMessage, CUpdateEntityPos, CUpdateEntityPosRot, CUpdateEntityRot, InitChat,
    PlayerAction, PlayerInfoFlags,
};
use pumpkin_protocol::java::server::play::{
    Action, ActionType, CommandBlockMode, FLAG_HORIZONTAL_COLLISION, FLAG_ON_GROUND, SAttack,
    SBundleItemSelected, SChangeGameMode, SChatCommand, SChatMessage, SChunkBatch, SClientCommand,
    SClientInformationPlay, SCloseContainer, SCommandSuggestion, SConfirmTeleport,
    SCookieResponse as SPCookieResponse, SEditBook, SInteract, SJigsawGenerate, SKeepAlive,
    SMoveVehicle, SPaddleBoat, SPickItemFromBlock, SPickItemFromEntity, SPlaceRecipe,
    SPlayPingRequest, SPlayerAbilities, SPlayerAction, SPlayerCommand, SPlayerInput,
    SPlayerPosition, SPlayerPositionRotation, SPlayerRotation, SPlayerSession,
    SRecipeBookChangeSettings, SRecipeBookSeenRecipe, SSeenAdvancement, SSelectTrade,
    SSetCommandBlock, SSetCreativeSlot, SSetHeldItem, SSetJigsawBlock, SSetPlayerGround,
    SSetTestBlock, SSwingArm, STeleportToEntity, STestInstanceBlockAction, SUpdateSign, SUseItem,
    SUseItemOn, Status,
};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{polynomial_rolling_hash, position::BlockPos, wrap_degrees};
use pumpkin_util::{GameMode, text::TextComponent};
use pumpkin_world::generation::structure::structures::jigsaw::JigsawJointType;
use pumpkin_world::world::BlockFlags;
use tokio::sync::Mutex;

/// In secure chat mode, Player will be kicked if they send a chat message with a timestamp that is older than this (in ms)
/// Vanilla: 2 minutes
const CHAT_MESSAGE_MAX_AGE: i64 = 1000 * 60 * 2;

/// `PvP` controls attacks against other players only; mobs remain attackable.
const fn pvp_allows_attack(pvp_enabled: bool, target_is_player: bool) -> bool {
    pvp_enabled || !target_is_player
}

const fn uses_main_hand(hand: Hand) -> bool {
    matches!(hand, Hand::Right)
}

// `ServerGamePacketListenerImpl::handleAttack` calls
// `Player::isWithinAttackRange(..., 3.0)`. The default attack-range component
// reaches 3 blocks in Survival and 5 in Creative, with a 0.3 hitbox margin.
const ATTACK_PACKET_RANGE_BUFFER: f64 = 3.0;
const DEFAULT_SURVIVAL_ATTACK_RANGE: f64 = 3.0;
const DEFAULT_CREATIVE_ATTACK_RANGE: f64 = 5.0;
const DEFAULT_ATTACK_HITBOX_MARGIN: f64 = 0.3;

fn attack_target_is_in_range(
    gamemode: GameMode,
    attacker_eye_position: Vector3<f64>,
    target_bounds: BoundingBox,
) -> bool {
    let weapon_range = if gamemode == GameMode::Creative {
        DEFAULT_CREATIVE_ATTACK_RANGE
    } else {
        DEFAULT_SURVIVAL_ATTACK_RANGE
    };
    let max_range = weapon_range + ATTACK_PACKET_RANGE_BUFFER + DEFAULT_ATTACK_HITBOX_MARGIN;

    target_bounds.squared_magnitude(attacker_eye_position) <= max_range * max_range
}

/// Vanilla only accepts the confirmation for the teleport that is currently pending.
/// Late, duplicate, and unsolicited confirmations are ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TeleportConfirmAction {
    ApplyPendingPosition,
    Ignore,
}

const fn teleport_confirm_action(
    awaiting_teleport_id: Option<i32>,
    confirmation_id: i32,
) -> TeleportConfirmAction {
    match awaiting_teleport_id {
        Some(awaiting_id) if awaiting_id == confirmation_id => {
            TeleportConfirmAction::ApplyPendingPosition
        }
        _ => TeleportConfirmAction::Ignore,
    }
}

// Vanilla: `ServerGamePacketListenerImpl::handleMovePlayer` rejects every non-finite
// position component before clamping it to the world border.
const fn has_finite_position(position: Vector3<f64>) -> bool {
    position.x.is_finite() && position.y.is_finite() && position.z.is_finite()
}

// Vanilla: `ServerGamePacketListenerImpl::shouldCheckPlayerMovement` only enables
// correction while ticks run normally, outside sleep, and when the corresponding
// gamerules permit it. Pumpkin does not yet track vanilla's first-good position or
// per-tick packet count, so this preserves the one-packet portion of the check.
#[derive(Debug, Clone, Copy)]
struct MovementCheckContext {
    fall_flying: bool,
    ticks_run_normally: bool,
    player_movement_check: bool,
    elytra_movement_check: bool,
    sleeping: bool,
    post_impulse_grace_time: bool,
}

const fn movement_requires_correction(
    current_position: Vector3<f64>,
    target_position: Vector3<f64>,
    expected_velocity: Vector3<f64>,
    context: MovementCheckContext,
) -> bool {
    // Vanilla skips the wrong-movement correction during `LivingEntity.isInPostImpulseGraceTime`
    // (`ServerGamePacketListenerImpl.java:1140-1145`).
    if !context.ticks_run_normally
        || !context.player_movement_check
        || (context.fall_flying && !context.elytra_movement_check)
        || context.sleeping
        || context.post_impulse_grace_time
    {
        return false;
    }

    let delta_x = target_position.x - current_position.x;
    let delta_y = target_position.y - current_position.y;
    let delta_z = target_position.z - current_position.z;
    let moved_distance_squared = delta_x * delta_x + delta_y * delta_y + delta_z * delta_z;
    let expected_distance_squared = expected_velocity.x * expected_velocity.x
        + expected_velocity.y * expected_velocity.y
        + expected_velocity.z * expected_velocity.z;
    let maximum_excess_distance_squared = if context.fall_flying { 300.0 } else { 100.0 };

    moved_distance_squared - expected_distance_squared > maximum_excess_distance_squared
}

#[derive(Debug, Error)]
pub enum BlockPlacingError {
    BlockOutOfReach,
    InvalidHand,
    InvalidBlockFace,
    BlockOutOfWorld,
    InvalidGamemode,
}

impl std::fmt::Display for BlockPlacingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl PumpkinError for BlockPlacingError {
    fn is_kick(&self) -> bool {
        match self {
            Self::BlockOutOfReach | Self::BlockOutOfWorld | Self::InvalidGamemode => false,
            Self::InvalidBlockFace | Self::InvalidHand => true,
        }
    }

    fn severity(&self) -> Level {
        match self {
            Self::BlockOutOfWorld | Self::InvalidGamemode => Level::TRACE,
            Self::BlockOutOfReach | Self::InvalidBlockFace | Self::InvalidHand => Level::WARN,
        }
    }

    fn client_kick_reason(&self) -> Option<String> {
        match self {
            Self::BlockOutOfReach | Self::BlockOutOfWorld | Self::InvalidGamemode => None,
            Self::InvalidBlockFace => Some("Invalid block face".into()),
            Self::InvalidHand => Some("Invalid hand".into()),
        }
    }
}

#[derive(Debug, Error)]
pub enum ChatError {
    #[error("sent an oversized message")]
    OversizedMessage,
    #[error("sent a message with illegal characters")]
    IllegalCharacters,
    #[error("sent a chat with invalid/no signature")]
    UnsignedChat,
    #[error("has too many unacknowledged chats queued")]
    TooManyPendingChats,
    #[error("sent a chat that couldn't be validated")]
    ChatValidationFailed,
    #[error("sent a chat with an out of order timestamp")]
    OutOfOrderChat,
    #[error("has an expired public key")]
    ExpiredPublicKey,
    #[error("attempted to initialize a session with an invalid public key")]
    InvalidPublicKey,
}

impl PumpkinError for ChatError {
    fn is_kick(&self) -> bool {
        true
    }

    fn severity(&self) -> Level {
        Level::WARN
    }

    fn client_kick_reason(&self) -> Option<String> {
        match self {
            Self::OversizedMessage => Some("Chat message too long".into()),
            Self::IllegalCharacters => Some(
                TextComponent::translate_cross(
                    translation::java::MULTIPLAYER_DISCONNECT_ILLEGAL_CHARACTERS,
                    translation::java::MULTIPLAYER_DISCONNECT_ILLEGAL_CHARACTERS,
                    [],
                )
                .get_text(),
            ),
            Self::UnsignedChat => Some(
                TextComponent::translate_cross(
                    translation::java::MULTIPLAYER_DISCONNECT_UNSIGNED_CHAT,
                    translation::java::MULTIPLAYER_DISCONNECT_UNSIGNED_CHAT,
                    [],
                )
                .get_text(),
            ),
            Self::TooManyPendingChats => Some(
                TextComponent::translate_cross(
                    translation::java::MULTIPLAYER_DISCONNECT_TOO_MANY_PENDING_CHATS,
                    translation::java::MULTIPLAYER_DISCONNECT_TOO_MANY_PENDING_CHATS,
                    [],
                )
                .get_text(),
            ),
            Self::ChatValidationFailed => Some(
                TextComponent::translate_cross(
                    translation::java::MULTIPLAYER_DISCONNECT_CHAT_VALIDATION_FAILED,
                    translation::java::MULTIPLAYER_DISCONNECT_CHAT_VALIDATION_FAILED,
                    [],
                )
                .get_text(),
            ),
            Self::OutOfOrderChat => Some(
                TextComponent::translate_cross(
                    translation::java::MULTIPLAYER_DISCONNECT_OUT_OF_ORDER_CHAT,
                    translation::java::MULTIPLAYER_DISCONNECT_OUT_OF_ORDER_CHAT,
                    [],
                )
                .get_text(),
            ),
            Self::ExpiredPublicKey => Some(
                TextComponent::translate_cross(
                    translation::java::MULTIPLAYER_DISCONNECT_EXPIRED_PUBLIC_KEY,
                    translation::java::MULTIPLAYER_DISCONNECT_EXPIRED_PUBLIC_KEY,
                    [],
                )
                .get_text(),
            ),
            Self::InvalidPublicKey => Some(
                TextComponent::translate_cross(
                    translation::java::MULTIPLAYER_DISCONNECT_INVALID_PUBLIC_KEY_SIGNATURE,
                    translation::java::MULTIPLAYER_DISCONNECT_INVALID_PUBLIC_KEY_SIGNATURE,
                    [],
                )
                .get_text(),
            ),
        }
    }
}

pub mod attack;
pub mod bundle_item_selected;
pub mod change_difficulty;
pub mod change_game_mode;
pub mod chat_ack;
pub mod chat_command;
pub mod chat_message;
pub mod chunk_batch;
pub mod client_command;
pub mod client_information;
pub mod close_container;
pub mod command_suggestion;
pub mod configuration_acknowledged;
pub mod confirm_teleport;
pub mod container_slot_state_changed;
pub mod cookie_response;
pub mod debug_sample_subscription;
pub mod debug_subscription_request;
pub mod edit_book;
pub mod interact;
pub mod jigsaw_generate;
pub mod keep_alive;
pub mod lock_difficulty;
pub mod move_vehicle;
pub mod paddle_boat;
pub mod pick_item;
pub mod ping_request;
pub mod place_recipe;
pub mod player_abilities;
pub mod player_action;
pub mod player_command;
pub mod player_ground;
pub mod player_input;
pub mod player_loaded;
pub mod player_position;
pub mod player_rotation;
pub mod pong;
pub mod recipe_book_change_settings;
pub mod recipe_book_seen_recipe;
pub mod resource_pack_response;
pub mod seen_advancement;
pub mod select_trade;
pub mod set_beacon;
pub mod set_command_block;
pub mod set_command_minecart;
pub mod set_creative_slot;
pub mod set_game_rule;
pub mod set_held_item;
pub mod set_jigsaw_block;
pub mod set_structure_block;
pub mod set_test_block;
pub mod spectate_entity;
pub mod spectator_action;
pub mod swing_arm;
pub mod tag_query;
pub mod teleport_to_entity;
pub mod test_instance_block_action;
pub mod update_sign;
pub mod use_item;
pub mod use_item_on;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::entity::player::adventure_predicate_matches_block;
    use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
    use pumpkin_util::{
        GameMode, Hand,
        math::{boundingbox::BoundingBox, vector3::Vector3},
    };

    use super::{
        MovementCheckContext, TeleportConfirmAction, attack_target_is_in_range,
        has_finite_position, movement_requires_correction, pvp_allows_attack,
        teleport_confirm_action, uses_main_hand,
    };

    #[test]
    fn right_hand_uses_the_selected_inventory_slot() {
        assert!(uses_main_hand(Hand::Right));
        assert!(!uses_main_hand(Hand::Left));
    }

    #[test]
    fn adventure_can_break_predicates_match_named_blocks() {
        let mut predicate = NbtCompound::new();
        predicate.put(
            "blocks",
            NbtTag::List(vec![NbtTag::String(Box::from("minecraft:stone"))]),
        );

        let stone = pumpkin_data::Block::from_name("stone").unwrap();
        let dirt = pumpkin_data::Block::from_name("dirt").unwrap();
        assert!(adventure_predicate_matches_block(
            &NbtTag::Compound(predicate.clone()),
            stone,
            stone.default_state,
        ));
        assert!(!adventure_predicate_matches_block(
            &NbtTag::Compound(predicate),
            dirt,
            dirt.default_state,
        ));
    }

    /// `InteractionHand` is `MAIN_HAND(0)`/`OFF_HAND(1)` on the wire, which is the
    /// opposite of the `HumanoidArm` `LEFT(0)`/`RIGHT(1)` encoding that
    /// `ClientInformation.mainHand` uses. Decoding an interaction packet with the
    /// latter silently swaps the player's hands, so pin the byte here rather than
    /// only asserting the naming convention above.
    #[test]
    fn interaction_hand_id_zero_is_the_main_hand() {
        assert_eq!(Hand::from_interaction_id(0).ok(), Some(Hand::Right));
        assert_eq!(Hand::from_interaction_id(1).ok(), Some(Hand::Left));
        assert!(Hand::from_interaction_id(2).is_err());

        assert!(uses_main_hand(Hand::from_interaction_id(0).unwrap()));

        // The client-settings encoding must stay as it was.
        assert_eq!(Hand::try_from(0).ok(), Some(Hand::Left));
        assert_eq!(Hand::try_from(1).ok(), Some(Hand::Right));
    }

    #[test]
    fn movement_position_rejects_non_finite_components() {
        assert!(has_finite_position(Vector3::new(0.0, -64.0, 0.0)));
        assert!(!has_finite_position(Vector3::new(f64::NAN, 0.0, 0.0)));
        assert!(!has_finite_position(Vector3::new(f64::INFINITY, 0.0, 0.0)));
        assert!(!has_finite_position(Vector3::new(
            0.0,
            f64::NEG_INFINITY,
            0.0
        )));
    }

    #[test]
    fn movement_speed_check_uses_vanilla_excess_distance_limits() {
        let origin = Vector3::default();
        let stationary = Vector3::default();

        assert!(!movement_requires_correction(
            origin,
            Vector3::new(10.0, 0.0, 0.0),
            stationary,
            MovementCheckContext {
                fall_flying: false,
                ticks_run_normally: true,
                player_movement_check: true,
                elytra_movement_check: true,
                sleeping: false,
                post_impulse_grace_time: false,
            },
        ));
        assert!(movement_requires_correction(
            origin,
            Vector3::new(10.001, 0.0, 0.0),
            stationary,
            MovementCheckContext {
                fall_flying: false,
                ticks_run_normally: true,
                player_movement_check: true,
                elytra_movement_check: true,
                sleeping: false,
                post_impulse_grace_time: false,
            },
        ));
        assert!(!movement_requires_correction(
            origin,
            Vector3::new(11.0, 0.0, 0.0),
            Vector3::new(5.0, 0.0, 0.0),
            MovementCheckContext {
                fall_flying: false,
                ticks_run_normally: true,
                player_movement_check: true,
                elytra_movement_check: true,
                sleeping: false,
                post_impulse_grace_time: false,
            },
        ));
        assert!(!movement_requires_correction(
            origin,
            Vector3::new(299.0f64.sqrt(), 0.0, 0.0),
            stationary,
            MovementCheckContext {
                fall_flying: true,
                ticks_run_normally: true,
                player_movement_check: true,
                elytra_movement_check: true,
                sleeping: false,
                post_impulse_grace_time: false,
            },
        ));
        assert!(movement_requires_correction(
            origin,
            Vector3::new(301.0f64.sqrt(), 0.0, 0.0),
            stationary,
            MovementCheckContext {
                fall_flying: true,
                ticks_run_normally: true,
                player_movement_check: true,
                elytra_movement_check: true,
                sleeping: false,
                post_impulse_grace_time: false,
            },
        ));
        assert!(!movement_requires_correction(
            origin,
            Vector3::new(301.0f64.sqrt(), 0.0, 0.0),
            stationary,
            MovementCheckContext {
                fall_flying: false,
                ticks_run_normally: true,
                player_movement_check: true,
                elytra_movement_check: true,
                sleeping: false,
                // `handleMovePlayer` skips this correction during impulse grace time
                // (`ServerGamePacketListenerImpl.java:1140-1145`).
                post_impulse_grace_time: true,
            },
        ));

        for (ticks_run_normally, player_check, elytra_check, sleeping, fall_flying) in [
            (false, true, true, false, false),
            (true, false, true, false, false),
            (true, true, true, true, false),
            (true, true, false, false, true),
        ] {
            assert!(!movement_requires_correction(
                origin,
                Vector3::new(100.0, 0.0, 0.0),
                stationary,
                MovementCheckContext {
                    fall_flying,
                    ticks_run_normally,
                    player_movement_check: player_check,
                    elytra_movement_check: elytra_check,
                    sleeping,
                    post_impulse_grace_time: false,
                },
            ));
        }
    }

    #[test]
    fn attack_range_uses_the_nearest_point_of_the_target_bounds() {
        let eye = Vector3::new(0.0, 0.0, 0.0);
        let just_in_range =
            BoundingBox::new(Vector3::new(6.3, 0.0, 0.0), Vector3::new(6.8, 1.0, 1.0));
        let out_of_range = BoundingBox::new(
            Vector3::new(6.300_001, 0.0, 0.0),
            Vector3::new(7.0, 1.0, 1.0),
        );

        assert!(attack_target_is_in_range(
            GameMode::Survival,
            eye,
            just_in_range
        ));
        assert!(!attack_target_is_in_range(
            GameMode::Survival,
            eye,
            out_of_range
        ));
    }

    #[test]
    fn creative_attack_range_extends_to_eight_point_three_blocks() {
        let eye = Vector3::new(0.0, 0.0, 0.0);
        let target = BoundingBox::new(Vector3::new(8.3, 0.0, 0.0), Vector3::new(9.0, 1.0, 1.0));

        assert!(attack_target_is_in_range(GameMode::Creative, eye, target));
        assert!(!attack_target_is_in_range(GameMode::Survival, eye, target));
    }

    #[test]
    fn teleport_confirm_only_accepts_the_current_pending_id() {
        use TeleportConfirmAction::{ApplyPendingPosition, Ignore};

        for (awaiting_id, confirmation_id, expected) in [
            (None, 7, Ignore),
            (Some(7), 6, Ignore),
            (Some(7), 7, ApplyPendingPosition),
            // A duplicate confirm becomes unsolicited after the first matching confirm clears it.
            (None, 7, Ignore),
        ] {
            assert_eq!(
                teleport_confirm_action(awaiting_id, confirmation_id),
                expected
            );
        }
    }

    #[test]
    fn disabled_pvp_allows_attacks_against_non_players() {
        assert!(pvp_allows_attack(false, false));
    }

    #[test]
    fn disabled_pvp_blocks_attacks_against_players() {
        assert!(!pvp_allows_attack(false, true));
    }

    #[test]
    fn enabled_pvp_allows_attacks_against_all_targets() {
        assert!(pvp_allows_attack(true, false));
        assert!(pvp_allows_attack(true, true));
    }
}
