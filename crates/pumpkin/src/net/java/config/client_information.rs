#[allow(clippy::wildcard_imports)]
use super::*;

impl PlayerConfig {
    /// Vanilla `ClientInformation.createDefault` (`ClientInformation.java:47-48`) supplies the
    /// client settings used when no client-information packet has arrived yet.
    pub(crate) fn create_default() -> Self {
        Self {
            locale: "en_us".to_string(),
            view_distance: NonZero::new(2).unwrap_or(NonZero::<u8>::MIN),
            chat_mode: ChatMode::Enabled,
            chat_colors: true,
            skin_parts: 0,
            main_hand: Hand::Right,
            text_filtering: false,
            server_listing: false,
        }
    }
}

impl JavaClient {
    pub async fn handle_client_information_config(
        &self,
        client_information: SClientInformationConfig<'_>,
    ) {
        debug!("Handling client settings");
        if client_information.view_distance <= 0 {
            self.kick(TextComponent::text(
                "Cannot have zero or negative view distance!",
            ))
            .await;
            return;
        }

        if let (Ok(main_hand), Ok(chat_mode)) = (
            Hand::try_from(client_information.main_hand.0),
            ChatMode::try_from(client_information.chat_mode.0),
        ) {
            self.config.store(Arc::new(PlayerConfig {
                locale: client_information.locale.to_string(),
                // client_information.view_distance was checked above to be > 0 so compiler should optimize this out.
                view_distance: NonZero::new(client_information.view_distance as u8)
                    .unwrap_or(NonZero::<u8>::MIN),
                chat_mode,
                chat_colors: client_information.chat_colors,
                skin_parts: client_information.skin_parts,
                main_hand,
                text_filtering: client_information.text_filtering,
                server_listing: client_information.server_listing,
            }));
        } else {
            self.kick(TextComponent::text("Invalid hand or chat type"))
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PlayerConfig;
    use crate::entity::player::ChatMode;
    use pumpkin_util::Hand;

    #[test]
    fn create_default_matches_vanilla_client_information() {
        // `ClientInformation.createDefault` (`ClientInformation.java:47-48`) is the source of
        // these fallback values; PlayerConfig has no particle-status field to populate.
        let config = PlayerConfig::create_default();
        assert_eq!(config.locale, "en_us");
        assert_eq!(config.view_distance.get(), 2);
        assert!(matches!(config.chat_mode, ChatMode::Enabled));
        assert!(config.chat_colors);
        assert_eq!(config.skin_parts, 0);
        assert!(matches!(config.main_hand, Hand::Right));
        assert!(!config.text_filtering);
        assert!(!config.server_listing);
    }
}
