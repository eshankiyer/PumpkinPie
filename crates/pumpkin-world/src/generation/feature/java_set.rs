//! Java `HashSet<BlockPos>` iteration order.
//!
//! Several features collect positions into a `Set<BlockPos>` and then iterate that set,
//! consuming the feature's random stream once per element. Java's iteration order is a pure
//! function of the elements' hashes, the table capacity and the insertion order within a bucket,
//! so it is reproducible; Rust's `HashSet` seeds its hasher per process, so iterating one makes
//! world generation differ between runs of the same binary on the same seed.
//!
//! Callers therefore keep insertion order in a `Vec` and run it through
//! [`vanilla_hash_set_order`] at the point where vanilla iterates the set.

use pumpkin_util::math::position::BlockPos;

/// Reproduces the iteration order of Java's default `HashSet<BlockPos>` for positions inserted
/// in `positions` order (duplicates must already be removed by the caller, as the set would).
#[must_use]
pub fn vanilla_hash_set_order(positions: &[BlockPos]) -> Vec<BlockPos> {
    let mut buckets = vec![Vec::new(); 16];
    let mut len = 0usize;

    for &pos in positions {
        let index = java_hash(pos) as usize & (buckets.len() - 1);
        buckets[index].push(pos);
        len += 1;
        after_insert(&mut buckets, index, len);
    }

    buckets.into_iter().flatten().collect()
}

/// `HashMap.putVal` after linking a new node into bucket `index`: a bin reaching
/// `TREEIFY_THRESHOLD + 1` (9) entries calls `treeifyBin`, which only resizes while the table is
/// below `MIN_TREEIFY_CAPACITY` (64); then the size check against the load-factor threshold.
fn after_insert(buckets: &mut Vec<Vec<BlockPos>>, index: usize, len: usize) {
    if buckets[index].len() > 8 && buckets.len() < 64 {
        resize(buckets);
    }
    if len > buckets.len() * 3 / 4 {
        resize(buckets);
    }
}

/// `HashMap.resize`: doubles the table, keeping each bucket's relative order.
fn resize(buckets: &mut Vec<Vec<BlockPos>>) {
    let capacity = buckets.len() * 2;
    let mut resized = vec![Vec::new(); capacity];
    for bucket in buckets.iter() {
        for &entry in bucket {
            resized[java_hash(entry) as usize & (capacity - 1)].push(entry);
        }
    }
    *buckets = resized;
}

/// `HashMap.hash(Vec3i.hashCode())`: `(y + z * 31) * 31 + x`, spread by `h ^ (h >>> 16)`.
const fn java_hash(pos: BlockPos) -> u32 {
    let hash = pos
        .0
        .y
        .wrapping_add(pos.0.z.wrapping_mul(31))
        .wrapping_mul(31)
        .wrapping_add(pos.0.x);
    hash as u32 ^ (hash as u32 >> 16)
}

/// A Java `HashSet<BlockPos>` that is mutated while it is being drained.
///
/// [`vanilla_hash_set_order`] covers sets that are filled once and then iterated. Vanilla
/// `TreeFeature.updateLeaves` instead keeps adding to its sets while repeatedly taking
/// `iterator().next()` and removing it, so the first element depends on the live table state.
/// Buckets keep Java's append-at-tail order and resizes keep each bucket's relative order, as
/// `HashMap.resize` does. A ninth entry in one bucket resizes the table early while it is below
/// 64 buckets, as `treeifyBin` does; at 64 or more the bin would be treeified, which reorders it
/// (`moveRootToFront`) and is not modelled.
pub struct JavaHashSet {
    buckets: Vec<Vec<BlockPos>>,
    len: usize,
    /// No bucket below this index is non-empty.
    first: usize,
}

impl Default for JavaHashSet {
    fn default() -> Self {
        Self::new()
    }
}

impl JavaHashSet {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buckets: vec![Vec::new(); 16],
            len: 0,
            first: 16,
        }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `Set.add`: a no-op when the position is already present.
    pub fn add(&mut self, pos: BlockPos) {
        let index = java_hash(pos) as usize & (self.buckets.len() - 1);
        if self.buckets[index].contains(&pos) {
            return;
        }
        self.buckets[index].push(pos);
        self.first = self.first.min(index);
        self.len += 1;
        let capacity = self.buckets.len();
        after_insert(&mut self.buckets, index, self.len);
        if self.buckets.len() != capacity {
            self.first = self
                .buckets
                .iter()
                .position(|bucket| !bucket.is_empty())
                .unwrap_or(self.buckets.len());
        }
    }

    /// `iterator().next()` followed by `iterator.remove()`.
    pub fn pop_first(&mut self) -> Option<BlockPos> {
        while self.first < self.buckets.len() {
            let bucket = &mut self.buckets[self.first];
            if !bucket.is_empty() {
                self.len -= 1;
                return Some(bucket.remove(0));
            }
            self.first += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{JavaHashSet, vanilla_hash_set_order};
    use pumpkin_util::math::position::BlockPos;

    #[test]
    fn drained_set_matches_filled_set_order() {
        let positions: Vec<BlockPos> = (0..40)
            .map(|i| BlockPos::new(i % 5 - 2, i / 5, (i * 7) % 3))
            .collect();
        let mut set = JavaHashSet::new();
        for &pos in &positions {
            set.add(pos);
            set.add(pos);
        }
        let mut drained = Vec::new();
        while let Some(pos) = set.pop_first() {
            drained.push(pos);
        }
        assert!(set.is_empty());
        assert_eq!(drained, vanilla_hash_set_order(&positions));
    }
}
