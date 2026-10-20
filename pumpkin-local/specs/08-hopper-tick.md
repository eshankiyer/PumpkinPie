WORKTREE: /home/eshanki/pumpkin-wt/hoppertick
FILES: pumpkin/src/block/entities/hopper.rs
CRATES: pumpkin
TARGET:             if self.cooldown_time.fetch_sub(1, Ordering::Relaxed) <= 0 {
ASSERT_ABSENT: fetch_sub(1, Ordering::Relaxed) <= 0
ASSERT_PRESENT: fetch_sub(1, Ordering::Relaxed) <= 1
---
This comparison is off by one, so hoppers transfer every 9 ticks instead of 8.

`fetch_sub` returns the value from BEFORE the subtraction. Comparing that
previous value against 0 means the branch fires one tick later than intended.
Vanilla decrements first and then tests the new value, so the transfer happens
on the tick the counter actually reaches zero.

Change the comparison from `<= 0` to `<= 1`. That is the entire fix.

Keep `fetch_sub(1, Ordering::Relaxed)` exactly as it is. Do NOT rewrite it as a
load followed by a store: a neighbouring hopper writes this same field from its
own concurrently spawned tick task, so it must stay a single atomic
read-modify-write.
