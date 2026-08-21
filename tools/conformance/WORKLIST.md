# Conformance worklist: what the PumpkinPie/Steel percentage gap actually is

Regenerated 2026-08-21 (third pass) from two runs of `tools/conformance/conformance.py`
against the same 26.2 decompile, AFTER the nested-class attribution fix and the fn-less-file
dropout fix described in section 4. Both runs are checked in beside this file:

    conformance.pumpkin.json   PumpkinPie   (probes: 18 groups passed)
    conformance.steel.json     Steel        (probes: 9 repo-agnostic groups passed)
    sample_verdicts_2109.json        the 15 hand-verified absent-leads behind section 3
    sample_verdicts_2108.json        the previous pass's sample, kept for the trend
    sample_verdicts_placement.json   the 15 hand-verified leads behind section 1.2

**Every number below moved again, and none of that movement is progress.** Two instrument
defects were fixed. Nested Java classes used to have their methods hung on the outer class,
so `Item.Properties.axe` was reported as a missing `Item` method. And a class that mapped to
a Rust file containing no literal `fn` was thrown away as "unresolved", which hit Steel 133
times and PumpkinPie 5 times because Steel writes its packets as derive-only structs. Of
Steel's 133, 68 are rescued by crediting derive-generated methods and 65 remain genuinely
fn-less and are now analysed rather than dropped. Read
every delta as a measurement correction.

Reproduce:

    python3 tools/conformance/conformance.py --out tools/conformance/conformance.pumpkin.json
    PUMPKIN_REPO=<steel> python3 tools/conformance/conformance.py \
        --out tools/conformance/conformance.steel.json
    python3 tools/conformance/compare.py tools/conformance/conformance.pumpkin.json \
        tools/conformance/conformance.steel.json --labels pumpkin steel

Steel no longer runs with `--no-probes`. The probe set is split: 9 repo-agnostic groups
(denominator shape, nested attribution, index sizes, macro-declared methods, arithmetic) run
against any repo, and the PumpkinPie-layout-specific rename and mapping probes are gated on
`IS_PUMPKIN`. No number in this document comes from an unprobed run.

**Nothing from Steel's source is reproduced here.** The only things taken from that repo are
which vanilla class names it maps and which derive macros its packet structs use. Every
behavioural line below was read from the Mojang decompile at
`~/pumpkin-vanilla-26.2/decompiled`, not from Steel.

---

## 1. How much of the gap is artifact

### 1.1 The headline numbers are still not comparable, but the dropout asymmetry is gone

|                          | PumpkinPie | Steel  | PumpkinPie, previous pass | Steel, previous pass |
| ------------------------ | ---------: | -----: | ------------------------: | -------------------: |
| vanilla classes enumerated |     3238 |   3238 |                      2645 |                 2645 |
| vanilla classes mapped   |       1094 |    889 |                      1041 |                  838 |
| analysed at method level |       1069 |    887 |                      1011 |                  703 |
| dropped, unresolved      |     **25** |  **2** |                        30 |              **135** |
| resolved to a fn-less file (kept) |  5 |     65 |                         - |                    - |
| methods scored           |       8653 |   5443 |                      9092 |                 5296 |
| strict                   |     20.65% | 33.09% |                    19.87% |               33.61% |
| loose                    |     49.24% | 61.88% |                    48.99% |               60.91% |
| absent (leads)           |       4392 |   2075 |                      4638 |                 2070 |

The class denominator grew from 2645 to 3238 because 593 public nested classes are now
enumerated in their own right instead of being folded into their outer class. PumpkinPie's
lead count fell by 246 - those were the phantom builder calls.

**Steel's dropout went 135 -> 2 and PumpkinPie's 30 -> 25.** That was the single reason
cross-repo figures could not be quoted, and it is closed: the two denominators are now pruned
at comparable rates. The remaining objection to the headline row is the ordinary one - the
denominator is "methods of the classes this repo maps", and PumpkinPie maps 182 more classes,
so it scores itself against 3210 more methods. **The absent counts 4392 vs 2075 are still not
a defect comparison.**

Normalising to all 17101 enumerated vanilla methods, unmapped counted absent:

|                        | PumpkinPie | Steel  |
| ---------------------- | ---------: | -----: |
| strict, all of vanilla |     10.41% | 10.53% |
| loose, all of vanilla  |     24.87% | 19.69% |

**On absolute strict coverage the two remain a dead heat (10.41 vs 10.53).** PumpkinPie leads the
absolute loose row by 5.2 points, and that row is now safe to quote in a way it was not
before, because Steel's 135 wrongly-dropped classes are back in its numerator.

### 1.2 The placement artifact (unchanged this pass)

Measured previously at 12/15 = 80% of PumpkinPie's strict-absent/loose-present methods:
the method exists, it just does not live in the one file the class maps to. See
`sample_verdicts_placement.json`. Nothing this pass touched that measurement.

### 1.3 The common-class gap is real depth

On the 465 classes both repos analyse (3907 scored methods each):

|            | strict | loose | absent |
| ---------- | -----: | ----: | -----: |
| PumpkinPie | 18.38% | 45.18% | 2142 |
| Steel      | 34.6% | 62.61% | 1461 |

Steel's loose lead (17.4 pts) is as large as its strict lead (16.2 pts). Loose scoring is
placement-blind, so the common-class gap is depth, not file layout. It sat at 17.4 pts before
the nested fix too, which is the best evidence that neither fix flattered either side.

Common-set loose % by subsystem:

| subsystem | scored | pumpkin | steel |
| --- | ---: | ---: | ---: |
| entity | 1643 | 36.28% | 59.65% |
| block | 1269 | 59.1% | 74.0% |
| item | 343 | 30.61% | 43.44% |
| protocol | 222 | 71.17% | 76.13% |
| worldgen | 179 | 28.49% | 34.08% |
| inventory | 54 | 59.26% | 72.22% |
| chunk | 52 | 38.46% | 46.15% |
| material | 51 | 50.98% | 74.51% |
| border | 36 | 30.56% | 63.89% |
| server_level | 27 | 0.0% | 22.22% |
| storage | 12 | 41.67% | 66.67% |
| food | 11 | 72.73% | 63.64% |
| commands | 4 | 25.0% | 0.0% |
| gameevent | 3 | 66.67% | 66.67% |
| saveddata | 1 | 0.0% | 100.0% |

### 1.4 One concrete gap the percentages hide

`pumpkin-data/src/blocks.rs:211` -

```rust
pub const fn mirror(&self, id: BlockStateId, _mirror: Mirror) -> &'static BlockState {
    BlockState::from_id(id)
}
```

`rotate` at :219 is the same stub. Both ignore their argument. `VineBlock.mirror`
(`world/level/block/VineBlock.java:332`) swaps NORTH/SOUTH on LEFT_RIGHT and EAST/WEST on
FRONT_BACK; every directional block does something similar. Every mirrored or rotated
structure placement therefore places unrotated states. This scores as a *loose match* -
the instrument counts `mirror` as present.

---

## 2. Which vanilla classes Steel covers that PumpkinPie does not

421 classes, against 601 the other way. As established in the previous pass, the
large majority of both directions are naming, not absence: `map_class` matches on the vanilla
class name while both repos name files after the block or the entity. The triage table from
that pass still holds and is not repeated here; the standing conclusion is unchanged.

**There is no meaningful class-level backlog.** The genuinely-absent list, confirmed by
targeted grep in the previous pass, is short: `SoundType`, `MoveTowardsTargetGoal`,
`FollowMobGoal`, `RestrictSunGoal`, `PathNavigation`, plus the infrastructure classes
`ChunkMap`, `WorldGenRegion`, `SectionStorage`, `ChunkStep`, `ChunkStatusTasks`. Ranked by
method count, the largest classes only Steel maps are `EnchantmentHelper` (50 methods),
`ChunkMap` (45), `WorldGenRegion` (43), `PathNavigation` (27), `DiodeBlock` (23),
`NoiseChunk` (22).

---

## 3. Ranked worklist

Source: methods absent workspace-wide in PumpkinPie that Steel has a counterpart for, on the
465 common classes. Ranked by (subsystem weight x lead count); weights entity/block 3.0,
inventory 2.5, item 2.0, material 1.5, worldgen/chunk/commands/border 1.0, protocol 0.3.

**Precision caveat, stated up front.** These are LEADS, not defects. Re-measured on this
instrument revision: **8/15 = 53% of absent-leads are real gaps** (`sample_verdicts_2109.json`,
seed 2109; with n=15 the Wilson 95% interval is roughly 30-75%). The other seven were 1
rename, 3 inlined-or-modelled-as-a-field, 2 generated data, 1 client-only. Scale any count
below by roughly 0.5 for an expected real backlog.

The trend across three samples of three instrument revisions - 2/18 = 11%, 5/15 = 33%,
8/15 = 53% - is what removing false-positive sources is supposed to do to precision. It is
directional only: the three samples are not from the same lead pool, and none of them is
large enough to separate 33% from 53% on its own.

Difficulty is unchanged from the previous pass and omitted here; the ranking is the payload.

| # | class | subsystem | leads | decompile path |
| --: | --- | --- | ---: | --- |
| 1 | `Entity` | entity | 308 | `net/minecraft/world/entity/Entity.java` |
| 2 | `LivingEntity` | entity | 226 | `net/minecraft/world/entity/LivingEntity.java` |
| 3 | `Player` | entity | 152 | `net/minecraft/world/entity/player/Player.java` |
| 4 | `Mob` | entity | 80 | `net/minecraft/world/entity/Mob.java` |
| 5 | `Block` | block | 47 | `net/minecraft/world/level/block/Block.java` |
| 6 | `ItemStack` | item | 69 | `net/minecraft/world/item/ItemStack.java` |
| 7 | `BlockEntity` | block | 28 | `net/minecraft/world/level/block/entity/BlockEntity.java` |
| 8 | `Enchantment` | item | 41 | `net/minecraft/world/item/enchantment/Enchantment.java` |
| 9 | `Inventory` | entity | 26 | `net/minecraft/world/entity/player/Inventory.java` |
| 10 | `EntityType` | entity | 25 | `net/minecraft/world/entity/EntityType.java` |
| 11 | `Projectile` | entity | 24 | `net/minecraft/world/entity/projectile/Projectile.java` |
| 12 | `Item` | item | 33 | `net/minecraft/world/item/Item.java` |
| 13 | `ItemEntity` | entity | 19 | `net/minecraft/world/entity/item/ItemEntity.java` |
| 14 | `Pig` | entity | 14 | `net/minecraft/world/entity/animal/pig/Pig.java` |
| 15 | `Animal` | entity | 13 | `net/minecraft/world/entity/animal/Animal.java` |
| 16 | `AttributeInstance` | entity | 13 | `net/minecraft/world/entity/ai/attributes/AttributeInstance.java` |
| 17 | `BeehiveBlockEntity` | block | 13 | `net/minecraft/world/level/block/entity/BeehiveBlockEntity.java` |
| 18 | `CampfireBlockEntity` | block | 11 | `net/minecraft/world/level/block/entity/CampfireBlockEntity.java` |
| 19 | `FallingBlockEntity` | entity | 11 | `net/minecraft/world/entity/item/FallingBlockEntity.java` |
| 20 | `CraftingMenu` | inventory | 12 | `net/minecraft/world/inventory/CraftingMenu.java` |
| 21 | `BeehiveBlock` | block | 10 | `net/minecraft/world/level/block/BeehiveBlock.java` |
| 22 | `CampfireBlock` | block | 10 | `net/minecraft/world/level/block/CampfireBlock.java` |
| 23 | `ExperienceOrb` | entity | 10 | `net/minecraft/world/entity/ExperienceOrb.java` |
| 24 | `Sheep` | entity | 10 | `net/minecraft/world/entity/animal/sheep/Sheep.java` |
| 25 | `WorldBorder` | border | 25 | `net/minecraft/world/level/border/WorldBorder.java` |
| 26 | `LeavesBlock` | block | 8 | `net/minecraft/world/level/block/LeavesBlock.java` |
| 27 | `PowderSnowBlock` | block | 8 | `net/minecraft/world/level/block/PowderSnowBlock.java` |
| 28 | `ChiseledBookShelfBlockEntity` | block | 7 | `net/minecraft/world/level/block/entity/ChiseledBookShelfBlockEntity.java` |
| 29 | `ComposterBlock` | block | 7 | `net/minecraft/world/level/block/ComposterBlock.java` |
| 30 | `CropBlock` | block | 7 | `net/minecraft/world/level/block/CropBlock.java` |
| 31 | `GoalSelector` | entity | 7 | `net/minecraft/world/entity/ai/goal/GoalSelector.java` |
| 32 | `WallHangingSignBlock` | block | 7 | `net/minecraft/world/level/block/WallHangingSignBlock.java` |
| 33 | `StructurePiece` | worldgen | 20 | `net/minecraft/world/level/levelgen/structure/StructurePiece.java` |
| 34 | `ChunkHolder` | server_level | 18 | `net/minecraft/server/level/ChunkHolder.java` |
| 35 | `AnvilBlock` | block | 6 | `net/minecraft/world/level/block/AnvilBlock.java` |

169 class/lead pairs and 1797 leads in total; full data in `compare.py --out`'s `depth_gaps`.

---

## 4. What was fixed in the instrument, and what is still wrong

Done this pass (2026-08-21), all in `tools/conformance/`, Python only:

1. **Nested-class attribution.** `enumerate_vanilla` now walks brace depth over a sanitized
   copy of each .java file (comments, strings, char literals and text blocks blanked out
   offset-preserving) and attributes every method to the innermost NAMED type containing it.
   Anonymous classes deliberately fall through to their enclosing named class. Public and
   protected nested types are emitted as their own units, `Outer.Inner`; package-private
   helper types in the same file own their methods but are not emitted, so they no longer
   inflate the outer class. **`Item` went from 83 methods and 70 leads to 43 and 33** - the
   41 removed were `Item.Properties` builder calls (`axe`, `durability`, `food`,
   `fireResistant`). Enumerated classes: 2645 -> 3238.
   Nested units are mapped by `Outer+Inner` first, then by the bare inner name only if it is
   multi-word and not structural (`Builder`, `Type`, `Entry`...). That rule was measured, not
   guessed: over the 65 nested classes that matched on a bare inner name, multi-word names
   were nearly always right and one-word names were wrong about half the time
   (`Column.Range` -> a spline `Range`, `VaultBlockEntity.Server` -> the game server).
2. **A resolved file with no `fn` is analysed, not dropped.** This conflated two states.
   "No file resolved" is a mapping failure; "file resolved, contains no callable" is a
   successful mapping onto a data struct. The second is now kept and counted separately as
   `classes_resolved_without_fns`. **Steel's dropout: 135 -> 2. PumpkinPie's: 30 -> 25.**
3. **Macro-declared methods are read.** `MACRO_METHODS` maps derive and attribute macro names
   to the methods they generate, verified by reading both macro crates
   (`pumpkin-macros/src/lib.rs:203/228/524/585/635`; steel-macros `read_from.rs:187`,
   `write_to.rs:241`, `packet.rs:53`). Without it a derive-only packet struct scans as an
   empty file. Note the brief's diagnosis was wrong on this point: Steel's packets ARE
   `pub struct`, and the declaration scanner always saw them. The dropout was defect 2 above.
4. **Same-stem contamination.** `SUBSYSTEM_PATH_HINTS` drops candidate files that sit in a
   demonstrably different area when at least one candidate sits in the right one, so vanilla
   `Slime` (an entity) no longer picks up `block/blocks/slime.rs`.
5. **Bedrock demotion and two hand-verified maps.** The decompile is the Java edition
   throughout, so a `/bedrock/` file is never preferred when another candidate exists;
   `Attribute` now resolves to `entity/attributes.rs` and `LevelChunk` to
   `pumpkin-world/src/chunk/mod.rs` via `CLASS_FILE_HINTS`. `CLASS_FILE_BLOCKLIST` is new: a
   path-keyed table of hand-verified WRONG maps, currently one entry - vanilla `BoundingBox`
   is the integer structure box, `pumpkin-util/src/math/boundingbox.rs` is the float entity
   AABB, and `rg BoundingBox crates/pumpkin-world/src` is empty, so its 22 "absences" were an
   artefact of a name collision. Being path-keyed, the entry is a no-op for any other repo.
6. **Probes: 15 groups -> 18 for PumpkinPie, and 9 of them now run for Steel too.** New:
   `Item.Properties` is enumerated and owns the builder calls while `Item` does not; a nested
   constructor is not a method; braces inside string and char literals do not close a class
   body; derive and attribute macro names are read as methods; an unknown macro contributes
   nothing; the fn-less dropout stays under a tenth of the analysed set; `Slime` stays an
   entity; `Attribute`/`LevelChunk` avoid `/bedrock/`.
   **Negative-controlled twice this pass.** Reversing the method-attribution sort to the old
   outermost-class behaviour failed `Item.Properties owns the builder calls` and the run
   exited 2; stubbing `macro_fns` to return an empty set failed `derive names are read as
   methods` and the run exited 2. Both were restored and 18 groups pass.

Still wrong, in priority order:

1. **Behaviour is never compared, only names.** Unchanged and unfixable at this level. A
   method counted present has a Rust fn whose name maps to it; nobody checked the body.
   Efficiency was registered and did nothing, and this instrument would have called it
   present.
2. **The loose tier credits trait defaults.** `normal_use` is declared once on the `Block`
   trait, so every block matches `useWithoutItem` whether or not it overrides it. This is why
   the truth is between strict and loose and neither number should be quoted alone.
3. **Bodiless declarations are outside the denominator.** `METHOD_RE` requires `{`, so
   `public abstract boolean canUse();` is not enumerated. Pinned by a probe.
4. **Curated exclusion lists are incomplete.** `CLIENT_ONLY_METHODS` and
   `DATA_MODELED_METHODS` are hand-written; this pass's sample found one more of each
   (`CampfireBlock.animateTick`, `ChunkStatus.getStatusList`). That makes lead counts
   pessimistic, not optimistic.
5. **Nested-class mapping is conservative by design.** Of 593 enumerated public nested
   classes PumpkinPie resolves a minority. Unresolved nested classes generate no leads, so
   this costs coverage rather than creating phantoms, but it means the class-coverage
   percentage (33.79%) is not comparable to the 39.36% printed before this pass.
6. **Precision is measured at n=15.** 53% has a 95% interval of roughly 30-75%. Do not quote
   it to two significant figures and do not build a schedule on it.

**Cross-repo quotability.** The dropout asymmetry that made every PumpkinPie-vs-Steel figure
unsafe is closed (135 -> 2 against 30 -> 25), and both runs are now probed. The common-class
table in 1.3 and the absolute table in 1.1 are quotable WITH their blind spots stated: name
level only, loose credits trait defaults, and Steel's `only_steel` set has not been
re-triaged on this revision.
