WORKTREE: /home/eshanki/pumpkin-wt/locate
FILES: pumpkin/src/command/commands/locate.rs:110-130
CRATES: pumpkin
FUNC: pub fn horizontal_distance(a: BlockPos, b: BlockPos) -> i32 {
TEST: locate::tests::horizontal_distance
CHAIN: none
ASSERT_ABSENT: todo!("STUB: see tests::horizontal_distance_*")
---
Fill this stub. It is independent of the other two stubs in this file.

    pub fn horizontal_distance(a: BlockPos, b: BlockPos) -> i32 {
        todo!("STUB: see tests::horizontal_distance_*")
    }

Return the horizontal distance in blocks between two positions, rounded to the
nearest integer. This is the number vanilla prints in
`commands.locate.structure.success` as "(N blocks away)".

Y is ignored on purpose. The structure finder returns a chunk-centre column whose
height is not known without generating the chunk, so a distance that included Y
would be comparing a real height against a placeholder.

`BlockPos` wraps a `Vector3<i32>` in field `.0`, so the coordinates are
`a.0.x`, `a.0.y`, `a.0.z`.

The tests at the bottom of this file are the specification and the gate:

- `(0, 64, 0)` to `(3, 200, 4)` is 5. A 3-4-5 triangle, and a version that
  counted the 136-block height difference would be far larger.
- Symmetric: swapping the arguments gives the same answer.
- Two positions in the same column are 0 apart regardless of height.
- `(0,0,0)` to `(1000,0,1000)` is 1414. That is `sqrt(2) * 1000 = 1414.21...`
  rounded to nearest. Truncation also gives 1414 here, but the rounding is what
  vanilla does, so round rather than truncate.

Watch the arithmetic: the differences must be computed in a type that cannot
overflow and cannot wrap. Coordinates are `i32` and can be tens of millions
apart at world edge, so subtracting in `i32` and then squaring in `i32` overflows.
Convert to `f64` before doing any arithmetic. `f64::hypot` is the clearest way to
get the magnitude and avoids the intermediate overflow entirely.

The cast back to `i32` at the end needs to be deliberate: clippy denies
`cast_possible_truncation` in this workspace, so if the lint fires, write the
conversion so it cannot, rather than adding an allow.

Do not change the signature, and do not touch the other two stubs in this file.

Clippy denies all, pedantic, nursery and cargo. No emoji, no em-dashes.
