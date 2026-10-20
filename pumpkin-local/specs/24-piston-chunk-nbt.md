WORKTREE: /home/eshanki/pumpkin-wt/pistonstate
FILES: pumpkin/src/block/entities/piston.rs:308-322
CRATES: pumpkin
FUNC: fn chunk_data_nbt(&self) -> Option<NbtCompound> {
CHAIN: none
ASSERT_PRESENT: put_compound
API: NbtCompound::put_compound
API: NbtCompound::put_string
API: BlockState::from_id_with_block
API: Block::properties
API: fn to_props
---
`chunk_data_nbt` is the piston block entity's chunk-snapshot serialiser. Like
`write_nbt` it skips `pushed_block_state`; the `// TODO: pushed_block_state`
comment in this function marks the gap. This spec fixes ONLY this function.

VERIFIED VANILLA: the piston block entity stores the moved block under the
NBT key `blockState`, a block state compound: a `Name` string (the namespaced
block id) and, when the block has properties, a `Properties` compound of
string property names to string values.
Source: https://minecraft.wiki/w/Moving_Block (Block entity data)

The exact recipe, using only APIs verified to exist in this tree:

1. `let (block, _) = BlockState::from_id_with_block(self.pushed_block_state.id);`
2. Build a child compound: `NbtCompound::new()`, then
   `child.put_string("Name", block.name.to_string())`. The `name` field on
   `Block` is the namespaced id.
3. If `block.properties(self.pushed_block_state.id)` returns `Some(props)`,
   build a `Properties` compound from `props.to_props()` with `put_string`
   per entry. If `None`, write no `Properties` compound.
4. `nbt.put_compound("blockState", child)` alongside the existing FACING,
   LAST_PROGRESS, EXTENDING and SOURCE writes, which must not change.

Delete the `// TODO: pushed_block_state` comment in this function only. Keep
the `// TODO: duplicated code because of async :c` comment; deduplication is
out of scope.

Clippy denies all, pedantic, nursery and cargo. Identifiers in doc comments
need backticks. No emoji, no em-dashes.
