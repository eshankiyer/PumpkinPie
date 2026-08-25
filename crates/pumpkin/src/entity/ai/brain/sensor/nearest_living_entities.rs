//! Port of `sensing/NearestLivingEntitySensor.java`.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;

use crate::entity::EntityBase;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::{
    MemoryKeyId, NearestLivingEntitiesMemory, NearestVisibleLivingEntities,
    NearestVisibleLivingEntitiesMemory,
};
use crate::entity::ai::brain::sensor::{Sensor, SensorFuture, randomly_delayed_start};
use crate::entity::mob::{Mob, MobEntity};

const REQUIRES: [MemoryKeyId; 2] = [
    MemoryKeyId::NearestLivingEntities,
    MemoryKeyId::NearestVisibleLivingEntities,
];

pub struct NearestLivingEntitySensor {
    ticks_until_scan: i64,
}

impl NearestLivingEntitySensor {
    // Returns a boxed trait object, not Self by name -- constructor pattern for this behavior/sensor family.
    #[allow(clippy::new_ret_no_self)]
    #[must_use]
    pub fn new() -> Box<dyn Sensor> {
        Box::new(Self {
            ticks_until_scan: randomly_delayed_start(20),
        })
    }

    /// `TargetingConditions.test` (`targeting/TargetingConditions.java:64-97`) for the
    /// non-combat conditions that `Sensor.isEntityTargetable` uses
    /// (`sensing/Sensor.java:16,66-70`): the combat-only branches do not apply, so the test
    /// is `canBeSeenByAnyone` + the visibility-modified range check + line of sight. The
    /// shared-static range mutation from `Sensor.updateTargetingConditionRanges`
    /// (`sensing/Sensor.java:52-60`) is folded in as the `follow_range` parameter instead.
    async fn is_targetable(
        mob_entity: &MobEntity,
        observer_type: &EntityType,
        follow_range: f64,
        distance_sq: f64,
        candidate: &Arc<dyn EntityBase>,
    ) -> bool {
        // `!target.canBeSeenByAnyone()` -> false
        // (LivingEntity.java:956-958: !isSpectator() && isAlive()).
        let target = candidate.get_entity();
        if target.is_spectator() || !target.is_alive() {
            return false;
        }

        // Range check with the invisibility modifier
        // (`TargetingConditions.java:81-89`, modifier from
        // `LivingEntity.getVisibilityPercent`, LivingEntity.java:919-946).
        let modifier = Self::visibility_percent(observer_type, candidate).await;
        let visibility_distance = (follow_range * modifier).max(2.0);
        if distance_sq > visibility_distance * visibility_distance {
            return false;
        }

        // `targeter instanceof Mob mob && !mob.getSensing().hasLineOfSight(target)`
        // (`TargetingConditions.java:91-93`); sensors only run on mobs.
        mob_entity.has_line_of_sight(candidate.as_ref()).await
    }

    /// `LivingEntity.getVisibilityPercent` (`LivingEntity.java:919-946`), evaluated on the
    /// observed entity with the observer's type driving the worn-head disguise branch.
    ///
    /// DEVIATION: vanilla reads the head slot synchronously; here the equipment map is a
    /// tokio mutex, so the read is awaited inside this helper before any brain lock is taken.
    async fn visibility_percent(observer_type: &EntityType, target: &Arc<dyn EntityBase>) -> f64 {
        let mut percent = 1.0;

        let entity = target.get_entity();
        // `isDiscrete()` -> the sneaking flag (`Entity.java`).
        if entity.sneaking.load(Ordering::Relaxed) {
            percent *= 0.8;
        }
        if entity.invisible.load(Ordering::Relaxed) {
            let Some(living) = target.get_living_entity() else {
                return percent;
            };
            // `getArmorCoverPercentage` (`LivingEntity.java:2289-2308`): fraction of the four
            // humanoid armor slots that hold an item, floored at 0.1 by the caller above.
            let equipment = living.entity_equipment.lock().await;
            let armor_slots = [
                EquipmentSlot::FEET,
                EquipmentSlot::LEGS,
                EquipmentSlot::CHEST,
                EquipmentSlot::HEAD,
            ];
            let covered = armor_slots
                .iter()
                .filter(|slot| !equipment.get(slot).is_empty())
                .count();
            let cover_percentage = f64::from(covered as u32) / f64::from(armor_slots.len() as u32);
            percent *= 0.7 * cover_percentage.max(0.1);
        }

        // Worn-head disguise (`LivingEntity.java:934-943`): half visibility against the mob
        // type whose head is being worn.
        if let Some(living) = target.get_living_entity() {
            let head_item = living
                .entity_equipment
                .lock()
                .await
                .get(&EquipmentSlot::HEAD);
            let disguised = (observer_type == &EntityType::SKELETON
                && head_item.item == &Item::SKELETON_SKULL)
                || (observer_type == &EntityType::ZOMBIE && head_item.item == &Item::ZOMBIE_HEAD)
                || ((observer_type == &EntityType::PIGLIN
                    || observer_type == &EntityType::PIGLIN_BRUTE)
                    && head_item.item == &Item::PIGLIN_HEAD)
                || (observer_type == &EntityType::CREEPER && head_item.item == &Item::CREEPER_HEAD);
            if disguised {
                percent *= 0.5;
            }
        }

        percent
    }
}

impl Sensor for NearestLivingEntitySensor {
    /// `requires()` (`NearestLivingEntitySensor.java:28-30`).
    fn requires(&self) -> &[MemoryKeyId] {
        &REQUIRES
    }

    fn ticks_until_scan(&mut self) -> &mut i64 {
        &mut self.ticks_until_scan
    }

    /// `doTick` (`NearestLivingEntitySensor.java:17-25`): collect every other living entity
    /// in a FOLLOW_RANGE-inflated bounding box, sort nearest-first, write the list into
    /// `NEAREST_LIVING_ENTITIES` and its per-entity visibility snapshot into
    /// `NEAREST_VISIBLE_LIVING_ENTITIES`.
    fn do_tick<'a>(&'a mut self, mob: &'a dyn Mob, brain: &'a Brain) -> SensorFuture<'a> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            let living_entity = &mob_entity.living_entity;
            let entity = &living_entity.entity;
            let mob_pos = entity.pos.load();
            let world = entity.world.load();
            let observer_type = entity.entity_type;

            let follow_range = living_entity.get_attribute_value(&Attributes::FOLLOW_RANGE);
            let search_box =
                entity
                    .bounding_box
                    .load()
                    .expand(follow_range, follow_range, follow_range);

            // `level.getEntitiesOfClass(LivingEntity.class, boundingBox, mob -> mob != body &&
            // mob.isAlive())` (`NearestLivingEntitySensor.java:20`).
            let mut candidates: Vec<(f64, Arc<dyn EntityBase>)> = world
                .get_entities_at_box(&search_box)
                .into_iter()
                .filter_map(|candidate| {
                    if candidate.get_entity().entity_id == entity.entity_id
                        || candidate.get_living_entity().is_none()
                        || !candidate.get_entity().is_alive()
                    {
                        return None;
                    }
                    let distance = candidate
                        .get_entity()
                        .pos
                        .load()
                        .squared_distance_to_vec(&mob_pos);
                    Some((distance, candidate))
                })
                .collect();

            // `livingEntities.sort(Comparator.comparingDouble(body::distanceToSqr))`
            // (`NearestLivingEntitySensor.java:21`). This order is preserved into both
            // memories, which is what makes findClosest "nearest".
            candidates.sort_by(|a, b| a.0.total_cmp(&b.0));

            let mut nearby: Vec<Weak<dyn EntityBase>> = Vec::with_capacity(candidates.len());
            let mut visible: Vec<(Weak<dyn EntityBase>, bool)> =
                Vec::with_capacity(candidates.len());
            for (distance_sq, candidate) in candidates {
                nearby.push(Arc::downgrade(&candidate));
                let targetable = Self::is_targetable(
                    mob_entity,
                    observer_type,
                    follow_range,
                    distance_sq,
                    &candidate,
                )
                .await;
                visible.push((Arc::downgrade(&candidate), targetable));
            }

            brain.set::<NearestLivingEntitiesMemory>(nearby);
            brain.set::<NearestVisibleLivingEntitiesMemory>(NearestVisibleLivingEntities::new(
                visible,
            ));
        })
    }
}
