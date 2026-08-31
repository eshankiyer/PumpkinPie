#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub async fn handle_attack(&self, player: &Arc<Player>, attack: SAttack, server: &Arc<Server>) {
        if !player.has_client_loaded() || player.gamemode.load() == GameMode::Spectator {
            return;
        }
        player.update_last_action_time();
        let entity_id = attack.entity_id;
        let player_entity = &player.get_entity();
        let world = player_entity.world.load_full();

        if entity_id.0 == player.entity_id() {
            self.kick(TextComponent::translate_cross(
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_ENTITY_ATTACKED,
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_ENTITY_ATTACKED,
                [],
            ))
            .await;
            return;
        }

        let player_target = world.get_player_by_id(entity_id.0);
        let target: Option<Arc<dyn EntityBase>> = player_target
            .as_ref()
            .map(|p| Arc::clone(p) as Arc<dyn EntityBase>)
            .or_else(|| world.get_entity_by_id(entity_id.0));
        let Some(target) = target else {
            self.kick(TextComponent::translate_cross(
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_ENTITY_ATTACKED,
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_ENTITY_ATTACKED,
                [],
            ))
            .await;
            return;
        };
        let main_hand = player.inventory().held_item().await;
        // `ServerGamePacketListenerImpl.handleAttack` passes the held weapon and a 3.0 buffer
        // to `Player.isWithinAttackRange` (`ServerGamePacketListenerImpl.java:1807-1819`).
        if !player.is_within_attack_range(
            &main_hand,
            target.get_entity().bounding_box.load(),
            ATTACK_PACKET_RANGE_BUFFER,
        ) {
            return;
        }
        let config = &server.advanced_config.pvp;
        if !pvp_allows_attack(config.enabled, player_target.is_some()) {
            return;
        }
        if let Some(player_victim) = &player_target {
            if player_victim.living_entity.health.load() <= 0.0 {
                return;
            }
            if config.protect_creative && player_victim.gamemode.load() == GameMode::Creative {
                world.play_sound(
                    Sound::EntityPlayerAttackNodamage,
                    SoundCategory::Players,
                    &player_victim.position(),
                );
                return;
            }
        }
        // Vanilla rejects an undercharged item before `Player.attack`
        // (`ServerGamePacketListenerImpl.java:1807-1819`).
        if player.cannot_attack_with_item(&main_hand, 5) {
            return;
        }
        player.attack(target).await;
    }
}
