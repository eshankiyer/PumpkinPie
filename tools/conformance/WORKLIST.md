# Conformance worklist: what the PumpkinPie/Steel percentage gap actually is

Regenerated 2026-08-21 (second pass) from two runs of `tools/conformance/conformance.py`
against the same 26.2 decompile, AFTER the many-to-many mapper fix described in section 4.
Both runs are checked in beside this file:

    conformance.pumpkin.json   PumpkinPie   (probes: 15 passed)
    conformance.steel.json     Steel        (probes skipped - foreign repo)
    sample_verdicts_2108.json        the 15 hand-verified absent-leads behind section 3
    sample_verdicts_placement.json   the 15 hand-verified leads behind section 1.2

**Every number below moved between the first pass and this one, and none of that movement is
progress.** The mapper used to allow one vanilla class exactly one Rust file and used to stop
at the first rule that produced a NAME even when that rule produced no file. Fixing both
rescued 130 PumpkinPie classes and 2094 methods that had been dropped from the denominator
entirely. Read the deltas as a measurement correction, not as work landed.

Reproduce:

    python3 tools/conformance/conformance.py --out tools/conformance/conformance.pumpkin.json
    PUMPKIN_REPO=<steel> python3 tools/conformance/conformance.py --no-probes \
        --out tools/conformance/conformance.steel.json
    python3 tools/conformance/compare.py tools/conformance/conformance.pumpkin.json \
        tools/conformance/conformance.steel.json --labels pumpkin steel

**Nothing from Steel's source is reproduced here.** The only thing taken from that repo is
which vanilla class names it maps. Every behavioural line below was read from the Mojang
decompile at `~/pumpkin-vanilla-26.2/decompiled`, not from Steel.

---

## 1. How much of the 14-point strict lead is artifact

### 1.1 The headline numbers are not comparable at all

|                          | PumpkinPie | Steel  | PumpkinPie before the mapper fix |
| ------------------------ | ---------: | -----: | -------------------------------: |
| vanilla classes mapped   |       1041 |    838 |                             1023 |
| analysed at method level |       1011 |    703 |                              897 |
| dropped, unresolved      |         30 |    135 |                              126 |
| methods scored           |       9092 |   5296 |                             6998 |
| strict                   |     19.80% | 33.61% |                           19.05% |
| loose                    |     48.97% | 60.91% |                           46.28% |
| absent (leads)           |       4640 |   2070 |                             3759 |

The lead count went UP (3759 -> 4640) while the percentages barely moved: the fix added 114
classes and their ~2100 methods to the denominator, most of them unimplemented. The
`unresolved` row is the mapper's own dropout, and it is wildly asymmetric - PumpkinPie now
loses 30 classes to it, Steel still loses 135 (mostly `Clientbound*Packet` classes Steel
declares through a macro rather than a `pub struct`). Steel's percentages are therefore
computed over a denominator that has been pruned harder than PumpkinPie's.

The denominator is "methods of the classes this repo maps". PumpkinPie maps 182 more vanilla
classes, so it scores itself against 1696 more methods and its absent-count is bigger for
the same reason. **The absent counts 3759 vs 2075 are not a defect comparison and must never
be quoted as one.**

Normalising to all 17309 enumerated vanilla methods, unmapped counted absent:

|                       | PumpkinPie | Steel  |
| --------------------- | ---------: | -----: |
| strict, all of vanilla|     10.36% | 10.28% |
| loose, all of vanilla |     25.67% | 18.64% |

**On absolute strict coverage the two are a dead heat (10.36 vs 10.28).** The loose row now
favours PumpkinPie by 7 points, but do not spend that: Steel's 135 unresolved classes have
their whole method sets counted as uncovered here, and nobody has checked how many of them
Steel really implements. The honest reading is that absolute coverage is close and the loose
spread is within the instrument's own dropout asymmetry.

### 1.2 The measured placement artifact: 12/15 = 80%

**Measured on the PRE-FIX instrument (seed 4211) and not re-drawn.** The mapper it sampled
allowed one file per class, which is precisely the thing section 4 changed, so treat this
section as directional history. Re-draw it if the placement question matters again.

Sample: methods that are strict-ABSENT but loose-PRESENT in PumpkinPie, restricted to the
348 classes both repos map. Pool 909. One lead per class, `random.Random(4211)`, n=15. Every
verdict was reached by reading the decompile AND the Rust this session; full evidence lines
in `sample_verdicts_placement.json`.

| Verdict | n | meaning |
| --- | ---: | --- |
| GENUINE | 12 | behaviour really exists; strict scoring under-credits it |
| NOT_GENUINE | 3 | the loose tier credited an unrelated or stubbed name |

The 12 genuine ones fail strict for four distinct reasons, all architectural:

* **modelled as a field, not a fn** - `Entity.getBoundingBox` (`entity/mod.rs:1050`),
  `CampfireBlockEntity.getItems` (`block/entities/campfire.rs:17`)
* **inlined into its caller** - `CaveVinesBlock.canGrowInto` (`plant/cave_vines.rs:77`),
  `ServerboundPlayerAbilitiesPacket.isFlying` (`net/java/play/player_abilities.rs:17`)
* **renamed** - `ScaffoldingBlock.getStateForPlacement` is `on_place` in the very same file
  (`block/blocks/scaffolding.rs:74`), `ClientboundTickingStepPacket.from` is `CTickingStep::new`
* **generated data** - block shapes (`pumpkin-data/src/block_state.rs:30,32`), item ids,
  recipes, `SculkSensorPhase`

n=15 is small; the binomial interval around 80% is roughly 55-93%.

### 1.3 But the artifact does not explain the whole gap

On the 378 common classes (3788 scored methods each), Steel's **loose** lead
(61.93 vs 44.54 = 17.4 pts) is as large as its strict lead (35.30 vs 18.40 = 16.9 pts). Loose
scoring is placement-blind - it counts a match anywhere in the workspace. So the common-class
gap is **real depth, not file layout**, and the mapper fix left it essentially where it was
(18.7 -> 17.4 pts), which is the best evidence that the fix did not flatter either side.

Common-set loose % by subsystem:

| subsystem | scored | pumpkin | steel |
| --- | ---: | ---: | ---: |
| entity | 1616 | 35.8% | 59.2% |
| block | 1298 | 59.1% | 73.2% |
| item | 391 | 28.4% | 39.6% |
| protocol | 133 | 69.2% | 75.2% |
| material | 55 | 50.9% | 72.7% |
| inventory | 54 | 59.3% | 72.2% |
| worldgen | 121 | 30.6% | 44.6% |
| server_level | 27 | 0.0% | 22.2% |

Inventory moved most (33.3% -> 59.3%), and all of that is the ten hand-verified `Slot`
renames added to `METHOD_ALIASES` this pass, not code.

**Summary of the three channels:**

1. **Denominator artifact** - explains the headline strict gap being wider than reality;
   removing it takes 13.8 pts down to a 0.08-pt absolute-strict tie.
2. **Placement artifact** - 80% (12/15) of PumpkinPie's strict-absent/loose-present methods.
3. **Genuine depth** - ~18.7 pts on the common classes, concentrated in entity, inventory
   and item. This is the part that is real, and section 3 is its worklist.

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

Raw diff: **351 classes**. That number is unusable as a worklist.

* **FIXED THIS PASS.** 28 of them were classes PumpkinPie *does* cover but whose Rust file the
  mapper could not resolve, so they never reached the `classes` list - `Pig`, `Sheep`, `Cow`
  and 25 more. All of these matched a generated `pub const PIG` (which has no declaring file)
  and the mapper stopped there instead of falling through to `entity/passive/pig.rs`. `Pig`
  and `Sheep` now appear in section 3 as ordinary method-depth entries. 30 classes still drop
  out this way; they are listed as `unresolved_classes` in the JSON.
* **FIXED THIS PASS.** The ten `WeatheringCopper*Block` classes and the sign blocks are now
  reached through `CLASS_FILE_HINTS`, because `copper_weathering.rs` declares no type at all
  and no name-based rule could ever have found it.
* Of the remainder, triage against Pumpkin's own module tree
  (`crates/pumpkin/src/block/blocks/`, `block/entities/`, `entity/ai/goal/`,
  `pumpkin-inventory/src/`, `pumpkin-data/src/data_component_impl/`) shows the large majority
  are **present under a different name**. `map_class` matches on the vanilla class name, and
  PumpkinPie names files after the *block* rather than the *Java class*:

  | vanilla class(es) | actually lives in |
  | --- | --- |
  | `WeatheringCopper{Bars,Bulb,Chain,Door,Full,Grate,Slab,Stair,TrapDoor}Block`, `WeatheringLanternBlock` (10 classes) | `block/blocks/copper_weathering.rs` |
  | `WallSignBlock`, `StandingSignBlock` | `block/blocks/signs.rs`, `block/entities/sign.rs` |
  | `WallHangingSignBlock`, `CeilingHangingSignBlock` | `block/entities/hanging_sign.rs` |
  | `PointedDripstoneBlock`, `SpeleothemBlock` | `block/blocks/dripstone.rs` |
  | `LayeredCauldronBlock`, `LavaCauldronBlock` | `block/blocks/cauldron.rs` |
  | `WallTorchBlock`, `RedstoneWallTorchBlock` | `block/blocks/torches.rs`, `redstone/redstone_torch.rs` |
  | `BaseRailBlock`, `RailState` | `block/blocks/redstone/rails/` |
  | `SnowLayerBlock` | `block/blocks/snow.rs` |
  | `MossyCarpetBlock` | `block/blocks/carpet.rs` (registered as `MossCarpetBlock`/`PaleMossCarpetBlock`) |
  | `BaseCoralWallFanBlock`, `CoralWallFanBlock` | `block/blocks/coral/coral_fan.rs` |
  | `MoveToBlockGoal` | `entity/ai/goal/move_to_target_pos.rs` |
  | `Equippable`, `Consumable`, `BundleContents`, `CustomData`, `BlocksAttacks`, `KineticWeapon` | `pumpkin-data/src/data_component_impl/` |
  | `ChestMenu`, `InventoryMenu`, `ResultContainer` | `pumpkin-inventory/src/` screen handlers |

  **Fix the mapper, not the code, for these.** Adding them to `KNOWN_ALIASES` in
  `tools/conformance/aliases.py` would move a large block of PumpkinPie's apparent class gap
  onto the measured side.

* Genuinely absent, confirmed by targeted grep this session: `SoundType`,
  `MoveTowardsTargetGoal`, `FollowMobGoal`, `RestrictSunGoal`, `PathNavigation` (a pathfinder
  exists at `entity/ai/pathfinder/` but no navigation abstraction on top of it - see the TODOs
  at `entity/ai/goal/melee_attack.rs:109,179,231`), plus the infrastructure classes
  `ChunkMap`, `WorldGenRegion`, `SectionStorage`, `ChunkStep`, `ChunkStatusTasks`.

**Conclusion for Q2: there is no meaningful class-level backlog.** The actionable work is
method depth inside classes both repos already map. That is what section 3 ranks.

---

## 3. Ranked worklist

Source: methods absent workspace-wide in PumpkinPie that Steel has a counterpart for, on the
348 common classes. Ranked by (subsystem weight x lead count); weights entity/block 3.0,
inventory 2.5, item 2.0, material 1.5, worldgen/chunk/commands 1.0, protocol 0.3.

**Precision caveat, stated up front.** These are LEADS, not defects. Re-measured after the
mapper fix: 5/15 = 33% of absent-leads are real gaps (`sample_verdicts_2108.json`, seed 2108;
with n=15 the interval is roughly 12-62%). Scale any count here by roughly 0.33 for an
expected real backlog. The other ten split 2 renames, 4 inlined-into-a-caller or
modelled-as-a-field, 3 generated-data or codec-builder plumbing, 1 client-only. Three cross-cutting names were
checked this session and are already known artifacts:

| name | appears in | verdict |
| --- | ---: | --- |
| `getFluidState` | 24 blocks | **ARTIFACT** - `block_state.rs:205 is_waterlogged` + `World::get_fluid` (`world/mod.rs:6508`) derive it generically |
| `hasAnalogOutputSignal` | 10 blocks | **MOSTLY ARTIFACT** - `get_comparator_output` is a Block-trait method (`block/mod.rs:237`); per-block overrides can still be genuinely missing (`ShelfBlock` was a confirmed real gap) |
| `isPathfindable` | 26 blocks | **PARTIAL** - a central `PathType` classifier exists (`ai/pathfinder/walk_node_evaluator.rs`); per-block overrides are worth auditing individually |

Difficulty: **S** small, self-contained; **M** touches several files; **L** large;
**ARCH** blocked on architecture (see CLAUDE.md's runtime-feature-placement blocker).

| # | class | subsystem | leads | decompile path | player-visible behaviour | diff |
| --: | --- | --- | ---: | --- | --- | --- |
| 1 | `Entity` | entity | 153 | `net/minecraft/world/entity/Entity.java` | core entity tick: bubble columns, portals, fall/step-on hooks, passenger control, movement emission | L |
| 2 | `LivingEntity` | entity | 121 | `net/minecraft/world/entity/LivingEntity.java` | damage application (`actuallyHurt`), gliding, dispenser equipping, fall-damage calc, being seen as an enemy | L |
| 3 | `Player` | entity | 65 | `net/minecraft/world/entity/player/Player.java` | food exhaustion, adventure-mode restrictions, extra knockback, attribute creation, drop rules | L |
| 4 | `Mob` | entity | 37 | `net/minecraft/world/entity/Mob.java` | shearing equipment off mobs (`attemptToShearEquipment` at :572), custom death loot, home/leash clearing, `customServerAiStep` | M |
| 5 | `Block` | block | 24 | `net/minecraft/world/level/block/Block.java` | `playerWillDestroy` (:514, angers piglins on GUARDED_BY_PIGLINS blocks), `fallOn` (:490, fall damage), `stepOn` (:459), `getDrops`, explosion resistance, friction | M |
| 6 | `Projectile` | entity | 14 | `net/minecraft/world/entity/projectile/Projectile.java` | `checkLeftOwner` (:110, when a projectile stops passing through its shooter), deflection, `onHitBlock`, bubble-column interaction | M |
| 7 | `ItemStack` | item | 20 | `net/minecraft/world/item/ItemStack.java` | durability (`hurtAndBreak`), hover name, destroy speed, `hurtEnemy`, `interactLivingEntity`, adventure-mode break check | M |
| 8 | `Animal` | entity | 10 | `net/minecraft/world/entity/animal/Animal.java` | breeding: `setInLove`, love-time countdown, `spawnChildFromBreeding`, spawn-rule light checks | M |
| 9 | `Item` | item | 15 | `net/minecraft/world/item/Item.java` | per-item attribute modifiers, use animation, destroy speed, POV hit result | M |
| 10 | `BlockEntity` | block | 9 | `net/minecraft/world/level/block/entity/BlockEntity.java` | `setChanged`/`setRemoved` dirty tracking, `getUpdateTag` client sync, `preRemoveSideEffects` | M |
| 11 | `Pig` | entity | 8 | `net/minecraft/world/entity/animal/pig/Pig.java` | ridden-pig control (`getRiddenInput`, `getRiddenSpeed`, `tickRidden`), dispenser-equipping the saddle, `finalizeSpawn`, step/equip sounds | S |
| 12 | `PowderSnowBlock` | block | 7 | `net/minecraft/world/level/block/PowderSnowBlock.java` | falling into powder snow, freezing, bucket pickup, leather-boot walking | S |
| 13 | `Enchantment` | item | 10 | `net/minecraft/world/item/enchantment/Enchantment.java` | enchantment effect components: damage/knockback modification, on-hit-block, on-projectile-spawn | M |
| 14 | `AttributeInstance` | entity | 6 | `net/minecraft/world/entity/ai/attributes/AttributeInstance.java` | attribute modifier queries and permanent-modifier persistence | S |
| 15 | `ItemEntity` | entity | 6 | `net/minecraft/world/entity/item/ItemEntity.java` | thrower tracking (pickup delay/ownership), fire immunity, lava hurt sound | S |
| 16 | `CraftingMenu` | inventory | 7 | `net/minecraft/world/inventory/CraftingMenu.java` | crafting-grid recompute on change, shift-click result move, returning the grid on close | M |
| 17 | `AgeableMob` | entity | 5 | `net/minecraft/world/entity/AgeableMob.java` | baby spawn chance, forced-age timer, spawn group size | S |
| 18 | `BeehiveBlock` | block | 5 | `net/minecraft/world/level/block/BeehiveBlock.java` | `playerDestroy` (:91, bees released and anger on silk-touch-less break), honey level comparator output | S |
| 19 | `CampfireBlock` | block | 5 | `net/minecraft/world/level/block/CampfireBlock.java` | lighting with flint/fire charge, waterlogging via `placeLiquid`, block-entity creation | S |
| 20 | `CropBlock` | block | 5 | `net/minecraft/world/level/block/CropBlock.java` | crop growth rate: `getGrowthSpeed` (:100) scans the 3x3 of blocks below for farmland and hydration, feeding the random-tick roll at :84 | M |
| 21 | `EntityType` | entity | 5 | `net/minecraft/world/entity/EntityType.java` | `canSummon` (/summon gating), fire immunity by type, `isBlockDangerous` for spawning | S |
| 22 | `ExperienceOrb` | entity | 5 | `net/minecraft/world/entity/ExperienceOrb.java` | orb spawning splits an amount into orb values and merges into a nearby orb where possible (`getExperienceValue`/`tryMergeToExisting`, :198-211), air drag, sound source | S |
| 23 | `FallingBlockEntity` | entity | 5 | `net/minecraft/world/entity/item/FallingBlockEntity.java` | anvil/gravel landing: `callOnBrokenAfterFall`, drop suppression, start-pos tracking | S |
| 24 | `SlabBlock` | block | 5 | `net/minecraft/world/level/block/SlabBlock.java` | slab waterlogging, light occlusion, pathfinding over slabs | S |
| 25 | `StairBlock` | block | 5 | `net/minecraft/world/level/block/StairBlock.java` | stair explosion resistance, light occlusion, pathfinding | S |
| 26 | `AnvilBlock` | block | 4 | `net/minecraft/world/level/block/AnvilBlock.java` | anvil landing: damage source, damage-on-fall degradation (`onBrokenAfterFall`) | S |
| 27 | `BedBlock` | block | 4 | `net/minecraft/world/level/block/BedBlock.java` | `fallOn` (:131) halves fall distance before applying it, head/foot pairing (`getConnectedDirection`), breaking both halves | S |
| 28 | `BubbleColumnBlock` | block | 4 | `net/minecraft/world/level/block/BubbleColumnBlock.java` | soul-sand/magma column push and drag; `updateColumn` (:77-82) propagates the column's drag state upward from the block below; bucket pickup | M |
| 29 | `CandleCakeBlock` | block | 4 | `net/minecraft/world/level/block/CandleCakeBlock.java` | lighting a candle on cake, pick-block, comparator output | S |
| 30 | `ComparatorBlock` | block | 4 | `net/minecraft/world/level/block/ComparatorBlock.java` | comparator input read from container/attached block, compare vs subtract mode | M |
| 31 | `ComposterBlock` | block | 4 | `net/minecraft/world/level/block/ComposterBlock.java` | composter fill level: collision shape by level, comparator output, `setChanged` on fill | S |
| 32 | `DoorBlock` | block | 4 | `net/minecraft/world/level/block/DoorBlock.java` | open state query, wooden-vs-iron distinction (mob door breaking), breaking both halves | S |
| 33 | `FenceGateBlock` | block | 4 | `net/minecraft/world/level/block/FenceGateBlock.java` | gate connection to walls, collision/occlusion when open, mob pathing through | S |
| 34 | `KelpBlock` | block | 4 | `net/minecraft/world/level/block/KelpBlock.java` | kelp bone-mealing and waterlogging | S |
| 35 | `LeavesBlock` | block | 4 | `net/minecraft/world/level/block/LeavesBlock.java` | leaf decay by distance, light dampening | S |
| 36 | `NoteBlockInstrument` | block | 4 | `net/minecraft/world/level/block/state/properties/NoteBlockInstrument.java` | note block instrument by supporting block: `hasCustomSound` (:60, CUSTOM type) and `worksAboveNoteBlock` (:64, anything but BASE_BLOCK plays from above) | S |
| 37 | `PistonHeadBlock` | block | 4 | `net/minecraft/world/level/block/piston/PistonHeadBlock.java` | breaking the head also removes the base; pick-block and light occlusion | S |
| 38 | `Sheep` | entity | 4 | `net/minecraft/world/entity/animal/sheep/Sheep.java` | `ate` (regrow wool after eating grass), `customServerAiStep`, step sound | S |
| 39 | `StructurePiece` | worldgen | 12 | `net/minecraft/world/level/levelgen/structure/StructurePiece.java` | structure generation primitives: `generateBox`, `fillColumnDown`, `createChest`, `makeBoundingBox` | M |
| 40 | `TallSeagrassBlock` | block | 4 | `net/minecraft/world/level/block/TallSeagrassBlock.java` | double seagrass waterlogging and pick-block | S |

Below the cap, retained for reference and untriaged: `WallHangingSignBlock` (4), `Attribute` (3), `BarrierBlock` (3), `BigDripleafBlock` (3), `BrushableBlockEntity` (3), `ButtonBlock` (3), `CampfireBlockEntity` (3), `CandleBlock` (3), `CaveVinesBlock` (3), `ChiseledBookShelfBlockEntity` (3), `DetectorRailBlock` (3), `EndCrystal` (3). 189 class/lead pairs and 903
leads in total; full data in `compare.py --out`'s `depth_gaps`.

Class-level items, kept separate because they are new files rather than method fills:

| class | subsystem | methods | decompile path | behaviour | diff |
| --- | --- | ---: | --- | --- | --- |
| `SoundType` | block | 7 | `world/level/block/SoundType.java` | per-block break/step/place/hit/fall sound group; no equivalent found under `crates/` | M |
| `MoveTowardsTargetGoal` | entity | 4 | `world/entity/ai/goal/MoveTowardsTargetGoal.java` | mob walks toward its current target when out of melee range | S |
| `FollowMobGoal` | entity | 5 | `world/entity/ai/goal/FollowMobGoal.java` | a mob trails a nearby mob of another type (bats, small slimes) | S |
| `RestrictSunGoal` | entity | 3 | `world/entity/ai/goal/RestrictSunGoal.java` | pairs with `FleeSunGoal`: disables sun-avoidance pathing malus at night | S |
| `PathNavigation` | entity | 27 | `world/entity/ai/navigation/PathNavigation.java` | the navigation layer over the pathfinder - path recompute, stuck detection, `trimPath`. `entity/ai/goal/melee_attack.rs:109,179,231` carries explicit TODOs for it | L |
| `ChunkMap`, `WorldGenRegion`, `SectionStorage`, `ChunkStep`, `ChunkStatusTasks` | server_level/chunk | 52/43/12/6/13 | `server/level/`, `world/level/chunk/` | chunk lifecycle and worldgen staging infrastructure | ARCH |

---

## 4. What was fixed in the instrument, and what is still wrong

Done this pass (2026-08-21), all in `tools/conformance/`:

1. **One class may now map to several Rust files, and several classes to one file.**
   `map_class` returns a list. Every stage contributes instead of the first stage that
   produces a NAME terminating the chain - that single behaviour was 112 of the 126 dropped
   classes, all of them matching a fileless generated `pub const`. 188 PumpkinPie classes now
   span more than one file (block + block entity, protocol struct + its handler, item +
   entity). Dropout: 126 -> 30.
2. **`CLASS_FILE_HINTS`**, a hand-verified class -> extra-files table with fnmatch keys, for
   files no name rule can reach: `copper_weathering.rs` (declares no type at all),
   `signs.rs`, `entity/combat.rs` for `MaceItem`, and `item_stack/mod.rs` +
   `item/mod.rs` for the `Item` behaviour base. Paths absent from the repo being measured are
   dropped silently, which keeps the table portable to Steel.
3. **`CLASS_METHOD_ALIASES`**, renames true for exactly one class, so `getAttackDamageBonus`
   -> `mace_smash_damage_bonus` cannot credit every other item.
4. **13 new `METHOD_ALIASES`**, all read off both sides: the ten `Slot` renames
   (`mayPlace`/`can_insert` and friends) and three `ItemStack` ones (`hurtAndBreak`,
   `getDestroySpeed`, `isCorrectToolForDrops`).
5. **Four new probe groups** (13 -> 15 reported): `Pig` falls through a fileless name match to
   `entity/passive/pig.rs`; `Item` spans >= 2 files and reaches `item_stack`;
   `WeatheringCopperSlabBlock` and `WeatheringCopperDoorBlock` resolve to the same file; hints
   never yield a nonexistent path; dropout stays under 40; >= 20 classes span >1 file. The
   WeatheringCopper probe was negative-controlled - the hint pattern was broken deliberately,
   the probe failed the run, and the hint was restored.
6. `unresolved_classes` and `classes_multi_file` are now emitted into the JSON, so the
   dropout is visible instead of silent.

Still wrong, in priority order:

1. **Steel drops 135 classes to the same dropout PumpkinPie now drops 30 to** - mostly
   `Clientbound*Packet` classes it declares through a macro rather than a `pub struct`. Until
   that is closed, the absolute-loose comparison in section 1.1 is not trustworthy, and the
   worklist below can only be built from the 378 classes both repos resolve.
2. **Same-stem contamination, the new failure mode.** Taking every file whose stem matches a
   candidate name pulls in unrelated files: `Slime` picks up `block/blocks/slime.rs` beside
   `entity/mob/slime.rs`. Bounded by `MAX_STEM_FILES = 3`, so it is small, but it inflates the
   strict tier slightly rather than the loose one.
3. **Wrong mappings survive.** `Attribute` still resolves to a bedrock `update_attributes`
   packet file and `LevelChunk` to a bedrock chunk packet. A wrong mapping moves methods
   between the two present tiers; it does not create absences.
4. **Nested-class methods are attributed to the outer class - now the single largest
   false-positive source.** `enumerate_vanilla` regexes a whole .java file and hangs every
   method it finds on the outermost class. `LootTable.setParamSet` is really
   `LootTable.Builder.setParamSet` (1 of 15 in the precision sample), and of vanilla `Item`'s
   70 remaining leads the majority - `axe`, `durability`, `food`, `component`,
   `fireResistant`, `enchantable`, `equippable`, `craftRemainder` - are `Item.Properties`
   builder calls, not behaviour. **This, not file mapping, is why `Item` looks empty.**
   Multi-file mapping moved `Item` from 0 to 5 strict matches and 8 elsewhere; it could not
   touch the lead count, because a lead is a name absent from the WHOLE workspace and is
   independent of which file the class maps to. Fixing this needs brace-depth tracking in
   `enumerate_vanilla`, which moves the denominator and so requires re-measuring precision.
5. **Bodiless declarations are still outside the denominator** (`METHOD_RE` requires a body),
   and behaviour is still never compared - only names. Both are pinned by probes.
