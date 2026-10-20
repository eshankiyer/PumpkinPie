WORKTREE: /home/eshanki/pumpkin-wt/skylightbounds
FILES: pumpkin-world/src/lighting/runtime.rs:449-480, pumpkin-world/src/lighting/runtime.rs:526-557
CRATES: pumpkin-world
FUNC: pub fn get_sky_light_level_sync(&self, level: &Level, position: &BlockPos) -> u8 {
ASSERT_ABSENT: as usize / BlockPalette::SIZE
ASSERT_PRESENT: < 0
---
# Bug: sky light reads as 15 for positions BELOW the world

You are shown two functions, `get_sky_light_level_sync` and
`get_sky_light_level`. Both compute a section index with an unsigned cast,
something like:

    let section_index = (relative.y - chunk.section.min_y) as usize / 16;

then return 15 when that index is past the end of the sky light array.

The unsigned cast is the bug. For a `y` BELOW `min_y` the subtraction is
negative, the cast wraps to a huge positive number, the same out-of-bounds
branch is taken, and a position under the world reports full daylight.

VERIFIED CORRECT BEHAVIOUR (do not re-derive):
- ABOVE the top of the world: 15. Keep this.
- BELOW the bottom of the world: 0.

# Change, in BOTH functions

1. Compute the offset as a SIGNED value first, keeping the integer type the
   coordinates already use. Do not cast to `usize` yet.
2. If the signed section index is negative, return 0.
3. If it is greater than or equal to the number of sections, return 15.
4. Only then convert to `usize` and index as before.

Add one comment at each site:
    // Above build height sky light is full, below the world it is zero. The
    // old unsigned cast wrapped negative offsets into the "above" branch.

# Do not

- Do not touch the unloaded-chunk path that already returns 0.
- Do not touch any other function in this file.
- Do not change pumpkin-world/src/lighting/storage.rs, even though it returns 0
  for the same condition. That inconsistency is a separate branch.
