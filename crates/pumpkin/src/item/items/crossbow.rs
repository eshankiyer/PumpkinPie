use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::entity::player::Player;
use crate::entity::projectile::arrow::{ArrowEntity, ArrowPickup};
use crate::entity::projectile::firework_rocket::FireworkRocketEntity;
use crate::entity::{Entity, EntityBase};
use crate::item::{ItemBehaviour, ItemMetadata};
use pumpkin_data::Enchantment;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{ChargedProjectilesImpl, EnchantmentsImpl};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::GameMode;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::inventory::Inventory;

/// Vanilla `CrossbowItem#getShootingPower`: `1.6F` if the charged ammo is a firework
/// rocket, else the usual `3.15F` arrow power.
fn projectile_power(item: &'static Item) -> f32 {
    if item == &Item::FIREWORK_ROCKET {
        1.6
    } else {
        3.15
    }
}

/// Vanilla `CrossbowItem#getDurabilityUse`: firework rockets cost 3 durability per shot,
/// arrows cost 1.
fn durability_use(item: &'static Item) -> i32 {
    if item == &Item::FIREWORK_ROCKET { 3 } else { 1 }
}

/// Converts `CrossbowItem`'s view-vector rotation around the shooter's up vector back to the
/// rotation arguments used by Pumpkin's projectile setters. Vanilla performs this around the
/// living entity's up vector for each spread angle (`CrossbowItem.java:107-130`).
fn rotate_shot_vector(view: Vector3<f64>, up: Vector3<f64>, angle: f32) -> Vector3<f64> {
    let angle = f64::from(angle).to_radians();
    view * angle.cos() + up.cross(&view) * angle.sin() + up * (up.dot(&view) * (1.0 - angle.cos()))
}

/// Converts the rotated shot vector to the rotation arguments accepted by the live projectile
/// setters (`CrossbowItem.java:107-130`).
fn shot_rotation(player: &Entity, angle: f32) -> (f32, f32) {
    let view = Vector3::from_yaw_pitch(player.yaw.load(), player.pitch.load());
    let rotated = rotate_shot_vector(view, player.get_up_vector(), angle);
    let horizontal = rotated.x.hypot(rotated.z);
    (
        (-rotated.y).atan2(horizontal).to_degrees() as f32,
        (-rotated.x).atan2(rotated.z).to_degrees() as f32,
    )
}

pub struct CrossbowItem;

impl ItemMetadata for CrossbowItem {
    fn ids() -> Box<[u16]> {
        Box::new([Item::CROSSBOW.id])
    }
}

/// The number of ticks (out of `get_use_duration`) a crossbow needs to charge before it
/// loads itself.
///
/// Vanilla `CrossbowItem#getChargeDuration` (CrossbowItem.java:245-248):
/// `floor(modifyCrossbowChargingTime(crossbow, user, 1.25F) * 20.0F)`. The base 1.25s is
/// 25 ticks; `quick_charge.json`'s `crossbow_charge_time` is in seconds, so scale by 20.
fn charge_duration_ticks(stack: &ItemStack) -> i32 {
    let mut charge_time = 25;
    if let Some(enchantments) = stack.get_data_component::<EnchantmentsImpl>() {
        for (enchantment, level) in enchantments.enchantment.iter() {
            for effect in crate::enchantment::effects_for(enchantment) {
                if let crate::enchantment::EnchantmentEffect::CrossbowChargeTime(value) = effect {
                    charge_time += (value.calculate(*level) * 20.0) as i32;
                }
            }
        }
    }
    charge_time.max(0)
}

impl ItemBehaviour for CrossbowItem {
    fn normal_use<'a>(
        &'a self,
        _item: &'a Item,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let inventory = player.inventory();
            let stack = inventory.held_item().await;

            // Every crossbow carries a ChargedProjectiles component by default, so its mere
            // presence does not mean the crossbow is loaded. Vanilla checks the list is also
            // non-empty (CrossbowItem.java:68).
            if stack
                .get_data_component::<ChargedProjectilesImpl>()
                .is_some_and(|charged| !charged.projectiles.is_empty())
            {
                Self::fire_projectiles(player).await;
                return;
            }

            let has_ammo = player.find_crossbow_projectile().await.is_some();
            if !has_ammo && player.gamemode.load() != GameMode::Creative {
                return;
            }

            player
                .living_entity
                .set_active_hand(pumpkin_util::Hand::Right, stack, 72000)
                .await;
        })
    }

    fn on_stopped_using<'a>(
        &'a self,
        _stack: &'a ItemStack,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let stack = player.inventory().held_item().await;

            // Vanilla `CrossbowItem#onUseTick` (CrossbowItem.java:222) already loaded the
            // crossbow once the charge completed while the button was still held, so a
            // release afterwards must not draw (and consume) ammo a second time. This
            // mirrors `releaseUsing`'s gate `power >= 1.0F && isCharged(itemStack)`
            // (CrossbowItem.java:86-89), which only succeeds for an already-charged bow.
            if stack
                .get_data_component::<ChargedProjectilesImpl>()
                .is_some_and(|charged| !charged.projectiles.is_empty())
            {
                player.living_entity.clear_active_hand().await;
                return;
            }

            let use_ticks = player.living_entity.item_use_time.load(Ordering::Relaxed);
            let use_ticks = 72000 - use_ticks;

            if use_ticks >= charge_duration_ticks(&stack) {
                Self::try_load_projectiles(player).await;
            }
            player.living_entity.clear_active_hand().await;
        })
    }

    fn on_use_tick<'a>(
        &'a self,
        _stack: &'a ItemStack,
        player: &'a Player,
        remaining_use_ticks: i32,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let stack = player.inventory().held_item().await;

            // Already loaded: vanilla's `isCharged` check (CrossbowItem.java:222) stops the
            // loading branch, and the start/mid sounds only fire once per use anyway.
            if stack
                .get_data_component::<ChargedProjectilesImpl>()
                .is_some_and(|charged| !charged.projectiles.is_empty())
            {
                return;
            }

            let charge_duration = charge_duration_ticks(&stack);
            if charge_duration == 0 {
                return;
            }

            let held_ticks = 72000 - remaining_use_ticks;

            // Vanilla `CrossbowItem#onUseTick` (CrossbowItem.java:202-238): the charging
            // progress is `(useDuration - ticksRemaining) / chargeDuration`, with sounds at
            // START_SOUND_PERCENT 0.2 and MID_SOUND_PERCENT 0.5 played exactly once per use.
            // Held ticks increase by one each tick, so "first tick at or past a threshold"
            // equals the smallest tick count reaching it: ceil(threshold * duration).
            let start_at = (0.2 * charge_duration as f32).ceil() as i32;
            let mid_at = (0.5 * charge_duration as f32).ceil() as i32;

            if held_ticks == start_at {
                player.world().play_sound_fine(
                    Sound::ItemCrossbowLoadingStart,
                    SoundCategory::Players,
                    &player.position(),
                    0.5,
                    1.0,
                );
            }
            if held_ticks == mid_at {
                player.world().play_sound_fine(
                    Sound::ItemCrossbowLoadingMiddle,
                    SoundCategory::Players,
                    &player.position(),
                    0.5,
                    1.0,
                );
            }

            if held_ticks >= charge_duration {
                // Vanilla CrossbowItem.java:222-236: loading at full charge; the
                // loading-end sound inside is gated on the load succeeding.
                Self::try_load_projectiles(player).await;
            }
        })
    }

    fn get_use_duration(&self) -> i32 {
        72000
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Vanilla `CrossbowItem#getShotPitch` / `#getRandomShotPitch`: the first shot is always 1.0,
/// later shots alternate between a high (0.63) and a low (0.43) offset.
fn crossbow_shot_pitch(index: u32, random_value: f32) -> f32 {
    if index == 0 {
        return 1.0;
    }
    let range_decider = if index & 1 == 1 { 0.63 } else { 0.43 };
    1.0 / random_value.mul_add(0.5, 1.8) + range_decider
}

/// Piercing's `minecraft:projectile_piercing` value, clamped to the byte an arrow stores.
///
/// `piercing.json`: `add` of `linear(base = 1.0, per_level_above_first = 1.0)` onto a base of 0,
/// so level N pierces N extra entities (`AbstractArrow` discards once it has hit
/// `pierceLevel + 1`).
fn piercing_count(level: i32) -> u8 {
    if level <= 0 {
        return 0;
    }
    let value = crate::enchantment::effects_for(&Enchantment::PIERCING)
        .iter()
        .filter_map(|effect| match effect {
            crate::enchantment::EnchantmentEffect::ProjectilePiercing(value) => {
                Some(value.calculate(level))
            }
            _ => None,
        })
        .sum::<f32>();
    // Vanilla truncates the accumulated float to an int (`MutableFloat.intValue()`).
    value.clamp(0.0, f32::from(u8::MAX)) as u8
}

impl CrossbowItem {
    /// Vanilla `CrossbowItem#getDefaultProjectileRange` (CrossbowItem.java:274-276): the
    /// distance (in blocks) over which projectile-weapon AI may attack with a crossbow.
    pub const DEFAULT_RANGE: i32 = 8;

    /// Vanilla `CrossbowItem#tryLoadProjectiles` (CrossbowItem.java:91-99): draw one ammo
    /// item into the `CHARGED_PROJECTILES` component. Returns whether anything was loaded;
    /// on success it also plays the loading-end sound exactly as `onUseTick` does
    /// (CrossbowItem.java:222-236).
    async fn try_load_projectiles(player: &Player) -> bool {
        let arrow_slot = player.find_crossbow_projectile().await;
        let (arrow_nbt_wrapper, slot) = {
            if let Some(slot) = arrow_slot {
                let inventory = player.inventory();

                let arrow_stack = inventory.get_stack(slot).await;
                let mut arrow_nbt = pumpkin_nbt::compound::NbtCompound::new();
                arrow_stack
                    .copy_with_count(1)
                    .write_item_stack(&mut arrow_nbt);
                (Some(arrow_nbt), slot)
            } else if player.gamemode.load() == GameMode::Creative {
                let mut arrow_nbt = pumpkin_nbt::compound::NbtCompound::new();
                let arrow_stack = ItemStack::new(1, &Item::ARROW);
                arrow_stack.write_item_stack(&mut arrow_nbt);

                (Some(arrow_nbt), 0)
            } else {
                (None, 0)
            }
        };
        let Some(arrow_nbt) = arrow_nbt_wrapper else {
            return false;
        };

        let mut stack = player.inventory().held_item().await;
        stack.patch.push((
            DataComponent::ChargedProjectiles,
            Some(Box::new(ChargedProjectilesImpl {
                projectiles: vec![arrow_nbt],
            })),
        ));
        let updated_stack = stack.clone();
        player.inventory().set_held_item(stack).await;

        if player.gamemode.load() != GameMode::Creative {
            player.consume_arrow(slot).await;
        }

        player
            .sync_hand_slot(player.inventory.get_selected_slot() as usize, updated_stack)
            .await;

        // Vanilla `CrossbowItem#onUseTick`: volume 1.0, pitch
        // 1.0F / (random.nextFloat() * 0.5F + 1.0F) + 0.2F.
        player.world().play_sound_fine(
            Sound::ItemCrossbowLoadingEnd,
            SoundCategory::Players,
            &player.position(),
            1.0,
            1.0 / rand::random::<f32>().mul_add(0.5, 1.0) + 0.2,
        );

        true
    }

    async fn fire_projectiles(player: &Player) {
        let mut held = player.inventory().held_item().await;
        let projectiles = held.get_data_component::<ChargedProjectilesImpl>().cloned();
        let has_multishot =
            held.get_data_component::<EnchantmentsImpl>()
                .is_some_and(|enchantments| {
                    enchantments
                        .enchantment
                        .iter()
                        .any(|(e, _)| **e == pumpkin_data::Enchantment::MULTISHOT)
                });
        // `AbstractArrow` ctor (AbstractArrow.java:111-114): an arrow fired from a weapon takes
        // its pierce level from `EnchantmentHelper.getPiercingCount(weapon, ammo)`, i.e.
        // piercing.json's `minecraft:projectile_piercing` -> linear(1, 1).
        let pierce_level = piercing_count(held.get_enchantment_level(&Enchantment::PIERCING));

        if let Some(charged) = projectiles {
            let world = player.world();
            let mut shot_index = 0u32;
            // Vanilla `ProjectileWeaponItem#shoot`: `weapon.hurtAndBreak(getDurabilityUse(
            // projectile), ...)` fires once per projectile *entry* in the charged list, not
            // once per multishot-fired arrow. Pumpkin only ever stores one entry per load
            // (multishot's spread is applied at fire time below, not by storing multiple
            // copies at draw time), so this is equivalent to "once per use" here.
            let mut total_durability_use = 0;

            for projectile_nbt in charged.projectiles {
                let Some(projectile) = ItemStack::read_item_stack(&projectile_nbt) else {
                    continue;
                };
                let is_firework = projectile.item == &Item::FIREWORK_ROCKET;
                let power = projectile_power(projectile.item);
                total_durability_use += durability_use(projectile.item);

                let angles = if has_multishot {
                    vec![-10.0, 0.0, 10.0]
                } else {
                    vec![0.0]
                };

                for angle in angles {
                    let (shot_pitch, shot_yaw) = shot_rotation(player.get_entity(), angle);
                    if is_firework {
                        let rocket_entity = Entity::new(
                            world.clone(),
                            player.position(),
                            &EntityType::FIREWORK_ROCKET,
                        );
                        let rocket = FireworkRocketEntity::new_crossbow_shot(
                            rocket_entity,
                            player.get_entity(),
                            &projectile,
                        );
                        rocket.set_shot_velocity(
                            player.get_entity(),
                            shot_pitch,
                            shot_yaw,
                            0.0,
                            power,
                            1.0,
                        );
                        let rocket_arc: Arc<dyn EntityBase> = Arc::new(rocket);
                        world.spawn_entity(rocket_arc).await;
                    } else {
                        let arrow_entity = Entity::new(
                            world.clone(),
                            player.position(),
                            ArrowEntity::entity_type_for_item(projectile.item),
                        );
                        let pickup = if player.gamemode.load() == GameMode::Creative {
                            ArrowPickup::CreativeOnly
                        } else {
                            ArrowPickup::Allowed
                        };

                        let arrow = ArrowEntity::new_shot(
                            arrow_entity,
                            player.get_entity(),
                            &projectile,
                            pickup,
                        );
                        arrow.set_velocity_from_rotation(shot_pitch, shot_yaw, 0.0, power, 1.0);
                        if pierce_level > 0 {
                            arrow.set_pierce_level(pierce_level);
                        }
                        let arrow_arc: Arc<dyn EntityBase> = Arc::new(arrow);
                        world.spawn_entity(arrow_arc).await;
                    }

                    // Vanilla `CrossbowItem#shootProjectile` plays CROSSBOW_SHOOT once per fired
                    // projectile at volume 1.0 with `getShotPitch(random, index)`.
                    world.play_sound_fine(
                        Sound::ItemCrossbowShoot,
                        SoundCategory::Players,
                        &player.position(),
                        1.0,
                        crossbow_shot_pitch(shot_index, rand::random()),
                    );
                    shot_index += 1;
                }
            }

            held.patch
                .retain(|(id, _)| *id != DataComponent::ChargedProjectiles);
            player.damage_held_item(total_durability_use).await;
            player.inventory().set_held_item(held).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{crossbow_shot_pitch, piercing_count, rotate_shot_vector};
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn piercing_count_is_one_per_level() {
        // piercing.json: linear(base = 1.0, per_level_above_first = 1.0), max_level 4.
        assert_eq!(piercing_count(0), 0);
        assert_eq!(piercing_count(1), 1);
        assert_eq!(piercing_count(2), 2);
        assert_eq!(piercing_count(3), 3);
        assert_eq!(piercing_count(4), 4);
    }

    #[test]
    fn crossbow_shot_pitch_matches_vanilla_shot_indices() {
        assert!((crossbow_shot_pitch(0, 0.5) - 1.0).abs() < 1e-6);
        assert!((crossbow_shot_pitch(1, 0.0) - (1.0 / 1.8 + 0.63)).abs() < 1e-6);
        assert!((crossbow_shot_pitch(2, 0.0) - (1.0 / 1.8 + 0.43)).abs() < 1e-6);
    }

    #[test]
    fn crossbow_spread_rotates_around_up_vector() {
        // Vanilla `CrossbowItem#shootProjectile` rotates the view vector around the up vector
        // (`CrossbowItem.java:107-130`).
        let view = Vector3::new(0.0, 0.0, 1.0);
        let up = Vector3::new(0.0, 1.0, 0.0);
        let rotated = rotate_shot_vector(view, up, 90.0);
        assert!((rotated.x - 1.0).abs() < 1e-12);
        assert!(rotated.y.abs() < 1e-12);
        assert!(rotated.z.abs() < 1e-12);
    }
}
