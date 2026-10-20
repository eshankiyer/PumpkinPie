WORKTREE: /home/eshanki/pumpkin-wt/diagvalid
FILES: pumpkin/src/block/blocks/plant/crop/nether_wart.rs
CRATES: pumpkin
TARGET:         if age < self.max_age() && rand::rng().random_range(0..=10) == 0 {
ASSERT_ABSENT: random_range(0..=10)
ASSERT_PRESENT: random_range(0..10)
---
Change the inclusive range `0..=10` to the exclusive range `0..10`.

That is the only change. `0..=10` has eleven possible values, so it is a 1 in 11
chance. Vanilla `NetherWartBlock.randomTick` uses `random.nextInt(10) == 0`,
and `nextInt` is exclusive of its bound, so it is 1 in 10.

Keep everything else on the line exactly as it is: the `age < self.max_age()`
check, the `&&`, the `== 0`, and the trailing `{`.
