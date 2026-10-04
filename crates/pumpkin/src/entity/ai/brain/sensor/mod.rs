//! Port of `net.minecraft.world.entity.ai.sensing` (26.2 decompile).
//!
//! Sensors are the only legitimate writers of *sensed world state* memories; behaviors write
//! intent memories (`WALK_TARGET`, `LOOK_TARGET`, ...).

pub mod nearest_item;
pub mod nearest_living_entities;

use std::pin::Pin;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_util::Difficulty;

use crate::entity::EntityBase;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::MemoryKeyId;
use crate::entity::ai::goal::track_target::TrackTargetGoal;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;

pub type SensorFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// `Sensor<E>` (`sensing/Sensor.java:13-64`).
///
/// Unlike [`super::behavior::Behavior`] this is async, because reading nearby entities and
/// their item stacks in Pumpkin goes through `tokio::sync::Mutex`. Implementations MUST copy
/// what they need out of the world first and only then take the brain's memory lock: the
/// memory guard is a `std::sync::Mutex` guard and must never be held across an `.await`.
pub trait Sensor: Send {
    /// `Sensor.requires()` (`sensing/Sensor.java:64`). Every returned memory is registered on
    /// the brain at construction (`Brain.java:87-89`).
    fn requires(&self) -> &[MemoryKeyId];

    /// `Sensor.doTick` (`sensing/Sensor.java:62`).
    fn do_tick<'a>(&'a mut self, mob: &'a dyn Mob, brain: &'a Brain) -> SensorFuture<'a>;

    /// Countdown state backing `Sensor.tick`'s scan-rate gate.
    fn ticks_until_scan(&mut self) -> &mut i64;

    /// `Sensor.DEFAULT_SCAN_RATE = 20` (`sensing/Sensor.java:14,36-38`).
    fn scan_rate(&self) -> i64 {
        20
    }

    /// `Sensor.tick` (`sensing/Sensor.java:44-50`): pre-decrement, fire at `<= 0`, reset to the
    /// full scan rate.
    ///
    /// `updateTargetingConditionRanges` (`:52-60`) is not ported: it mutates shared static
    /// `TargetingConditions` singletons, which has no Rust analogue and is only consulted by
    /// the targeting sensors this stage does not port.
    fn tick<'a>(&'a mut self, mob: &'a dyn Mob, brain: &'a Brain) -> SensorFuture<'a> {
        let scan_rate = self.scan_rate();
        let remaining = self.ticks_until_scan();
        *remaining -= 1;
        if *remaining > 0 {
            return Box::pin(async {});
        }
        *remaining = scan_rate;
        self.do_tick(mob, brain)
    }
}

/// `Sensor.randomlyDelayStart` (`sensing/Sensor.java:40-42`), called once per sensor at brain
/// creation (`Brain.java:84`) so that mobs spawned in the same tick do not all scan on the
/// same tick.
#[must_use]
pub fn randomly_delayed_start(scan_rate: i64) -> i64 {
    use rand::RngExt;
    rand::rng().random_range(0..scan_rate)
}

/// `Sensor.isEntityAttackable` (`check_line_of_sight = true`) and
/// `Sensor.isEntityAttackableIgnoringLineOfSight` (`false`), `sensing/Sensor.java:72-86`.
///
/// This is the combat `TargetingConditions.test` (`targeting/TargetingConditions.java:59-96`),
/// with the invisibility scaling dropped when `target` is already the mob's attack target.
///
/// DEVIATION: the range is the mob's own `FOLLOW_RANGE`; vanilla's static conditions carry the
/// range of whichever mob last ran `updateTargetingConditionRanges` (`Sensor.java:52-60`).
pub async fn is_entity_attackable(
    mob: &dyn Mob,
    target: &dyn EntityBase,
    check_line_of_sight: bool,
) -> bool {
    let Some(target_living) = target.get_living_entity() else {
        return false;
    };
    let mob_entity = mob.get_mob_entity();
    let world = mob_entity.living_entity.entity.world.load_full();
    let target_entity = target.get_entity();

    // `body.getBrain().isMemoryValue(ATTACK_TARGET, target)`.
    let current_target = mob_entity.target.lock().await.clone();
    let is_current_target = current_target
        .is_some_and(|current| current.get_entity().entity_id == target_entity.entity_id);

    // Combat branch (`TargetingConditions.java:77-80`): `Mob.canAttack` excludes ghasts
    // (`Mob.java:256-258`) on top of species overrides; `LivingEntity.canAttack`
    // (`LivingEntity.java:948-950`) refuses players in Peaceful and anything that cannot be
    // seen as an enemy; then `isAlliedTo`.
    if target_entity.entity_type.id == EntityType::GHAST.id
        || !mob.can_attack(target_entity)
        || (target_entity.entity_type.id == EntityType::PLAYER.id
            && world.level_info.load().difficulty == Difficulty::Peaceful)
        || !target_living.can_be_seen_as_enemy()
        || TrackTargetGoal::is_allied(mob, target).await
    {
        return false;
    }

    // Self check, `canBeSeenByAnyone` and the visibility-scaled range with its 2.0 floor
    // (`TargetingConditions.java:60-66,81-89`). The non-attackable predicate is used because
    // the combat checks were done above: its attackable branch refuses every target in
    // Peaceful, which vanilla only does with no targeter.
    let mut predicate = TargetPredicate::create_non_attackable()
        .set_base_max_distance(
            mob_entity
                .living_entity
                .get_attribute_value(&Attributes::FOLLOW_RANGE),
        )
        .ignore_visibility();
    if is_current_target {
        predicate = predicate.ignore_distance_scaling_factor();
    }
    if !predicate
        .test(&world, Some(&mob_entity.living_entity), target_living)
        .await
    {
        return false;
    }

    // `mob.getSensing().hasLineOfSight(target)` (`TargetingConditions.java:91-93`).
    !check_line_of_sight || mob_entity.has_line_of_sight(target).await
}
