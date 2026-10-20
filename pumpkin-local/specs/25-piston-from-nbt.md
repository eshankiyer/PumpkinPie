WORKTREE: /home/eshanki/pumpkin-wt/pistonstate
FILES: pumpkin/src/block/entities/piston.rs:274-296
CRATES: pumpkin
FUNC: fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
CHAIN: none
ASSERT_PRESENT: get_compound
API: NbtCompound::get_compound
API: NbtCompound::get_string
API: Block::from_registry_key
API: Block::properties
API: fn from_props
API: fn to_state_id
API: BlockState::from_id
---
A moving piston comes back as air after a save and load, because `from_nbt`
hardcodes `pushed_block_state` to `Block::AIR.default_state` instead of
reading it. This spec fixes ONLY this function: read the state the save side
writes.

VERIFIED VANILLA: the piston block entity stores the moved block under the
NBT key `blockState`, a block state compound: a `Name` string (the namespaced
block id) and, when the block has properties, a `Properties` compound of
string property names to string values.
Source: https://minecraft.wiki/w/Moving_Block (Block entity data)

The exact recipe, using only APIs verified to exist in this tree. It mirrors
`BlockStateResolver::resolve` in
`pumpkin-world/src/generation/structure/template/block_state_resolver.rs`,
which reads the same compound shape for structure templates:

1. `nbt.get_compound("blockState")`. If absent, keep
   `Block::AIR.default_state`: worlds written before this fix have no tag,
   and air is the only honest reading of them.
2. From the child, `get_string("Name")`. Resolve the block the way the
   resolver does: try `Block::from_registry_key` on the name with a
   `minecraft:` prefix stripped via `strip_prefix("minecraft:")` with an
   `unwrap_or` fallback to the raw name. Unknown block: keep air.
3. If a `Properties` compound is present and the block has properties
   (`block.properties(block.default_state.id)` is `Some`), rebuild the state:
   collect the child's string entries as name-value pairs, call
   `from_props(&pairs, block)` followed by `to_state_id(block)`, and turn the
   id into a state with `BlockState::from_id`. If there is no `Properties`
   compound, use `block.default_state`.
4. Assign the result to `pushed_block_state`. FACING, LAST_PROGRESS,
   EXTENDING and SOURCE reads must not change.

Delete the bare `// TODO` comment above the hardcoded line.

Clippy denies all, pedantic, nursery and cargo. Identifiers in doc comments
need backticks. No emoji, no em-dashes.
