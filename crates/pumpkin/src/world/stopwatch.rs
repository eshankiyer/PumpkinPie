use std::time::{Duration, Instant};

use pumpkin_util::identifier::Identifier;
use rustc_hash::FxHashMap;

/// A single named stopwatch.
///
/// Mirrors `net.minecraft.world.Stopwatch` (Stopwatch.java:3-16). Vanilla stores
/// the creation time as a monotonic millisecond stamp from `Util.getMillis()`
/// (Util.java:179-185, backed by `System.nanoTime`), so an [`Instant`] is the
/// direct equivalent here.
#[derive(Debug, Clone, Copy)]
pub struct Stopwatch {
    creation_time: Instant,
    accumulated_elapsed_time: Duration,
}

impl Stopwatch {
    #[must_use]
    pub const fn new(creation_time: Instant) -> Self {
        Self {
            creation_time,
            accumulated_elapsed_time: Duration::ZERO,
        }
    }

    /// Stopwatch.java:8-11.
    #[must_use]
    pub fn elapsed(&self, current_time: Instant) -> Duration {
        self.accumulated_elapsed_time + current_time.saturating_duration_since(self.creation_time)
    }

    /// Stopwatch.java:13-15.
    #[must_use]
    pub fn elapsed_seconds(&self, current_time: Instant) -> f64 {
        self.elapsed(current_time).as_secs_f64()
    }
}

/// The server's set of named stopwatches.
///
/// Mirrors `net.minecraft.world.Stopwatches` (Stopwatches.java:16-84), minus the
/// `SavedData` half: vanilla persists the accumulated milliseconds into a
/// `stopwatches` saved-data file, which this codebase has no equivalent for, so
/// these live only for the lifetime of the server process.
#[derive(Debug, Default)]
pub struct Stopwatches {
    stopwatches: FxHashMap<Identifier, Stopwatch>,
}

impl Stopwatches {
    /// Stopwatches.java:82-84.
    #[must_use]
    pub fn current_time() -> Instant {
        Instant::now()
    }

    /// Stopwatches.java:44-46.
    #[must_use]
    pub fn get(&self, id: &Identifier) -> Option<&Stopwatch> {
        self.stopwatches.get(id)
    }

    /// Stopwatches.java:48-55. Returns `false` if the id is already taken.
    pub fn add(&mut self, id: Identifier, stopwatch: Stopwatch) -> bool {
        match self.stopwatches.entry(id) {
            std::collections::hash_map::Entry::Occupied(_) => false,
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(stopwatch);
                true
            }
        }
    }

    /// Stopwatches.java:57-64. Returns `false` if there is no such stopwatch.
    pub fn update(&mut self, id: &Identifier, update: impl FnOnce(Stopwatch) -> Stopwatch) -> bool {
        self.stopwatches.get_mut(id).is_some_and(|stopwatch| {
            *stopwatch = update(*stopwatch);
            true
        })
    }

    /// Stopwatches.java:66-73.
    pub fn remove(&mut self, id: &Identifier) -> bool {
        self.stopwatches.remove(id).is_some()
    }

    /// Stopwatches.java:78-80.
    #[must_use]
    pub fn ids(&self) -> Vec<Identifier> {
        self.stopwatches.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(path: &'static str) -> Identifier {
        Identifier::vanilla_static(path)
    }

    #[test]
    fn elapsed_accumulates_from_creation() {
        let start = Instant::now();
        let stopwatch = Stopwatch::new(start);
        let later = start + Duration::from_millis(1500);
        assert_eq!(stopwatch.elapsed(later), Duration::from_millis(1500));
        assert!((stopwatch.elapsed_seconds(later) - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn add_refuses_duplicate_ids() {
        let mut stopwatches = Stopwatches::default();
        let now = Stopwatches::current_time();
        assert!(stopwatches.add(id("a"), Stopwatch::new(now)));
        assert!(!stopwatches.add(id("a"), Stopwatch::new(now)));
        assert_eq!(stopwatches.ids().len(), 1);
    }

    #[test]
    fn update_and_remove_report_missing_ids() {
        let mut stopwatches = Stopwatches::default();
        let now = Stopwatches::current_time();
        assert!(!stopwatches.update(&id("a"), |s| s));
        assert!(!stopwatches.remove(&id("a")));

        assert!(stopwatches.add(id("a"), Stopwatch::new(now)));
        assert!(stopwatches.update(&id("a"), |_| Stopwatch::new(now)));
        assert!(stopwatches.remove(&id("a")));
        assert!(stopwatches.get(&id("a")).is_none());
    }

    #[test]
    fn restart_drops_accumulated_time() {
        let start = Instant::now();
        let mut stopwatches = Stopwatches::default();
        assert!(stopwatches.add(id("a"), Stopwatch::new(start)));

        let restart_at = start + Duration::from_secs(10);
        assert!(stopwatches.update(&id("a"), |_| Stopwatch::new(restart_at)));

        let query_at = restart_at + Duration::from_secs(2);
        assert_eq!(
            stopwatches.get(&id("a")).unwrap().elapsed(query_at),
            Duration::from_secs(2)
        );
    }
}
