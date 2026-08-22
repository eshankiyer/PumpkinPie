//! Shared helpers for the `Fox.FoxBehaviorGoal` family (`Fox.java:1103-1131`) and the
//! world predicates its subclasses gate on.
//!
//! `alertable` here is the same predicate `fox_sleep.rs` keeps privately
//! (`Fox.FoxAlertableEntitiesSelector`, `Fox.java:1119-1122`); it is duplicated rather than
//! shared because the sleep goal predates this module.

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::Block;
use pumpkin_data::entity::{EntityPose, EntityType, MobCategory};
use pumpkin_util::math::position::BlockPos;

use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::passive::fox::FoxEntity;
use crate::world::World;

const BRIGHT_OUTSIDE_THRESHOLD: u8 = 4;
const ALERT_RADIUS: f64 = 12.0;
const ALERT_VERTICAL_RADIUS: f64 = 6.0;
const ALERT_SEARCH_RADIUS: f64 = 24.0;

/// `Level.isBrightOutside` (`Level.java:385-387`).
#[must_use]
pub fn is_bright_outside(world: &World) -> bool {
    world.dimension.fixed_time.is_none()
        && world.sky_darken.load(Relaxed) < BRIGHT_OUTSIDE_THRESHOLD
}

/// `Animal.getWalkTargetValue` (`Animal.java:91-94`).
///
/// Grass below scores 10; everything else falls back to `LevelReader.getPathfindingCostFromLightLevels`
/// (`LevelReader.java:108-116`), which is negative in the dark.
#[must_use]
pub fn animal_walk_target_value(world: &World, pos: &BlockPos) -> f32 {
    if world.get_block(&pos.down()) == &Block::GRASS_BLOCK {
        return 10.0;
    }
    let brightness = f32::from(world.get_max_local_raw_brightness(pos)) / 15.0;
    let curved = brightness / (4.0 - 3.0 * brightness);
    let magic = curved + world.dimension.ambient_light * (1.0 - curved);
    magic - 0.5
}

/// `Fox.FoxStrollThroughVillageGoal.canFoxMove` (`Fox.java:1298-1300`).
pub async fn can_fox_move(mob: &dyn Mob, fox: &FoxEntity) -> bool {
    !fox.is_sleeping()
        && !fox.is_sitting()
        && !fox.is_defending()
        && mob.get_mob_entity().target.lock().await.is_none()
}

fn is_untamed_tamable(other: &dyn EntityBase) -> bool {
    let entity_type = other.get_entity().entity_type;
    [&EntityType::CAT, &EntityType::WOLF, &EntityType::PARROT]
        .into_iter()
        .any(|tamable| entity_type == tamable)
        && other
            .get_mob()
            .is_some_and(|mob| !mob.get_mob_entity().is_tamed())
}

/// `Fox.FoxBehaviorGoal.alertable` (`Fox.java:1119-1122`).
#[must_use]
pub fn alertable(mob: &dyn Mob) -> bool {
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
