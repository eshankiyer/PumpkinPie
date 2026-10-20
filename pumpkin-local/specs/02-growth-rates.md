WORKTREE: /home/eshanki/pumpkin-wt/growthrates
FILES: pumpkin/src/block/blocks/plant/crop/sweet_berry_bush.rs:180-205
CRATES: pumpkin
FUNC: async fn random_tick(&self, world: &Arc<World>, pos: &BlockPos) {
ASSERT_ABSENT: random_range(0..=25)
ASSERT_ABSENT: random_range(0..5)
---
# Bug: sweet berry bushes grow about 26 times slower than vanilla

Filed upstream as issue #2582. The vanilla rule below is already verified.

You are shown this type's own `CropBlockBase::random_tick` override. It
contains a gate:

    if rand::rng().random_range(0..=25) == 0 {

That gate is spurious. The caller, `BlockBehaviour::random_tick` (not shown,
do not edit it), ALREADY applies vanilla's only gate,
`rand::rng().random_range(0..5) == 0`, before calling into this function.

VANILLA `SweetBerryBushBlock.randomTick` is:

    if (age < 3 && random.nextInt(5) == 0 && level.getRawBrightness(pos.above(), 0) >= 9)

One gate, 1 in 5. Together the two gates give about 1 in 130.

# Change

Remove the `random_range(0..=25) == 0` condition so that once the caller's
gate has passed, the bush advances its age. Keep the surrounding
`if age < self.max_age()` check and everything inside the body exactly as it
is. Add a short comment where the gate was:

    // The caller already applies vanilla's only growth gate of 1 in 5.

# Do not

- Do not touch the `is_full_cube() || is_solid()` check on the block above the
  bush. It has no vanilla counterpart but removing it changes behaviour the
  light check does not cover, and that is a separate question.
- Do not edit any other file. Nether wart is a separate spec.
- Do not change `max_age`, any age value, or the berry drop counts.
