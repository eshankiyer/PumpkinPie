WORKTREE: /home/eshanki/pumpkin-wt/tripwireshears
FILES: pumpkin/src/block/blocks/redstone/tripwire.rs:75-115
CRATES: pumpkin
FUNC: fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
ASSERT_ABSENT: // TODO: Deduct 1 durability from held shears (skip in Creative mode).
---
Disarming a tripwire with shears costs no durability, so a single pair of shears
disarms an unlimited number of tripwires.

The disarm path sets the `disarmed` property and then stops:

    props.disarmed = true;
    args.world.set_block_state(...).await;
    // TODO world.emitGameEvent(player, GameEvent.SHEAR, pos);
    // TODO: Deduct 1 durability from held shears (skip in Creative mode).

Vanilla's `TripWireBlock#playerWillDestroy` damages the shears by 1 when a
non-creative player disarms the wire, the same as any other shear use.

Deduct one point of durability from the shears the player is holding, and skip it
in creative mode. Use whatever item-damage helper this codebase already uses when
a tool is used up elsewhere; look at how another block damages a held tool on use
and follow that exactly, including how it handles the item breaking at zero
durability and how it picks the correct hand.

Leave the game-event TODO on the line above alone. Pumpkin has no game-event
dispatch layer at all yet, so that line cannot be resolved here, and deleting the
comment without implementing it would erase a real known gap.

Do not change the disarm logic, the `disarmed` property, or the block flags on
the `set_block_state` call.

Enchantments: if the existing damage helper already accounts for Unbreaking, use
it and let it. Do not hand-roll an Unbreaking roll here.

Clippy denies all, pedantic, nursery and cargo. No emoji, no em-dashes.
