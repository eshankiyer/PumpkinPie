WORKTREE: /home/eshanki/pumpkin-wt/aecnbt
FILES: pumpkin/src/entity/area_effect_cloud.rs:1-130
CRATES: pumpkin
ASSERT_ABSENT: impl NBTStorage for AreaEffectCloudEntity {}
---
An area effect cloud saves and loads nothing, so any cloud restored through the
generic entity dispatch path comes back with constructor defaults.

    impl NBTStorage for AreaEffectCloudEntity {}

The empty impl means radius, duration, the effect list, the reapplication delay
and both on-use decays are all lost. A lingering potion cloud that survives a
chunk unload reappears as a default cloud with the wrong size, the wrong lifetime
and no effects.

Vanilla persists these on the `area_effect_cloud` entity: `Age`, `Duration`,
`Radius`, `RadiusOnUse`, `RadiusPerTick`, `DurationOnUse`, `ReapplicationDelay`,
`Owner`, and the potion contents.

Implement `write_nbt` and `read_nbt` for the fields this struct actually has.
Read the struct definition at the top of the file and persist every field that
represents cloud state rather than a derived or per-tick cache. Use vanilla's tag
names above so a cloud written by Pumpkin is readable by vanilla and the reverse.

On read, treat every tag as optional and fall back to the value the constructor
would have used. Worlds written by the current code have none of these tags, so a
strict read would fail on every existing cloud.

Follow the `NBTStorage` implementation of a neighbouring entity in the same
module tree for the exact method signatures and for how this codebase reads an
optional tag. Do not invent a serialisation shape.

If a field is behind an atomic or a lock, read it once into a local before
writing rather than holding the guard across the write, and never replace an
atomic read-modify-write with a load, arithmetic and store: entity ticks run
concurrently as spawned futures and that pattern is a lost-update race.

Do not change the cloud's tick behaviour, its effect application, or the potency
scaling. Persistence only.

Clippy denies all, pedantic, nursery and cargo. No emoji, no em-dashes.
