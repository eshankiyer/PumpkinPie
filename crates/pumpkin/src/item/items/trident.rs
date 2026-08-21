use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::entity::player::Player;
use crate::entity::projectile::arrow::ArrowPickup;
use crate::entity::projectile::trident::TridentEntity;
use crate::entity::{Entity, EntityBase};
use crate::item::{ItemBehaviour, ItemMetadata};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::inventory::Inventory;

pub struct TridentItem;

impl ItemMetadata for TridentItem {
    fn ids() -> Box<[u16]> {
        [Item::TRIDENT.id].into()
    }
}

impl ItemBehaviour for TridentItem {
    fn normal_use<'a>(
        &'a self,
        _item: &'a Item,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let inventory = player.inventory();
            let stack = inventory.held_item().await;

            player
                .living_entity
                .set_active_hand(pumpkin_util::Hand::Right, stack, 72000)
                .await;
        })
    }

    #[expect(clippy::too_many_lines)]
    fn on_stopped_using<'a>(
        &'a self,
        _stack: &'a ItemStack,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let use_ticks = player
                .living_entity
                .item_use_time
                .load(std::sync::atomic::Ordering::Relaxed);
            let use_ticks = 72000 - use_ticks;

            if use_ticks < 10 {
                return;
            }

            let world = player.world();
            let stack_guard = player.inventory().held_item().await;

            // Check Riptide level
            let mut riptide_level = 0u32;
            if let Some(enchantments) = stack_guard
                .get_data_component::<pumpkin_data::data_component_impl::EnchantmentsImpl>(
            ) {
                for (enchantment, level) in enchantments.enchantment.iter() {
                    if **enchantment == pumpkin_data::Enchantment::RIPTIDE {
                        riptide_level = *level as u32;
                    }
                }
            }

            if riptide_level > 0 {
                // Vanilla `TridentItem.releaseUsing` (`TridentItem.java:69`): with a spin-attack
                // enchantment the release only does anything while the player is in water or
                // rain and is not riding another entity.
                let is_touching_water = player
                    .living_entity
                    .entity
                    .touching_water
                    .load(std::sync::atomic::Ordering::Relaxed);
                let block_pos = player.get_entity().block_pos.load();
                let rain_pos = BlockPos::floored(
                    f64::from(block_pos.0.x),
                    player.get_entity().bounding_box.load().max.y,
                    f64::from(block_pos.0.z),
                );
                let is_raining =
                    world.is_raining_at(&block_pos).await || world.is_raining_at(&rain_pos).await;
                let is_passenger = player.get_entity().has_vehicle().await;

                if !(is_touching_water || is_raining) || is_passenger {
                    player.living_entity.clear_active_hand().await;
                    return;
                }

                // `ItemStack.nextDamageWillBreak` (`TridentItem.java:70`).
                if stack_guard.is_damageable()
                    && stack_guard
                        .get_max_damage()
                        .is_some_and(|max| stack_guard.get_damage() + 1 >= max)
                {
                    player.living_entity.clear_active_hand().await;
                    return;
                }

                let (yaw, pitch) = player.rotation();
                let look_vec = Vector3::rotation_vector(pitch as f64, yaw as f64);
                // Riptide's `trident_spin_attack_strength` is
                // `LevelBasedValue.perLevel(1.5F, 0.75F)` (`Enchantments.java:993`), i.e.
                // 1.5 at level 1 and +0.75 per level above the first.
                let speed = f64::from(riptide_level - 1).mul_add(0.75, 1.5);
                let launch_velocity = look_vec.multiply(speed, speed, speed);

                if player.gamemode.load() != GameMode::Creative {
                    player.damage_held_item(1).await;
                }
                let spin_item = stack_guard.clone();
                player.get_entity().add_velocity(launch_velocity);
                if player
                    .get_entity()
                    .on_ground
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    player
                        .get_entity()
                        .move_self_with_collisions(player, Vector3::new(0.0, 1.2, 0.0))
                        .await;
                }
                player
                    .living_entity
                    .start_auto_spin_attack(20, 8.0, spin_item)
                    .await;

                let sound = match riptide_level {
                    1 => Sound::ItemTridentRiptide1,
                    2 => Sound::ItemTridentRiptide2,
                    _ => Sound::ItemTridentRiptide3,
                };
                world.play_sound(
                    sound,
                    pumpkin_data::sound::SoundCategory::Players,
                    &player.position(),
                );

                player.living_entity.clear_active_hand().await;
                return;
            }

            // Normal throw - spawn thrown trident
            let (yaw, pitch) = player.rotation();
            let entity = Entity::new(world.clone(), player.position(), &EntityType::TRIDENT);
            let trident = TridentEntity::new_shot(
                entity,
                player.get_entity(),
                stack_guard.clone(),
                ArrowPickup::Allowed,
            );
            trident.set_velocity_from_rotation(pitch, yaw, 0.0, 2.5, 1.0);
            world.spawn_entity(Arc::new(trident)).await;

            world.play_sound(
                Sound::ItemTridentThrow,
                pumpkin_data::sound::SoundCategory::Players,
                &player.position(),
            );

            if player.gamemode.load() != GameMode::Creative {
                let inventory = player.inventory();
                let selected_slot = inventory.get_selected_slot() as usize;

                let main_hand_item = inventory.get_stack(selected_slot).await;
                if main_hand_item.item.id == Item::TRIDENT.id {
                    inventory
                        .set_stack(selected_slot, ItemStack::EMPTY.clone())
                        .await;
                    player
                        .sync_hand_slot(selected_slot, ItemStack::EMPTY.clone())
                        .await;
                } else {
                    let off_hand_slot =
                        pumpkin_inventory::player::player_inventory::PlayerInventory::OFF_HAND_SLOT;
                    let off_hand_item = inventory.get_stack(off_hand_slot).await;
                    if off_hand_item.item.id == Item::TRIDENT.id {
                        inventory
                            .set_stack(off_hand_slot, ItemStack::EMPTY.clone())
                            .await;
                        player
                            .sync_hand_slot(off_hand_slot, ItemStack::EMPTY.clone())
                            .await;
                    }
                }
            }

            player.living_entity.clear_active_hand().await;
        })
    }

    fn can_mine(&self, player: &Player) -> bool {
        player.gamemode.load() != GameMode::Creative
    }

    fn get_use_duration(&self) -> i32 {
        72000
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
