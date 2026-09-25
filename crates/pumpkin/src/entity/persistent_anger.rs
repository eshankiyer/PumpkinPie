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
    pub async fn angry_at(&self) -> Option<Uuid> {
        *self
            .angry_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn is_angry(&self) -> bool {
        self.remaining_ticks.load(Relaxed) > 0
    }

    pub async fn is_angry_at(&self, target: Uuid) -> bool {
        self.is_angry()
            && *self
                .angry_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                == Some(target)
    }

    /// Vanilla `NeutralMob.isAngryAtAllPlayers`: true while angry with no specific grudge
    /// target, gated behind the `universal_anger` game rule (checked by the caller).
    pub async fn is_angry_at_all_players(&self, universal_anger_rule: bool) -> bool {
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
    pub async fn forget_current_target_and_refresh_universal_anger(&self) {
        self.stop_being_angry().await;
        self.start_timer();
    }

    pub async fn set_angry_at(&self, target: Option<Uuid>) {
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
    pub async fn stop_being_angry(&self) {
        *self
            .angry_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.remaining_ticks.store(0, Relaxed);
    }

    /// Per-tick decrement, auto-clearing the target once the timer expires.
    pub async fn tick(&self) {
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
    pub async fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_nbt_sync(nbt);
    }

    pub fn write_nbt_sync(&self, nbt: &mut NbtCompound) {
        nbt.put_int("AngerTime", self.remaining_ticks.load(Relaxed).max(0));
        if let Some(uuid) = *self
            .angry_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            nbt.put_uuid("angry_at", uuid);
        }
    }

    pub async fn read_nbt(&self, nbt: &NbtCompound) {
        self.read_nbt_sync(nbt);
    }

    pub fn read_nbt_sync(&self, nbt: &NbtCompound) {
        self.remaining_ticks
            .store(nbt.get_int("AngerTime").unwrap_or(0).max(0), Relaxed);
        *self
            .angry_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = nbt.get_uuid("angry_at");
    }

    /// Modern NeutralMob persistence uses an absolute world game tick, while the
    /// runtime currently keeps a remaining-tick counter.
    pub fn write_nbt_at_game_time(&self, nbt: &mut NbtCompound, game_time: i64) {
        self.write_nbt_sync(nbt);
        let remaining = i64::from(self.remaining_ticks.load(Relaxed).max(0));
        nbt.put_long(
            "anger_end_time",
            if remaining > 0 {
                game_time.saturating_add(remaining)
            } else {
                -1
            },
        );
    }

    pub fn read_nbt_at_game_time(&self, nbt: &NbtCompound, game_time: i64) {
        self.read_nbt_sync(nbt);
        if let Some(end_time) = nbt.get_long("anger_end_time") {
            let remaining = end_time
                .saturating_sub(game_time)
                .clamp(0, i64::from(i32::MAX));
            self.remaining_ticks.store(remaining as i32, Relaxed);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn modern_nbt_preserves_uuid_and_accounts_for_elapsed_game_time() {
        let anger = PersistentAnger::default();
        let target = Uuid::new_v4();
        let mut legacy = NbtCompound::new();
        legacy.put_int("AngerTime", 123);
        legacy.put_uuid("angry_at", target);
        anger.read_nbt_sync(&legacy);
        let mut saved = NbtCompound::new();
        anger.write_nbt_at_game_time(&mut saved, 1000);
        assert_eq!(saved.get_long("anger_end_time"), Some(1123));

        let restored = PersistentAnger::default();
        restored.read_nbt_at_game_time(&saved, 1050);
        assert_eq!(restored.remaining_ticks.load(Relaxed), 73);
        let mut round_trip = NbtCompound::new();
        restored.write_nbt_sync(&mut round_trip);
        assert_eq!(round_trip.get_uuid("angry_at"), Some(target));

        restored.read_nbt_at_game_time(&saved, 1200);
        assert!(!restored.is_angry());
        restored.read_nbt_at_game_time(&legacy, 1200);
        assert_eq!(restored.remaining_ticks.load(Relaxed), 123);
        restored.read_nbt_at_game_time(&NbtCompound::new(), 1200);
        assert!(!restored.is_angry());
        let mut empty = NbtCompound::new();
        restored.write_nbt_sync(&mut empty);
        assert_eq!(empty.get_uuid("angry_at"), None);
    }

    #[test]
    fn fresh_is_not_angry() {
        let anger = PersistentAnger::default();
        assert!(!anger.is_angry());
    }

    #[tokio::test]
    async fn start_timer_sets_angry_in_range() {
        let anger = PersistentAnger::default();
        anger.start_timer();
        assert!(anger.is_angry());
        let ticks = anger.remaining_ticks.load(Relaxed);
        assert!((PERSISTENT_ANGER_MIN_TICKS..=PERSISTENT_ANGER_MAX_TICKS).contains(&ticks));
    }

    #[tokio::test]
    async fn tick_decrements_and_expires() {
        let anger = PersistentAnger::default();
        let target = Uuid::new_v4();
        anger.set_angry_at(Some(target)).await;
        anger.remaining_ticks.store(2, Relaxed);

        anger.tick().await;
        assert!(anger.is_angry());
        assert!(anger.is_angry_at(target).await);

        anger.tick().await;
        assert!(!anger.is_angry());
        assert_eq!(anger.angry_at().await, None);
    }

    #[tokio::test]
    async fn tick_on_expired_timer_is_noop() {
        let anger = PersistentAnger::default();
        anger.tick().await;
        assert!(!anger.is_angry());
    }

    #[tokio::test]
    async fn stop_being_angry_clears_state() {
        let anger = PersistentAnger::default();
        anger.set_angry_at(Some(Uuid::new_v4())).await;
        anger.start_timer();

        anger.stop_being_angry().await;

        assert!(!anger.is_angry());
        assert_eq!(anger.angry_at().await, None);
    }

    #[tokio::test]
    async fn nbt_round_trip() {
        let anger = PersistentAnger::default();
        let target = Uuid::new_v4();
        anger.set_angry_at(Some(target)).await;
        anger.remaining_ticks.store(123, Relaxed);

        let mut nbt = NbtCompound::new();
        anger.write_nbt(&mut nbt).await;

        let restored = PersistentAnger::default();
        restored.read_nbt(&nbt).await;

        assert_eq!(restored.remaining_ticks.load(Relaxed), 123);
        assert_eq!(restored.angry_at().await, Some(target));
    }
}
