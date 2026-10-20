#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub async fn handle_use_item(
        &self,
        player: &Arc<Player>,
        use_item: &SUseItem,
        server: &Arc<Server>,
    ) {
        if !player.has_client_loaded() {
            return;
        }
        player.update_last_action_time();

        self.update_sequence(player, use_item.sequence.0);
        let inventory = player.inventory();
        let Ok(hand) = Hand::from_packet_id(use_item.hand.0) else {
            self.kick(TextComponent::text("InvalidHand")).await;
            return;
        };

        let mut item_in_hand = inventory.get_stack_in_hand(hand).await;
        if item_in_hand.is_empty() {
            return;
        }

        let entity = player.get_entity();
        let target_yaw = wrap_degrees(use_item.yaw) % 360.0;
        let target_pitch = wrap_degrees(use_item.pitch);
        if target_yaw != entity.yaw.load() || target_pitch != entity.pitch.load() {
            entity.set_rotation(target_yaw, target_pitch);
        }

        // Vanilla ServerPlayerGameMode.useItem returns PASS for spectators after
        // packet bookkeeping, but before touching the held item or firing events.
        if player.gamemode.load() == GameMode::Spectator {
            return;
        }

        // ServerPlayerGameMode.useItem returns PASS when the stack's cooldown group is
        // active, before the item is read, consumed or dispatched. ItemCooldowns keys
        // the cooldown by the use_cooldown component's group, falling back to the item
        // id, so the lookup happens for every stack and not only for stacks carrying
        // the component.
        let cooldown_group = item_in_hand
            .get_use_cooldown()
            .and_then(|cooldown| cooldown.cooldown_group.clone())
            .unwrap_or_else(|| item_in_hand.item.registry_key.to_string());
        if player.is_on_cooldown(&cooldown_group).await {
            return;
        }

        let mut consume_event =
            crate::plugin::api::events::player::player_item_consume::PlayerItemConsumeEvent::new(
                player.clone(),
                item_in_hand.item.registry_key.to_string(),
            );
        server.plugin_manager.fire(server, &mut consume_event).await;
        if consume_event.cancelled {
            return;
        }

        let hit_result = player
            .world()
            .raycast(
                player.eye_position(),
                player.eye_position().add(
                    &(Vector3::rotation_vector(f64::from(use_item.pitch), f64::from(use_item.yaw))
                        * 4.5),
                ),
                async |pos, world| {
                    let block = world.get_block(pos);
                    block != &Block::AIR && block != &Block::WATER && block != &Block::LAVA
                },
            )
            .await;

        let event = if let Some((hit_pos, _hit_dir)) = hit_result {
            PlayerInteractEvent::new(
                player,
                InteractAction::RightClickBlock,
                player.world().get_block(&hit_pos),
                Some(hit_pos),
            )
        } else {
            PlayerInteractEvent::new(player, InteractAction::RightClickAir, &Block::AIR, None)
        };
        let (item_for_use, stack_for_use) = (item_in_hand.item, item_in_hand.clone());
        self.prepare_hand_item_for_use(player, hand, &mut item_in_hand)
            .await;

        if !self
            .should_continue_use_after_fish_event(server, player, hand, item_for_use)
            .await
        {
            return;
        }

        send_cancellable! {{
            server;
            event;
            'after: {
                server.item_registry.on_use(&stack_for_use, player).await;
            }
        }}
    }

    /// `Item.use` (`Item.java:189-212`) with `Equippable.swapWithEquipmentSlot` (`Equippable.java:128-158`).
    async fn prepare_hand_item_for_use(
        &self,
        player: &Arc<Player>,
        hand: Hand,
        held: &mut ItemStack,
    ) {
        let inventory = player.inventory();

        if held.get_data_component::<ConsumableImpl>().is_some() {
            let can_consume = match held.get_data_component::<FoodImpl>() {
                Some(food) => {
                    player.abilities.lock().await.invulnerable
                        || food.can_always_eat
                        || player.hunger_manager.level.load() < 20
                }
                None => true,
            };
            if can_consume {
                player
                    .living_entity
                    .set_active_hand(hand, held.clone(), held.get_max_use_time())
                    .await;
            }
            return;
        }

        if let Some(equippable) = held.get_data_component::<EquippableImpl>()
            && equippable.swappable
        {
            let slot = equippable.slot.clone();
            let creative = player.gamemode.load() == GameMode::Creative;
            // The equipment lock has to be released before touching the hand again:
            // the off hand lives in the same map, so holding it here would deadlock.
            let current_equipped = inventory.entity_equipment.lock().await.get(&slot);
            if !equip_swap_allowed(equippable, held, &current_equipped, creative) {
                return;
            }

            if held.item_count <= 1 {
                let to_equipment = held.clone();
                let to_hand = if !current_equipped.is_empty() {
                    current_equipped
                } else if creative {
                    held.clone()
                } else {
                    ItemStack::EMPTY.clone()
                };
                player.enqueue_equipment_change(&slot, &to_equipment).await;
                inventory
                    .entity_equipment
                    .lock()
                    .await
                    .put(&slot, to_equipment);
                *held = to_hand;
            } else {
                let to_equipment = held.copy_with_count(1);
                held.decrement_unless_creative(player.gamemode.load(), 1);
                player.enqueue_equipment_change(&slot, &to_equipment).await;
                inventory
                    .entity_equipment
                    .lock()
                    .await
                    .put(&slot, to_equipment);
                if !current_equipped.is_empty() {
                    inventory
                        .offer_or_drop_stack(current_equipped, player.as_ref())
                        .await;
                }
            }
            inventory.set_stack_in_hand(hand, held.clone()).await;
            return;
        }

        if held.get_data_component::<BlocksAttacksImpl>().is_some()
            || held.get_data_component::<KineticWeaponImpl>().is_some()
        {
            player
                .living_entity
                .set_active_hand(hand, held.clone(), held.get_max_use_time())
                .await;
        }
    }

    async fn should_continue_use_after_fish_event(
        &self,
        server: &Arc<Server>,
        player: &Arc<Player>,
        hand: Hand,
        item_for_use: &Item,
    ) -> bool {
        if item_for_use.id != Item::FISHING_ROD.id {
            return true;
        }

        // TODO: Apply fishing rod durability on retrieval based on catch type.
        let mut fish_event = PlayerFishEvent::new(
            player.clone(),
            None,
            uuid::Uuid::nil(),
            String::new(),
            PlayerFishState::Fishing,
            hand,
            0,
        );
        server.plugin_manager.fire(server, &mut fish_event).await;
        !fish_event.cancelled
    }
}

/// `Item.use` equippable gates (`Item.java:195-196`, `Equippable.java:129-132,175-177`).
fn equip_swap_allowed(
    equippable: &EquippableImpl,
    in_hand: &ItemStack,
    in_slot: &ItemStack,
    creative: bool,
) -> bool {
    use pumpkin_data::data_component_impl::IDSet;
    use pumpkin_data::entity::EntityType;
    use pumpkin_data::tag::Taggable;

    equippable.swappable
        && equippable
            .allowed_entities
            .as_ref()
            .is_none_or(|allowed| match allowed {
                IDSet::IDs(ids) => ids
                    .iter()
                    .any(|entity_type| entity_type.id == EntityType::PLAYER.id),
                IDSet::Tag(tag) => EntityType::PLAYER.is_tagged_with(tag).unwrap_or(false),
            })
        && (creative
            || in_slot.get_enchantment_level(&pumpkin_data::Enchantment::BINDING_CURSE) <= 0)
        && !ItemStack::is_same_item_same_components(in_hand, in_slot)
}

#[cfg(test)]
mod tests {
    use super::equip_swap_allowed;
    use pumpkin_data::Enchantment;
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{EnchantmentsImpl, EquippableImpl};
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;

    fn equippable(stack: &ItemStack) -> &EquippableImpl {
        stack.get_data_component::<EquippableImpl>().unwrap()
    }

    fn cursed(item: &'static Item) -> ItemStack {
        let mut stack = ItemStack::new(1, item);
        stack.patch.push((
            DataComponent::Enchantments,
            Some(Box::new(EnchantmentsImpl {
                enchantment: std::borrow::Cow::from(vec![(&Enchantment::BINDING_CURSE, 1)]),
            })),
        ));
        stack
    }

    #[test]
    fn equip_swap_follows_vanilla_gates() {
        let empty = ItemStack::EMPTY.clone();
        let helmet = ItemStack::new(1, &Item::DIAMOND_HELMET);
        assert!(equip_swap_allowed(equippable(&helmet), &helmet, &empty, false));
        assert!(!equip_swap_allowed(equippable(&helmet), &helmet, &helmet.clone(), false));

        let cursed_helmet = cursed(&Item::IRON_HELMET);
        assert!(!equip_swap_allowed(equippable(&helmet), &helmet, &cursed_helmet, false));
        assert!(equip_swap_allowed(equippable(&helmet), &helmet, &cursed_helmet, true));

        for item in [&Item::CARVED_PUMPKIN, &Item::CREEPER_HEAD, &Item::SHIELD] {
            let stack = ItemStack::new(1, item);
            assert!(!equip_swap_allowed(equippable(&stack), &stack, &empty, false));
        }

        for item in [&Item::SADDLE, &Item::WHITE_CARPET, &Item::WHITE_HARNESS, &Item::WOLF_ARMOR] {
            let stack = ItemStack::new(1, item);
            assert!(!equip_swap_allowed(equippable(&stack), &stack, &empty, false));
        }

        let elytra = ItemStack::new(1, &Item::ELYTRA);
        assert!(equip_swap_allowed(equippable(&elytra), &elytra, &helmet, false));
    }
}
