use std::sync::atomic::Ordering;

use crate::entity::EntityBase;
use pumpkin_data::{
    attributes::Attributes,
    effect::StatusEffect,
    particle::Particle,
    sound::{Sound, SoundCategory},
};
use pumpkin_util::math::vector3::Vector3;

use crate::{
    entity::{Entity, player::Player},
    world::World,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackType {
    Knockback,
    Critical,
    Sweeping,
    Strong,
    Weak,
    MaceSmash,
}

#[derive(Debug, Clone, Copy)]
pub struct AttackConditions {
    pub cooldown_progress: f32,
    pub is_mace: bool,
    pub is_sword: bool,
    pub target_is_living: bool,
    pub sprinting: bool,
    pub on_ground: bool,
    pub touching_water: bool,
    pub climbing: bool,
    pub mobility_restricted: bool,
    pub has_vehicle: bool,
    pub fall_distance: f32,
    pub horizontal_movement: f64,
    pub movement_speed: f64,
}

impl AttackType {
    pub async fn new(
        player: &Player,
        attack_cooldown_progress: f32,
        target_is_living: bool,
    ) -> Self {
        let entity = &player.get_entity();
        let held_item = player.inventory().held_item();
        let (is_mace, is_sword) = {
            let stack = held_item.lock().await;
            (
                stack.item.id == pumpkin_data::item::Item::MACE.id,
                stack.is_sword(),
            )
        };

        Self::classify(AttackConditions {
            cooldown_progress: attack_cooldown_progress,
            is_mace,
            is_sword,
            target_is_living,
            sprinting: entity.is_sprinting(),
            on_ground: entity.on_ground.load(Ordering::Relaxed),
            touching_water: entity.touching_water.load(Ordering::Relaxed),
            climbing: player.living_entity.climbing.load(Ordering::Relaxed),
            mobility_restricted: player
                .living_entity
                .has_effect(&StatusEffect::BLINDNESS)
                .await,
            has_vehicle: entity.has_vehicle().await,
            fall_distance: player.living_entity.fall_distance.load(),
            horizontal_movement: player.living_entity.get_movement().horizontal_length(),
            movement_speed: player
                .living_entity
                .get_attribute_value(&Attributes::MOVEMENT_SPEED),
        })
    }

    pub const fn classify(conditions: AttackConditions) -> Self {
        if conditions.is_mace && !conditions.on_ground && conditions.fall_distance > 1.5 {
            return Self::MaceSmash;
        }

        let full_strength = conditions.cooldown_progress > 0.9;
        if conditions.sprinting && full_strength {
            return Self::Knockback;
        }

        let critical = full_strength
            && conditions.target_is_living
            && !conditions.on_ground
            && conditions.fall_distance > 0.0
            && !conditions.sprinting
            && !conditions.touching_water
            && !conditions.climbing
            && !conditions.mobility_restricted
            && !conditions.has_vehicle;
        if critical {
            return Self::Critical;
        }

        if conditions.is_sword
            && full_strength
            && conditions.on_ground
            && !conditions.sprinting
            && conditions.horizontal_movement < conditions.movement_speed * 2.5
        {
            return Self::Sweeping;
        }

        if full_strength {
            Self::Strong
        } else {
            Self::Weak
        }
    }
}

/// Scales a knockback `strength` by a living entity's knockback resistance,
/// mirroring vanilla `LivingEntity.knockback`: `strength *= 1.0 - resistance`.
/// A resistance of 1.0 (iron golem, warden, ...) cancels the knockback entirely.
pub fn knockback_after_resistance(strength: f64, resistance: f64) -> f64 {
    strength * (1.0 - resistance)
}

pub fn handle_knockback(attacker: &Entity, victim: &dyn EntityBase, strength: f64) {
    let resistance = victim.get_living_entity().map_or(0.0, |living| {
        living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE)
    });
    let strength = knockback_after_resistance(strength * 0.5, resistance);

    if strength > 0.0 {
        let yaw = attacker.yaw.load();
        victim.get_entity().knockback(
            strength,
            f64::from((yaw.to_radians()).sin()),
            f64::from(-(yaw.to_radians()).cos()),
        );
    }

    let velocity = attacker.velocity.load();
    attacker.velocity.store(velocity.multiply(0.6, 1.0, 0.6));
}

pub fn spawn_sweep_particle(attacker_entity: &Entity, world: &World, pos: &Vector3<f64>) {
    let yaw = attacker_entity.yaw.load();
    let d = -f64::from((yaw.to_radians()).sin());
    let e = f64::from((yaw.to_radians()).cos());

    let scale = 0.5;
    let body_y = f64::from(attacker_entity.height()).mul_add(scale, pos.y);

    world.spawn_particle(
        Vector3::new(pos.x + d, body_y, pos.z + e),
        Vector3::new(0.0, 0.0, 0.0),
        0.0,
        0,
        Particle::SweepAttack,
    );
}

pub async fn player_attack_sound(pos: &Vector3<f64>, world: &World, attack_type: AttackType) {
    match attack_type {
        AttackType::Knockback => {
            world.play_sound(
                Sound::EntityPlayerAttackKnockback,
                SoundCategory::Players,
                pos,
            );
        }
        AttackType::Critical => {
            world.play_sound(Sound::EntityPlayerAttackCrit, SoundCategory::Players, pos);
        }
        AttackType::Sweeping => {
            world.play_sound(Sound::EntityPlayerAttackSweep, SoundCategory::Players, pos);
        }
        AttackType::Strong => {
            world.play_sound(Sound::EntityPlayerAttackStrong, SoundCategory::Players, pos);
        }
        AttackType::Weak => {
            world.play_sound(Sound::EntityPlayerAttackWeak, SoundCategory::Players, pos);
        }
        AttackType::MaceSmash => {
            world.play_sound(Sound::ItemMaceSmashAir, SoundCategory::Players, pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AttackConditions, AttackType, knockback_after_resistance};

    const fn critical_conditions() -> AttackConditions {
        AttackConditions {
            cooldown_progress: 1.0,
            is_mace: false,
            is_sword: false,
            target_is_living: true,
            sprinting: false,
            on_ground: false,
            touching_water: false,
            climbing: false,
            mobility_restricted: false,
            has_vehicle: false,
            fall_distance: 1.0,
            horizontal_movement: 0.0,
            movement_speed: 0.1,
        }
    }

    const fn sweeping_conditions() -> AttackConditions {
        AttackConditions {
            cooldown_progress: 1.0,
            is_mace: false,
            is_sword: true,
            target_is_living: false,
            sprinting: false,
            on_ground: true,
            touching_water: false,
            climbing: false,
            mobility_restricted: false,
            has_vehicle: false,
            fall_distance: 0.0,
            horizontal_movement: 0.249,
            movement_speed: 0.1,
        }
    }

    #[test]
    fn critical_attack_decision_table() {
        let mut non_living_target = critical_conditions();
        non_living_target.target_is_living = false;
        let mut grounded = critical_conditions();
        grounded.on_ground = true;
        let mut no_fall = critical_conditions();
        no_fall.fall_distance = 0.0;
        let mut sprinting = critical_conditions();
        sprinting.sprinting = true;
        let mut in_water = critical_conditions();
        in_water.touching_water = true;
        let mut climbing = critical_conditions();
        climbing.climbing = true;
        let mut mobility_restricted = critical_conditions();
        mobility_restricted.mobility_restricted = true;
        let mut riding = critical_conditions();
        riding.has_vehicle = true;
        let mut insufficient_charge = critical_conditions();
        insufficient_charge.cooldown_progress = 0.9;

        let cases = [
            (critical_conditions(), AttackType::Critical),
            (non_living_target, AttackType::Strong),
            (grounded, AttackType::Strong),
            (no_fall, AttackType::Strong),
            (sprinting, AttackType::Knockback),
            (in_water, AttackType::Strong),
            (climbing, AttackType::Strong),
            (mobility_restricted, AttackType::Strong),
            (riding, AttackType::Strong),
            (insufficient_charge, AttackType::Weak),
        ];

        for (conditions, expected) in cases {
            assert_eq!(AttackType::classify(conditions), expected);
        }
    }

    #[test]
    fn sweeping_attack_decision_table() {
        let mut insufficient_charge = sweeping_conditions();
        insufficient_charge.cooldown_progress = 0.9;
        let mut airborne = sweeping_conditions();
        airborne.on_ground = false;
        let mut not_a_sword = sweeping_conditions();
        not_a_sword.is_sword = false;
        let mut critical = sweeping_conditions();
        critical.target_is_living = true;
        critical.on_ground = false;
        critical.fall_distance = 1.0;
        let mut sprinting = sweeping_conditions();
        sprinting.sprinting = true;
        let mut at_speed_limit = sweeping_conditions();
        at_speed_limit.horizontal_movement = 0.25;
        let mut above_speed_limit = sweeping_conditions();
        above_speed_limit.horizontal_movement = 0.251;

        let cases = [
            (sweeping_conditions(), AttackType::Sweeping),
            (insufficient_charge, AttackType::Weak),
            (airborne, AttackType::Strong),
            (not_a_sword, AttackType::Strong),
            (critical, AttackType::Critical),
            (sprinting, AttackType::Knockback),
            (at_speed_limit, AttackType::Strong),
            (above_speed_limit, AttackType::Strong),
        ];

        for (conditions, expected) in cases {
            assert_eq!(AttackType::classify(conditions), expected);
        }
    }

    #[test]
    fn zero_resistance_keeps_full_strength() {
        assert_eq!(knockback_after_resistance(0.4, 0.0), 0.4);
    }

    #[test]
    fn full_resistance_cancels_knockback() {
        // Iron golem / warden have KNOCKBACK_RESISTANCE == 1.0.
        assert_eq!(knockback_after_resistance(0.4, 1.0), 0.0);
    }

    #[test]
    fn partial_resistance_scales_strength() {
        // Ravager has KNOCKBACK_RESISTANCE == 0.75.
        assert!((knockback_after_resistance(0.4, 0.75) - 0.1).abs() < 1e-9);
    }

    #[test]
    fn over_full_resistance_is_negative_so_callers_skip_it() {
        // Stacked armour modifiers can push resistance above 1.0; the result is
        // negative and callers guard on `strength > 0.0`.
        assert!(knockback_after_resistance(0.4, 1.2) < 0.0);
    }
}
