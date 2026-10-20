BLOCKED 2026-07-28: wrong mode for the pipeline. This is a coordinated
multi-file change (fix in rails/common.rs, TODO deletions in two other files),
so it runs in diff mode, and diff mode never landed a single edit across three
rounds: "context not found or not unique" and "repository lacks the necessary
blob to perform 3-way merge" on every attempt. The model is not the bottleneck;
the applier cannot place its output. To requeue: scaffold it like the /locate
chain - one FUNC spec against the search helper in common.rs, then TARGET line
edits to drop the TODO comments, chained with CHAIN. Verify the helper's real
signature first.

WORKTREE: /home/eshanki/pumpkin-wt/railpower
FILES: pumpkin/src/block/blocks/redstone/rails/activator_rail.rs, pumpkin/src/block/blocks/redstone/rails/powered_rail.rs
CRATES: pumpkin
ASSERT_ABSENT: // TODO: Fix redstone rail power extension behavior
---
Powered and activator rails propagate power from a redstone source placed
anywhere along an already-powered chain. The author documented the bug in a
comment that is copy-pasted into both files:

    // TODO: Fix redstone rail power extension behavior
    // Currently, redstone sources (like redstone torch) can incorrectly extend
    // rail power when placed at any powered rail position. In Minecraft, power
    // should only extend when a redstone source is placed at the LAST powered
    // rail or at an unpowered rail.

with a worked example: with `redstone_torch [powered_rail x9] [unpowered_rail x3]`,
placing the torch at the 6th powered rail currently extends power, and should
not. Power should extend only from the 9th rail, the last powered one, or from an
unpowered rail.

Vanilla's `PoweredRailBlock#findPoweredRailSignal` walks the chain from the
placed rail and stops at the first rail whose `powered` property is already true,
rather than continuing through it. The chain-search limit is 8 rails in each
direction.

Fix the search so it terminates on an already-powered rail. The two files share
this logic, so the fix belongs in the shared helper both call, in
`rails/common.rs`, rather than being written twice. Delete the duplicated TODO
comment block from both files once it is actually fixed.

Do not raise or lower the 8-rail search limit; it is correct.

Do not change rail shape computation, placement validity, or
`update_flanking_rails_shape`. Only the power search changes.

Be careful with the direction handling: the search runs both ways from the placed
rail and the two directions are independent. A fix that terminates one direction
and not the other looks correct in the example above, which is one-sided, and is
still wrong.

Clippy denies all, pedantic, nursery and cargo. No emoji, no em-dashes.
