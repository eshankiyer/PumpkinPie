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

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::vector3::Vector3;

use super::random_pos::default_get_pos_towards;
use super::{Controls, Goal, GoalFuture};
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

#[derive(Default)]
pub struct PathfindToRaidGoal {
    /// `PathfindToRaidGoal.recruitmentTick` (`PathfindToRaidGoal.java:18`).
    recruitment_tick: i32,
}

impl PathfindToRaidGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self::default())
    }
}

/// The mob's cached membership plus its world, or `None` when the mob has no cached raid.
///
/// Vanilla's `Raider.hasActiveRaid()` is `getCurrentRaid() != null && raid.isActive()`
/// (`Raider.java`); pumpkin approximates liveness with the membership captured at `join_raid`
/// time (see `LivingEntity::has_active_raid`), so this reports whatever that proxy reports.
async fn raid_membership(mob: &dyn Mob) -> Option<i32> {
    let living = &mob.get_mob_entity().living_entity;
    let membership = living.raid_membership.load()?;
    let raid_exists = living
        .entity
        .world
        .load()
        .raids
        .lock()
        .await
        .raid(membership.raid_id)
        .is_some();
    raid_exists.then_some(membership.raid_id)
}

impl Goal for PathfindToRaidGoal {
    /// Vanilla `canUse` (`PathfindToRaidGoal.java:26-32`): no target, no controlling passenger,
    /// active raid, and the mob outside the village.
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            if mob_entity.target.lock().await.is_some() {
                return false;
            }
            if mob.has_controlling_passenger().await {
                return false;
            }
            if raid_membership(mob).await.is_none() {
                return false;
            }
            // `!getServerLevel(this.mob.level()).isVillage(this.mob.blockPosition())`;
            // `ServerLevel.isVillage` is `isCloseToVillage(pos, 1)` (see
            // `world/custom_spawners.rs` for the established reading).
            let block_pos = mob_entity.living_entity.entity.block_pos.load();
            !mob_entity
                .living_entity
                .entity
                .world
                .load()
                .is_close_to_village(block_pos, 1)
                .await
        })
    }

    /// Vanilla `canContinueToUse` (`PathfindToRaidGoal.java:35-37`): same raid-liveness and
    /// village gates without the target/passenger checks.
    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if raid_membership(mob).await.is_none() {
                return false;
            }
            let entity = &mob.get_mob_entity().living_entity.entity;
            let block_pos = entity.block_pos.load();
            !entity.world.load().is_close_to_village(block_pos, 1).await
        })
    }

    /// Vanilla `tick` (`PathfindToRaidGoal.java:40-55`): while the raid runs, recruit nearby
    /// idle raiders every 20 ticks and keep pathing toward the raid center whenever the mob is
    /// not currently pathfinding.
    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(raid_id) = raid_membership(mob).await else {
                return;
            };
            let mob_entity = mob.get_mob_entity();
            let world = mob_entity.living_entity.entity.world.load();

            let (should_recruit, raid_center) = {
                let raids = world.raids.lock().await;
                let Some(raid) = raids.raid(raid_id) else {
                    return;
                };
                if !raid.is_active() || raid.is_over() {
                    return;
                }
                let tick_count = mob_entity.tick_count.load(Relaxed);
                let should_recruit = tick_count > self.recruitment_tick;
                if should_recruit {
                    self.recruitment_tick = tick_count + RECRUITMENT_SEARCH_TICK_DELAY;
                }
                let center = raid.center();
                // `Vec3.atBottomCenterOf(raid.getCenter())` (`PathfindToRaidGoal.java:49`).
                let raid_center = Vector3::new(
                    f64::from(center.0.x) + 0.5,
                    f64::from(center.0.y),
                    f64::from(center.0.z) + 0.5,
                );
                (should_recruit, raid_center)
            };

            if should_recruit {
                Self::recruit_nearby(&mob_entity.living_entity, &world, raid_id).await;
            }

            // `if (!this.mob.isPathFinding())` (`PathfindToRaidGoal.java:48`): pumpkin's
            // navigator reports idle exactly when there is no live path being followed.
            let navigator_idle = mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle();
            if !navigator_idle {
                return;
            }
            if let Some(pos_towards) = default_get_pos_towards(
                mob,
                HORIZONTAL_RANGE,
                VERTICAL_RANGE,
                raid_center,
                MAX_ANGLE_RADIANS,
            ) {
                let pos = mob_entity.living_entity.entity.pos.load();
                mob_entity
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_progress(NavigatorGoal::new(pos, pos_towards, SPEED_MODIFIER));
            }
        })
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
    async fn recruit_nearby(recruiter: &LivingEntity, world: &std::sync::Arc<World>, raid_id: i32) {
        let search_pos = recruiter.entity.pos.load();
        let self_id = recruiter.entity.entity_id;
        let candidates: Vec<_> = world
            .get_nearby_entities(search_pos, RECRUIT_RADIUS)
            .into_values()
            .filter(|entity| entity.get_entity().entity_id != self_id)
            // `getEntitiesOfClass(Raider.class, ...)`: pumpkin marks raider types with the
            // `minecraft:raiders` entity tag (same test as
            // `nearest_healable_raider_target.rs`).
            .filter(|entity| {
                entity
                    .get_entity()
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
            })
            .collect();

        for entity in candidates {
            let Some(recruit_mob) = entity.get_mob() else {
                continue;
            };
            let recruit = &recruit_mob.get_mob_entity().living_entity;
            if recruit.has_active_raid()
                || !recruit.entity.is_alive()
                || !recruit.can_join_raid.load(Relaxed)
                || recruit_mob.get_mob_entity().no_action_time.load(Relaxed) > MAX_NO_ACTION_TIME
            {
                continue;
            }
            // `raid.joinRaid(level, raid.getGroupsSpawned(), raider, null, true)`
            // (`PathfindToRaidGoal.java:67`). The lock must be released between recruits to
            // stay await-safe, hence the per-candidate scope.
            let mut raids = world.raids.lock().await;
            let Some(raid) = raids.raid_mut(raid_id) else {
                continue;
            };
            if !raid.is_active() {
                continue;
            }
            raid.join_raid(
                raid.groups_spawned(),
                recruit.entity.entity_uuid,
                recruit_mob.get_mob_entity(),
                false,
            );
        }
    }
}
