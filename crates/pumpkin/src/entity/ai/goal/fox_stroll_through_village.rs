//! `Fox.FoxStrollThroughVillageGoal` (`Fox.java:1277-1301`), registered at priority 9
//! (`Fox.java:197`), and its base `StrollThroughVillageGoal` (`StrollThroughVillageGoal.java`).
//!
//! Deviation: vanilla passes `p -> -level.sectionsToVillage(SectionPos.of(p))` as the position
//! weight into `LandRandomPos.getPos` (`StrollThroughVillageGoal.java:47`), evaluated inside
//! `RandomPos.generateRandomPos`'s 10-candidate loop (`RandomPos.java:96-112`). Here
//! `World::sections_to_village` is async and `random_pos::land_get_pos` takes a synchronous
//! closure, so the same 10-candidate best-weight selection is run one level up: draw ten
//! land positions, keep the one closest to a village in sections.

use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use rand::RngExt;

use super::fox_behavior::{can_fox_move, is_bright_outside};
use super::random_pos::land_get_pos;
use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::passive::fox::FoxEntity;

/// `StrollThroughVillageGoal.DISTANCE_THRESHOLD` (`StrollThroughVillageGoal.java:15`).
const DISTANCE_THRESHOLD: f64 = 10.0;
/// `isCloseToVillage(pos, 6)` (`StrollThroughVillageGoal.java:44`).
const VILLAGE_SECTION_DISTANCE: i32 = 6;
const HORIZONTAL_DIST: i32 = 15;
const VERTICAL_DIST: i32 = 7;
const CANDIDATES: usize = 10;

pub struct FoxStrollThroughVillageGoal {
    interval: i32,
    wanted: Option<BlockPos>,
}

impl FoxStrollThroughVillageGoal {
    #[must_use]
    pub fn new(interval: i32) -> Box<Self> {
        Box::new(Self {
            interval: to_goal_ticks(interval).max(1),
            wanted: None,
        })
    }

    async fn best_village_pos(mob: &dyn Mob) -> Option<BlockPos> {
        let world = mob.get_entity().world.load();
        let mut best: Option<(i32, BlockPos)> = None;
        for _ in 0..CANDIDATES {
            let Some(candidate) = land_get_pos(mob, HORIZONTAL_DIST, VERTICAL_DIST) else {
                continue;
            };
            let pos = BlockPos::floored(candidate.x, candidate.y, candidate.z);
            let sections = world.sections_to_village(pos).await;
            if best.is_none_or(|(best_sections, _)| sections < best_sections) {
                best = Some((sections, pos));
            }
        }
        best.map(|(_, pos)| pos)
    }

    fn move_randomly(mob: &dyn Mob) {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let origin = entity.block_pos.load();
        let mut rng = mob.get_random();
        let x = origin.0.x - 8 + rng.random_range(0..16);
        let z = origin.0.z - 8 + rng.random_range(0..16);
        let y = world.get_heightmap_height(ChunkHeightmapType::MotionBlockingNoLeaves, x, z);
        Self::move_to(mob, Vector3::new(f64::from(x), f64::from(y), f64::from(z)));
    }

    fn move_to(mob: &dyn Mob, destination: Vector3<f64>) {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(
                mob.get_entity().pos.load(),
                destination,
                1.0,
            ));
    }
}

impl Goal for FoxStrollThroughVillageGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() else {
                return false;
            };
            let entity = mob.get_entity();
            if entity.has_passengers().await {
                return false;
            }
            let world = entity.world.load();
            if is_bright_outside(&world) {
                return false;
            }
            let roll = { mob.get_random().random_range(0..self.interval) };
            if roll != 0 {
                return false;
            }
            if !world
                .is_close_to_village(entity.block_pos.load(), VILLAGE_SECTION_DISTANCE)
                .await
            {
                return false;
            }
            if !can_fox_move(mob, fox).await {
                return false;
            }
            self.wanted = Self::best_village_pos(mob).await;
            self.wanted.is_some()
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if self.wanted.is_none() {
                return false;
            }
            let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() else {
                return false;
            };
            if !can_fox_move(mob, fox).await {
                return false;
            }
            // Vanilla additionally requires `navigation.getTargetPos().equals(wantedPos)`
            // (`StrollThroughVillageGoal.java:53`); this navigator re-targets itself in
            // `tick` below, so only the "still pathing" half is kept.
            !mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle()
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() {
                fox.clear_states();
            }
            if let Some(wanted) = self.wanted {
                Self::move_to(mob, wanted.to_centered_f64());
            }
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(wanted) = self.wanted else {
                return;
            };
            let idle = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle();
            if !idle {
                return;
            }

            let entity = mob.get_entity();
            let self_pos = entity.pos.load();
            let target = wanted.to_centered_f64();
            if self_pos.squared_distance_to_vec(&target) < DISTANCE_THRESHOLD * DISTANCE_THRESHOLD {
                return;
            }

            // `StrollThroughVillageGoal.tick` (`StrollThroughVillageGoal.java:62-72`): step
            // 10 blocks along the direction to `wantedPos`, offset 40% back towards the mob.
            let scaled = Vector3::new(
                (self_pos.x - target.x).mul_add(0.4, target.x),
                (self_pos.y - target.y).mul_add(0.4, target.y),
                (self_pos.z - target.z).mul_add(0.4, target.z),
            );
            let delta = Vector3::new(
                scaled.x - self_pos.x,
                scaled.y - self_pos.y,
                scaled.z - self_pos.z,
            );
            let length = delta.length();
            if length <= f64::EPSILON {
                Self::move_randomly(mob);
                return;
            }
            let step = Vector3::new(
                (delta.x / length).mul_add(DISTANCE_THRESHOLD, self_pos.x),
                (delta.y / length).mul_add(DISTANCE_THRESHOLD, self_pos.y),
                (delta.z / length).mul_add(DISTANCE_THRESHOLD, self_pos.z),
            );
            let world = entity.world.load();
            let x = step.x.floor() as i32;
            let z = step.z.floor() as i32;
            let y = world.get_heightmap_height(ChunkHeightmapType::MotionBlockingNoLeaves, x, z);
            Self::move_to(mob, Vector3::new(f64::from(x), f64::from(y), f64::from(z)));
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
