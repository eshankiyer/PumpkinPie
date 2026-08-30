use std::sync::atomic::{AtomicU16, Ordering};

#[derive(Debug)]
pub struct ViewerCountTracker {
    pub old: AtomicU16,
    pub current: AtomicU16,
}

impl Default for ViewerCountTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewerCountTracker {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            old: AtomicU16::new(0),
            current: AtomicU16::new(0),
        }
    }

    pub fn open_container(&self) {
        self.current.fetch_add(1, Ordering::Relaxed);
    }

    pub fn close_container(&self) {
        // `ContainerOpenersCounter.decrementOpeners` cannot produce a negative count
        // (`ContainerOpenersCounter.java:40-48`); preserve that invariant if a scheduled
        // recheck has already removed a stale viewer.
        let _ = self
            .current
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                count.checked_sub(1)
            });
    }

    /// Returns the current number of players viewing this container
    pub fn get_viewer_count(&self) -> u16 {
        self.current.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::ViewerCountTracker;

    #[test]
    fn closing_without_viewers_does_not_wrap() {
        let tracker = ViewerCountTracker::new();
        tracker.close_container();
        assert_eq!(tracker.get_viewer_count(), 0);
    }
}
