// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::atomic::Ordering::Relaxed;

use super::{Controls, Goal};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};

/// Vanilla: `Armadillo.isScaredBy` inflates the armadillo's bounding box by these amounts to
/// find nearby threats.
const SCARE_HORIZONTAL_RANGE: f64 = 7.0;
const SCARE_VERTICAL_RANGE: f64 = 2.0;
/// Vanilla: `Armadillo.ArmadilloState.SCARED.animationDuration()` -- the minimum time an
/// armadillo stays balled up once it curls, before it may start peeking/unrolling again.
const MIN_CURL_TICKS: i32 = 40;

/// Makes an armadillo curl into a ball when a threat is nearby, freezing its movement.
///
/// This is a simplified port of vanilla's `ArmadilloAi.ArmadilloBallUp` behavior (itself driven
/// by the `DANGER_DETECTED_RECENTLY` brain memory, which is set for 80 ticks by
/// `Armadillo.actuallyHurt` whenever the damage source isn't environmental panic). Notable
/// simplifications:
/// - No brain memory/expiry system: threat detection is re-evaluated every tick via
///   `is_threatened` instead of a memory value with an 80-tick expiry window, and there is no
///   "peek" sub-state or unrolling animation timer.
/// - Damage reduction while curled (`Armadillo.hurtServer`: `damage = (damage - 1) / 2`) is not
///   implemented here since it requires touching the generic damage pipeline, which is out of
///   scope for a Goal-only port. Only the movement-freezing/behavioral part of curling up is
///   implemented.
///
/// Vanilla source: `net/minecraft/world/entity/animal/armadillo/{Armadillo,ArmadilloAi}.java`.
pub struct ArmadilloCurlUpGoal {
    goal_control: Controls,
    ticks_curled: i32,
}

impl ArmadilloCurlUpGoal {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            goal_control: Controls::MOVE,
            ticks_curled: 0,
        }
    }

    fn is_threatened(mob: &dyn Mob) -> bool {
        let mob_entity = mob.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        let world = entity.world.load();
        let last_attacker_id = mob_entity.living_entity.last_attacker_id.load(Relaxed);
        let threat_box = entity.bounding_box.load().expand(
            SCARE_HORIZONTAL_RANGE,
            SCARE_VERTICAL_RANGE,
            SCARE_HORIZONTAL_RANGE,
        );

        // Armadillo.isScaredBy checks intersecting living entities, including players.
        for candidate in world.get_all_at_box(&threat_box) {
            if candidate.get_entity().entity_uuid == entity.entity_uuid
                || candidate.get_living_entity().is_none()
            {
                continue;
            }

            let candidate_entity = candidate.get_entity();

            if candidate_entity.entity_id == last_attacker_id {
                return true;
            }

            // Vanilla: `livingEntity.is(EntityTypeTags.UNDEAD)`.
            if candidate_entity
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_UNDEAD)
            {
                return true;
            }

            if let Some(player) = candidate.get_player()
                && !player.is_spectator()
                && (candidate_entity.is_sprinting()
                    || player
                        .get_entity()
                        .vehicle
                        .try_lock()
                        .is_ok_and(|v| v.is_some()))
            {
                return true;
            }
        }

        false
    }
}

impl Default for ArmadilloCurlUpGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for ArmadilloCurlUpGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        // Vanilla: `ArmadilloBallUp.checkExtraStartConditions` requires being grounded.
        if !mob.get_entity().on_ground.load(Relaxed) {
            return false;
        }
        Self::is_threatened(mob)
    }

    fn should_continue(&self, mob: &dyn Mob) -> bool {
        // Stay curled for at least MIN_CURL_TICKS even if the threat leaves immediately,
        // mirroring vanilla's ROLLING/SCARED animation-duration gating.
        self.ticks_curled < MIN_CURL_TICKS || Self::is_threatened(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.ticks_curled = 0;
        mob.get_mob_entity().navigator.lock().unwrap().stop();

        let entity = mob.get_entity();
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityArmadilloRoll,
            SoundCategory::Neutral,
            &entity.pos.load(),
        );
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityArmadilloUnrollFinish,
            SoundCategory::Neutral,
            &entity.pos.load(),
        );
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.ticks_curled += 1;
        // Keep the armadillo from being pushed/navigating away while curled.
        mob.get_mob_entity().navigator.lock().unwrap().stop();
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}
