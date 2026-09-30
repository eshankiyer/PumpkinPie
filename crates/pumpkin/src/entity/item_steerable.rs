use crate::entity::mob::Mob;
use rand::RngExt;
use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

pub trait ItemSteerable: Send + Sync {
    /// Attempts to boost speed. Returns `true` if boost was successfully activated.
    fn boost(&self) -> bool;

    fn as_any(&self) -> &dyn Any;
}

#[derive(Default)]
pub struct ItemBasedSteering {
    pub boosting: AtomicBool,
    pub boost_time: AtomicI32,
    pub boost_time_total: AtomicI32,
}

impl ItemBasedSteering {
    pub const MIN_BOOST_TIME: i32 = 140;
    /// `random.nextInt(841)` (`ItemBasedSteering.java:32`), so totals span 140..=980.
    const BOOST_TIME_SPREAD: i32 = 841;

    /// Starts a boost unless one is running. Returns the new boost length in ticks, which the
    /// caller must sync through the entity's `DATA_BOOST_TIME` (`ItemBasedSteering.boost`).
    #[must_use]
    pub fn boost(&self) -> Option<i32> {
        if self.boosting.swap(true, Ordering::Relaxed) {
            return None;
        }
        self.boost_time.store(0, Ordering::Relaxed);
        let total = rand::rng().random_range(0..Self::BOOST_TIME_SPREAD) + Self::MIN_BOOST_TIME;
        self.boost_time_total.store(total, Ordering::Relaxed);
        Some(total)
    }

    /// `ItemBasedSteering.tickBoost`: `boostTime++ > boostTimeTotal`.
    pub fn tick_boost(&self) {
        if self.boosting.load(Ordering::Relaxed) {
            let previous = self.boost_time.fetch_add(1, Ordering::Relaxed);
            if previous > self.boost_time_total.load(Ordering::Relaxed) {
                self.boosting.store(false, Ordering::Relaxed);
            }
        }
    }

    #[must_use]
    pub fn boost_factor(&self) -> f32 {
        if self.boosting.load(Ordering::Relaxed) {
            let current = self.boost_time.load(Ordering::Relaxed) as f32;
            let total = self.boost_time_total.load(Ordering::Relaxed).max(1) as f32;
            1.0 + 1.15 * (current / total * std::f32::consts::PI).sin()
        } else {
            1.0
        }
    }

    /// `Pig.tickRidden` / `Strider.tickRidden` (`Pig.java:212-218`, `Strider.java:257-262`): while
    /// a player controls the mob it takes the rider's yaw and half its pitch, then the boost
    /// timer advances. Movement itself stays client-authoritative.
    pub async fn tick_ridden(&self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        if !entity.is_alive() || !mob.has_controlling_passenger().await {
            return;
        }
        let (yaw, pitch) = {
            let passengers = entity.passengers.lock().await;
            let Some(controller) = passengers.first().and_then(|p| p.get_player()) else {
                return;
            };
            (
                controller.living_entity.entity.yaw.load(),
                controller.living_entity.entity.pitch.load() * 0.5,
            )
        };
        entity.yaw.store(yaw);
        entity.pitch.store(pitch);
        entity.head_yaw.store(yaw);
        entity.body_yaw.store(yaw);
        self.tick_boost();
    }

    /// Called when the boost time data is synced to the client.
    /// Resets the boosting state to start a new boost animation.
    /// Vanilla: `ItemBasedSteering.onSynced()` (ItemBasedSteering.java:21-24).
    pub fn on_synced(&self) {
        self.boosting.store(true, Ordering::Relaxed);
        self.boost_time.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::ItemBasedSteering;
    use std::sync::atomic::Ordering;

    #[test]
    fn boost_length_is_in_vanilla_range_and_blocks_reboost() {
        let steering = ItemBasedSteering::default();
        assert!(matches!(steering.boost(), Some(total) if (140..=980).contains(&total)));
        assert!(steering.boost().is_none());
    }

    #[test]
    fn tick_boost_ends_after_total_plus_one_ticks() {
        let steering = ItemBasedSteering::default();
        steering.boosting.store(true, Ordering::Relaxed);
        steering.boost_time_total.store(3, Ordering::Relaxed);
        // `boostTime++ > total` first holds on the fifth call (old value 4 > 3).
        for _ in 0..4 {
            steering.tick_boost();
            assert!(steering.boosting.load(Ordering::Relaxed));
        }
        steering.tick_boost();
        assert!(!steering.boosting.load(Ordering::Relaxed));
    }
}
