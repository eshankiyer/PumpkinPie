use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::{BlockPos, BlockPosIterator};
use pumpkin_util::math::vector3::Vector3;
use std::sync::atomic::Ordering;

/// `TryFindLiquidGoal` with the water fluid tag (vanilla's `TryFindWaterGoal`): a stranded,
/// grounded mob steers back to the nearest water block.
///
/// Searches a small box around itself (`Mth.floor(x-2)..=Mth.floor(x+2)`,
/// `Mth.floor(y-2)..=blockY`, `Mth.floor(z-2)..=Mth.floor(z+2)`), first match wins, no distance
/// comparison (`TryFindLiquidGoal.java:24-37`).
pub struct TryFindWaterGoal;

impl Default for TryFindWaterGoal {
    fn default() -> Self {
        Self
    }
}

impl TryFindWaterGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self)
    }

    /// The inclusive search box of `TryFindLiquidGoal.start` (`TryFindLiquidGoal.java:27-34`).
    #[must_use]
    pub fn find_water_range(pos: Vector3<f64>) -> (BlockPos, BlockPos) {
        (
            BlockPos::new(
                (pos.x - 2.0).floor() as i32,
                (pos.y - 2.0).floor() as i32,
                (pos.z - 2.0).floor() as i32,
            ),
            BlockPos::new(
                (pos.x + 2.0).floor() as i32,
                pos.y.floor() as i32,
                (pos.z + 2.0).floor() as i32,
            ),
        )
    }
}

impl Goal for TryFindWaterGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let entity = mob.get_entity();
            if !entity.on_ground.load(Ordering::Relaxed) {
                return false;
            }
            let pos = entity.block_pos.load();
            let world = entity.world.load();
            let (fluid, _) = world.get_fluid_and_fluid_state(&pos);
            !fluid.has_tag(&tag::Fluid::MINECRAFT_WATER)
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let entity = mob.get_entity();
            let world = entity.world.load();
            let (min, max) = Self::find_water_range(entity.pos.load());

            let mut water_pos: Option<BlockPos> = None;
            for pos in BlockPosIterator::new(min.0.x, min.0.y, min.0.z, max.0.x, max.0.y, max.0.z) {
                let (fluid, _) = world.get_fluid_and_fluid_state(&pos);
                if fluid.has_tag(&tag::Fluid::MINECRAFT_WATER) {
                    water_pos = Some(pos);
                    break;
                }
            }

            // `TryFindLiquidGoal.start` (:39-41) hands the block corner straight to the
            // `MoveControl`; it does not path there.
            if let Some(pos) = water_pos {
                mob.get_mob_entity()
                    .move_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_wanted_position(
                        f64::from(pos.0.x),
                        f64::from(pos.0.y),
                        f64::from(pos.0.z),
                        1.0,
                    );
            }
        })
    }

    /// `TryFindLiquidGoal` never calls `setFlags`, so it claims no controls.
    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_water_range_calculation() {
        let pos = Vector3::new(10.5, 64.0, -5.2);
        let (min, max) = TryFindWaterGoal::find_water_range(pos);

        assert_eq!(min.0.x, 8);
        assert_eq!(min.0.y, 62);
        assert_eq!(min.0.z, -8);

        assert_eq!(max.0.x, 12);
        assert_eq!(max.0.y, 64);
        assert_eq!(max.0.z, -4);
    }
}
