use std::sync::atomic::{AtomicI32, Ordering::Relaxed};

use pumpkin_nbt::compound::NbtCompound;
use rand::RngExt;
use std::sync::Mutex;
use uuid::Uuid;

// Vanilla `Wolf`/`ZombifiedPiglin`/`Bee`/`PolarBear`/`IronGolem`/`EnderMan`:
// `PERSISTENT_ANGER_TIME = TimeUtil.rangeOfSeconds(20, 39)`.
pub const PERSISTENT_ANGER_MIN_TICKS: i32 = 20 * 20;
pub const PERSISTENT_ANGER_MAX_TICKS: i32 = 39 * 20;

/// Shared `NeutralMob`-equivalent state: a timed grudge against a specific entity, surviving reloads.
///
/// Unlike vanilla's absolute `anger_end_time` game tick, this tracks a remaining-tick
/// counter decremented in `tick`.
pub struct PersistentAnger {
    angry_at: Mutex<Option<Uuid>>,
    remaining_ticks: AtomicI32,
}

impl Default for PersistentAnger {
    fn default() -> Self {
        Self {
            angry_at: Mutex::new(None),
            remaining_ticks: AtomicI32::new(0),
        }
    }
}

impl PersistentAnger {
    pub fn angry_at(&self) -> Option<Uuid> {
        *self
            .angry_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn is_angry(&self) -> bool {
        self.remaining_ticks.load(Relaxed) > 0
    }

    pub fn is_angry_at(&self, target: Uuid) -> bool {
        self.is_angry()
            && *self
                .angry_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                == Some(target)
    }

    /// Vanilla `NeutralMob.isAngryAtAllPlayers`: true while angry with no specific grudge
    /// target, gated behind the `universal_anger` game rule (checked by the caller).
    pub fn is_angry_at_all_players(&self, universal_anger_rule: bool) -> bool {
        universal_anger_rule
            && self.is_angry()
            && self
                .angry_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
    }

    /// Vanilla `NeutralMob.forgetCurrentTargetAndRefreshUniversalAnger`: `stopBeingAngry()`
    /// then `startPersistentAngerTimer()`, leaving `angry_at` cleared so `isAngryAtAllPlayers`
    /// becomes true for the duration of the new timer.
    pub fn forget_current_target_and_refresh_universal_anger(&self) {
        self.stop_being_angry();
        self.start_timer();
    }

    pub fn set_angry_at(&self, target: Option<Uuid>) {
        *self
            .angry_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
    }

    /// Vanilla `startPersistentAngerTimer`: `setTimeToRemainAngry(PERSISTENT_ANGER_TIME.sample(random))`.
    pub fn start_timer(&self) {
        let ticks =
            rand::rng().random_range(PERSISTENT_ANGER_MIN_TICKS..=PERSISTENT_ANGER_MAX_TICKS);
        self.remaining_ticks.store(ticks, Relaxed);
    }

    /// Vanilla `stopBeingAngry`: clears target and anger end time (last-hurt-by/target
    /// clearing is the consumer's responsibility since it touches entity/AI state).
    pub fn stop_being_angry(&self) {
        *self
            .angry_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.remaining_ticks.store(0, Relaxed);
    }

    /// Per-tick decrement, auto-clearing the target once the timer expires.
    pub fn tick(&self) {
        let prev = self.remaining_ticks.fetch_sub(1, Relaxed);
        if prev <= 0 {
            self.remaining_ticks.store(0, Relaxed);
        } else if prev == 1 {
            *self
                .angry_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    }

    // Vanilla `NeutralMob.java:44-46` legacy read path: a plain int holding the
    // remaining-ticks count (`AngerTime` -> `setTimeToRemainAngry`), unlike the modern
    // `anger_end_time` long which stores an absolute game tick this primitive doesn't track.
    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("AngerTime", self.remaining_ticks.load(Relaxed).max(0));
        if let Some(uuid) = self.angry_at() {
            nbt.put_uuid("angry_at", uuid);
        }
    }

    pub fn read_nbt(&self, nbt: &NbtCompound) {
        self.remaining_ticks
            .store(nbt.get_int("AngerTime").unwrap_or(0).max(0), Relaxed);
        *self
            .angry_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = nbt.get_uuid("angry_at");
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn fresh_is_not_angry() {
        let anger = PersistentAnger::default();
        assert!(!anger.is_angry());
    }

    #[tokio::test]
    fn start_timer_sets_angry_in_range() {
        let anger = PersistentAnger::default();
        anger.start_timer();
        assert!(anger.is_angry());
        let ticks = anger.remaining_ticks.load(Relaxed);
        assert!((PERSISTENT_ANGER_MIN_TICKS..=PERSISTENT_ANGER_MAX_TICKS).contains(&ticks));
    }

    #[tokio::test]
    fn tick_decrements_and_expires() {
        let anger = PersistentAnger::default();
        let target = Uuid::new_v4();
        anger.set_angry_at(Some(target));
        anger.remaining_ticks.store(2, Relaxed);

        anger.tick();
        assert!(anger.is_angry());
        assert!(anger.is_angry_at(target));

        anger.tick();
        assert!(!anger.is_angry());
        assert_eq!(anger.angry_at(), None);
    }

    #[tokio::test]
    fn tick_on_expired_timer_is_noop() {
        let anger = PersistentAnger::default();
        anger.tick();
        assert!(!anger.is_angry());
    }

    #[tokio::test]
    fn stop_being_angry_clears_state() {
        let anger = PersistentAnger::default();
        anger.set_angry_at(Some(Uuid::new_v4()));
        anger.start_timer();

        anger.stop_being_angry();

        assert!(!anger.is_angry());
        assert_eq!(anger.angry_at(), None);
    }

    #[tokio::test]
    fn nbt_round_trip() {
        let anger = PersistentAnger::default();
        let target = Uuid::new_v4();
        anger.set_angry_at(Some(target));
        anger.remaining_ticks.store(123, Relaxed);

        let mut nbt = NbtCompound::new();
        anger.write_nbt(&mut nbt);

        let restored = PersistentAnger::default();
        restored.read_nbt(&nbt);

        assert_eq!(restored.remaining_ticks.load(Relaxed), 123);
        assert_eq!(restored.angry_at(), Some(target));
    }
}
