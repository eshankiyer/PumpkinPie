use std::sync::atomic::Ordering::Relaxed;

use super::{Controls, Goal, GoalFuture, follow_owner::FollowOwnerGoal};
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob};
use pumpkin_data::damage::DamageType;
use pumpkin_data::tag::{self, Tag, Taggable};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::random_pos::default_get_pos;

const RANGE: i32 = 5;
/// `PanicGoal.lookForWater(level, mob, 5)` from `PanicGoal.canUse` (`PanicGoal.java:49`).
const WATER_RANGE: i32 = 5;
const RECENT_DAMAGE_TICKS: i64 = 40;

const fn panic_damage_is_recent(last_damage: i64, game_time: i64) -> bool {
    last_damage >= 0 && game_time - last_damage <= RECENT_DAMAGE_TICKS
}

/// Whether the stored last damage type id (`LivingEntity::last_damage_state`) is in `tag`.
#[must_use]
pub fn damage_type_id_has_tag(damage_type_id: Option<u8>, tag: &'static Tag) -> bool {
    damage_type_id
        .and_then(DamageType::from_id)
        .is_some_and(|damage_type| damage_type.has_tag(tag))
}

/// `PanicGoal.panicCausingDamageTypes` (`PanicGoal.java:25-40`), chosen per goal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanicCauses {
    /// `DamageTypeTags.PANIC_CAUSES`, the default constructor.
    #[default]
    Default,
    /// `DamageTypeTags.PANIC_ENVIRONMENTAL_CAUSES` (wolf, panda, armadillo).
    Environmental,
    /// `PolarBear.java:90`: `PANIC_CAUSES` for babies, `PANIC_ENVIRONMENTAL_CAUSES` otherwise.
    BabyDefaultElseEnvironmental,
}

impl PanicCauses {
    const fn tag(self, is_baby: bool) -> &'static Tag {
        match self {
            Self::Default => &tag::DamageType::MINECRAFT_PANIC_CAUSES,
            Self::Environmental => &tag::DamageType::MINECRAFT_PANIC_ENVIRONMENTAL_CAUSES,
            Self::BabyDefaultElseEnvironmental => {
                if is_baby {
                    &tag::DamageType::MINECRAFT_PANIC_CAUSES
                } else {
                    &tag::DamageType::MINECRAFT_PANIC_ENVIRONMENTAL_CAUSES
                }
            }
        }
    }
}

pub struct EscapeDangerGoal {
    speed: f64,
    goal_control: Controls,
    target: Option<Vector3<f64>>,
    /// `TamableAnimal.TamableAnimalPanicGoal`: also teleports to a far-away owner each tick.
    tamable: bool,
    panic_causes: PanicCauses,
    /// Extra `shouldPanic` condition from a subclass override, checked first.
    extra_gate: Option<fn(&dyn Mob) -> bool>,
    /// `Turtle.TurtlePanicGoal.canUse` (`Turtle.java:532-546`): always looks for water within
    /// this horizontal range, not only while on fire.
    water_always: Option<i32>,
}

impl EscapeDangerGoal {
    #[must_use]
    pub fn new(speed: f64) -> Box<Self> {
        Box::new(Self {
            speed,
            goal_control: Controls::MOVE,
            target: None,
            tamable: false,
            panic_causes: PanicCauses::Default,
            extra_gate: None,
            water_always: None,
        })
    }

    /// Vanilla `TamableAnimal.TamableAnimalPanicGoal` (`TamableAnimal.java:298-316`).
    #[must_use]
    pub fn new_tamable(speed: f64) -> Box<Self> {
        Box::new(Self {
            speed,
            goal_control: Controls::MOVE,
            target: None,
            tamable: true,
            panic_causes: PanicCauses::Default,
            extra_gate: None,
            water_always: None,
        })
    }

    /// Selects the damage-type tag that makes this goal panic.
    #[must_use]
    pub fn with_panic_causes(mut self: Box<Self>, panic_causes: PanicCauses) -> Box<Self> {
        self.panic_causes = panic_causes;
        self
    }

    /// Adds a subclass `shouldPanic` condition evaluated before the damage check.
    #[must_use]
    pub fn with_extra_gate(mut self: Box<Self>, gate: fn(&dyn Mob) -> bool) -> Box<Self> {
        self.extra_gate = Some(gate);
        self
    }

    /// Always searches for water within `xz_range` before a random position.
    #[must_use]
    pub fn water_always(mut self: Box<Self>, xz_range: i32) -> Box<Self> {
        self.water_always = Some(xz_range);
        self
    }

    /// `PanicGoal.shouldPanic` (`PanicGoal.java:61-63`).
    async fn is_in_danger(&self, mob: &dyn Mob) -> bool {
        if self.extra_gate.is_some_and(|gate| !gate(mob)) {
            return false;
        }
        let living = &mob.get_mob_entity().living_entity;

        // `last_damage_state` is (sequence, tick, damage type id); the sequence only orders
        // concurrent writers, so the goal reads the tick and the damage type.
        let (_, last_damage, damage_type_id) = living.last_damage_state.load();
        let is_baby = living.entity.age.load(Relaxed) < 0;
        if !damage_type_id_has_tag(damage_type_id, self.panic_causes.tag(is_baby)) {
            return false;
        }

        let world = living.entity.world.load();
        let game_time = world.level_time.lock().await.world_age;
        panic_damage_is_recent(last_damage, game_time)
    }

    fn find_water_target(mob: &dyn Mob, xz_range: i32) -> Option<BlockPos> {
        let entity = mob.get_entity();
        let origin = entity.block_pos.load();
        let world = entity.world.load();

        // `PanicGoal.lookForWater` refuses to search when the mob's current block has a
        // collision shape, then finds the closest water fluid within `xz_range` horizontal and
        // 1 vertical block.
        if world
            .get_block_state(&origin)
            .get_block_collision_shapes()
            .next()
            .is_some()
        {
            return None;
        }

        // This is the iteration order of `BlockPos.withinManhattan`, which checks positive Z
        // before its mirrored negative-Z position at each distance.
        for depth in 0..=(xz_range + 1 + xz_range) {
            let max_x = xz_range.min(depth);
            for x in -max_x..=max_x {
                let max_y = 1.min(depth - x.abs());
                for y in -max_y..=max_y {
                    let z = depth - x.abs() - y.abs();
                    if z > xz_range {
                        continue;
                    }
                    let positive = BlockPos::new(origin.0.x + x, origin.0.y + y, origin.0.z + z);
                    if world
                        .get_fluid_and_fluid_state(&positive)
                        .0
                        .has_tag(&tag::Fluid::MINECRAFT_WATER)
                    {
                        return Some(positive);
                    }
                    if z != 0 {
                        let negative =
                            BlockPos::new(origin.0.x + x, origin.0.y + y, origin.0.z - z);
                        if world
                            .get_fluid_and_fluid_state(&negative)
                            .0
                            .has_tag(&tag::Fluid::MINECRAFT_WATER)
                        {
                            return Some(negative);
                        }
                    }
                }
            }
        }
        None
    }

    fn find_escape_target(&self, mob: &dyn Mob) -> Option<Vector3<f64>> {
        let water_range = self
            .water_always
            .or_else(|| (mob.get_entity().fire_ticks.load(Relaxed) > 0).then_some(WATER_RANGE));
        if let Some(xz_range) = water_range
            && let Some(water) = Self::find_water_target(mob, xz_range)
        {
            return Some(water.to_f64());
        }

        default_get_pos(mob, RANGE, 4)
    }
}

impl Goal for EscapeDangerGoal {
    fn is_panic_goal(&self) -> bool {
        true
    }

    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if !self.is_in_danger(mob).await {
                return false;
            }
            self.target = self.find_escape_target(mob);
            self.target.is_some()
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            !navigator.is_idle()
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(target) = self.target {
                let pos = mob.get_mob_entity().living_entity.entity.pos.load();
                let mut navigator = mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                navigator.set_progress(NavigatorGoal::new(pos, target, self.speed));
            }
        })
    }

    fn stop<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.target = None;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if !self.tamable || FollowOwnerGoal::blocked_from_owner(mob).await {
                return;
            }
            if let Some(owner) = FollowOwnerGoal::find_owner(mob)
                && FollowOwnerGoal::should_try_teleport_to_owner(mob, &owner)
            {
                FollowOwnerGoal::try_teleport_to_owner(mob, &owner);
            }
        })
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

#[cfg(test)]
mod tests {
    use super::{PanicCauses, damage_type_id_has_tag, panic_damage_is_recent};
    use pumpkin_data::damage::DamageType;

    #[test]
    fn environmental_panic_ignores_attacks() {
        let tag = PanicCauses::Environmental.tag(false);
        assert!(damage_type_id_has_tag(Some(DamageType::LAVA.id), tag));
        assert!(damage_type_id_has_tag(Some(DamageType::CACTUS.id), tag));
        assert!(!damage_type_id_has_tag(
            Some(DamageType::PLAYER_ATTACK.id),
            tag
        ));
        assert!(!damage_type_id_has_tag(
            Some(DamageType::MOB_ATTACK.id),
            tag
        ));
        assert!(!damage_type_id_has_tag(None, tag));
    }

    #[test]
    fn baby_polar_bear_panics_from_attacks() {
        let causes = PanicCauses::BabyDefaultElseEnvironmental;
        let attack = Some(DamageType::PLAYER_ATTACK.id);
        assert!(damage_type_id_has_tag(attack, causes.tag(true)));
        assert!(!damage_type_id_has_tag(attack, causes.tag(false)));
        assert!(damage_type_id_has_tag(
            attack,
            PanicCauses::Default.tag(false)
        ));
    }

    #[test]
    fn panic_damage_expires_after_vanillas_forty_ticks() {
        assert!(panic_damage_is_recent(100, 140));
        assert!(!panic_damage_is_recent(100, 141));
        assert!(!panic_damage_is_recent(-1, 0));
    }
}
