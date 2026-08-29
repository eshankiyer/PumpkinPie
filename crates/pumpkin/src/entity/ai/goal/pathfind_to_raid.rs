//! Port of vanilla `PathfindToRaidGoal<T extends Raider>`
//! (`net/minecraft/world/entity/ai/goal/PathfindToRaidGoal.java:14-71`).
//!
//! While a raider has an active raid, no attack target and is not inside the raided village,
//! this goal periodically (every 20 ticks) enlists idle raiders standing nearby into the raid
//! (`recruitNearby`, `PathfindToRaidGoal.java:57-70`) and paths toward the raid center
//! (`DefaultRandomPos.getPosTowards(mob, 15, 4, center-bottom, PI/2)`,
//! `PathfindToRaidGoal.java:49`).
//!
//! Vanilla registers it for every `Raider` at priority 3 via `Raider.registerGoals`
//! (`Raider.java:65`); pumpkin mirrors that by adding it to pillager, vindicator, evoker,
//! illusioner, ravager and witch goal selectors.
//!
//! The mob's raid handle is `LivingEntity.raid_membership`'s cached `raid_id`; the live `Raid`
//! (`isActive`/`isOver`/`getCenter`) is re-read from `World::raids` on every use, matching
//! vanilla's repeated `getCurrentRaid()` calls.

use std::sync::atomic::{AtomicI32, Ordering::Relaxed};

use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::vector3::Vector3;

use super::random_pos::default_get_pos_towards;
use super::{Controls, Goal};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::world::World;

/// `PathfindToRaidGoal.RECRUITMENT_SEARCH_TICK_DELAY` (`PathfindToRaidGoal.java:15`).
const RECRUITMENT_SEARCH_TICK_DELAY: i32 = 20;
/// `PathfindToRaidGoal.SPEED_MODIFIER` (`PathfindToRaidGoal.java:16`).
const SPEED_MODIFIER: f64 = 1.0;
/// `DefaultRandomPos.getPosTowards(mob, 15, 4, ...)` (`PathfindToRaidGoal.java:49`).
const HORIZONTAL_RANGE: i32 = 15;
const VERTICAL_RANGE: i32 = 4;
/// `getBoundingBox().inflate(16.0)` in `recruitNearby` (`PathfindToRaidGoal.java:61-63`).
const RECRUIT_RADIUS: f64 = 16.0;
/// `Raids.canJoinRaid`'s `noActionTime <= 2400` gate (`Raids.java:98-100`).
const MAX_NO_ACTION_TIME: i32 = 2400;
/// `Math.PI / 2` cone passed to `getPosTowards` (`PathfindToRaidGoal.java:49`).
const MAX_ANGLE_RADIANS: f64 = std::f64::consts::FRAC_PI_2;

pub struct PathfindToRaidGoal {
    /// `PathfindToRaidGoal.recruitmentTick` (`PathfindToRaidGoal.java:18`).
    recruitment_tick: AtomicI32,
    speed_modifier: f64,
}

impl Default for PathfindToRaidGoal {
    fn default() -> Self {
        Self::new(SPEED_MODIFIER)
    }
}

impl PathfindToRaidGoal {
    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            recruitment_tick: AtomicI32::new(0),
            speed_modifier,
        }
    }

    /// Returns the mob's cached raid membership only while its live raid is active.
    fn active_raid_id(mob: &dyn Mob) -> Option<i32> {
        let raider = mob.as_raider()?;
        let raid_id = raider.get_raider_data().raid_id.load()?;
        let world = mob.get_entity().world.load();
        let raids = world
            .raids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let raid = raids.get(raid_id)?;
        (raid.is_active() && !raid.is_over()).then_some(raid_id)
    }

    /// The base `hasControllingPassenger` check is asynchronous in the older entity hierarchy.
    /// The synchronous goal scheduler uses the current passenger snapshot at this decision point.
    fn has_controlling_passenger(mob: &dyn Mob) -> bool {
        mob.get_entity()
            .passengers
            .try_lock()
            .ok()
            .and_then(|passengers| passengers.first().cloned())
            .is_some_and(|passenger| passenger.get_player().is_some())
    }

    fn is_outside_village(mob: &dyn Mob, raid_id: i32) -> bool {
        let pos = mob.get_entity().block_pos.load();
        let world = mob.get_entity().world.load();
        let raids = world
            .raids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(raid) = raids.get(raid_id) else {
            return false;
        };
        if !raid.is_active() || raid.is_over() {
            return false;
        }
        let is_village = world
            .villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_nearest_job_site(pos, 32)
            .is_some();
        !is_village
    }
}

impl Goal for PathfindToRaidGoal {
    /// Vanilla `canUse` (`PathfindToRaidGoal.java:26-32`): no target, no controlling passenger,
    /// active raid, and the mob outside the village.
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        if mob.get_mob_entity().get_target().is_some()
            || Self::has_controlling_passenger(mob)
            || !raider.has_active_raid()
        {
            return false;
        }
        let Some(raid_id) = Self::active_raid_id(mob) else {
            return false;
        };
        Self::is_outside_village(mob, raid_id)
    }

    /// Vanilla `canContinueToUse` (`PathfindToRaidGoal.java:35-37`): same raid-liveness and
    /// village gates without the target/passenger checks.
    fn should_continue(&self, mob: &dyn Mob) -> bool {
        let Some(raid_id) = Self::active_raid_id(mob) else {
            return false;
        };
        Self::is_outside_village(mob, raid_id)
    }

    /// Vanilla `tick` (`PathfindToRaidGoal.java:40-55`): while the raid runs, recruit nearby
    /// idle raiders every 20 ticks and keep pathing toward the raid center whenever the mob is
    /// not currently pathfinding.
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(raid_id) = Self::active_raid_id(mob) else {
            return;
        };
        let mob_entity = mob.get_mob_entity();
        let world = mob_entity.living_entity.entity.world.load();

        let raid_center = {
            let raids = world
                .raids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(raid) = raids.get(raid_id) else {
                return;
            };
            if !raid.is_active() || raid.is_over() {
                return;
            }
            Vector3::new(
                f64::from(raid.center.0.x) + 0.5,
                f64::from(raid.center.0.y),
                f64::from(raid.center.0.z) + 0.5,
            )
        };

        let current_age = mob_entity.living_entity.entity.age.load(Relaxed);
        let next_recruit = self.recruitment_tick.load(Relaxed);
        if current_age >= next_recruit {
            self.recruitment_tick
                .store(current_age + RECRUITMENT_SEARCH_TICK_DELAY, Relaxed);
            Self::recruit_nearby(&mob_entity.living_entity, &world, raid_id);
        }

        // `if (!this.mob.isPathFinding())` (`PathfindToRaidGoal.java:48`): pumpkin's
        // navigator reports idle exactly when there is no live path being followed.
        let mut navigator = mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !navigator.is_idle() {
            return;
        }
        let Some(pos_towards) = default_get_pos_towards(
            mob,
            HORIZONTAL_RANGE,
            VERTICAL_RANGE,
            raid_center,
            MAX_ANGLE_RADIANS,
        ) else {
            return;
        };
        let pos = mob_entity.living_entity.entity.pos.load();
        navigator.set_progress(NavigatorGoal {
            current_progress: pos,
            destination: pos_towards,
            speed: self.speed_modifier,
        });
    }

    /// Vanilla registers `EnumSet.of(Goal.Flag.MOVE)` (`PathfindToRaidGoal.java:22`).
    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

impl PathfindToRaidGoal {
    /// Vanilla `recruitNearby` (`PathfindToRaidGoal.java:57-70`): every living raider within
    /// 16 blocks of the recruiter that has no raid yet and passes `Raids.canJoinRaid`
    /// (`Raids.java:98-100`: alive, `canJoinRaid`, `noActionTime <= 2400`) joins this raid's
    /// current wave.
    fn recruit_nearby(recruiter: &LivingEntity, world: &std::sync::Arc<World>, raid_id: i32) {
        let search_pos = recruiter.entity.pos.load();
        let self_id = recruiter.entity.entity_id;
        let candidates: Vec<_> = world
            .get_nearby_entities(search_pos, RECRUIT_RADIUS)
            .into_values()
            .filter(|entity| entity.get_entity().entity_id != self_id)
            // `getEntitiesOfClass(Raider.class, ...)`: pumpkin marks raider types with the
            // `minecraft:raiders` entity tag (same test as `nearest_healable_raider_target.rs`).
            .filter(|entity| {
                entity
                    .get_entity()
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
            })
            .collect();

        for entity in candidates {
            let Some(candidate_mob) = entity.get_mob() else {
                continue;
            };
            let Some(candidate_raider) = candidate_mob.as_raider() else {
                continue;
            };
            let candidate = &candidate_mob.get_mob_entity().living_entity;
            if candidate_raider.has_active_raid()
                || !candidate.entity.is_alive()
                || !candidate_raider.can_join_raid()
                || candidate_mob.get_mob_entity().no_action_time.load(Relaxed)
                    > MAX_NO_ACTION_TIME
            {
                continue;
            }

            let mut raids = world
                .raids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(raid) = raids.get_mut(raid_id) else {
                continue;
            };
            if !raid.is_active() || raid.is_over() {
                continue;
            }
            let wave = raid.get_groups_spawned();
            // `raid.joinRaid(level, raid.getGroupsSpawned(), raider, null, true)`
            // (`PathfindToRaidGoal.java:67`). The membership is stored alongside the upstream
            // raid map enrollment so later active-raid checks see the new recruit.
            raid.join_raid(wave, candidate.entity.entity_uuid, &entity);
            candidate_raider.set_wave(wave);
            candidate_raider.get_raider_data().raid_id.store(Some(raid_id));
        }
    }
}
