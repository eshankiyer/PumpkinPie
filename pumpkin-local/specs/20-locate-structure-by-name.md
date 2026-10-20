WORKTREE: /home/eshanki/pumpkin-wt/locate
FILES: pumpkin/src/command/commands/locate.rs:55-105
CRATES: pumpkin
FUNC: pub fn structure_by_name(name: &str) -> Option<StructureKeys> {
TEST: locate::tests::structure_by_name
CHAIN: none
ASSERT_ABSENT: todo!("STUB: see tests::structure_by_name_*")
---
Fill this stub. It is one of three independent pure functions in the `/locate`
scaffold; nothing depends on the other two, so this can land on its own.

    pub fn structure_by_name(name: &str) -> Option<StructureKeys> {
        todo!("STUB: see tests::structure_by_name_*")
    }

Resolve a structure argument to its `StructureKeys` value using the
`STRUCTURE_NAMES` table declared directly above the function in the same file.

The behaviour is defined by the tests in the `tests` module at the bottom of this
file. Read them. They are the specification and they are the gate:

- `village_plains` resolves to `StructureKeys::VillagePlains`.
- `minecraft:ancient_city` resolves to `StructureKeys::AncientCity`, so the
  vanilla namespace prefix is accepted and stripped.
- `not_a_structure` and `minecraft:not_a_structure` both resolve to `None`.
- `othermod:village_plains` resolves to `None`. This one matters: a foreign
  namespace is not a vanilla structure. Stripping everything up to the first
  colon would wrongly accept it, so strip the literal prefix `minecraft:` only.
- Every entry in `STRUCTURE_NAMES` round-trips.

Do not add entries to `STRUCTURE_NAMES`, do not reorder it, and do not change the
function signature. Do not touch any other function in the file; the other two
stubs are separate specs and a diff that fills more than one is harder to review
and will be rejected whole.

A linear scan of a 34 entry table is fine here. This runs once per command
invocation, not per tick, so do not introduce a lazily-built map for it;
`missing_const_for_fn` and the extra dependency are not worth it.

Clippy denies all, pedantic, nursery and cargo. Identifiers named in doc comments
need backticks. No emoji, no em-dashes.
