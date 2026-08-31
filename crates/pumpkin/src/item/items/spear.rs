use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::GameMode;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;
use tokio::sync::Mutex;

use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};

/// `Item.getUseDuration` (`Item.java:315`): an item carrying `KINETIC_WEAPON` (or
/// `BLOCKS_ATTACKS`) is usable for 72000 ticks, i.e. effectively until released.
const USE_DURATION: i32 = 72000;

/// `KineticWeapon.contactCooldownTicks`, fixed at `10` for every spear by
/// `Item.Properties.spear` (`Item.java:505`). A given target can only be stabbed once per
/// this many ticks by the same attacker.
const CONTACT_COOLDOWN_TICKS: u64 = 10;

/// `KineticWeapon.getMotion` (`KineticWeapon.java:78-84`) converts the per-tick known speed
/// into blocks per second before any threshold comparison.
const TICKS_PER_SECOND: f64 = 20.0;

/// `KineticWeapon.damageEntities` (`KineticWeapon.java:107`): a player contributes a factor of
/// `1.0` to every condition test, where a mob contributes `0.2`.
const PLAYER_ACTION_FACTOR: f64 = 1.0;

/// `LivingEntity.stabAttack` (`LivingEntity.java:2908`) always applies this base knockback
/// before the attacker's own `ATTACK_KNOCKBACK` contribution.
const BASE_STAB_KNOCKBACK: f64 = 0.4;

/// One `KineticWeapon.Condition` (`KineticWeapon.java:148-169`): the stab effect it guards
/// applies only while `ticks_used <= max_duration_ticks`, the attacker's own forward speed is
/// at least `min_speed * factor`, and the closing speed is at least
/// `min_relative_speed * factor`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Condition {
    max_duration_ticks: i32,
    min_speed: f64,
    min_relative_speed: f64,
}

impl Condition {
    /// `KineticWeapon.Condition.ofAttackerSpeed` (`KineticWeapon.java:171-173`).
    const fn of_attacker_speed(until_ticks: i32, min_attacker_speed: f64) -> Self {
        Self {
            max_duration_ticks: until_ticks,
            min_speed: min_attacker_speed,
            min_relative_speed: 0.0,
        }
    }

    /// `KineticWeapon.Condition.ofRelativeSpeed` (`KineticWeapon.java:175-177`).
    const fn of_relative_speed(until_ticks: i32, min_relative_speed: f64) -> Self {
        Self {
            max_duration_ticks: until_ticks,
            min_speed: 0.0,
            min_relative_speed,
        }
    }

    /// `KineticWeapon.Condition.test` (`KineticWeapon.java:167-169`).
    fn test(self, ticks_used: i32, attacker_speed: f64, relative_speed: f64, factor: f64) -> bool {
        ticks_used <= self.max_duration_ticks
            && attacker_speed >= self.min_speed * factor
            && relative_speed >= self.min_relative_speed * factor
    }
}

/// The `minecraft:kinetic_weapon` component values a spear carries.
///
/// Vanilla stores these on the stack, built by `Item.Properties.spear` (`Item.java:486-530`)
/// from the per-material argument list at `Items.java:1659-1679`. Pumpkin's generated
/// `KineticWeaponImpl` (`crates/pumpkin-data/src/data_component_impl/combat.rs:692`) is a
/// fieldless marker that carries none of them, so the table is reproduced here rather than
/// read off the stack. `spear_use.rs` takes the same approach for the mob-side goal.
#[derive(Clone, Copy, Debug)]
struct SpearParams {
    /// `(int)(delay * 20.0F)` (`Item.java:506`): ticks of windup before any stab can land.
    delay_ticks: i32,
    dismount: Condition,
    knockback: Condition,
    damage: Condition,
    /// `damageMultiplier` (`Item.java:512`), applied to the closing speed.
    damage_multiplier: f64,
    /// `SPEAR_WOOD_USE` for wood, `SPEAR_USE` otherwise (`Item.java:513`).
    use_sound: Sound,
    /// `SPEAR_WOOD_HIT` for wood, `SPEAR_HIT` otherwise (`Item.java:514`).
    hit_sound: Sound,
}

/// Builds the component the way `Item.Properties.spear` does, from the raw per-material
/// arguments so each row below reads in the same order as its `Items.java` call site:
/// `(damageMultiplier, delay, dismountTime, dismountThreshold, knockbackTime,
/// knockbackThreshold, damageTime, damageThreshold)`. `attackDuration` is only used for the
/// `SwingAnimation` component, which is client-side presentation, so it is not carried here.
#[expect(clippy::too_many_arguments)]
const fn spear(
    is_wood: bool,
    damage_multiplier: f64,
    delay: f64,
    dismount_time: f64,
    dismount_threshold: f64,
    knockback_time: f64,
    knockback_threshold: f64,
    damage_time: f64,
    damage_threshold: f64,
) -> SpearParams {
    SpearParams {
        delay_ticks: (delay * TICKS_PER_SECOND) as i32,
        dismount: Condition::of_attacker_speed(
            (dismount_time * TICKS_PER_SECOND) as i32,
            dismount_threshold,
        ),
        knockback: Condition::of_attacker_speed(
            (knockback_time * TICKS_PER_SECOND) as i32,
            knockback_threshold,
        ),
        damage: Condition::of_relative_speed(
            (damage_time * TICKS_PER_SECOND) as i32,
            damage_threshold,
        ),
        damage_multiplier,
        use_sound: if is_wood {
            Sound::ItemSpearWoodUse
        } else {
            Sound::ItemSpearUse
        },
        hit_sound: if is_wood {
            Sound::ItemSpearWoodHit
        } else {
            Sound::ItemSpearHit
        },
    }
}

/// Per-material spear parameters, transcribed from `Items.java:1659-1679`.
const fn params_for(item_id: u16) -> Option<SpearParams> {
    // `Items.java:1659-1661`
    if item_id == Item::WOODEN_SPEAR.id {
        return Some(spear(true, 0.7, 0.75, 5.0, 14.0, 10.0, 5.1, 15.0, 4.6));
    }
    // `Items.java:1662-1664`
    if item_id == Item::STONE_SPEAR.id {
        return Some(spear(false, 0.82, 0.7, 4.5, 13.0, 9.0, 5.1, 13.75, 4.6));
    }
    // `Items.java:1665-1667`
    if item_id == Item::COPPER_SPEAR.id {
        return Some(spear(false, 0.82, 0.65, 4.0, 12.0, 8.25, 5.1, 12.5, 4.6));
    }
    // `Items.java:1668-1670`
    if item_id == Item::IRON_SPEAR.id {
        return Some(spear(false, 0.95, 0.6, 2.5, 11.0, 6.75, 5.1, 11.25, 4.6));
    }
    // `Items.java:1671-1673`
    if item_id == Item::GOLDEN_SPEAR.id {
        return Some(spear(false, 0.7, 0.7, 3.5, 13.0, 8.5, 5.1, 13.75, 4.6));
    }
    // `Items.java:1674-1676`
    if item_id == Item::DIAMOND_SPEAR.id {
        return Some(spear(false, 1.075, 0.5, 3.0, 10.0, 6.5, 5.1, 10.0, 4.6));
    }
    // `Items.java:1677-1679`
    if item_id == Item::NETHERITE_SPEAR.id {
        return Some(spear(false, 1.2, 0.4, 2.5, 9.0, 5.5, 5.1, 8.75, 4.6));
    }
    None
}

/// Segment-versus-AABB overlap by the slab method, used in place of vanilla's
/// `AABB.clip` (`ProjectileUtil.getManyEntityHitResult` -> `AABB.clip`). Only whether the
/// segment touches the box matters here: `KineticWeapon.damageEntities` pierces every entity
/// along the sweep, so the hit *position* vanilla computes is never read.
fn segment_intersects_box(from: Vector3<f64>, to: Vector3<f64>, aabb: &BoundingBox) -> bool {
    let delta = to - from;
    let mut t_min = 0.0f64;
    let mut t_max = 1.0f64;

    for axis in 0..3 {
        let (origin, direction, low, high) = match axis {
            0 => (from.x, delta.x, aabb.min.x, aabb.max.x),
            1 => (from.y, delta.y, aabb.min.y, aabb.max.y),
            _ => (from.z, delta.z, aabb.min.z, aabb.max.z),
        };

        if direction.abs() < 1.0e-7 {
            // Parallel to this slab: a miss unless the segment already lies inside it.
            if origin < low || origin > high {
                return false;
            }
            continue;
        }

        let t1 = (low - origin) / direction;
        let t2 = (high - origin) / direction;
        let (near, far) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
        t_min = t_min.max(near);
        t_max = t_max.min(far);
        if t_min > t_max {
            return false;
        }
    }

    true
}

/// `LivingEntity.recentKineticEnemies` (`LivingEntity.java:274`), which vanilla stores on the
/// attacker and resets in `startUsingItem` (`LivingEntity.java:3506-3508`).
///
/// Pumpkin's `LivingEntity` has no such field, so the map lives on the shared item behaviour,
/// keyed by `(attacker entity id, target entity id)`. Vanilla drops the whole map when a new
/// use begins; here entries simply expire, which is equivalent for the only thing it is read
/// for (`wasRecentlyStabbed`, `LivingEntity.java:2871-2877`), since an entry older than the
/// contact cooldown never blocks a stab. Expired entries are pruned on every access so the
/// map stays bounded by the number of targets currently being stabbed.
#[derive(Default)]
pub struct SpearItem {
    recent_stabs: Mutex<HashMap<(i32, i32), u64>>,
}

impl SpearItem {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `LivingEntity.wasRecentlyStabbed` + `rememberStabbedEntity`
    /// (`LivingEntity.java:2871-2883`) in one step: returns whether the target is still on
    /// cooldown, and records this tick as the latest stab when it is not.
    async fn was_recently_stabbed_then_remember(
        &self,
        attacker_id: i32,
        target_id: i32,
        game_time: u64,
    ) -> bool {
        let mut map = self.recent_stabs.lock().await;
        map.retain(|_, last| game_time.saturating_sub(*last) < CONTACT_COOLDOWN_TICKS);

        if let Some(last) = map.get(&(attacker_id, target_id))
            && game_time.saturating_sub(*last) < CONTACT_COOLDOWN_TICKS
        {
            return true;
        }

        map.insert((attacker_id, target_id), game_time);
        false
    }
}

impl ItemMetadata for SpearItem {
    fn ids() -> Box<[u16]> {
        [
            Item::WOODEN_SPEAR.id,
            Item::STONE_SPEAR.id,
            Item::COPPER_SPEAR.id,
            Item::IRON_SPEAR.id,
            Item::GOLDEN_SPEAR.id,
            Item::DIAMOND_SPEAR.id,
            Item::NETHERITE_SPEAR.id,
        ]
        .into()
    }
}

impl ItemBehaviour for SpearItem {
    /// `Item.use` (`Item.java:202-207`): a stack carrying `KINETIC_WEAPON` starts being used
    /// and plays its use sound. There is no throw and no charged release - a spear is a braced
    /// lance, and all of its effect is produced per-tick while held (`ItemStack.onUseTick`,
    /// `ItemStack.java:1100-1103`).
    fn normal_use<'a>(
        &'a self,
        item: &'a Item,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let Some(params) = params_for(item.id) else {
                return;
            };

            let stack = player.inventory().held_item().await;
            player
                .living_entity
                .set_active_hand(pumpkin_util::Hand::Right, stack, USE_DURATION)
                .await;

            // `KineticWeapon.makeSound` (`KineticWeapon.java:86-91`) plays at the user, for
            // everyone, at volume and pitch 1.0.
            player
                .world()
                .play_sound(params.use_sound, SoundCategory::Players, &player.position());
        })
    }

    /// `ItemStack.onUseTick` -> `KineticWeapon.damageEntities`
    /// (`ItemStack.java:1100-1103`, `KineticWeapon.java:101-146`).
    #[expect(clippy::too_many_lines)]
    fn on_use_tick<'a>(
        &'a self,
        stack: &'a ItemStack,
        player: &'a Player,
        remaining_use_ticks: i32,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let Some(params) = params_for(stack.item.id) else {
                return;
            };

            // `KineticWeapon.java:102-104`.
            let ticks_used = USE_DURATION - remaining_use_ticks;
            if ticks_used < params.delay_ticks {
                return;
            }
            let ticks_used = ticks_used - params.delay_ticks;

            let world = player.world();
            let entity = player.get_entity();
            // `ProjectileUtil.getHitEntitiesAlong` uses the attacker's head look angle
            // (`ProjectileUtil.java:38-45`), including for the kinetic-weapon sweep.
            let look = player.get_head_look_angle();

            // `KineticWeapon.getMotion` (`KineticWeapon.java:78-84`): the attacker's known
            // speed in blocks per second, projected onto the look vector
            // (`KineticWeapon.java:105-106`). `Entity.getKnownSpeed` is the per-tick position
            // delta (`Entity.java:4007-4009`).
            let attacker_speed = look.dot(&(entity.get_known_speed() * TICKS_PER_SECOND));

            // `AttackRange.effectiveMinRange`/`effectiveMaxRange`
            // (`AttackRange.java:88-102`): a player uses the creative pair in creative mode
            // and the survival pair otherwise, with no `mobFactor` applied.
            // `LivingEntity.getAttackRangeWith` supplies the component or its interaction-range
            // default (`LivingEntity.java:2230-2233`; `AttackRange.java:55-59`).
            let attack_range = player.living_entity.get_attack_range_with(stack);
            let creative = player.gamemode.load() == GameMode::Creative;
            let (min_reach, max_reach, margin) = if creative {
                (
                    f64::from(attack_range.min_creative_reach),
                    f64::from(attack_range.max_creative_reach),
                    f64::from(attack_range.hitbox_margin),
                )
            } else {
                (
                    f64::from(attack_range.min_reach),
                    f64::from(attack_range.max_reach),
                    f64::from(attack_range.hitbox_margin),
                )
            };

            // `ProjectileUtil.getHitEntitiesAlong` (`ProjectileUtil.java:38-47`): the sweep
            // runs from `eye + look * minRange` to `eye + look * (maxRange + max(0, movement
            // projected on look))`, so a moving attacker reaches further ahead.
            let eye = player.eye_position();
            let movement_component = look.dot(&entity.get_known_speed()).max(0.0);
            let from = eye + look * min_reach;
            let mut to = eye + look * (max_reach + movement_component);

            // `ProjectileUtil.java:96-102`: the sweep is clipped at the first solid block, and
            // is abandoned entirely when that block is nearer than the sweep's own start.
            if let Some((hit_pos, _)) = world
                .raycast(eye, to, async |pos, world_inner| {
                    !world_inner.get_block_state(pos).is_air()
                })
                .await
            {
                let hit = Vector3::new(
                    f64::from(hit_pos.0.x) + 0.5,
                    f64::from(hit_pos.0.y) + 0.5,
                    f64::from(hit_pos.0.z) + 0.5,
                );
                if eye.squared_distance_to_vec(&hit) < eye.squared_distance_to_vec(&from) {
                    return;
                }
                to = hit;
            }

            // `KineticWeapon.java:109`: the *base* attribute value, deliberately excluding the
            // weapon's own attack-damage modifier, which the spear's kinetic damage replaces.
            let base_damage = player
                .living_entity
                .get_attribute_base(&Attributes::ATTACK_DAMAGE);

            // `ProjectileUtil.java:104`: search box around the whole sweep, inflated by 1.0.
            let search = BoundingBox {
                min: Vector3::new(
                    from.x.min(to.x) - margin,
                    from.y.min(to.y) - margin,
                    from.z.min(to.z) - margin,
                ),
                max: Vector3::new(
                    from.x.max(to.x) + margin,
                    from.y.max(to.y) + margin,
                    from.z.max(to.z) + margin,
                ),
            }
            .expand_all(1.0);

            let attacker_id = entity.entity_id;
            let attacker_root_id = entity.root_vehicle_id().await;
            let mut candidates: Vec<Arc<dyn EntityBase>> = Vec::new();
            world.extend_entities_in_box_where(&mut candidates, 64, search, |candidate| {
                // `PiercingWeapon.canHitEntity` (`PiercingWeapon.java:61-73`), reduced to the
                // parts that can be checked without awaiting: never the attacker, a dead entity,
                // or an entity invulnerable to piercing weapons.
                candidate.get_entity().entity_id != attacker_id
                    && candidate.get_entity().is_alive()
                    && !candidate.is_invulnerable_to_piercing_weapon()
            });

            let game_time = world.level_time.lock().await.world_age.unsigned_abs();
            let mut affected = false;

            for target in candidates {
                let target_entity = target.get_entity();
                // `PiercingWeapon.canHitEntity` (`PiercingWeapon.java:71-73`) excludes a target
                // sharing the attacker's root vehicle, including nested passengers.
                if target_entity.root_vehicle_id().await == attacker_root_id {
                    continue;
                }
                // Vanilla widens each candidate's box by its pick radius and clips the
                // segment against it; the hitbox margin plays that role here.
                let hitbox = target_entity.bounding_box.load().expand_all(margin);
                if !segment_intersects_box(from, to, &hitbox) {
                    continue;
                }

                // `KineticWeapon.java:121-123`.
                if self
                    .was_recently_stabbed_then_remember(
                        attacker_id,
                        target_entity.entity_id,
                        game_time,
                    )
                    .await
                {
                    continue;
                }

                // `KineticWeapon.java:124-125`: closing speed, never negative.
                let target_speed = look.dot(&(target_entity.get_known_speed() * TICKS_PER_SECOND));
                let relative_speed = (attacker_speed - target_speed).max(0.0);

                // `KineticWeapon.java:126-131`.
                let deals_dismount = params.dismount.test(
                    ticks_used,
                    attacker_speed,
                    relative_speed,
                    PLAYER_ACTION_FACTOR,
                );
                let deals_knockback = params.knockback.test(
                    ticks_used,
                    attacker_speed,
                    relative_speed,
                    PLAYER_ACTION_FACTOR,
                );
                let deals_damage = params.damage.test(
                    ticks_used,
                    attacker_speed,
                    relative_speed,
                    PLAYER_ACTION_FACTOR,
                );

                if !(deals_dismount || deals_knockback || deals_damage) {
                    continue;
                }

                // `KineticWeapon.java:133`: the closing speed, not the attacker's own speed,
                // is what the multiplier scales, and it is floored before being added.
                let damage_dealt =
                    base_damage + (relative_speed * params.damage_multiplier).floor();

                // `LivingEntity.stabAttack` (`LivingEntity.java:2889-2930`). A dismount is a
                // server-side state change, so route it through the vehicle's existing
                // passenger-removal path just as `target.stopRiding()` does in vanilla
                // (`LivingEntity.java:2912-2915`).
                if deals_dismount {
                    let vehicle = target_entity.vehicle.lock().await.clone();
                    if let Some(vehicle) = vehicle {
                        vehicle
                            .get_entity()
                            .remove_passenger(target_entity.entity_id)
                            .await;
                        affected = true;
                    }
                }

                if deals_damage {
                    let attacker = world.get_entity_by_id(attacker_id);
                    let dealt = target
                        .damage_with_context(
                            target.as_ref(),
                            damage_dealt as f32,
                            DamageType::SPEAR,
                            Some(entity.pos.load()),
                            attacker.as_deref(),
                            attacker.as_deref(),
                        )
                        .await;
                    affected |= dealt;
                }

                if deals_knockback {
                    // `LivingEntity.causeExtraKnockback` (`LivingEntity.java:2734-2748`) is
                    // called twice by `stabAttack`: once at a flat 0.4, once at the
                    // attacker's own `ATTACK_KNOCKBACK`, both directed along the attacker's
                    // facing (`sin(yaw)`, `-cos(yaw)`).
                    let yaw_rad = f64::from(player.get_entity().yaw.load().to_radians());
                    let (dir_x, dir_z) = (yaw_rad.sin(), -yaw_rad.cos());
                    if let Some(living) = target.get_living_entity() {
                        living.knockback_with_resistance(BASE_STAB_KNOCKBACK, dir_x, dir_z);
                        let extra = player
                            .living_entity
                            .get_attribute_value(&Attributes::ATTACK_KNOCKBACK);
                        if extra > 0.0 {
                            living.knockback_with_resistance(extra, dir_x, dir_z);
                        }
                    }
                    affected = true;
                }

                // `LivingEntity.stabAttack` (`LivingEntity.java:2917-2919`):
                // `weaponItem.hurtEnemy(livingTarget, this)` runs for every *living* target the
                // stab reached, whether or not the damage branch landed. Spears carry
                // `Weapon { item_damage_per_attack: 1 }`, so each such target costs one point
                // of durability.
                if target.get_living_entity().is_some()
                    && player.gamemode.load() != GameMode::Creative
                {
                    player.damage_held_item(1).await;
                }
            }

            // `LivingEntity.onKineticHit` (`LivingEntity.java:2156-2164`) plays the hit sound,
            // rate-limited to `KineticWeapon.HIT_FEEDBACK_TICKS`. That limiter lives on the
            // entity in vanilla; the contact cooldown above is the same 10 ticks and already
            // gates every path that reaches here, so the sound cannot repeat faster either.
            if affected {
                world.play_sound(params.hit_sound, SoundCategory::Players, &player.position());
            }
        })
    }

    /// `Item.getUseDuration` (`Item.java:310-316`).
    fn get_use_duration(&self) -> i32 {
        USE_DURATION
    }

    fn can_mine(&self, player: &Player) -> bool {
        player.gamemode.load() != GameMode::Creative
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{Condition, SpearItem, params_for, segment_intersects_box};
    use pumpkin_data::item::Item;
    use pumpkin_util::math::boundingbox::BoundingBox;
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn every_spear_has_parameters_and_nothing_else_does() {
        for item in [
            &Item::WOODEN_SPEAR,
            &Item::STONE_SPEAR,
            &Item::COPPER_SPEAR,
            &Item::IRON_SPEAR,
            &Item::GOLDEN_SPEAR,
            &Item::DIAMOND_SPEAR,
            &Item::NETHERITE_SPEAR,
        ] {
            assert!(params_for(item.id).is_some(), "{}", item.registry_key);
        }
        assert!(params_for(Item::TRIDENT.id).is_none());
        assert!(params_for(Item::IRON_SWORD.id).is_none());
    }

    #[test]
    fn wooden_spear_matches_items_java() {
        // `Items.java:1659-1661`: spear(WOOD, 0.65, 0.7, 0.75, 5.0, 14.0, 10.0, 5.1, 15.0, 4.6)
        // run through `Item.Properties.spear` (`Item.java:504-512`).
        let params = params_for(Item::WOODEN_SPEAR.id).unwrap();
        assert_eq!(params.delay_ticks, 15);
        assert_eq!(params.dismount, Condition::of_attacker_speed(100, 14.0));
        assert_eq!(params.knockback, Condition::of_attacker_speed(200, 5.1));
        assert_eq!(params.damage, Condition::of_relative_speed(300, 4.6));
        assert!((params.damage_multiplier - 0.7).abs() < 1.0e-9);
    }

    #[test]
    fn netherite_spear_matches_items_java() {
        // `Items.java:1677-1679`.
        let params = params_for(Item::NETHERITE_SPEAR.id).unwrap();
        assert_eq!(params.delay_ticks, 8);
        assert_eq!(params.dismount, Condition::of_attacker_speed(50, 9.0));
        assert_eq!(params.knockback, Condition::of_attacker_speed(110, 5.1));
        assert_eq!(params.damage, Condition::of_relative_speed(175, 4.6));
        assert!((params.damage_multiplier - 1.2).abs() < 1.0e-9);
    }

    #[test]
    fn condition_requires_window_and_both_speeds() {
        // `KineticWeapon.Condition.test` (`KineticWeapon.java:167-169`).
        let condition = Condition {
            max_duration_ticks: 100,
            min_speed: 5.0,
            min_relative_speed: 2.0,
        };
        assert!(condition.test(100, 5.0, 2.0, 1.0));
        // Past the window.
        assert!(!condition.test(101, 50.0, 50.0, 1.0));
        // Attacker too slow.
        assert!(!condition.test(10, 4.9, 50.0, 1.0));
        // Closing speed too low.
        assert!(!condition.test(10, 50.0, 1.9, 1.0));
    }

    #[test]
    fn condition_thresholds_scale_by_the_action_factor() {
        // A mob contributes 0.2 (`KineticWeapon.java:107`), so it clears a threshold a player
        // would not.
        let condition = Condition::of_attacker_speed(100, 5.0);
        assert!(!condition.test(10, 1.5, 0.0, 1.0));
        assert!(condition.test(10, 1.5, 0.0, 0.2));
    }

    #[test]
    fn segment_hits_a_box_it_passes_through() {
        let aabb = BoundingBox {
            min: Vector3::new(2.0, -0.5, -0.5),
            max: Vector3::new(3.0, 0.5, 0.5),
        };
        assert!(segment_intersects_box(
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(5.0, 0.0, 0.0),
            &aabb
        ));
    }

    #[test]
    fn segment_stops_short_of_a_box_beyond_its_end() {
        let aabb = BoundingBox {
            min: Vector3::new(6.0, -0.5, -0.5),
            max: Vector3::new(7.0, 0.5, 0.5),
        };
        assert!(!segment_intersects_box(
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(5.0, 0.0, 0.0),
            &aabb
        ));
    }

    #[test]
    fn segment_misses_a_box_off_to_the_side() {
        let aabb = BoundingBox {
            min: Vector3::new(2.0, 4.0, -0.5),
            max: Vector3::new(3.0, 5.0, 0.5),
        };
        assert!(!segment_intersects_box(
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(5.0, 0.0, 0.0),
            &aabb
        ));
    }

    #[test]
    fn segment_starting_inside_a_box_hits_it() {
        let aabb = BoundingBox {
            min: Vector3::new(-1.0, -1.0, -1.0),
            max: Vector3::new(1.0, 1.0, 1.0),
        };
        assert!(segment_intersects_box(
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(5.0, 0.0, 0.0),
            &aabb
        ));
    }

    #[tokio::test]
    async fn contact_cooldown_blocks_a_repeat_stab_then_expires() {
        // `LivingEntity.wasRecentlyStabbed` (`LivingEntity.java:2871-2877`) with
        // `contactCooldownTicks = 10` (`Item.java:505`).
        let item = SpearItem::new();
        assert!(!item.was_recently_stabbed_then_remember(1, 2, 100).await);
        assert!(item.was_recently_stabbed_then_remember(1, 2, 105).await);
        assert!(item.was_recently_stabbed_then_remember(1, 2, 109).await);
        // Exactly `allowedTime` later is no longer "recent": the check is `< allowedTime`.
        assert!(!item.was_recently_stabbed_then_remember(1, 2, 110).await);
    }

    #[tokio::test]
    async fn contact_cooldown_is_tracked_per_target() {
        let item = SpearItem::new();
        assert!(!item.was_recently_stabbed_then_remember(1, 2, 100).await);
        // A different target is unaffected by the first one's cooldown.
        assert!(!item.was_recently_stabbed_then_remember(1, 3, 100).await);
        // As is a different attacker.
        assert!(!item.was_recently_stabbed_then_remember(9, 2, 100).await);
    }
}
