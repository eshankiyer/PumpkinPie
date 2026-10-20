WORKTREE: /home/eshanki/pumpkin-wt/pistonstate
FILES: pumpkin/src/block/entities/piston.rs:270-325
CRATES: pumpkin
FUNC: fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
ASSERT_ABSENT: let pushed_block_state = Block::AIR.default_state;
ASSERT_ABSENT: // TODO: pushed_block_state
---
A moving piston loses the block it is carrying when its chunk is saved and
reloaded. This is data corruption, not a cosmetic bug.

`PistonBlockEntity` carries the identity of the block in transit:

    pub pushed_block_state: &'static BlockState,

but `from_nbt` hardcodes it instead of reading it:

    let pushed_block_state = Block::AIR.default_state;

and neither `write_nbt` nor `chunk_data_nbt` writes it out. Three separate
`// TODO: pushed_block_state` comments in this file mark the gap. Save a world
while a piston is mid-extension and the carried block becomes air on load.

Vanilla serialises the pushed block state in the piston block entity NBT under
`blockState`, as a block state compound with a `Name` and an optional
`Properties` map, and reads it back on load.

Serialise and deserialise it. Write the state in `write_nbt` and in
`chunk_data_nbt`, and read it in `from_nbt`, falling back to
`Block::AIR.default_state` only when the tag is genuinely absent, which is what a
world written by the current code will look like. Remove the three TODO comments
as you go, since they describe the bug you are fixing.

Use whatever block-state NBT helper this codebase already has for reading and
writing a state. Look at how neighbouring block entities in the same directory
persist a `&'static BlockState` and match that, rather than inventing a new
encoding. A bespoke format here would round-trip within Pumpkin and be unreadable
to vanilla, which defeats the point.

Do not change `pushed_block_state`'s type, and do not touch the extension or
retraction logic above line 270. The bug is entirely in persistence.

Clippy denies all, pedantic, nursery and cargo. Identifiers in doc comments need
backticks. No emoji, no em-dashes.
