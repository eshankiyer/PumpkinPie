use std::sync::atomic::Ordering::Relaxed;

use super::breed::BreedGoal;
use super::{Controls, Goal, GoalFuture};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::passive::panda::{PandaEntity, TOTAL_UNHAPPY_TIME};
use pumpkin_data::Block;
use pumpkin_util::math::vector3::Vector3;

/// `PandaBreedGoal.canFindBamboo`'s search box: three y layers, and a widening square ring out to
/// radius 7 (`for (int r = 0; r < 8; r++)`).
const BAMBOO_SEARCH_HEIGHT: i32 = 3;
const BAMBOO_SEARCH_RADIUS: i32 = 8;
/// `PandaBreedGoal.unhappyCooldown`'s 600-tick re-arm.
const UNHAPPY_COOLDOWN_TICKS: i32 = 600;

/// `Panda.PandaBreedGoal` (`Panda.java:855-921`).
///
/// Pandas only breed within reach of bamboo. When there is none, the panda plays the "can't
/// breed" animation instead -- a 32-tick unhappy counter (which `Panda.tick` counts down, playing
/// `PANDA_CANT_BREED` at 29 and 14) and a look directed at the nearest player, re-armed at most
/// once every 600 ticks.
pub struct PandaBreedGoal {
    inner: Box<BreedGoal>,
    /// `PandaBreedGoal.unhappyCooldown`.
    unhappy_cooldown: i32,
}

impl PandaBreedGoal {
    #[must_use]
    pub fn new(speed: f64) -> Box<Self> {
        Box::new(Self {
            inner: BreedGoal::new(speed),
            unhappy_cooldown: 0,
        })
    }

    /// `PandaBreedGoal.canFindBamboo` (`Panda.java:892-911`): vanilla's spiral offset walk,
    /// which visits exactly the same set of positions as a plain square scan of radius 7 over
    /// three y layers. Written as the square scan since only the membership test matters, not the
    /// visit order (the method returns on the first hit either way).
    fn can_find_bamboo(mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let origin = entity.block_pos.load();
        let world = entity.world.load();
        let r = BAMBOO_SEARCH_RADIUS - 1;

        for y in 0..BAMBOO_SEARCH_HEIGHT {
            for x in -r..=r {
                for z in -r..=r {
                    let pos = origin.offset(Vector3::new(x, y, z));
                    if world.get_block(&pos) == &Block::BAMBOO {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// `this.panda.lookAtPlayerGoal.setTarget(player)` with `BREED_TARGETING`'s 8-block
    /// non-combat range.
    fn point_look_goal_at_nearest_player(panda: &PandaEntity) {
        let entity = &panda.get_mob_entity().living_entity.entity;
        let pos = entity.pos.load();
        let world = entity.world.load();
        let nearest = world
            .players
            .load()
            .iter()
            .filter(|p| {
                let p_entity = p.get_entity();
                p_entity.is_alive()
                    && p_entity.pos.load().squared_distance_to_vec(&pos) <= 8.0 * 8.0
            })
            .min_by(|a, b| {
                let a_d = a.get_entity().pos.load().squared_distance_to_vec(&pos);
                let b_d = b.get_entity().pos.load().squared_distance_to_vec(&pos);
                a_d.total_cmp(&b_d)
            })
            .map(|p| p.gameprofile.id);
        panda.set_forced_look_target(nearest);
    }
}

impl Goal for PandaBreedGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return false;
            };
            if !self.inner.can_start(mob).await || panda.get_unhappy_counter() != 0 {
                return false;
            }

            if Self::can_find_bamboo(mob) {
                return true;
            }

            let tick_count = panda.get_mob_entity().tick_count.load(Relaxed);
            if self.unhappy_cooldown <= tick_count {
                panda.set_unhappy_counter(TOTAL_UNHAPPY_TIME);
                self.unhappy_cooldown = tick_count + UNHAPPY_COOLDOWN_TICKS;
                Self::point_look_goal_at_nearest_player(panda);
            }
            false
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.should_continue(mob)
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.start(mob)
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.stop(mob)
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.tick(mob)
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
