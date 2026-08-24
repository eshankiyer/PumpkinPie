use super::{Controls, Goal, GoalFuture};
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob};
use pumpkin_data::{Block, BlockDirection, tag::Taggable};
use pumpkin_util::math::position::BlockPos;

/// Goal-system port of `TryFindLand.create` for the frog's swim activity.
///
/// Vanilla source: `net/minecraft/world/entity/ai/behavior/TryFindLand.java:15-60`.
/// The surrounding Brain activity is represented by the frog's Goal selector, so this goal
/// keeps the same water gate, outward Manhattan search, land checks, and cooldown while writing
/// the result directly to the navigator.
pub struct TryFindLandGoal {
    range: i32,
    speed: f64,
    next_ok_start_time: i64,
}

impl TryFindLandGoal {
    /// `TryFindLand.create(range, speedModifier)` (`TryFindLand.java:18-19`).
    #[must_use]
    pub const fn new(range: i32, speed: f64) -> Self {
        Self {
            range,
            speed,
            next_ok_start_time: 0,
        }
    }

    /// `TryFindLand`'s `BlockPos.withinManhattan` search and acceptance test
    /// (`TryFindLand.java:34-49`).
    fn find_land(mob: &dyn Mob, range: i32) -> Option<BlockPos> {
        let entity = mob.get_entity();
        let origin = entity.block_pos.load();
        let world = entity.world.load();

        for pos in BlockPos::iterate_outwards(origin, range, range, range) {
            if pos.0.x == origin.0.x && pos.0.z == origin.0.z {
                continue;
            }

            let state = world.get_block_state(&pos);
            let below = pos.down();
            let below_state = world.get_block_state(&below);
            let (_, fluid_state) = world.get_fluid_and_fluid_state(&pos);
            if world.get_block(&pos).id == Block::WATER.id
                || !fluid_state.is_empty
                || state.get_block_collision_shapes().next().is_some()
                || !below_state.is_side_solid(BlockDirection::Up)
            {
                continue;
            }

            return Some(pos);
        }
        None
    }
}

impl Goal for TryFindLandGoal {
    /// `TryFindLand`'s water gate (`TryFindLand.java:25-26`) and its 60-tick cooldown
    /// (`TryFindLand.java:16,29-32,54`).
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let entity = mob.get_entity();
            if !entity
                .world
                .load()
                .get_fluid(&entity.block_pos.load())
                .has_tag(&pumpkin_data::tag::Fluid::MINECRAFT_WATER)
            {
                return false;
            }

            let game_time = entity.world.load().get_world_age().await;
            game_time >= self.next_ok_start_time
        })
    }

    /// The declarative vanilla behavior is a one-shot (`TryFindLand.java:20-58`); after installing
    /// a destination, let the Goal selector retry only after the behavior's cooldown.
    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }

    /// `TryFindLand`'s destination writes (`TryFindLand.java:46-49`) and cooldown update
    /// (`TryFindLand.java:54`).
    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let entity = mob.get_entity();
            let game_time = entity.world.load().get_world_age().await;
            self.next_ok_start_time = game_time + 60;

            if let Some(target) = Self::find_land(mob, self.range) {
                let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
                navigator.set_progress(NavigatorGoal::new(
                    entity.pos.load(),
                    target.to_f64(),
                    self.speed,
                ));
            }
        })
    }

    /// `TryFindLand` writes a `WALK_TARGET` (`TryFindLand.java:48`), so it owns movement.
    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
