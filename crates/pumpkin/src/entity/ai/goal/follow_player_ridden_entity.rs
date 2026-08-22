//! `FollowPlayerRiddenEntityGoal` (`FollowPlayerRiddenEntityGoal.java`), registered twice on
//! Dolphin at priority 8 for `AbstractBoat` and `AbstractNautilus` (`Dolphin.java:168-169`).
//!
//! `Entity.hasMovedHorizontallyRecently` (`Entity.java:942-944`) is
//! `|lastKnownSpeed.horizontalDistance()| > 1.0E-5`. This codebase already keeps the same
//! quantity: `Entity::update_last_pos` (`entity/mod.rs:2373-2379`) runs from `Entity::tick`
//! and stores `pos - last_pos` in `Entity::movement`, so no new per-entity state is needed.
//!
//! Not ported: the per-tick `moveRelative`/`move(MoverType.SELF, ...)` nudge
//! (`FollowPlayerRiddenEntityGoal.java:68-70`), which reads `xxa`/`yya`/`zza` AI move inputs
//! this codebase does not expose to goals. The navigation half, which is what actually makes
//! the dolphin escort the boat, is kept in full.

use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;

/// `getBoundingBox().inflate(5.0)` (`FollowPlayerRiddenEntityGoal.java:29`).
const SEARCH_RADIUS: f64 = 5.0;
/// `Entity.hasMovedHorizontallyRecently`'s threshold (`Entity.java:943`).
const MOVED_EPSILON: f64 = 1.0e-5;
const RECALC_INTERVAL: i32 = 10;
/// `distanceTo(following) < 4.0F` switches to leading (`FollowPlayerRiddenEntityGoal.java:77`).
const SWITCH_TO_LEAD_DISTANCE: f64 = 4.0;
/// `distanceTo(following) > 12.0F` switches back to chasing
/// (`FollowPlayerRiddenEntityGoal.java:85`).
const SWITCH_TO_CHASE_DISTANCE: f64 = 12.0;
/// `blockPosition().relative(direction, 10)` (`FollowPlayerRiddenEntityGoal.java:83`).
const LEAD_AHEAD_BLOCKS: i32 = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    GoToEntity,
    GoInEntityDirection,
}

pub struct FollowPlayerRiddenEntityGoal {
    /// Which vehicle types count. Vanilla passes a class (`AbstractBoat`,
    /// `AbstractNautilus`), each of which covers several concrete `EntityType`s, so the
    /// caller supplies the membership test instead.
    is_vehicle: fn(&'static EntityType) -> bool,
    following: Option<Uuid>,
    time_to_recalc_path: i32,
    stage: Stage,
}

impl FollowPlayerRiddenEntityGoal {
    #[must_use]
    pub fn new(is_vehicle: fn(&'static EntityType) -> bool) -> Box<Self> {
        Box::new(Self {
            is_vehicle,
            following: None,
            time_to_recalc_path: 0,
            stage: Stage::GoToEntity,
        })
    }

    fn has_moved_horizontally_recently(entity: &crate::entity::Entity) -> bool {
        entity.movement.load().horizontal_length() > MOVED_EPSILON
    }

    /// The controlling passenger of a nearby vehicle of one of `vehicle_types`, if it is a
    /// player (`FollowPlayerRiddenEntityGoal.java:29-34`).
    async fn nearby_riding_player(&self, mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let search_box =
            entity
                .bounding_box
                .load()
                .expand(SEARCH_RADIUS, SEARCH_RADIUS, SEARCH_RADIUS);
        for candidate in world.get_entities_at_box(&search_box) {
            let candidate_type = candidate.get_entity().entity_type;
            if !(self.is_vehicle)(candidate_type) {
                continue;
            }
            let passengers = candidate.get_entity().passengers.lock().await;
            // `getControllingPassenger` for a boat/nautilus is its first passenger.
            let rider = passengers.first().cloned();
            drop(passengers);
            if let Some(rider) = rider
                && rider.get_player().is_some()
            {
                return Some(rider);
            }
        }
        None
    }

    fn resolve_following(&self, mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let uuid = self.following?;
        let entity = mob.get_entity();
        let world = entity.world.load();
        world
            .get_nearby_entities(entity.pos.load(), 64.0)
            .into_iter()
            .find(|(candidate_uuid, _)| *candidate_uuid == uuid)
            .map(|(_, candidate)| candidate)
    }

    fn move_to(mob: &dyn Mob, target: Vector3<f64>) {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(mob.get_entity().pos.load(), target, 1.0));
    }

    const fn facing_offset(facing: pumpkin_data::block_properties::HorizontalFacing) -> (i32, i32) {
        use pumpkin_data::block_properties::HorizontalFacing as F;
        match facing {
            F::North => (0, -1),
            F::South => (0, 1),
            F::West => (-1, 0),
            F::East => (1, 0),
        }
    }
}

impl Goal for FollowPlayerRiddenEntityGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if let Some(following) = self.resolve_following(mob)
                && Self::has_moved_horizontally_recently(following.get_entity())
            {
                return true;
            }
            self.nearby_riding_player(mob)
                .await
                .is_some_and(|rider| Self::has_moved_horizontally_recently(rider.get_entity()))
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(following) = self.resolve_following(mob) else {
                return false;
            };
            // `following.isPassenger()` (`FollowPlayerRiddenEntityGoal.java:46`).
            following.get_entity().vehicle.lock().await.is_some()
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.following = self
                .nearby_riding_player(mob)
                .await
                .map(|rider| rider.get_entity().entity_uuid);
            self.time_to_recalc_path = 0;
            self.stage = Stage::GoToEntity;
        })
    }

    fn stop<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.following = None;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.time_to_recalc_path -= 1;
            if self.time_to_recalc_path > 0 {
                return;
            }
            self.time_to_recalc_path = to_goal_ticks(RECALC_INTERVAL);

            let Some(following) = self.resolve_following(mob) else {
                return;
            };
            let followed = following.get_entity();
            let followed_block = followed.block_pos.load();
            let distance = mob
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&followed.pos.load())
                .sqrt();

            match self.stage {
                Stage::GoToEntity => {
                    let (dx, dz) = Self::facing_offset(followed.get_horizontal_facing());
                    let behind = BlockPos::new(
                        followed_block.0.x - dx,
                        followed_block.0.y - 1,
                        followed_block.0.z - dz,
                    );
                    Self::move_to(mob, behind.to_f64());
                    if distance < SWITCH_TO_LEAD_DISTANCE {
                        self.time_to_recalc_path = 0;
                        self.stage = Stage::GoInEntityDirection;
                    }
                }
                Stage::GoInEntityDirection => {
                    let (dx, dz) = Self::facing_offset(followed.get_horizontal_facing());
                    let ahead = BlockPos::new(
                        followed_block.0.x + dx * LEAD_AHEAD_BLOCKS,
                        followed_block.0.y - 1,
                        followed_block.0.z + dz * LEAD_AHEAD_BLOCKS,
                    );
                    Self::move_to(mob, ahead.to_f64());
                    if distance > SWITCH_TO_CHASE_DISTANCE {
                        self.time_to_recalc_path = 0;
                        self.stage = Stage::GoToEntity;
                    }
                }
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    /// `Goal.isInterruptable` returns true (`FollowPlayerRiddenEntityGoal.java:38-41`) and the
    /// goal sets no flags, so it holds no controls.
    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
