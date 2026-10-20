WORKTREE: /home/eshanki/pumpkin-wt/tripwiretraps
FILES: pumpkin/src/block/blocks/redstone/tripwire.rs:130-175
CRATES: pumpkin
FUNC: fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
ASSERT_ABSENT: // TODO entity.canAvoidTraps()
---
Foxes and cats trigger tripwire. Vanilla exempts them.

`on_scheduled_tick` decides whether a powered tripwire should release by asking
whether the block's bounding box is empty of entities:

    let aabb = BoundingBox::from_block(args.position);
    // TODO entity.canAvoidTraps()
    if args.world.get_entities_at_box(&aabb).is_empty()
        && args.world.get_players_at_box(&aabb).is_empty()

Vanilla's `TripWireBlock#checkPressed` iterates the entities in the box and skips
any for which `Entity#isIgnoringBlockTriggers` is true. That method returns true
for exactly two vanilla entity types: fox and cat. A tripwire with only a fox
standing on it stays unpowered.

Add the predicate and use it to filter the entity list before the emptiness test.
Express it as a small helper that takes an entity type and returns whether it
ignores block triggers, so the same rule can be reused by pressure plates later,
and match on `EntityType::FOX` and `EntityType::CAT`.

Players are never exempt, so leave the `get_players_at_box` half of the condition
alone.

There is a second, unrelated TODO earlier in this file about shears durability.
Do not touch it. One logical fix per branch, and a diff that quietly fixes two
things is harder to review and more likely to be rejected whole.

Do not invert the sense of the test by accident. The condition is "nothing that
can trigger it is present, so unpower it". After filtering, an exempt entity must
count as absent, not present.

Clippy denies all, pedantic, nursery and cargo. Identifiers named in doc comments
need backticks. No emoji, no em-dashes.
