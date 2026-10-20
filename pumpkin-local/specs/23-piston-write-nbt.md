WORKTREE: /home/eshanki/pumpkin-wt/pistonstate
FILES: pumpkin/src/block/entities/piston.rs:295-310
CRATES: pumpkin
FUNC: fn write_nbt<'a>(
CHAIN: none
ASSERT_PRESENT: put_compound
API: NbtCompound::put_compound
API: NbtCompound::put_string
API: BlockState::from_id_with_block
API: Block::properties
API: fn to_props
---
A moving piston loses the block it is carrying when its chunk is saved,
because `write_nbt` never writes `pushed_block_state`. The `// TODO:
pushed_block_state` comment in this function marks the exact gap. This spec
fixes ONLY the save side in ONLY this function; the load side and the chunk
snapshot are separate specs.

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
   build a `Properties` compound from `props.to_props()`, which yields
   `Vec<(&str, &str)>` of property name and value, each written with
   `put_string`. If it returns `None` the block is stateless: write no
   `Properties` compound at all.
4. `nbt.put_compound("blockState", child)` alongside the existing FACING,
   LAST_PROGRESS, EXTENDING and SOURCE writes, which must not change.

Delete the `// TODO: pushed_block_state` comment in this function only.

Clippy denies all, pedantic, nursery and cargo. Identifiers in doc comments
need backticks. No emoji, no em-dashes.
