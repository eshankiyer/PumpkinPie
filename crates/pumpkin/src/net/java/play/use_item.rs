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

    async fn prepare_hand_item_for_use(
        &self,
        player: &Arc<Player>,
        hand: Hand,
        held: &mut ItemStack,
    ) {
        // Vanilla `Item.use` (`Item.java:189-210`) is an exclusive chain: a consumable
        // starts consuming, else a swappable equippable swaps into its slot, else a
        // shield or kinetic weapon starts being used. All three uses take the stack's
        // long-use duration (`Item.java:310-316`).
        if held.get_data_component::<ConsumableImpl>().is_some() {
            // If its food we want to make sure we can actually consume it
            if let Some(food) = held.get_data_component::<FoodImpl>() {
                if player.abilities.lock().await.invulnerable
                    || food.can_always_eat
                    || player.hunger_manager.level.load() < 20
                {
                    player
                        .living_entity
                        .set_active_hand(hand, held.clone(), held.get_max_use_time())
                        .await;
                }
            } else {
                player
                    .living_entity
                    .set_active_hand(hand, held.clone(), held.get_max_use_time())
                    .await;
            }
        } else if let Some(equippable) = held
            .get_data_component::<EquippableImpl>()
            .filter(|equippable| equippable.swappable)
            .cloned()
        {
            if swap_with_equipment_slot(player, held, &equippable).await {
                player
                    .inventory()
                    .set_stack_in_hand(hand, held.clone())
                    .await;
            }
        } else if held.get_data_component::<BlocksAttacksImpl>().is_some()
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

/// Vanilla `Equippable.swapWithEquipmentSlot` (`Equippable.java:128-158`). Mutates `held` into
/// the stack the hand should hold afterwards and returns whether a swap happened; the caller
/// writes `held` back to the hand.
pub(crate) async fn swap_with_equipment_slot(
    player: &Arc<Player>,
    held: &mut ItemStack,
    equippable: &EquippableImpl,
) -> bool {
    // `canBeEquippedBy` (`Equippable.java:175-177`); `Player.canUseSlot` is always true.
    let entity_type = player.living_entity.entity.entity_type;
    let allowed = equippable
        .allowed_entities
        .as_ref()
        .is_none_or(|allowed| match allowed {
            pumpkin_data::data_component_impl::IDSet::IDs(ids) => {
                ids.iter().any(|ty| ty.id == entity_type.id)
            }
            pumpkin_data::data_component_impl::IDSet::Tag(tag) => {
                pumpkin_data::tag::Taggable::is_tagged_with(entity_type, tag).unwrap_or(false)
            }
        });
    if !allowed {
        return false;
    }

    let slot = equippable.slot;
    let inventory = player.inventory();
    // The equipment lock has to be released before touching the hand again: the off hand
    // lives in the same map, so holding it here would deadlock.
    let mut in_equipment_slot = inventory.entity_equipment.lock().await.get(slot);
    let creative = player.is_creative();
    // Only the binding curse carries `PREVENT_ARMOR_CHANGE`.
    if (!creative
        && in_equipment_slot
            .get_enchantment_level(&pumpkin_data::enchantment::Enchantment::BINDING_CURSE)
            != 0)
        || held.are_items_and_components_equal(&in_equipment_slot)
    {
        return false;
    }

    player
        .increment_stat(StatisticCategory::Used, i32::from(held.item.id), 1)
        .await;

    if held.item_count <= 1 {
        let to_equipment = if creative {
            held.clone()
        } else {
            held.copy_and_clear()
        };
        if !in_equipment_slot.is_empty() {
            *held = in_equipment_slot.copy_and_clear();
        }
        player.enqueue_equipment_change(slot, &to_equipment).await;
        inventory
            .entity_equipment
            .lock()
            .await
            .put(slot, to_equipment);
        return true;
    }

    let to_inventory = in_equipment_slot.copy_and_clear();
    let to_equipment = held.consume_and_return(1, creative);
    player.enqueue_equipment_change(slot, &to_equipment).await;
    inventory
        .entity_equipment
        .lock()
        .await
        .put(slot, to_equipment);
    if !to_inventory.is_empty() {
        inventory
            .offer_or_drop_stack(to_inventory, player.as_ref())
            .await;
    }
    true
}
