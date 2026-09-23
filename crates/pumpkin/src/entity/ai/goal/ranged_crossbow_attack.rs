use std::sync::Arc;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::Hand;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ai::goal::{Controls, Goal, GoalFuture};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::projectile::arrow::{ArrowEntity, ArrowPickup};
use crate::entity::{Entity, EntityBase};

/// Vanilla: `RangedCrossbowAttackGoal.CrossbowState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrossbowState {
    Uncharged,
    Charging,
    Charged,
    ReadyToAttack,
}

/// Vanilla: `CrossbowItem.getChargeDuration` -- `Mth.floor(1.25F * 20.0F)` with no Quick Charge
/// enchant applied.
const CHARGE_DURATION_TICKS: i32 = 25;
/// `CrossbowItem.onUseTick`: the loading-start sound plays once charge progress reaches 0.2
/// and the loading-middle sound once it reaches 0.5 (`ceil(0.5 * 25) = 13`).
const LOADING_START_TICKS: i32 = 5;
const LOADING_MIDDLE_TICKS: i32 = 13;
/// `Pillager.performRangedAttack` -> `performCrossbowAttack(this, 1.6F)`.
const CROSSBOW_POWER: f64 = 1.6;

/// Ranged crossbow attack used by Pillagers and Piglins.
///
/// Vanilla source: `net/minecraft/world/entity/ai/goal/RangedCrossbowAttackGoal.java`.
pub struct RangedCrossbowAttackGoal {
    state: CrossbowState,
    speed_modifier: f64,
    attack_radius_sqr: f64,
    see_time: i32,
    attack_delay: i32,
    update_path_delay: i32,
    /// `Mob.getTicksUsingItem()` for the crossbow being charged. Tracked here since only this
    /// goal starts the mob's crossbow use.
    charge_ticks: i32,
}

impl RangedCrossbowAttackGoal {
    /// `RangedCrossbowAttackGoal(mob, speedModifier, attackRadius)`.
    #[must_use]
    pub fn new(speed_modifier: f64, attack_radius: f32) -> Self {
        Self {
            state: CrossbowState::Uncharged,
            speed_modifier,
            attack_radius_sqr: f64::from(attack_radius * attack_radius),
            see_time: 0,
            attack_delay: 0,
            update_path_delay: 0,
            charge_ticks: 0,
        }
    }

    /// `ProjectileUtil.getWeaponHoldingHand(mob, Items.CROSSBOW)`: the main hand if it holds a
    /// crossbow, otherwise the off hand. `None` when neither hand does (`Mob.isHolding`).
    async fn crossbow_hand(mob: &dyn Mob) -> Option<(Hand, ItemStack)> {
        let equipment = mob
            .get_mob_entity()
            .living_entity
            .entity_equipment
            .lock()
            .await;
        let main = equipment.get(&EquipmentSlot::MAIN_HAND);
        if main.item.id == Item::CROSSBOW.id {
            return Some((Hand::Right, main));
        }
        let off = equipment.get(&EquipmentSlot::OFF_HAND);
        (off.item.id == Item::CROSSBOW.id).then_some((Hand::Left, off))
    }

    /// `isValidTarget() && isHoldingCrossbow()`.
    async fn can_use(mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .target
            .lock()
            .await
            .as_ref()
            .is_some_and(|target| target.get_entity().is_alive())
            && Self::crossbow_hand(mob).await.is_some()
    }

    fn play_sound(mob: &dyn Mob, sound: Sound) {
        let entity = mob.get_entity();
        entity
            .world
            .load()
            .play_sound(sound, SoundCategory::Hostile, &entity.pos.load());
    }

    /// `CrossbowAttackMob.performCrossbowAttack(body, 1.6F)` -> `CrossbowItem.performShooting`.
    async fn shoot(mob: &dyn Mob, target: &dyn EntityBase) {
        let shooter = mob.get_entity();
        let world = shooter.world.load_full();

        let mut event =
            crate::plugin::api::events::entity::entity_shoot_bow::EntityShootBowEvent::new(
                shooter.entity_id,
                "minecraft:crossbow".to_string(),
                CROSSBOW_POWER as f32,
            );
        if let Some(server) = world.server.upgrade() {
            server.plugin_manager.fire(&server, &mut event).await;
        }
        if event.cancelled {
            return;
        }

        let arrow_entity = Entity::new(world.clone(), shooter.pos.load(), &EntityType::ARROW);
        let arrow_item = ItemStack::new(1, &Item::ARROW);
        // A mob-owned arrow is never pickup-able (`AbstractArrow` only allows pickup for a
        // player owner).
        let arrow =
            ArrowEntity::new_shot(arrow_entity, shooter, &arrow_item, ArrowPickup::Disallowed);
        // `CrossbowItem.createProjectile` (`CrossbowItem.java:158-161`).
        arrow.set_sound_event(Sound::ItemCrossbowHit);

        // `CrossbowAttackMob.getProjectileShotVector` inputs: horizontal delta between the
        // bodies, and `target.getY(1/3) - projectile.getY()` plus 20% of the horizontal distance.
        let shooter_pos = shooter.pos.load();
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let dx = target_pos.x - shooter_pos.x;
        let dz = target_pos.z - shooter_pos.z;
        let horizontal = dx.hypot(dz);
        let dy = target_pos.y + f64::from(target_entity.entity_dimension.load().height) / 3.0
            - arrow.entity.pos.load().y;
        let direction = Vector3::new(dx, horizontal.mul_add(0.2, dy), dz);

        // `RangedAttackMob.rangedAttackUncertainty`: `14 - difficulty.getId() * 4`.
        let difficulty = world.level_info.load().difficulty as i32;
        let divergence = f64::from(14 - difficulty * 4);
        arrow.set_velocity(
            direction.x,
            direction.y,
            direction.z,
            CROSSBOW_POWER,
            divergence,
        );
        world.spawn_entity(Arc::new(arrow)).await;

        world.play_sound(
            Sound::ItemCrossbowShoot,
            SoundCategory::Hostile,
            &shooter_pos,
        );

        if let Some(crossbow_mob) = mob.as_crossbow_attack_mob() {
            crossbow_mob.on_crossbow_attack_performed();
        }
    }
}

impl Goal for RangedCrossbowAttackGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { Self::can_use(mob).await })
    }

    /// `isValidTarget() && (canUse() || !navigation.isDone()) && isHoldingCrossbow()`, which
    /// reduces to `canUse()`.
    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { Self::can_use(mob).await })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.state = CrossbowState::Uncharged;
            self.see_time = 0;
            self.attack_delay = 0;
            self.update_path_delay = 0;
            self.charge_ticks = 0;
            mob.get_mob_entity().set_attacking(true);
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            mob_entity.set_attacking(false);
            mob_entity.set_target(None).await;
            self.see_time = 0;
            if mob_entity.living_entity.is_using_item() {
                mob_entity.living_entity.clear_active_hand().await;
                mob.set_charging_crossbow(false);
            }
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(target) = mob.get_mob_entity().target.lock().await.clone() else {
                return;
            };
            let mob_entity = mob.get_mob_entity();
            let entity = mob.get_entity();

            let has_line_of_sight = mob_entity.has_line_of_sight(target.as_ref()).await;
            let had_line_of_sight = self.see_time > 0;
            if has_line_of_sight != had_line_of_sight {
                self.see_time = 0;
            }
            if has_line_of_sight {
                self.see_time += 1;
            } else {
                self.see_time -= 1;
            }

            let target_pos = target.get_entity().pos.load();
            let distance_squared = entity.pos.load().squared_distance_to_vec(&target_pos);
            let needs_to_move = (distance_squared > self.attack_radius_sqr || self.see_time < 5)
                && self.attack_delay == 0;

            if needs_to_move {
                self.update_path_delay -= 1;
                if self.update_path_delay <= 0 {
                    // `canRun()`: full speed only while uncharged.
                    let speed = if self.state == CrossbowState::Uncharged {
                        self.speed_modifier
                    } else {
                        self.speed_modifier * 0.5
                    };
                    mob_entity
                        .navigator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .set_progress(NavigatorGoal {
                            current_progress: entity.pos.load(),
                            destination: target_pos,
                            speed,
                        });
                    // `PATHFINDING_DELAY_RANGE = TimeUtil.rangeOfSeconds(1, 2)`: [20, 40].
                    self.update_path_delay = mob.get_random().random_range(20..=40);
                }
            } else {
                self.update_path_delay = 0;
                mob_entity
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
            }

            mob_entity
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_entity_with_range(&target, 30.0, 30.0);

            match self.state {
                CrossbowState::Uncharged => {
                    if !needs_to_move && let Some((hand, stack)) = Self::crossbow_hand(mob).await {
                        mob_entity
                            .living_entity
                            .set_active_hand(hand, stack, i32::MAX)
                            .await;
                        self.state = CrossbowState::Charging;
                        self.charge_ticks = 0;
                        mob.set_charging_crossbow(true);
                    }
                }
                CrossbowState::Charging => {
                    if !mob_entity.living_entity.is_using_item() {
                        self.state = CrossbowState::Uncharged;
                    }

                    self.charge_ticks += 1;
                    if self.charge_ticks == LOADING_START_TICKS {
                        Self::play_sound(mob, Sound::ItemCrossbowLoadingStart);
                    } else if self.charge_ticks == LOADING_MIDDLE_TICKS {
                        Self::play_sound(mob, Sound::ItemCrossbowLoadingMiddle);
                    }
                    if self.charge_ticks >= CHARGE_DURATION_TICKS {
                        // `releaseUsingItem` -> `CrossbowItem.releaseUsing` loads the crossbow
                        // and plays the loading-end sound.
                        mob_entity.living_entity.clear_active_hand().await;
                        Self::play_sound(mob, Sound::ItemCrossbowLoadingEnd);
                        self.state = CrossbowState::Charged;
                        self.attack_delay = 20 + mob.get_random().random_range(0..20);
                        mob.set_charging_crossbow(false);
                    }
                }
                CrossbowState::Charged => {
                    self.attack_delay -= 1;
                    if self.attack_delay == 0 {
                        self.state = CrossbowState::ReadyToAttack;
                    }
                }
                CrossbowState::ReadyToAttack => {
                    if has_line_of_sight {
                        Self::shoot(mob, target.as_ref()).await;
                        self.state = CrossbowState::Uncharged;
                    }
                }
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_squares_attack_radius() {
        let goal = RangedCrossbowAttackGoal::new(1.0, 8.0);
        assert!((goal.attack_radius_sqr - 64.0).abs() < f64::EPSILON);
        assert!((goal.speed_modifier - 1.0).abs() < f64::EPSILON);
        assert_eq!(goal.state, CrossbowState::Uncharged);
        let controls = goal.controls();
        assert!(controls.get(Controls::MOVE));
        assert!(controls.get(Controls::LOOK));
    }
}
