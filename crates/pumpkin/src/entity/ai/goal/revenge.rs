use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use super::{Controls, Goal};
use crate::entity::EntityBase;
use crate::entity::ai::goal::GoalFuture;
use crate::entity::ai::goal::track_target::TrackTargetGoal;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

/// Vanilla `Raider.class` membership check, approximated via the `#minecraft:raiders` tag
/// (Witch, Pillager, Vindicator, Evoker, Illusioner, Ravager, Ravager rider Pillager, etc.).
#[must_use]
pub fn is_raider(entity_type: &EntityType) -> bool {
    entity_type.has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
}

/// `HurtByTargetGoal.alertOthers` (`HurtByTargetGoal.java:75`) collects
/// `getEntitiesOfClass(this.mob.getClass(), ...)`, which includes subclasses. Of the alerting
/// mobs only `Zombie` has subclasses (`Husk`, `Drowned`, `ZombieVillager`, `ZombifiedPiglin`);
/// `ZombifiedPiglin` is excluded again by `setAlertOthers(ZombifiedPiglin.class)`
/// (`Zombie.java:124`), so a plain zombie alerts zombies, husks, drowned and zombie villagers.
fn alert_class_includes(own: &EntityType, other: &EntityType) -> bool {
    other == own
        || (own == &EntityType::ZOMBIE
            && [
                &EntityType::HUSK,
                &EntityType::DROWNED,
                &EntityType::ZOMBIE_VILLAGER,
            ]
            .contains(&other))
}

pub struct RevengeGoal {
    track_target_goal: TrackTargetGoal,
    target: Option<Arc<dyn EntityBase>>,
    last_attacked_time: i32,
    target_predicate: TargetPredicate,
    /// Vanilla `HurtByTargetGoal(this, Raider.class)`'s `toIgnoreDamage`: an attacker of this
    /// class is ignored entirely, so raid-mates don't retaliate against friendly fire from
    /// other raiders. Used by Vex (`Vex.java:93`) and Witch (`Witch.java:72`).
    exclude_raiders: bool,
    /// Vanilla `PolarBear.PolarBearHurtByTargetGoal::alertOther` override: only alert nearby
    /// same-species mobs that aren't babies (`PolarBear.java:296-301`).
    alert_only_adults: bool,
    /// Vanilla `PolarBear.PolarBearHurtByTargetGoal` never calls `setAlertOthers()`; its
    /// `start()` override calls `alertOthers()` only when the hurt bear is a baby
    /// (`PolarBear.java:284-291`). When set, gates the opt-in alert loop below on that condition.
    alert_only_when_self_is_baby: bool,
    /// Vanilla `HurtByTargetGoal.setAlertOthers` (`HurtByTargetGoal.java:53-57`) is opt-in.
    alert_others: bool,
    /// `PandaHurtByTargetGoal.alertOther` (`Panda.java:868-871`) only alerts aggressive pandas.
    alert_only_aggressive: bool,
    /// `HurtByTargetGoal(mob, ignoreDamageFromTheseTypes...)`: attackers of these exact types are
    /// ignored (`HurtByTargetGoal.java:41-45`). Shulker and Drowned pass their own class, which
    /// has no subclasses, so an exact type match is the vanilla `isAssignableFrom` test.
    ignore_damage_from: &'static [&'static EntityType],
    /// `Bee.BeeHurtByOtherGoal.alertOther` (`Bee.java:1010-1014`) only alerts when the hurt bee
    /// itself can see the attacker.
    alert_requires_line_of_sight: bool,
}

impl RevengeGoal {
    #[must_use]
    pub fn new(check_visibility: bool) -> Self {
        let target_predicate = TargetPredicate::create_attackable()
            .ignore_visibility()
            .ignore_distance_scaling_factor();
        Self {
            track_target_goal: TrackTargetGoal::with_default(check_visibility),
            target: None,
            last_attacked_time: 0,
            target_predicate,
            exclude_raiders: false,
            alert_only_adults: false,
            alert_only_when_self_is_baby: false,
            alert_others: false,
            alert_only_aggressive: false,
            ignore_damage_from: &[],
            alert_requires_line_of_sight: false,
        }
    }

    /// Passthrough for [`TrackTargetGoal::set_attackable_grace_ticks`].
    #[must_use]
    pub fn set_attackable_grace_ticks(mut self, server_ticks: i32) -> Self {
        self.track_target_goal = self
            .track_target_goal
            .set_attackable_grace_ticks(server_ticks);
        self
    }

    #[must_use]
    pub const fn exclude_raiders(mut self) -> Self {
        self.exclude_raiders = true;
        self
    }

    #[must_use]
    pub const fn alert_only_adults(mut self) -> Self {
        self.alert_only_adults = true;
        self
    }

    #[must_use]
    pub const fn alert_only_when_self_is_baby(mut self) -> Self {
        self.alert_only_when_self_is_baby = true;
        self
    }

    /// Mirrors `HurtByTargetGoal.setAlertOthers` (`HurtByTargetGoal.java:53-57`); callers opt into
    /// the `alertOthers` loop from `HurtByTargetGoal.start` (`HurtByTargetGoal.java:60-70`).
    #[must_use]
    pub const fn alert_others(mut self) -> Self {
        self.alert_others = true;
        self
    }

    /// Mirrors the `ignoreDamageFromTheseTypes` constructor varargs.
    #[must_use]
    pub const fn ignore_damage_from(mut self, types: &'static [&'static EntityType]) -> Self {
        self.ignore_damage_from = types;
        self
    }

    /// Mirrors Bee's `alertOther` line-of-sight requirement (`Bee.java:1010-1014`).
    #[must_use]
    pub const fn alert_requires_line_of_sight(mut self) -> Self {
        self.alert_requires_line_of_sight = true;
        self
    }

    /// Mirrors Panda's `alertOther` species filter (`Panda.java:868-871`).
    #[must_use]
    pub const fn alert_only_aggressive(mut self) -> Self {
        self.alert_only_aggressive = true;
        self
    }
}

const fn should_alert_other(
    alert_others: bool,
    alert_only_aggressive: bool,
    other_is_aggressive: bool,
) -> bool {
    // `HurtByTargetGoal.start` enters `alertOthers` only when `alertSameType` is set
    // (`HurtByTargetGoal.java:60-67`).
    alert_others && (!alert_only_aggressive || other_is_aggressive)
}

impl Goal for RevengeGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            let living = &mob_entity.living_entity;

            // `LivingEntity.getLastHurtByMobTimestamp` supplies this revenge-goal timestamp
            // (`LivingEntity.java:629-631`).
            let attacked_time = living.get_last_hurt_by_mob_timestamp();
            if attacked_time == self.last_attacked_time {
                return false;
            }

            let attacker_id = living.last_attacker_id.load(Relaxed);
            if attacker_id == 0 {
                return false;
            }

            let world = living.entity.world.load();
            let Some(attacker) = world.get_entity_by_id(attacker_id) else {
                return false;
            };

            let Some(attacker_living) = attacker.get_living_entity() else {
                return false;
            };

            if self.exclude_raiders && is_raider(attacker.get_entity().entity_type) {
                return false;
            }
            if self
                .ignore_damage_from
                .contains(&attacker.get_entity().entity_type)
            {
                return false;
            }

            // Vanilla `TamableAnimal::canAttack` unconditionally excludes the mob's own owner
            // from any attack target, regardless of which targeting goal found them; for
            // non-tameable mobs `get_owner_uuid()` is always `None` so this is a no-op.
            if mob.get_owner_uuid() == Some(attacker.get_entity().entity_uuid) {
                return false;
            }

            // Vanilla `TargetingConditions.test`'s combat branch (`TargetingConditions.java:78`)
            // consults `targeter.canAttack(target)`; `HurtByTargetGoal` reaches it through
            // `TargetGoal.canAttack`. This is what stops a player-created iron golem from
            // retaliating against the player who punched it.
            if !mob.can_attack(attacker.get_entity()) {
                return false;
            }

            if TrackTargetGoal::is_allied(mob, attacker.as_ref()).await {
                return false;
            }

            if !self
                .target_predicate
                .test(&world, Some(&mob_entity.living_entity), attacker_living)
                .await
            {
                return false;
            }

            self.target = Some(attacker);
            true
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { self.track_target_goal.should_continue(mob).await })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            mob.set_mob_target(self.target.clone()).await;

            let mob_entity = mob.get_mob_entity();
            self.last_attacked_time = mob_entity.living_entity.get_last_hurt_by_mob_timestamp();
            self.track_target_goal.max_time_without_visibility = 300;
            self.track_target_goal.start(mob).await;

            let Some(target) = self.target.as_ref() else {
                return;
            };
            if self.alert_only_when_self_is_baby && mob.get_entity().age.load(Relaxed) >= 0 {
                return;
            }
            let mob_entity = mob.get_mob_entity();
            let entity = &mob_entity.living_entity.entity;
            let world = entity.world.load();
            let position = entity.pos.load();
            let follow_range = mob_entity
                .living_entity
                .get_attribute_value(&Attributes::FOLLOW_RANGE);
            let entity_type = entity.entity_type;

            if !self.alert_others {
                return;
            }
            // `BeeHurtByOtherGoal.alertOther` tests the hurt bee's own line of sight to the
            // attacker, which is the same for every candidate.
            if self.alert_requires_line_of_sight
                && !mob_entity.has_line_of_sight(target.as_ref()).await
            {
                return;
            }
            // `TamableAnimal.getOwner` resolves the owner entity, so an owner that is not loaded
            // compares as `null` (`HurtByTargetGoal.java:88`).
            let resolved_owner = |owner: Option<uuid::Uuid>| {
                owner.filter(|uuid| world.get_player_by_uuid(*uuid).is_some())
            };
            let own_owner = mob
                .as_tamable()
                .map(|_| resolved_owner(mob.get_owner_uuid()));
            // `AABB.unitCubeFromLowerCorner(position).inflate(within, 10.0, within)`
            // (`HurtByTargetGoal.java:74`).
            let search_box = BoundingBox::new(
                Vector3::new(
                    position.x - follow_range,
                    position.y - 10.0,
                    position.z - follow_range,
                ),
                Vector3::new(
                    position.x + 1.0 + follow_range,
                    position.y + 11.0,
                    position.z + 1.0 + follow_range,
                ),
            );
            for nearby in world.get_entities_at_box(&search_box) {
                if nearby.get_entity().entity_id == entity.entity_id
                    || !alert_class_includes(entity_type, nearby.get_entity().entity_type)
                {
                    continue;
                }
                let other_is_aggressive = nearby
                    .cast_any()
                    .downcast_ref::<PandaEntity>()
                    .is_some_and(PandaEntity::is_aggressive_gene);
                if !should_alert_other(
                    self.alert_others,
                    self.alert_only_aggressive,
                    other_is_aggressive,
                ) {
                    continue;
                }
                let Some(nearby_mob) = nearby.get_mob() else {
                    continue;
                };
                if self.alert_only_adults && nearby.get_entity().age.load(Relaxed) < 0 {
                    continue;
                }
                if nearby_mob.get_mob_entity().target.lock().await.is_some() {
                    continue;
                }
                if own_owner
                    .is_some_and(|owner| owner != resolved_owner(nearby_mob.get_owner_uuid()))
                {
                    continue;
                }
                if TrackTargetGoal::entities_allied(nearby.as_ref(), target.as_ref()).await {
                    continue;
                }
                nearby_mob.set_mob_target(Some(target.clone())).await;
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.target = None;
            self.track_target_goal.stop(mob).await;
        })
    }

    fn controls(&self) -> Controls {
        self.track_target_goal.controls()
    }
}

#[cfg(test)]
mod tests {
    use super::{alert_class_includes, is_raider, should_alert_other};
    use pumpkin_data::entity::EntityType;

    #[test]
    fn zombie_alerts_its_subclasses_except_zombified_piglin() {
        let zombie = &EntityType::ZOMBIE;
        assert!(alert_class_includes(zombie, &EntityType::ZOMBIE));
        assert!(alert_class_includes(zombie, &EntityType::HUSK));
        assert!(alert_class_includes(zombie, &EntityType::DROWNED));
        assert!(alert_class_includes(zombie, &EntityType::ZOMBIE_VILLAGER));
        assert!(!alert_class_includes(zombie, &EntityType::ZOMBIFIED_PIGLIN));
        assert!(!alert_class_includes(&EntityType::HUSK, zombie));
        assert!(alert_class_includes(&EntityType::HUSK, &EntityType::HUSK));
    }

    #[test]
    fn raid_mates_are_raiders() {
        assert!(is_raider(&EntityType::WITCH));
        assert!(is_raider(&EntityType::PILLAGER));
        assert!(is_raider(&EntityType::EVOKER));
        assert!(is_raider(&EntityType::RAVAGER));
        assert!(is_raider(&EntityType::VINDICATOR));
        assert!(is_raider(&EntityType::ILLUSIONER));
    }

    #[test]
    fn non_raiders_are_not_raiders() {
        // Vex is a raid participant but not tagged `#minecraft:raiders` in vanilla data,
        // matching `Vex` not extending `Raider` (it implements `OwnableEntity` instead).
        assert!(!is_raider(&EntityType::VEX));
        assert!(!is_raider(&EntityType::ZOMBIE));
        assert!(!is_raider(&EntityType::PLAYER));
    }

    #[test]
    // `setAlertOthers` is opt-in before `HurtByTargetGoal.start` invokes `alertOthers`
    // (`HurtByTargetGoal.java:53-67`).
    fn alert_others_is_opt_in() {
        assert!(!should_alert_other(false, false, true));
        assert!(should_alert_other(true, false, false));
        assert!(should_alert_other(true, true, true));
        assert!(!should_alert_other(true, true, false));
    }
}
