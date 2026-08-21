# Conformance worklist: what the PumpkinPie/Steel percentage gap actually is

Generated 2026-08-21 from two runs of `tools/conformance/conformance.py` against the same
26.2 decompile. Both runs are checked in beside this file:

    conformance.pumpkin.json   PumpkinPie   (probes: 13 passed)
    conformance.steel.json     Steel        (probes skipped - foreign repo)
    sample_verdicts_placement.json   the 15 hand-verified leads behind section 1

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

|                          | PumpkinPie | Steel  |
| ------------------------ | ---------: | -----: |
| vanilla classes mapped   |       1023 |    841 |
| analysed at method level |        897 |    699 |
| methods scored           |       6998 |   5302 |
| strict                   |     19.05% | 33.38% |
| loose                    |     46.28% | 60.86% |
| absent (leads)           |       3759 |   2075 |

The denominator is "methods of the classes this repo maps". PumpkinPie maps 182 more vanilla
classes, so it scores itself against 1696 more methods and its absent-count is bigger for
the same reason. **The absent counts 3759 vs 2075 are not a defect comparison and must never
be quoted as one.**

Normalising to all 17309 enumerated vanilla methods, unmapped counted absent:

|                       | PumpkinPie | Steel  |
| --------------------- | ---------: | -----: |
| strict, all of vanilla|      7.66% | 10.23% |
| loose, all of vanilla |     18.66% | 18.64% |

**On absolute loose coverage the two are a dead heat (18.66 vs 18.64).** That single line is
the most important thing on this page for anyone reading only percentages.

### 1.2 The measured placement artifact: 12/15 = 80%

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

On the 348 common classes, Steel's **loose** lead (61.48 vs 42.78 = 18.7 pts) is as large as
its strict lead (35.14 vs 17.63 = 17.5 pts). Loose scoring is placement-blind - it counts a
match anywhere in the workspace. So the common-class gap is **real depth, not file layout**.

Common-set loose % by subsystem:

| subsystem | scored | pumpkin | steel |
| --- | ---: | ---: | ---: |
| entity | 1547 | 35.0% | 58.4% |
| block | 1232 | 57.4% | 72.9% |
| item | 375 | 25.6% | 40.0% |
| protocol | 116 | 69.0% | 75.9% |
| material | 55 | 50.9% | 72.7% |
| inventory | 54 | 33.3% | 72.2% |
| worldgen | 113 | 29.2% | 44.2% |
| server_level | 29 | 3.5% | 24.1% |

**Summary of the three channels:**

1. **Denominator artifact** - explains the headline strict gap being wider than reality;
   removing it takes 14.3 pts down to a 2.6-pt absolute-strict gap and a 0.02-pt loose tie.
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

* **28** are classes PumpkinPie *does* cover but whose Rust file the mapper could not resolve,
  so they never reach the `classes` list at all - `Pig`, `Sheep`, `Cow`, `BucketItem`,
  `AzaleaBlock`, `ChunkStatus`, `Attribute`, `Marker` and 20 more. Not gaps.
* Of the remaining 323, triage against Pumpkin's own module tree
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

**Precision caveat, stated up front.** These are LEADS, not defects. The instrument's own
measured precision on absent-leads is 4/15 = 27% (`sample_verdicts.json`, seed 815). Scale
any count here by roughly 0.27 for an expected real backlog. Three cross-cutting names were
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
| 1 | `Entity` | entity | 153 | `world/entity/Entity.java` | core entity tick: bubble columns, portals, fall/step-on hooks, passenger control, movement emission | L |
| 2 | `LivingEntity` | entity | 121 | `world/entity/LivingEntity.java` | damage application (`actuallyHurt`), gliding, dispenser equipping, fall-damage calc, being seen as an enemy | L |
| 3 | `Player` | entity | 66 | `world/entity/player/Player.java` | food exhaustion, adventure-mode restrictions, extra knockback, attribute creation, drop rules | L |
| 4 | `Mob` | entity | 38 | `world/entity/Mob.java` | shearing equipment off mobs (`attemptToShearEquipment` at :572), custom death loot, home/leash clearing, `customServerAiStep` | M |
| 5 | `Block` | block | 25 | `world/level/block/Block.java` | `playerWillDestroy` (:514, angers piglins on GUARDED_BY_PIGLINS blocks), `fallOn` (:490, fall damage), `stepOn` (:459), `getDrops`, explosion resistance, friction | M |
| 6 | `ItemStack` | item | 23 | `world/item/ItemStack.java` | durability (`hurtAndBreak`), hover name, destroy speed, `hurtEnemy`, `interactLivingEntity`, adventure-mode break check | M |
| 7 | `Projectile` | entity | 14 | `world/entity/projectile/Projectile.java` | `checkLeftOwner` (:110, when a projectile stops passing through its shooter), deflection, `onHitBlock`, bubble-column interaction | M |
| 8 | `Item` | item | 17 | `world/item/Item.java` | per-item attribute modifiers, use animation, destroy speed, POV hit result | M |
| 9 | `BlockEntity` | block | 10 | `world/level/block/entity/BlockEntity.java` | `setChanged`/`setRemoved` dirty tracking, `getUpdateTag` client sync, `preRemoveSideEffects` | M |
| 10 | `Animal` | entity | 10 | `world/entity/animal/Animal.java` | breeding: `setInLove`, love-time countdown, `spawnChildFromBreeding`, spawn-rule light checks | M |
| 11 | `PowderSnowBlock` | block | 7 | `world/level/block/PowderSnowBlock.java` | falling into powder snow, freezing, bucket pickup, leather-boot walking | S |
| 12 | `Enchantment` | item | 10 | `world/item/enchantment/Enchantment.java` | enchantment effect components: damage/knockback modification, on-hit-block, on-projectile-spawn | M |
| 13 | `Slot` | inventory | 8 | `world/inventory/Slot.java` | `mayPlace` insert validation, `safeInsert`, `setByPlayer`, `tryRemove` - the container-click safety net | M |
| 14 | `ItemEntity` | entity | 6 | `world/entity/item/ItemEntity.java` | thrower tracking (pickup delay/ownership), fire immunity, lava hurt sound | S |
| 15 | `AttributeInstance` | entity | 6 | `world/entity/ai/attributes/AttributeInstance.java` | attribute modifier queries and permanent-modifier persistence | S |
| 16 | `CraftingMenu` | inventory | 7 | `world/inventory/CraftingMenu.java` | crafting-grid recompute on change, shift-click result move, returning the grid on close | M |
| 17 | `EntityType` | entity | 5 | `world/entity/EntityType.java` | `canSummon` (/summon gating), fire immunity by type, `isBlockDangerous` for spawning | S |
| 18 | `FallingBlockEntity` | entity | 5 | `world/entity/item/FallingBlockEntity.java` | anvil/gravel landing: `callOnBrokenAfterFall`, drop suppression, start-pos tracking | S |
| 19 | `AgeableMob` | entity | 5 | `world/entity/AgeableMob.java` | baby spawn chance, forced-age timer, spawn group size | S |
| 20 | `BeehiveBlock` | block | 5 | `world/level/block/BeehiveBlock.java` | `playerDestroy` (:91, bees released and anger on silk-touch-less break), honey level comparator output | S |
| 21 | `CampfireBlock` | block | 5 | `world/level/block/CampfireBlock.java` | lighting with flint/fire charge, waterlogging via `placeLiquid`, block-entity creation | S |
| 22 | `ComposterBlock` | block | 5 | `world/level/block/ComposterBlock.java` | composter fill level: collision shape by level, comparator output, `setChanged` on fill | S |
| 23 | `ExperienceOrb` | entity | 5 | `world/entity/ExperienceOrb.java` | orb spawning splits an amount into orb values and merges into a nearby orb where possible (`getExperienceValue`/`tryMergeToExisting`, :198-211), air drag, sound source | S |
| 24 | `CropBlock` | block | 5 | `world/level/block/CropBlock.java` | crop growth rate: `getGrowthSpeed` (:100) scans the 3x3 of blocks below for farmland and hydration, feeding the random-tick roll at :84 | M |
| 25 | `SlabBlock` | block | 5 | `world/level/block/SlabBlock.java` | slab waterlogging, light occlusion, pathfinding over slabs | S |
| 26 | `StairBlock` | block | 5 | `world/level/block/StairBlock.java` | stair explosion resistance, light occlusion, pathfinding | S |
| 27 | `LeavesBlock` | block | 4 | `world/level/block/LeavesBlock.java` | leaf decay by distance, light dampening | S |
| 28 | `AnvilBlock` | block | 4 | `world/level/block/AnvilBlock.java` | anvil landing: damage source, damage-on-fall degradation (`onBrokenAfterFall`) | S |
| 29 | `BedBlock` | block | 4 | `world/level/block/BedBlock.java` | `fallOn` (:131) halves fall distance before applying it, head/foot pairing (`getConnectedDirection`), breaking both halves | S |
| 30 | `BubbleColumnBlock` | block | 4 | `world/level/block/BubbleColumnBlock.java` | soul-sand/magma column push and drag; `updateColumn` (:77-82) propagates the column's drag state upward from the block below; bucket pickup | M |
| 31 | `ComparatorBlock` | block | 4 | `world/level/block/ComparatorBlock.java` | comparator input read from container/attached block, compare vs subtract mode | M |
| 32 | `FenceGateBlock` | block | 4 | `world/level/block/FenceGateBlock.java` | gate connection to walls, collision/occlusion when open, mob pathing through | S |
| 33 | `DoorBlock` | block | 4 | `world/level/block/DoorBlock.java` | open state query, wooden-vs-iron distinction (mob door breaking), breaking both halves | S |
| 34 | `PistonHeadBlock` | block | 4 | `world/level/block/piston/PistonHeadBlock.java` | breaking the head also removes the base; pick-block and light occlusion | S |
| 35 | `KelpBlock` | block | 4 | `world/level/block/KelpBlock.java` | kelp bone-mealing and waterlogging | S |
| 36 | `CandleCakeBlock` | block | 4 | `world/level/block/CandleCakeBlock.java` | lighting a candle on cake, pick-block, comparator output | S |
| 37 | `NoteBlockInstrument` | block | 4 | `world/level/block/state/properties/NoteBlockInstrument.java` | note block instrument by supporting block: `hasCustomSound` (:60, CUSTOM type) and `worksAboveNoteBlock` (:64, anything but BASE_BLOCK plays from above) | S |
| 38 | `MaceItem` | item | 5 | `world/item/MaceItem.java` | mace smash attack: `canSmashAttack` zeroes Y motion and suppresses fall damage on impact (:52-58) then resets fall distance (:83-85), plus `getAttackDamageBonus` scaling with fall distance | S |
| 39 | `TallSeagrassBlock` | block | 4 | `world/level/block/TallSeagrassBlock.java` | double seagrass waterlogging and pick-block | S |
| 40 | `StructurePiece` | worldgen | 12 | `world/level/levelgen/structure/StructurePiece.java` | structure generation primitives: `generateBox`, `fillColumnDown`, `createChest`, `makeBoundingBox` | M |

Below the cap, retained for reference and untriaged: `ChunkHolder` (18), `Structure` (19),
`BeehiveBlockEntity` (15), `PotionContents` (14), `StructurePlacement` (12),
`ChunkGenerator` (11), `CampfireBlockEntity` (11), `FluidState` (10), `FlowingFluid` (9),
`ServerEntity` (9), `BlockItem` (8), `Fluid` (8). Full data in `compare.py --out`'s
`depth_gaps`.

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

## 4. What to fix in the instrument itself

1. Add the section-2 renames to `KNOWN_ALIASES` in `aliases.py`. About 100 of the 323
   "only Steel" classes are name-mapping failures, not gaps.
2. Add `getFluidState` to `DATA_MODELED_METHODS` - 24 spurious leads from one entry.
3. `map_class` resolving to a file whose fns are unreadable drops the class silently; 126
   PumpkinPie classes vanish that way, including `Pig`, `Sheep` and `Cow`. Emitting the
   unresolved list into the JSON would make that visible instead of invisible.
