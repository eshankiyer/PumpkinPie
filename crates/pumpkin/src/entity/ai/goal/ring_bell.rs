//! Goal-system port of `net.minecraft.world.entity.ai.behavior.RingBell`.
//!
//! Villagers in this worktree use `GoalSelector` for their live dispatch, while the vanilla
//! class is a zero-control Brain one-shot. This goal preserves that one-shot shape and delegates
//! the actual bell side effects to `BellBlock`, the existing block implementation.

use pumpkin_data::Block;
use rand::RngExt;

use super::{Controls, Goal, GoalFuture};
use crate::block::blocks::redstone::bell::ring_bell_from_ai;
use crate::entity::mob::Mob;

const BELL_RING_CHANCE: f32 = 0.95;
pub const RING_BELL_FROM_DISTANCE: f64 = 3.0;

#[derive(Default)]
pub struct RingBellGoal;

impl Goal for RingBellGoal {
    /// `RingBell.create`'s `MEETING_POINT` gate and 95% early return
    /// (`RingBell.java:15-22`). The pre-raid package dispatch is represented by the world raid
    /// check because villagers in this worktree are Goal-driven rather than Brain-activity-driven.
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if mob.get_random().random::<f32>() <= BELL_RING_CHANCE {
                return false;
            }

            let Some(meeting_point) = mob.get_meeting_point() else {
                return false;
            };
            let world = mob.get_entity().world.load_full();
            if !world
                .is_raid_pre_raid_at(mob.get_entity().block_pos.load())
                .await
            {
                return false;
            }
            if meeting_point
                .to_f64()
                .squared_distance_to_vec(&mob.get_entity().pos.load())
                > RING_BELL_FROM_DISTANCE * RING_BELL_FROM_DISTANCE
            {
                return false;
            }
            world.get_block(&meeting_point) == &Block::BELL
        })
    }

    /// `RingBell` is a declarative one-shot: the block is checked and rung once, then the
    /// behavior reports success (`RingBell.java:21-30`).
    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(meeting_point) = mob.get_meeting_point() else {
                return;
            };
            let world = mob.get_entity().world.load_full();
            if world.get_block(&meeting_point) == &Block::BELL {
                ring_bell_from_ai(meeting_point, &world).await;
            }
        })
    }

    /// The vanilla behavior has no control flags (`RingBell.java:15-16`); it must not block
    /// movement, looking, or other goals.
    fn controls(&self) -> Controls {
        Controls::empty()
    }

    /// `OneShot` stops after its trigger tick (`OneShot.java:25-29`), so this goal is never kept
    /// running into a second selector tick.
    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }
}
