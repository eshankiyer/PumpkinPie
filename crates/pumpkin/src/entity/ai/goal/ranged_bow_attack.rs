// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;

use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{
    DataComponentImpl, EquipmentSlot, PotionContentsImpl, StatusEffectInstance,
};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_protocol::IdOr;
use pumpkin_protocol::java::client::play::CSoundEffect;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::{Difficulty, Hand};
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{Controls, Goal, GoalFuture},
    mob::Mob,
    projectile::arrow::{ArrowEntity, ArrowPickup},
};

/// The common bow attack loop used by skeleton-family mobs.
///
/// This follows vanilla's `RangedBowAttackGoal` timing and active-hand state. A
/// shot is created by the same projectile path as player bows, with the vanilla
/// skeleton trajectory adjustment from `AbstractSkeleton#performRangedAttack`.
pub struct RangedBowAttackGoal {
    attack_interval: i32,
    attack_cooldown: i32,
    bow_draw_ticks: i32,
    drawing: bool,
    see_time: i32,
    strafing_clockwise: bool,
    strafing_backwards: bool,
    strafing_time: i32,
    range: f64,
    /// Effects an `AbstractSkeleton#getArrow` override attaches to every arrow it fires
    /// (`Parched#getArrow`).
    arrow_effects: &'static [StatusEffectInstance],
}

impl RangedBowAttackGoal {
    #[must_use]
    pub const fn new(attack_interval: i32, range: f64) -> Self {
        Self::with_arrow_effects(attack_interval, range, &[])
    }

    #[must_use]
    pub const fn with_arrow_effects(
        attack_interval: i32,
        range: f64,
        arrow_effects: &'static [StatusEffectInstance],
    ) -> Self {
        Self {
            attack_interval,
            attack_cooldown: -1,
            bow_draw_ticks: 0,
            drawing: false,
            see_time: 0,
            strafing_clockwise: false,
            strafing_backwards: false,
            strafing_time: -1,
            range,
            arrow_effects,
        }
    }

    #[must_use]
    pub const fn with_difficulty_intervals(
        hard_attack_interval: i32,
        normal_attack_interval: i32,
        difficulty: Difficulty,
        range: f64,
        arrow_effects: &'static [StatusEffectInstance],
    ) -> Self {
        Self {
            attack_interval: match difficulty {
                Difficulty::Hard => hard_attack_interval,
                _ => normal_attack_interval,
            },
            attack_cooldown: -1,
            bow_draw_ticks: 0,
            drawing: false,
            see_time: 0,
            strafing_clockwise: false,
            strafing_backwards: false,
            strafing_time: -1,
            range,
            arrow_effects,
        }
    }

    /// `ProjectileUtil#getWeaponHoldingHand` prefers the main hand, then the off hand.
    async fn bow_hand_and_stack(mob: &dyn Mob) -> Option<(Hand, ItemStack)> {
        let equipment = mob
            .get_mob_entity()
            .living_entity
            .entity_equipment
            .lock()
            .await;
        let main_hand = equipment.get(&EquipmentSlot::MAIN_HAND);
        if main_hand.item.registry_key == Item::BOW.registry_key {
            return Some((Hand::Right, main_hand));
        }
        let off_hand = equipment.get(&EquipmentSlot::OFF_HAND);
        (off_hand.item.registry_key == Item::BOW.registry_key).then_some((Hand::Left, off_hand))
    }

    /// `RangedBowAttackGoal#isHoldingBow` (RangedBowAttackGoal.java:40-42).
    async fn is_holding_bow(mob: &dyn Mob) -> bool {
        Self::bow_hand_and_stack(mob).await.is_some()
    }

    /// `Monster#getProjectile` searches the held projectile slots and falls back
    /// to a plain arrow when no supported projectile is held.
    async fn projectile_stack(mob: &dyn Mob) -> ItemStack {
        let equipment = mob
            .get_mob_entity()
            .living_entity
            .entity_equipment
            .lock()
            .await;
        for slot in [EquipmentSlot::OFF_HAND, EquipmentSlot::MAIN_HAND] {
            let stack = equipment.get(&slot);
            if matches!(
                stack.item.registry_key,
                key if key == Item::ARROW.registry_key
                    || key == Item::TIPPED_ARROW.registry_key
                    || key == Item::SPECTRAL_ARROW.registry_key
            ) {
                return stack.copy_with_count(1);
            }
        }
        ItemStack::new(1, &Item::ARROW)
    }

    async fn has_line_of_sight(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
        mob.get_mob_entity().has_line_of_sight(target).await
    }

    fn target_vector(shooter: &Entity, target: &dyn EntityBase) -> Vector3<f64> {
        // `ArrowEntity::new_shot` starts an arrow at eye height minus 0.1.
        let mut arrow_pos = shooter.get_eye_pos();
        arrow_pos.y -= 0.1;
        let target_pos = target.get_entity().pos.load();
        Self::target_vector_from_positions(arrow_pos, target_pos, target.get_entity().height())
    }

    fn target_vector_from_positions(
        arrow_pos: Vector3<f64>,
        target_pos: Vector3<f64>,
        target_height: f32,
    ) -> Vector3<f64> {
        let horizontal_distance = (target_pos.x - arrow_pos.x).hypot(target_pos.z - arrow_pos.z);

        // Vanilla: target.getY(1 / 3) - arrow.getY() + horizontalDistance * 0.2.
        Vector3::new(
            target_pos.x - arrow_pos.x,
            target_pos.y + f64::from(target_height) / 3.0 - arrow_pos.y + horizontal_distance * 0.2,
            target_pos.z - arrow_pos.z,
        )
    }

    async fn shoot(&self, mob: &dyn Mob, target: &dyn EntityBase, power: f64) {
        let shooter = mob.get_entity();
        let world = shooter.world.load_full();
        let Some((_, bow_stack)) = Self::bow_hand_and_stack(mob).await else {
            return;
        };
        let mut arrow_item = Self::projectile_stack(mob).await;
        if !self.arrow_effects.is_empty()
            && arrow_item.item.registry_key != Item::SPECTRAL_ARROW.registry_key
        {
            if let Some(contents) = arrow_item.get_data_component_mut::<PotionContentsImpl>() {
                contents
                    .custom_effects
                    .extend_from_slice(self.arrow_effects);
            } else {
                arrow_item.patch.push((
                    DataComponent::PotionContents,
                    Some(
                        PotionContentsImpl {
                            potion_id: None,
                            custom_color: None,
                            custom_effects: self.arrow_effects.to_vec(),
                            custom_name: None,
                        }
                        .to_dyn(),
                    ),
                ));
            }
        }
        let arrow_entity = Entity::new(
            world.clone(),
            shooter.pos.load(),
            ArrowEntity::entity_type_for_item(arrow_item.item),
        );
        let arrow = ArrowEntity::new_shot_with_weapon(
            arrow_entity,
            shooter,
            &arrow_item,
            ArrowPickup::Disallowed,
            &bow_stack,
        );
        // `ProjectileUtil#getMobArrow` calls `setBaseDamageFromMob(power)`.
        // Vanilla adds triangle(difficulty_id * 0.11, 0.57425).
        let difficulty = world.level_info.load().difficulty as i32;
        let difficulty_mean = f64::from(difficulty) * 0.11;
        let random_damage = {
            let mut random = mob.get_random();
            difficulty_mean + 0.57425 * (random.random::<f64>() - random.random::<f64>())
        };
        arrow.set_base_damage(2.0 * power + random_damage);
        let direction = Self::target_vector(shooter, target);

        // `AbstractSkeleton#performRangedAttack`: power 1.6, inaccuracy
        // `14 - level.getDifficulty().getId() * 4`.
        let inaccuracy = f64::from(14 - difficulty * 4);
        arrow.set_velocity(direction.x, direction.y, direction.z, 1.6, inaccuracy);
        world.spawn_entity(Arc::new(arrow)).await;

        let sound = CSoundEffect::new(
            IdOr::Id(Sound::EntitySkeletonShoot as u16),
            SoundCategory::Hostile,
            &shooter.pos.load(),
            1.0,
            1.0 / (rand::random::<f32>() * 0.4 + 0.8),
            0.0,
        );
        world.broadcast_to_chunk(shooter.chunk_pos.load(), &sound);
    }
}

impl Goal for RangedBowAttackGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let has_target = mob
                .get_mob_entity()
                .target
                .lock()
                .await
                .as_ref()
                .is_some_and(|target| target.get_entity().is_alive());
            has_target && Self::is_holding_bow(mob).await
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let has_target = mob.get_mob_entity().target.lock().await.as_ref().is_some();
            let navigation_active = !mob.get_mob_entity().navigator.lock().unwrap().is_idle();
            (has_target || navigation_active) && Self::is_holding_bow(mob).await
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.attack_cooldown = -1;
            self.bow_draw_ticks = 0;
            self.drawing = false;
            self.see_time = 0;
            self.strafing_time = -1;
            mob.get_mob_entity().set_attacking(true);
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if self.drawing {
                mob.get_mob_entity().living_entity.clear_active_hand().await;
            }
            mob.get_mob_entity().navigator.lock().unwrap().stop();
            self.attack_cooldown = -1;
            self.bow_draw_ticks = 0;
            self.drawing = false;
            self.see_time = 0;
            self.strafing_time = -1;
            mob.get_mob_entity().set_attacking(false);
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(target) = mob.get_mob_entity().target.lock().await.clone() else {
                return;
            };
            let shooter = mob.get_entity();
            let target_pos = target.get_entity().pos.load();
            let distance_squared = shooter.pos.load().squared_distance_to_vec(&target_pos);

            let has_line_of_sight = Self::has_line_of_sight(mob, target.as_ref()).await;
            let had_line_of_sight = self.see_time > 0;
            if has_line_of_sight != had_line_of_sight {
                self.see_time = 0;
            }
            if has_line_of_sight {
                self.see_time += 1;
            } else {
                self.see_time -= 1;
            }

            let range_squared = self.range * self.range;
            if distance_squared <= range_squared && self.see_time >= 20 {
                mob.get_mob_entity().navigator.lock().unwrap().stop();
                self.strafing_time += 1;
            } else {
                mob.get_mob_entity().navigator.lock().unwrap().set_progress(
                    crate::entity::ai::pathfinder::NavigatorGoal {
                        current_progress: shooter.pos.load(),
                        destination: target_pos,
                        speed: 1.0,
                    },
                );
                self.strafing_time = -1;
            }

            if self.strafing_time >= 20 {
                if mob.get_random().random::<f32>() < 0.3 {
                    self.strafing_clockwise = !self.strafing_clockwise;
                }
                if mob.get_random().random::<f32>() < 0.3 {
                    self.strafing_backwards = !self.strafing_backwards;
                }
                self.strafing_time = 0;
            }

            if self.strafing_time > -1 {
                if distance_squared > range_squared * 0.75 {
                    self.strafing_backwards = false;
                } else if distance_squared < range_squared * 0.25 {
                    self.strafing_backwards = true;
                }
                mob.get_mob_entity().move_control.lock().unwrap().strafe(
                    if self.strafing_backwards { -0.5 } else { 0.5 },
                    if self.strafing_clockwise { 0.5 } else { -0.5 },
                );
            }
            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap()
                .look_at_entity_with_range(&target, 30.0, 30.0);

            if self.drawing {
                self.bow_draw_ticks += 1;
                if !has_line_of_sight && self.see_time < -60 {
                    mob.get_mob_entity().living_entity.clear_active_hand().await;
                    self.drawing = false;
                    self.bow_draw_ticks = 0;
                } else if has_line_of_sight && self.bow_draw_ticks >= 20 {
                    mob.get_mob_entity().living_entity.clear_active_hand().await;
                    self.shoot(mob, target.as_ref(), 1.0).await;
                    self.attack_cooldown = self.attack_interval;
                    self.bow_draw_ticks = 0;
                    self.drawing = false;
                }
            } else {
                self.attack_cooldown -= 1;
                if self.attack_cooldown <= 0
                    && self.see_time >= -60
                    && let Some((hand, stack)) = Self::bow_hand_and_stack(mob).await
                {
                    mob.get_mob_entity()
                        .living_entity
                        .set_active_hand(
                            hand,
                            stack,
                            crate::item::items::bow::BowItem::USE_DURATION,
                        )
                        .await;
                    self.drawing = true;
                    self.bow_draw_ticks = 0;
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
    use super::RangedBowAttackGoal;
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn preserves_vanilla_skeleton_bow_interval() {
        let goal = RangedBowAttackGoal::new(20, 15.0);
        assert_eq!(goal.attack_interval, 20);
        assert_eq!(goal.range, 15.0);
    }

    #[test]
    fn difficulty_intervals_keep_hard_and_normal_values_separate() {
        let goal = RangedBowAttackGoal::with_difficulty_intervals(
            20,
            40,
            pumpkin_util::Difficulty::Hard,
            15.0,
            &[],
        );
        assert_eq!(goal.attack_interval, 20);

        let goal = RangedBowAttackGoal::with_difficulty_intervals(
            20,
            40,
            pumpkin_util::Difficulty::Normal,
            15.0,
            &[],
        );
        assert_eq!(goal.attack_interval, 40);
    }

    #[test]
    fn starts_ready_to_draw_the_bow() {
        let goal = RangedBowAttackGoal::new(20, 15.0);
        assert_eq!(goal.attack_cooldown, -1);
        assert_eq!(goal.bow_draw_ticks, 0);
        assert!(!goal.drawing);
    }

    #[test]
    fn adds_vanilla_ballistic_vertical_lead() {
        let direction = RangedBowAttackGoal::target_vector_from_positions(
            Vector3::new(0.0, 1.52, 0.0),
            Vector3::new(3.0, 0.0, 4.0),
            1.8,
        );

        assert_eq!(direction.x, 3.0);
        assert_eq!(direction.z, 4.0);
        // target Y + targetEyeHeight / 3 - arrow Y + horizontalDistance * 0.2
        assert!((direction.y - 0.08).abs() < 1.0e-6);
    }
}
