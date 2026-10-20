WORKTREE: /home/eshanki/pumpkin-wt/lightningrain
FILES: pumpkin/src/world/mod.rs:1622-1700, pumpkin/src/world/mod.rs:1730-1760
CRATES: pumpkin
FUNC: pub async fn tick_spawning_chunk(
ASSERT_ABSENT: if true {
ASSERT_PRESENT: is_raining_at_unchecked
---
Lightning strikes anywhere in a thundering world, including indoors, underground
and in biomes that have no rain.

`tick_spawning_chunk` picks a random position in the chunk and then gates the
strike on a literal:

    // TODO findLightningRod(blockPos)
    // TODO encapsulatingFullBlocks
    if true {

Vanilla gates on `Level#isRainingAt(pos)`, which requires all of: it is raining,
the position can see the sky, the motion-blocking heightmap at that column is not
above the position, and the biome's precipitation at that position is RAIN rather
than SNOW or NONE.

This repo already has that helper. `World::is_raining_at` is at line 1738 and
delegates to `is_raining_at_unchecked`, and its doc comment says it mirrors
vanilla's `Level#isRainingAt`. Replace `if true` with a call to it, passing the
strike position.

Note the surrounding code already computed `is_raining` and `is_thundering` from
the weather lock at line 1631, so use `is_raining_at_unchecked` with those rather
than re-locking. Taking the weather lock again inside this loop widens a critical
section that `tick_environment` also holds, and the established lock order in this
file is weather then level_time. Do not take the weather lock while holding
another lock.

The specific mistake to avoid, because it has already been made here once: do not
probe the biome at `heightmap + delta.y`. `delta` is a 0 to 15 offset, so that
samples a point up to fifteen blocks above the surface and suppresses strikes
vanilla allows. Probe the strike position itself.

Leave the skeleton-horse trap spawn, the 0.0675 roll and the lightning-rod check
below it exactly as they are. Only the gate changes.

Clippy denies all, pedantic, nursery and cargo. No emoji, no em-dashes.
