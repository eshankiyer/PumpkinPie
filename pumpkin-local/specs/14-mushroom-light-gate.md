WORKTREE: /home/eshanki/pumpkin-wt/mushroomlight
FILES: pumpkin/src/block/blocks/plant/mushroom_plant.rs
CRATES: pumpkin
FUNC: fn can_plant_on_top(&self, block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
ASSERT_ABSENT: // TODO: Check light level and isOpaqueFullCube
---
Small mushrooms can be placed and can spread in light levels vanilla forbids.

`can_plant_on_top` checks only a tag:

    fn can_plant_on_top(&self, block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        let block = block_accessor.get_block(pos);
        block.has_tag(&tag::Block::MINECRAFT_OVERRIDES_MUSHROOM_LIGHT_REQUIREMENT)
        // TODO: Check light level and isOpaqueFullCube
    }

Vanilla's `MushroomBlock#mayPlaceOn` returns true if the block below is in
`#minecraft:mushroom_grow_block`, and otherwise requires that the block below is
an opaque full cube AND the light level at the mushroom position is at most 12.
The tag check is the override branch, and the rest of the rule is missing
entirely, so the function currently returns false for ordinary stone and dirt and
true only for the override blocks.

Implement the full rule: keep the tag check as an early true, then fall back to
the opaque-full-cube test on the block below combined with a light level of 12 or
less at the position above it.

Use the light accessor this codebase already exposes on `BlockAccessor` or the
world for reading a light level, and its existing solid or opaque-full-cube
predicate on `BlockState`. Do not add a new light lookup path.

Note carefully which position each test applies to. The opacity test is on the
block below, the light test is on the mushroom's own position. Getting these the
wrong way round produces a rule that looks plausible and is wrong in exactly the
cases that matter.

If a light value comes back as a `u8` and you subtract from it anywhere,
saturate. An underflow here wraps to about 250 and a later clamp turns that into
15, a silent false-bright that reads as "mushrooms spread in daylight".

Do not touch the random-tick spread rate or the huge-mushroom growth code.

Clippy denies all, pedantic, nursery and cargo. No emoji, no em-dashes.
