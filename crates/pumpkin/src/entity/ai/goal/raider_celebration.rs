//! Port of vanilla `Raider.RaiderCelebration` (`Raider.java:436-476`).
//!
//! Registered for every raider at priority 5 (`Raider.java:67`). While the mob's raid is lost
//! (the raiders won) and it has no target, it flags itself as celebrating (synced
//! `IS_CELEBRATING`, which drives the cheering arm pose client-side), occasionally plays its
//! celebrate sound and jumps in place.

use pumpkin_data::sound::SoundCategory;
use rand::RngExt;

use super::{Controls, Goal, GoalFuture};
use crate::entity::mob::Mob;

#[derive(Default)]
pub struct RaiderCelebrationGoal;

impl RaiderCelebrationGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self)
    }
}

impl Goal for RaiderCelebrationGoal {
    /// Vanilla `canUse` (`Raider.java:446-449`): alive, no target, and the current raid is lost.
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            let living = &mob_entity.living_entity;
            if !living.entity.is_alive() || mob_entity.target.lock().await.is_some() {
                return false;
            }
            let Some(membership) = living.raid_membership.load() else {
                return false;
            };
            living
                .entity
                .world
                .load()
                .raids
                .lock()
                .await
                .raid(membership.raid_id)
                .is_some_and(crate::world::raid::Raid::is_loss)
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            mob.set_celebrating(true);
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            mob.set_celebrating(false);
        })
    }

    /// Vanilla `tick` (`Raider.java:463-473`): a 1-in-100 celebrate sound and a 1-in-50 jump
    /// (delays halved by `adjustedTickDelay`).
    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            let entity = &mob_entity.living_entity.entity;
            if !entity.is_silent()
                && mob.get_random().random_range(0..self.get_tick_count(100)) == 0
                && let Some(sound) = mob.get_celebrate_sound()
            {
                // `Mob.makeSound`: volume 1.0 and `getVoicePitch`.
                let pitch = (mob.get_random().random::<f32>() - mob.get_random().random::<f32>())
                    .mul_add(0.2, 1.0);
                entity.world.load().play_sound_fine(
                    sound,
                    SoundCategory::Hostile,
                    &entity.pos.load(),
                    1.0,
                    pitch,
                );
            }
            if !entity.has_vehicle().await
                && mob.get_random().random_range(0..self.get_tick_count(50)) == 0
            {
                mob_entity
                    .jump_requested
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
