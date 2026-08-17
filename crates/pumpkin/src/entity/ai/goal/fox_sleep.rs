use std::sync::atomic::Ordering::{Relaxed, SeqCst};

use pumpkin_data::entity::{EntityPose, EntityType, MobCategory};
use rand::RngExt;

use super::{Controls, Goal, GoalFuture};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::passive::fox::FoxEntity;
use crate::world::World;

const WAIT_TIME_BEFORE_SLEEP: i32 = 140;
const BRIGHT_OUTSIDE_THRESHOLD: u8 = 4;
const ALERT_RADIUS: f64 = 12.0;
const ALERT_VERTICAL_RADIUS: f64 = 6.0;
// Broad candidate radius around the fox. The final test below uses the exact expanded
// bounding boxes from vanilla's getNearbyEntities query.
const ALERT_SEARCH_RADIUS: f64 = 24.0;

fn is_bright_outside(world: &World) -> bool {
    world.dimension.has_skylight && world.sky_darken.load(Relaxed) < BRIGHT_OUTSIDE_THRESHOLD
}

fn is_untamed_tamable(other: &dyn EntityBase) -> bool {
    let entity_type = other.get_entity().entity_type;
    let tamable_type = [
        &EntityType::CAT,
        &EntityType::WOLF,
        &EntityType::PARROT,
    ]
    .into_iter()
    .any(|tamable| entity_type == tamable);
    tamable_type
        && other
            .get_mob()
            .is_some_and(|mob| !mob.get_mob_entity().is_tamed())
}

/// `Fox.FoxAlertableEntitiesSelector`/`FoxBehaviorGoal.alertable`: whether anything nearby
/// makes it unsafe to sleep. The server-side player checks here mirror vanilla's selector:
/// creative and spectator players, sleeping players, and trusted players do not alert a fox.
fn alertable(mob: &dyn Mob) -> bool {
    let entity = mob.get_entity();
    let world = entity.world.load();
    let pos = entity.pos.load();
    let self_uuid = entity.entity_uuid;
    let alert_box =
        entity
            .bounding_box
            .load()
            .expand(ALERT_RADIUS, ALERT_VERTICAL_RADIUS, ALERT_RADIUS);

    world
        .get_nearby_entities(pos, ALERT_SEARCH_RADIUS)
        .into_iter()
        .any(|(uuid, other)| {
            if uuid == self_uuid {
                return false;
            }
            if !alert_box.intersects(&other.get_entity().bounding_box.load()) {
                return false;
            }
            let other_type = other.get_entity().entity_type;
            if other_type == &EntityType::FOX {
                return false;
            }
            if other_type == &EntityType::PLAYER {
                let Some(player) = other.get_player() else {
                    return false;
                };
                let trusted = mob
                    .cast_any()
                    .downcast_ref::<FoxEntity>()
                    .is_some_and(|fox| fox.trusts(uuid));
                return !trusted
                    && !player.is_creative()
                    && !player.is_spectator()
                    && !other.get_entity().is_sneaking()
                    && player.get_entity().pose.load() != EntityPose::Sleeping;
            }
            if is_untamed_tamable(other.as_ref()) {
                return true;
            }
            other_type.category == &MobCategory::MONSTER
                || other_type == &EntityType::CHICKEN
                || other_type == &EntityType::RABBIT
        })
}

/// `Fox.SleepGoal`: after a randomized wait, sleeps during the day in a sheltered spot with
/// nothing nearby to disturb it.
///
/// Vanilla's `canUse` additionally requires `xxa == 0 && yya == 0 && zza == 0` (no AI
/// movement input currently applied); this codebase approximates that with an idle navigator.
/// `hasShelter` still drops vanilla's `getWalkTargetValue(pos) >= 0.0F` half because the world
/// API has no equivalent primitive, and keeps the `!canSeeSky` half.
pub struct FoxSleepGoal {
    countdown: i32,
}

impl FoxSleepGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            countdown: rand::rng().random_range(0..WAIT_TIME_BEFORE_SLEEP),
        })
    }

    fn can_sleep(&mut self, mob: &dyn Mob) -> bool {
        if self.countdown > 0 {
            self.countdown -= 1;
            return false;
        }

        let entity = mob.get_entity();
        let world = entity.world.load();
        let pos = entity.block_pos.load();

        is_bright_outside(&world)
            && !world.can_see_sky(&pos)
            && !alertable(mob)
            && !entity.is_in_powder_snow.load(Relaxed)
    }
}

impl Goal for FoxSleepGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() else {
                return false;
            };
            let navigator_idle = mob.get_mob_entity().navigator.lock().unwrap().is_idle();
            if !navigator_idle && !fox.is_sleeping() {
                return false;
            }
            self.can_sleep(mob) || fox.is_sleeping()
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { self.can_sleep(mob) })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() {
                fox.set_sitting(false);
                fox.set_is_crouching(false);
                fox.set_is_interested(false);
                fox.set_sleeping(true);
            }
            mob.get_mob_entity()
                .living_entity
                .jumping
                .store(false, SeqCst);
            mob.get_mob_entity().navigator.lock().unwrap().stop();
            let pos = mob.get_entity().pos.load();
            mob.get_mob_entity()
                .move_control
                .lock()
                .unwrap()
                .set_wanted_position(pos.x, pos.y, pos.z, 0.0);
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.countdown = mob.get_random().random_range(0..WAIT_TIME_BEFORE_SLEEP);
            if let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() {
                fox.clear_states();
            }
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}
