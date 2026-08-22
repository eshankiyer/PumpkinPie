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
    let mut capacity = 16usize;
    let mut threshold = capacity * 3 / 4;
    let mut buckets = vec![Vec::new(); capacity];

    for &pos in positions {
        if buckets.iter().map(Vec::len).sum::<usize>() + 1 > threshold {
            capacity *= 2;
            threshold = capacity * 3 / 4;
            let mut resized = vec![Vec::new(); capacity];
            for bucket in buckets {
                for entry in bucket {
                    let index = java_hash(entry) as usize & (capacity - 1);
                    resized[index].push(entry);
                }
            }
            buckets = resized;
        }

        let index = java_hash(pos) as usize & (capacity - 1);
        buckets[index].push(pos);
    }

    buckets.into_iter().flatten().collect()
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
