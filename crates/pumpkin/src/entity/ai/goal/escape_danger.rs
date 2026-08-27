use std::sync::atomic::Ordering::Relaxed;

use super::{Controls, Goal, GoalFuture};
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::random_pos::default_get_pos;

const RANGE: i32 = 5;
const RECENT_DAMAGE_TICKS: i64 = 40;

const fn panic_damage_is_recent(last_damage: i64, game_time: i64) -> bool {
    last_damage >= 0 && game_time - last_damage <= RECENT_DAMAGE_TICKS
}

pub struct EscapeDangerGoal {
    speed: f64,
    goal_control: Controls,
    target: Option<Vector3<f64>>,
}

impl EscapeDangerGoal {
    #[must_use]
    pub fn new(speed: f64) -> Box<Self> {
        Box::new(Self {
            speed,
            goal_control: Controls::MOVE,
            target: None,
        })
    }

    async fn is_in_danger(mob: &dyn Mob) -> bool {
        let living = &mob.get_mob_entity().living_entity;

        // `last_damage_state` is (sequence, tick, causes_panic); the sequence only orders
        // concurrent writers, so the goal reads the tick and the flag.
        let (_, last_damage, causes_panic) = living.last_damage_state.load();
        if !causes_panic {
            return false;
        }

        let world = living.entity.world.load();
        let game_time = world.level_time.lock().await.world_age;
        panic_damage_is_recent(last_damage, game_time)
    }

    fn find_water_target(mob: &dyn Mob) -> Option<BlockPos> {
        let entity = mob.get_entity();
        let origin = entity.block_pos.load();
        let world = entity.world.load();

        // `PanicGoal.lookForWater` refuses to search when the mob's current block has a
        // collision shape, then finds the closest water fluid within 5 horizontal and 1
        // vertical block.
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
        for depth in 0..=(RANGE + 1 + RANGE) {
            let max_x = RANGE.min(depth);
            for x in -max_x..=max_x {
                let max_y = 1.min(depth - x.abs());
                for y in -max_y..=max_y {
                    let z = depth - x.abs() - y.abs();
                    if z > RANGE {
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

    fn find_escape_target(mob: &dyn Mob) -> Option<Vector3<f64>> {
        if mob.get_entity().fire_ticks.load(Relaxed) > 0
            && let Some(water) = Self::find_water_target(mob)
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
            if !Self::is_in_danger(mob).await {
                return false;
            }
            self.target = Self::find_escape_target(mob);
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

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

#[cfg(test)]
mod tests {
    use super::panic_damage_is_recent;

    #[test]
    fn panic_damage_expires_after_vanillas_forty_ticks() {
        assert!(panic_damage_is_recent(100, 140));
        assert!(!panic_damage_is_recent(100, 141));
        assert!(!panic_damage_is_recent(-1, 0));
    }
}
