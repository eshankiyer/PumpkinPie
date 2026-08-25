//! Goal-based server port of `VillagerPanicTrigger`.
//!
//! Vanilla source: `net/minecraft/world/entity/ai/behavior/VillagerPanicTrigger.java:11-48`,
//! registered in every villager's CORE package at priority 0
//! (`VillagerGoalPackages.java:39`). Pumpkin villagers run on the goal selector rather than
//! the Brain (see the module comment in `villager/mod.rs`), so this is the same split the
//! other villager CORE behaviors use (`InteractWithDoorGoal`, `SocializeAtBellGoal`).
//!
//! The vanilla class itself never picks a flee destination - it only *enters* the PANIC
//! state, keeps it alive while hurt or a hostile is near, and periodically attempts an iron
//! golem summon while panicking. The actual running-away lives in the PANIC activity package
//! (`VillagerGoalPackages.getPanicPackage`, `VillagerGoalPackages.java:221-230`), which this
//! codebase already approximates with the villager's `AvoidEntityGoal` set. This goal
//! therefore claims no `Controls` and runs alongside those goals.

use super::{Controls, Goal, GoalFuture};
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::entity::passive::villager::VillagerEntity;
use pumpkin_data::entity::EntityType;

/// `timestamp % 100L` cadence in `tick` (`VillagerPanicTrigger.java:36`).
const GOLEM_CHECK_INTERVAL: i64 = 100;

/// The literal `3` passed to `spawnGolemIfNeeded` (`VillagerPanicTrigger.java:37`). Vanilla's
/// gossip path uses 5 (`Villager.java:820`); the panic path deliberately needs only 3.
const GOLEM_VILLAGERS_NEEDED_TO_AGREE: usize = 3;

/// Search bound for hostiles: the largest entry in
/// `VillagerHostilesSensor.ACCEPTABLE_DISTANCE_FROM_HOSTILES`
/// (`VillagerHostilesSensor.java:16`, PILLAGER 15.0).
const HOSTILE_SCAN_RADIUS: f64 = 15.0;

/// How long a panic-causing hit keeps `isHurt` true.
///
/// DEVIATION: vanilla keeps the `HURT_BY` memory until `VillagerCalmDown` erases it once no
/// scare source remains; that memory graph does not exist here. This follows the same
/// recent-damage window every other panic implementation in this codebase uses
/// (`escape_danger.rs`, backed by the packed `last_damage_state` written by
/// `LivingEntity::damage_with_context`).
const RECENT_DAMAGE_TICKS: i64 = 40;

/// `VillagerHostilesSensor.ACCEPTABLE_DISTANCE_FROM_HOSTILES`
/// (`VillagerHostilesSensor.java:11-23`) - the exact table whose matches populate vanilla's
/// `NEAREST_HOSTILE` memory, which `hasHostile` reads (`VillagerPanicTrigger.java:41-43`).
const HOSTILE_DISTANCES: &[(&EntityType, f64)] = &[
    (&EntityType::DROWNED, 8.0),
    (&EntityType::EVOKER, 12.0),
    (&EntityType::HUSK, 8.0),
    (&EntityType::ILLUSIONER, 12.0),
    (&EntityType::PILLAGER, 15.0),
    (&EntityType::RAVAGER, 12.0),
    (&EntityType::VEX, 8.0),
    (&EntityType::VINDICATOR, 10.0),
    (&EntityType::ZOGLIN, 10.0),
    (&EntityType::ZOMBIE, 8.0),
    (&EntityType::ZOMBIE_VILLAGER, 8.0),
];

fn hostile_distance(entity_type: &EntityType) -> Option<f64> {
    HOSTILE_DISTANCES
        .iter()
        .find(|(ty, _)| *ty == entity_type)
        .map(|(_, distance)| *distance)
}

pub struct VillagerPanicGoal;

impl VillagerPanicGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self)
    }

    /// `isHurt` (`VillagerPanicTrigger.java:45-47`): vanilla checks `HURT_BY` memory
    /// presence; see `RECENT_DAMAGE_TICKS` for the documented stand-in used here.
    async fn is_hurt(mob: &dyn Mob) -> bool {
        let living = &mob.get_mob_entity().living_entity;
        // `(sequence, damage tick, causes panic)`; the sequence only orders concurrent
        // writers, so read the tick and the flag like `escape_danger.rs` does.
        let (_, last_damage, causes_panic) = living.last_damage_state.load();
        if !causes_panic {
            return false;
        }
        let world = living.entity.world.load();
        let game_time = world.level_time.lock().await.world_age;
        last_damage >= 0 && game_time - last_damage <= RECENT_DAMAGE_TICKS
    }

    /// `hasHostile` (`VillagerPanicTrigger.java:41-43`) evaluated against the
    /// `VillagerHostilesSensor` threat table (`VillagerHostilesSensor.java:26-33`) instead of
    /// the sensor memory: per-type detection radius, nearest candidate wins. Like the
    /// existing `AvoidEntityGoal` set, candidates are filtered by liveness only - no
    /// visibility raycast.
    fn has_hostile(mob: &dyn Mob) -> bool {
        let entity = &mob.get_mob_entity().living_entity.entity;
        let pos = entity.pos.load();
        let world = entity.world.load();
        world
            .get_closest_entity_where(pos, HOSTILE_SCAN_RADIUS, None, |candidate| {
                candidate
                    .get_living_entity()
                    .is_some_and(LivingEntity::is_part_of_game)
                    && hostile_distance(candidate.get_entity().entity_type).is_some_and(
                        |distance| {
                            pos.squared_distance_to_vec(&candidate.get_entity().pos.load())
                                <= distance * distance
                        },
                    )
            })
            .is_some()
    }
}

impl Default for VillagerPanicGoal {
    fn default() -> Self {
        Self
    }
}

impl Goal for VillagerPanicGoal {
    /// `start`'s own gate (`VillagerPanicTrigger.java:21`): hurt or hostile present.
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { Self::is_hurt(mob).await || Self::has_hostile(mob) })
    }

    /// `canStillUse` (`VillagerPanicTrigger.java:16-18`): identical condition.
    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { Self::is_hurt(mob).await || Self::has_hostile(mob) })
    }

    /// `start` (`VillagerPanicTrigger.java:20-33`): entering panic erases the `PATH` and
    /// `WALK_TARGET` memories, i.e. drops whatever movement was in progress so the flee
    /// behaviors re-path from scratch. The `LOOK_TARGET`/`BREED_TARGET`/`INTERACTION_TARGET`
    /// erases have nothing to clear in the goal world (breed/interact targets live inside
    /// their goals' own fields); `setActiveActivityIfPossible(Activity.PANIC)` maps onto the
    /// goal-selector state machine itself.
    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
        })
    }

    /// `tick` (`VillagerPanicTrigger.java:35-39`): while panicking, every 100 ticks attempt
    /// `spawnGolemIfNeeded(level, timestamp, 3)`. The gating logic inside
    /// `spawn_golem_if_needed` (`LAST_SLEPT` recency + `GOLEM_DETECTED_RECENTLY`, checked
    /// again per nearby voter) is `Villager.java:834-852` and is reused verbatim.
    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() else {
                return;
            };
            let world = mob.get_entity().world.load();
            let game_time = world.level_time.lock().await.world_age;
            if game_time % GOLEM_CHECK_INTERVAL != 0 {
                return;
            }
            villager
                .spawn_golem_if_needed(&world, game_time, GOLEM_VILLAGERS_NEEDED_TO_AGREE)
                .await;
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    /// Marks the villager as panicking for `PathfinderMob.isPanicking` consumers
    /// (`goal/mod.rs:210-216`); vanilla expresses the same fact via `Activity.PANIC`.
    fn is_panic_goal(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{GOLEM_CHECK_INTERVAL, GOLEM_VILLAGERS_NEEDED_TO_AGREE, hostile_distance};
    use pumpkin_data::entity::EntityType;

    #[test]
    fn golem_cadence_matches_vanillas_mod_100_gate() {
        assert_eq!(GOLEM_CHECK_INTERVAL, 100);
        assert_eq!(GOLEM_VILLAGERS_NEEDED_TO_AGREE, 3);
    }

    #[test]
    fn hostile_table_matches_villager_hostiles_sensor() {
        assert_eq!(hostile_distance(&EntityType::DROWNED), Some(8.0));
        assert_eq!(hostile_distance(&EntityType::EVOKER), Some(12.0));
        assert_eq!(hostile_distance(&EntityType::HUSK), Some(8.0));
        assert_eq!(hostile_distance(&EntityType::ILLUSIONER), Some(12.0));
        assert_eq!(hostile_distance(&EntityType::PILLAGER), Some(15.0));
        assert_eq!(hostile_distance(&EntityType::RAVAGER), Some(12.0));
        assert_eq!(hostile_distance(&EntityType::VEX), Some(8.0));
        assert_eq!(hostile_distance(&EntityType::VINDICATOR), Some(10.0));
        assert_eq!(hostile_distance(&EntityType::ZOGLIN), Some(10.0));
        assert_eq!(hostile_distance(&EntityType::ZOMBIE), Some(8.0));
        assert_eq!(hostile_distance(&EntityType::ZOMBIE_VILLAGER), Some(8.0));
        assert_eq!(hostile_distance(&EntityType::PLAYER), None);
    }
}
