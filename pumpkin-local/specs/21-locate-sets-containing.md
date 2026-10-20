WORKTREE: /home/eshanki/pumpkin-wt/locate
FILES: pumpkin/src/command/commands/locate.rs:100-125
CRATES: pumpkin
FUNC: pub fn sets_containing(key: StructureKeys) -> Vec<&'static StructureSet> {
TEST: locate::tests::sets_containing
CHAIN: none
ASSERT_ABSENT: todo!("STUB: see tests::sets_containing_*")
---
Fill this stub. It is independent of the other two stubs in this file.

    pub fn sets_containing(key: StructureKeys) -> Vec<&'static StructureSet> {
        todo!("STUB: see tests::sets_containing_*")
    }

Return every structure set that lists this structure. A `StructureSet` is one
placement rule plus a weighted list of structures, so the `villages` set governs
all five village variants, and locating one variant means searching every set
that lists it.

The data you need is `StructureSet::ALL`, a `&'static [StructureSet]` in
`pumpkin_data::structures`. Each set has a `structures` field, a
`&'static [WeightedEntry]`, and each `WeightedEntry` has a `structure` field of
type `StructureKeys`. `StructureKeys` derives `PartialEq` and `Copy`.

The tests at the bottom of this file are the specification and the gate:

- `VillagePlains` returns a non-empty list, and the set it returns also lists
  `VillageDesert`, which proves you returned the whole set and not a filtered
  view of it.
- `Stronghold` returns exactly one set. Strongholds use concentric-rings
  placement rather than random spread, so this also proves the lookup does not
  filter by placement type.
- Every key in `STRUCTURE_NAMES` belongs to at least one set.

Do not filter by placement type, by dimension, or by biome. Callers decide what
to do with the placements; this function's only job is membership.

Do not change the signature, and do not touch the other two stubs in this file.

Clippy denies all, pedantic, nursery and cargo. Prefer an iterator chain over an
index loop here, `needless_range_loop` is denied. Identifiers named in doc
comments need backticks. No emoji, no em-dashes.
