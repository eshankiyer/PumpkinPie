# Design: five blocks with no (or minimal) `BlockBehaviour`

Scope per the task brief: Scaffolding, Structure Block, Loom, Grindstone, Respawn
Anchor. Vanilla source is `/tmp/pumpkin-vanilla-26.2/decompiled/`. All line numbers
below are from that decompile.

Correction to the task's premise, established by direct file reads: **Grindstone is
not a zero-implementation block.** `pumpkin/src/block/blocks/grindstone.rs` already
implements `on_place`, `can_place_at`, `get_state_for_neighbor_update`, and
`WallMountedBlock` (wall/floor/ceiling attachment). The only missing piece is the menu.
The Grindstone section below is a **modify**, not a **create**, and lists the existing
methods so a fix worker doesn't regenerate the file and drop wall-mounting.

The **Respawn Anchor** section is also not a from-scratch design: an unmerged local
branch `fix/respawn-anchor-block` (commit `17a2849`, one commit ahead of `master`,
today's date) already contains a 135-line implementation. It is not stale or
abandoned — it's a working first pass with specific, listable gaps against vanilla.
That section is a gap review, not a blank design.

---

## 1. Scaffolding

**Vanilla**: `net.minecraft.world.level.block.ScaffoldingBlock` (`ScaffoldingBlock.java`).
**Current Rust state**: no file under `pumpkin/src/block/blocks/`. `scaffolding` only
appears in `pumpkin-data` generated tables (item/block/recipe/sound IDs) and in
`pumpkin/src/block/fluid/physics.rs` (presumably a fluid-passthrough special case, not
behavior). Zero `BlockBehaviour` implementation confirmed.

### Distance propagation — `getDistance` (ScaffoldingBlock.java:158-179)

```java
public static int getDistance(final BlockGetter level, final BlockPos pos) {
   BlockPos.MutableBlockPos relativePos = pos.mutable().move(Direction.DOWN);
   BlockState belowState = level.getBlockState(relativePos);
   int distance = 7;
   if (belowState.is(Blocks.SCAFFOLDING)) {
      distance = belowState.getValue(DISTANCE);
   } else if (belowState.isFaceSturdy(level, relativePos, Direction.UP)) {
      return 0;
   }
   for (Direction direction : Direction.Plane.HORIZONTAL) {
      BlockState relativeState = level.getBlockState(relativePos.setWithOffset(pos, direction));
      ...
   }
   return distance;
}
```

Note the quirk that must be ported faithfully (precedent: `falling.rs`'s
`touches_liquid` doc comment already documents an analogous vanilla mutable-cursor
quirk in this codebase — same discipline applies here): `relativePos` starts at
`pos.below()`, is read once for the vertical check, then is **reused** via
`relativePos.setWithOffset(pos, direction)` for the horizontal scan — i.e. the
horizontal neighbors examined are around `pos` itself, not around `pos.below()`. The
scan breaks early the moment `distance == 1` is reached (line 172-174) since 1 is the
best possible value once a horizontal scaffold is found (a horizontal neighbor
contributes `+1`, so anything better than 1 can only come from directly below).
Result: 0 if directly supported by a sturdy block, else `min(vertical, horizontal+1)`
capped in practice at 7 (the "unsupported" sentinel).

Rust: a free function mirroring this signature, `fn get_distance(accessor: &dyn
BlockAccessor, pos: &BlockPos) -> u8`, callable from `on_place`/`get_state_for_placement`
and from the scheduled tick without needing a live `World`.

### canSurvive / placement (lines 72-85, 132-134)

- `canBeReplaced`: only replaceable by another scaffolding placement
  (`context.getItemInHand().is(this.asItem())`) — maps to `can_place_at`/`on_place`
  replacement logic checking the item in hand against `Block::SCAFFOLDING`.
- `getStateForPlacement`: sets `WATERLOGGED` from the target fluid state, `DISTANCE`
  from `getDistance`, `BOTTOM` from `isBottom` (line 154-156: `distance > 0 &&
  !level.getBlockState(pos.below()).is(this)` — true when this scaffold isn't resting
  directly on another scaffold but also isn't fully supported).
- `canSurvive`: `getDistance(level, pos) < 7`.

### Tick-driven fall — `tick` (lines 116-129), delay 1 (line 28, `onPlace` line 88-92)

```java
protected void tick(BlockState state, ServerLevel level, BlockPos pos, RandomSource random) {
   int distance = getDistance(level, pos);
   BlockState newState = state.setValue(DISTANCE, distance).setValue(BOTTOM, isBottom(level, pos, distance));
   if (newState.getValue(DISTANCE) == 7) {
      if (state.getValue(DISTANCE) == 7) {
         FallingBlockEntity.fall(level, pos, newState);
      } else {
         level.destroyBlock(pos, true);
      }
   } else if (state != newState) {
      level.setBlock(pos, newState, 3);
   }
}
```

Three-way branch, parity-critical:
1. New distance is 7 **and** old distance was already 7 → spawn a falling-block entity.
   Since the previous tick already recomputed and stored distance 7 (case 2, below)
   without the block breaking, this is the *following* tick confirming the block is
   still unsupported — that's when it actually starts falling as an entity.
2. New distance is 7 but old distance was not 7 → `destroyBlock(pos, true)`, i.e. break
   with drops, **no falling entity**. This is the tick where support is first found to
   be lost: the block is destroyed on the spot rather than immediately becoming a
   falling entity. Read together with case 1, the two-tick sequence is: tick N support
   is lost, distance recomputes to 7, `state != newState` so case 3 applies and the
   state is written with `DISTANCE=7`; tick N+1 recomputes distance again, this time
   both old and new are 7, so case 1 fires and it falls as an entity. In other words, a
   freshly-unsupported scaffold gets exactly one tick of grace at `DISTANCE=7` before
   falling, and only genuinely produces the `destroyBlock` no-entity path if some other
   external change (e.g. direct `setBlock`) puts it at 7 from a non-7 state without that
   grace tick ever recomputing at 7 twice in a row. **Transcribe this branch literally
   from the Java** rather than re-deriving it from this summary — the two paths
   (destroy-no-entity vs. fall-as-entity) have different drop semantics and swapping
   the condition is an easy, hard-to-notice mistake.
3. Otherwise, if the recomputed state differs, `setBlock` with the new distance/bottom.

Ticks are scheduled with delay **1** (`ScaffoldingBlock.java:28,90`), not delay 2 like
sand/gravel in `pumpkin/src/block/blocks/falling.rs`. `updateShape` (94-114)
schedules the same tick-delay-1 on every neighbor update, plus a water tick if
waterlogged.

Reuse concern: `FallingEntity::replace_spawn` (`pumpkin/src/entity/falling.rs:26-45`)
unconditionally overwrites the source position with `Block::AIR` and has a `// TODO:
use fluid state` — scaffolding is waterloggable (`WATERLOGGED` property,
`getFluidState` line 150-152), so a fully faithful port needs `replace_spawn` (or a
scaffolding-specific variant) to restore water instead of air when the scaffold block
was waterlogged. Flag this as a shared-infra dependency, not something to silently
special-case only here.

### Shape/collision — infrastructure gap

`getShape` (57-64) and `getCollisionShape` (137-147) both depend on a
`CollisionContext` with `isHoldingItem`, `isPlacement`, `isAbove(shape, pos, bool)`,
and `isDescending`. **A grep across `pumpkin/src`, `pumpkin-world/src`, and
`pumpkin-data/src` found no `CollisionContext`-equivalent type and no
`get_collision_shape`/`isPlacement`/`isAbove`/`isDescending`/`isHoldingItem` API at
all** — `BlockBehaviour` in this codebase currently has no per-entity, per-context
collision-shape hook (the closest thing found, `tall_seagrass.rs`, only computes a
fixed shape from block state, not caller context). This means:

- `getShape`'s "holding scaffolding item → full block, climbable" shortcut and
  `getCollisionShape`'s "player standing above → solid platform, else pass-through
  unless standing on the BOTTOM ladder-rung shape" behavior **cannot be implemented
  without first adding a context-aware collision-shape hook to `BlockBehaviour`**. That
  hook is infrastructure shared by any block whose collision shape depends on the
  querying entity (vanilla also uses this for scaffolding specifically — it's one of
  the few vanilla blocks that needs it). Do not build a scaffolding-only workaround;
  design the trait method once (e.g. `get_collision_shape(&self, args:
  GetCollisionShapeArgs) -> BoxedVoxelShape` with a `CollisionContext`-like struct
  carrying "is this query from block placement", "is the querying box above this
  block", "is the entity descending", "is the entity holding item X") and have
  Scaffolding be the first (and possibly only, for now) consumer.
- Until that lands, Scaffolding can ship with a static shape (e.g. always
  `SHAPE_STABLE`/full-height column) as a stopgap, clearly marked as a known
  divergence — climbing scaffolding by holding it in hand, and falling through the
  ladder-only "unstable bottom" rung, will not work correctly without the real hook.

### Files to create/modify

- Create `pumpkin/src/block/blocks/scaffolding.rs`: `ScaffoldingBlock` with
  `get_distance`, `on_place`/`get_state_for_placement`, `placed` (schedule tick delay
  1), `get_state_for_neighbor_update` (schedule tick delay 1 + water tick), `
  on_scheduled_tick` (the three-way branch), `can_place_at`.
- Register in `pumpkin/src/block/blocks/mod.rs` and `pumpkin/src/block/registry.rs`
  (same pattern as every other block file).
- Possibly modify `pumpkin/src/entity/falling.rs` (`FallingEntity::replace_spawn`) to
  accept a "restore fluid" parameter instead of hardcoding `Block::AIR`, if the
  waterlogging-on-fall case is in scope.
- If pursuing the collision-shape hook: new variant on the `BlockBehaviour` trait in
  `pumpkin/src/block/mod.rs` plus a new args struct, analogous to the existing
  `GetStateForNeighborUpdateArgs` pattern.

### Needs wiki/vanilla verification
- Exact drop behavior difference between the `destroyBlock(pos, true)` path and the
  `FallingBlockEntity.fall` path (does the falling-entity path also eventually drop the
  item, and does `destroyBlock(true)` play a different sound/particle set?) — verify
  against `Block.dropResources` / `FallingBlockEntity` semantics before assuming.

---

## 2. Structure Block

**Vanilla**: `net.minecraft.world.level.block.StructureBlock` (`StructureBlock.java`)
+ `net.minecraft.world.level.block.entity.StructureBlockEntity`
(`StructureBlockEntity.java`).
**Current Rust state**: `pumpkin/src/block/entities/structure_block.rs` exists as a
pure NBT data holder (`StructureBlockBlockEntity`: name/author/metadata/pos/size/
rotation/mirror/mode/ignore_entities/show_air/show_bounding_box/integrity/seed — all
`Mutex`-wrapped fields, `from_nbt`/`write_nbt`/`chunk_data_nbt`, no behavior methods).
**No block file** exists under `pumpkin/src/block/blocks/` for `structure_block`. This
is the largest of the five subsystems.

### Existing reusable machinery (checked, confirmed present)

- `pumpkin-world/src/generation/structure/template/` already has a full
  `StructureTemplate` model (`structure_template.rs`: `size`, `palette: Vec<PaletteEntry>`,
  `blocks: Vec<TemplateBlock>`, `entities`), an NBT **reader**
  (`StructureTemplate::from_nbt_bytes`), and a `place_template` function
  (`template/mod.rs:71-140+`) that rotates/mirrors and places a template into anything
  implementing the `BlockPlacer` trait.
- `pumpkin/src/world/block_placer.rs` (`WorldBlockPlacer`) already implements
  `BlockPlacer` against a live `World` (used today by the `/place template` command,
  `pumpkin/src/command/commands/place.rs`). **This gives LOAD (placeStructure) most of
  what it needs for free** — call `place_template` with a `WorldBlockPlacer` the same
  way `/place` does.
- `TemplateCache` (`template/cache.rs`) is **read-only and `include_bytes!`-backed** —
  it loads from binary-embedded resources (worldgen structures baked into the
  executable), keyed by name, with **no filesystem I/O and no writer**. A grep for
  `to_nbt`/`fn save`/`fn serialize`/`fn write` in `structure_template.rs` found nothing
  — there is no NBT **writer** for structure templates anywhere in the codebase today.

### Gap: SAVE mode has no infrastructure at all

Vanilla's `saveStructure` (`StructureBlockEntity.java:322-363`) does three things none
of which exist in Rust:
1. `StructureTemplateManager.getOrCreate(name)` / `.get(name)` against a
   **world-save-directory** path (vanilla: `<world>/generated/<namespace>/structures/
   <path>.nbt`), not the binary-embedded worldgen set. This needs a new,
   filesystem-backed sibling to `TemplateCache` — call it e.g.
   `WorldStructureManager` — that reads/writes under the current world's save
   directory, separate from and non-conflicting with the existing embedded cache
   (worldgen still wants the embedded, compile-time set; player-created/edited
   structures need on-disk read-write).
2. `structureTemplate.fillFromWorld(level, pos, size, includeEntities, ignoreBlocks)` —
   a **capture** routine (world → `StructureTemplate`) that does not exist; only the
   reverse (`StructureTemplate` → world, via `place_template`) exists today.
3. `manager.save(name)` — gzip-NBT **serialization** of a `StructureTemplate` back to
   bytes; the inverse of `StructureTemplate::from_nbt_bytes`. Also does not exist.

Building SAVE therefore requires, roughly in dependency order: (a) an NBT writer for
`StructureTemplate` (palette + block list + entity list → `NbtCompound`, gzip-compressed
to match the reader), (b) a `fillFromWorld`-equivalent that scans a `BlockBox` in world
coordinates and builds a `StructureTemplate` from what it finds (needs both block state
and block-entity NBT capture — block entities aren't touched by the current
`StructureTemplate` model at all as far as writing goes; check whether `TemplateEntity`
covers block entities or only mobs), and (c) a small filesystem-backed manager
(get-or-create / get / save / remove) mirroring vanilla's `StructureTemplateManager`
but scoped to `<world save dir>/generated/...`.

### detectSize — bounding-box auto-detection (`StructureBlockEntity.java:264-299`)

```java
public boolean detectSize() {
   if (this.mode != StructureMode.SAVE) return false;
   BlockPos pos = this.getBlockPos();
   int radius = 80;
   BlockPos corner1 = new BlockPos(pos.getX() - 80, level.getMinY(), pos.getZ() - 80);
   BlockPos corner2 = new BlockPos(pos.getX() + 80, level.getMaxY(), pos.getZ() + 80);
   Stream<BlockPos> relatedCorners = getRelatedCorners(corner1, corner2);
   return calculateEnclosingBoundingBox(pos, relatedCorners).filter(bb -> {
      int deltaX = bb.maxX() - bb.minX(), deltaY = ..., deltaZ = ...;
      if (deltaX > 1 && deltaY > 1 && deltaZ > 1) {
         this.structurePos = new BlockPos(bb.minX()-pos.getX()+1, bb.minY()-pos.getY()+1, bb.minZ()-pos.getZ()+1);
         this.structureSize = new Vec3i(deltaX-1, deltaY-1, deltaZ-1);
         ...
         return true;
      }
      return false;
   }).isPresent();
}
```

Scans an 80-block XZ radius (full world height) for **other structure blocks in
CORNER mode with the same `structureName`** (`getRelatedCorners`,
`StructureBlockEntity.java:291-299`, filters `input.mode == StructureMode.CORNER &&
Objects.equals(this.structureName, input.structureName)`), takes the minimal
enclosing `BoundingBox` of those corner positions (plus `pos` itself if only one corner
found, `calculateEnclosingBoundingBox` line 301-316), and only commits if every axis
delta exceeds 1 (i.e. at least a 1×1×1 usable interior after the corner-inclusive
math). The `-1` in both `structurePos` and `structureSize` accounts for corner blocks
being inclusive endpoints, not interior bounds. Only fires when `mode == SAVE`.

Positions and sizes are clamped to `[-48, 48]` per axis on load (`loadAdditional`,
`StructureBlockEntity.java:108-115`) — this clamp must be replicated in Rust's
equivalent of `from_nbt` on `StructureBlockBlockEntity` (currently unclamped — check
whether that's in scope for this task or a follow-on bug).

### saveStructure / placeStructure / placeStructureIfSameSize control flow

- `saveStructure(bool saveToDisk)` (318-329): computes world position as
  `getBlockPos().offset(structurePos)`, delegates to the static
  `saveStructure(level, name, pos, size, ignoreEntities, author, saveToDisk,
  ignoreBlocks=[])` (331-363) which always also ignores `Blocks.STRUCTURE_VOID`.
- `placeStructure(level)` (402-407) resolves the template via `getStructureTemplate`
  (409-411, looks up by name in the manager, `Optional::orElse(null)`), then the
  private overload (413-430) builds a `StructurePlaceSettings` (mirror, rotation,
  ignoreEntities, `knownShape=strict`), applies a `BlockRotProcessor` when
  `integrity < 1.0` (weighted-random block dropout, seeded via `createRandom(seed)` —
  seed 0 means "use `Util.getMillis()`", i.e. non-deterministic, line 365-367), and
  calls `template.placeInWorld(...)`. **Note**: `place_template` in this codebase
  already skips `minecraft:structure_void` and `minecraft:structure_block` palette
  entries during placement (`template/mod.rs`, confirmed by reading the source) — this
  matches vanilla's void/marker semantics and interacts with CORNER-mode round-tripping
  (a saved structure's own corner/structure-void markers won't be re-placed on load,
  which is correct).
- `placeStructureIfSameSize(level)` (369-384): only proceeds in LOAD mode; if the named
  template's `getSize()` doesn't match the block entity's recorded `structureSize`, it
  calls `loadStructureInfo` (396-400, refreshes author/size from the template, marks
  dirty) and returns `false` **without placing**; only places when sizes already match.
  This is the redstone-trigger path (`StructureBlock.trigger`, `StructureBlock.java:
  85-97`, dispatches on mode: SAVE → `saveStructure(false)`, LOAD →
  `placeStructure(level)` **not** `placeStructureIfSameSize` — that distinction
  matters: redstone-triggered LOAD always places (loads fresh), while
  `placeStructureIfSameSize` is invoked separately, from the GUI "load" button path via
  a client packet, not from `neighborChanged`).
- `unloadStructure()` (432-438): removes the template from the manager's cache/registry
  (does not delete from disk in vanilla — `StructureTemplateManager.remove` only drops
  the in-memory entry).

### usedBy — GUI open + permission (`StructureBlockEntity.java:148-158`,
`StructureBlock.java:42-51`)

```java
protected InteractionResult useWithoutItem(...) {
   if (level.getBlockEntity(pos) instanceof StructureBlockEntity sbe) {
      return sbe.usedBy(player) ? InteractionResult.SUCCESS : InteractionResult.PASS;
   }
   return InteractionResult.PASS;
}
// StructureBlockEntity:
public boolean usedBy(Player player) {
   if (!player.canUseGameMasterBlocks()) return false;
   if (player.level().isClientSide()) player.openStructureBlock(this);
   return true;
}
```

`canUseGameMasterBlocks()` gates on operator permission (permission level 2 in
vanilla, matching `GameMasterBlock` marker interface — `StructureBlock implements
GameMasterBlock`). **Closest existing precedent in this codebase is
`pumpkin/src/block/blocks/jigsaw.rs`'s `normal_use`**, which is also a `GameMasterBlock`
analog with a client-driven-update block entity:

```rust
fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
    Box::pin(async move {
        if args.player.permission_lvl.load() < PermissionLvl::Two { return BlockActionResult::Pass; }
        if args.player.gamemode.load() != GameMode::Creative { return BlockActionResult::Pass; }
        let Some(block_entity) = args.world.get_block_entity(args.position) else { return BlockActionResult::Pass; };
        args.world.update_block_entity(&block_entity);
        BlockActionResult::SuccessServer
    })
}
```

Structure Block should follow the same `permission_lvl < PermissionLvl::Two → Pass`
gate (vanilla does not additionally require creative mode for structure blocks the way
jigsaw's Rust port does — jigsaw's creative check has no vanilla citation found here
and may itself be worth separately verifying — so don't copy that part uncritically).

### GUI protocol: missing entirely

Unlike jigsaw (which has `pumpkin-protocol/src/java/server/play/set_jigsaw_block.rs`
and `jigsaw_generate.rs` as its C2S packets), **a grep of
`pumpkin-protocol/src/java/server/play/` and a repo-wide filename search for
`*structure*` found no structure-block packet at all**. Vanilla's real behavior lives
almost entirely in the C2S `ServerboundSetStructureBlockPacket` (not present in this
decompile's captured methods above, but implied by `StructureBlockEntity.UpdateType`,
lines 547-552: `UPDATE_DATA`, `SAVE_AREA`, `LOAD_AREA`, `SCAN_AREA`) — the block's
`useWithoutItem` only opens the GUI; the actual save/load/detect-size actions are all
triggered from that packet after the player edits fields client-side and hits a button.
**This packet must be added** (new file under
`pumpkin-protocol/src/java/server/play/`, wired into the packet dispatch table)
carrying: structure name, positions/size ints, mirror/rotation enums, `ignoreEntities`/
`showAir`/`showBoundingBox`/`strict` bools, integrity float, seed long, and the
`UpdateType` (which of update-data/save/load/detect-size the client is requesting) —
modeled on vanilla's field list, which is exactly the block entity's own NBT fields
(compare `StructureBlockBlockEntity`'s field list to `saveAdditional`,
`StructureBlockEntity.java:79-100` — they already match 1:1, so the packet is
effectively "the block entity's fields plus one `UpdateType` enum, minus the runtime
`powered` flag").

### neighborChanged / redstone trigger (`StructureBlock.java:67-97`)

On a redstone-signal rising edge (`hasNeighborSignal(pos)` false→true), dispatches by
`mode`: SAVE → `saveStructure(false)` (does not write to disk, only refreshes the
in-memory template — matches the "detect and preview" GUI workflow), LOAD →
`placeStructure(level)` (always places, no size-match gating), CORNER →
`unloadStructure()` (falls through with no `break`, deliberate Java fallthrough — CORNER
mode unloads on trigger, does nothing else), DATA → no-op.

### Files to create/modify

- Create `pumpkin/src/block/blocks/structure_block.rs`: `StructureBlock` with
  `normal_use` (permission-gated GUI open, mirroring `jigsaw.rs`), `placed`
  (author-from-placer, mirrors `setPlacedBy`), and `on_synced_block_event` /
  `neighbor_update`-equivalent for the redstone trigger dispatch.
- Add behavior methods to the existing `StructureBlockBlockEntity`
  (`pumpkin/src/block/entities/structure_block.rs`): `detect_size`, `save_structure`,
  `place_structure`, `place_structure_if_same_size`, `load_structure_info`,
  `unload_structure`, mirroring the vanilla method set onto the existing fields.
- New: filesystem-backed structure manager (likely
  `pumpkin-world/src/generation/structure/template/world_manager.rs` or similar) with
  get/get-or-create/save/remove against the world save directory.
- New: NBT **writer** for `StructureTemplate` (extend `structure_template.rs` with
  `to_nbt_bytes`/`fill_from_world`).
- New: C2S structure-block-update packet under
  `pumpkin-protocol/src/java/server/play/`, plus its handler (likely alongside jigsaw's
  handler in the play packet-processing module) and any S2C data-sync packet needed to
  push the block entity's current fields to the client when the GUI opens (vanilla uses
  `getUpdatePacket`/`getUpdateTag`, `StructureBlockEntity.java:139-146` — check whether
  this codebase's generic block-entity update-packet path already covers this before
  adding a bespoke one).
- Modify `pumpkin/src/block/blocks/mod.rs` and `pumpkin/src/block/registry.rs` to
  register the new block.

### Needs wiki/vanilla verification
- Whether this decompile's `StructureBlockEntity` omits an actual
  `ServerboundSetStructureBlockPacket` class definition because it lives outside
  `net.minecraft.world.level.block*` (check `net.minecraft.network.protocol.game`) —
  confirm its exact field layout from the network protocol source, not just inferred
  from the block entity's NBT fields, before implementing the new packet.
- Whether `manager.remove` (`unloadStructure`) also evicts from any client-visible
  cache or only the server template manager.

---

## 3. Loom

**Vanilla**: `net.minecraft.world.level.block.LoomBlock` (`LoomBlock.java`) +
`net.minecraft.world.inventory.LoomMenu` (`LoomMenu.java`).
**Current Rust state**: no `loom.rs` under `pumpkin/src/block/blocks/`; "loom" only
appears in generated data tables and unrelated GUI/villager/window-property code.
Zero `BlockBehaviour` implementation confirmed. `WindowType::Loom` and the
`container.loom` translation key already exist in generated data
(`pumpkin-data/src/generated/screen.rs`, `pumpkin-data/src/generated/translation.rs`),
so the client-facing plumbing for opening the window already has a home — only the
server-side block behavior and screen handler are missing.

### Block (`LoomBlock.java:20-61`)

Simple: `HorizontalDirectionalBlock`, `getStateForPlacement` sets `FACING` to the
opposite of the placer's horizontal direction (line 54), `useWithoutItem` opens the
menu and awards `Stats.INTERACT_WITH_LOOM` (lines 34-43):

```java
protected InteractionResult useWithoutItem(...) {
   if (!level.isClientSide()) {
      player.openMenu(state.getMenuProvider(level, pos));
      player.awardStat(Stats.INTERACT_WITH_LOOM);
   }
   return InteractionResult.SUCCESS;
}
```

Directly analogous to `pumpkin/src/block/blocks/stonecutter.rs`'s `normal_use`
(`args.player.increment_stat(...)` then `args.player.open_handled_screen(&Factory,
Some(*args.position))`) — same pattern, different `Statistic`/factory/handler.

### Menu (`LoomMenu.java`)

Three input slots (banner @ (13,26), dye @ (33,26), pattern-item @ (23,45)) each
type-gated by `mayPlace` (banner: `itemStack.getItem() instanceof BannerItem`; dye:
`itemStack.is(ItemTags.LOOM_DYES) && itemStack.has(DataComponents.DYE)`; pattern:
`itemStack.is(ItemTags.LOOM_PATTERNS) && itemStack.has(DataComponents
.PROVIDES_BANNER_PATTERNS)` — lines 111-117), one result slot (143,57) that is never
directly placeable and whose `onTake` (89-104) decrements banner and dye by 1 each,
clears the selected-pattern index if either input is now empty, and plays
`UI_LOOM_TAKE_RESULT` at most once per game tick (`lastSoundTime` dedup, lines 96-102).

Pattern selection is button-driven, not slot-driven: `clickMenuButton` (125-133) sets
`selectedBannerPatternIndex` (a synced `DataSlot`) from the clicked button id and
recomputes the result. `slotsChanged` (149-196) is the reactive core: whenever the
input container changes, it recomputes `selectablePatterns` from the pattern-item slot
(`getSelectablePatterns`, 135-142 — empty pattern slot → patterns tagged
`#minecraft:no_item_required`; non-empty → the item's `PROVIDES_BANNER_PATTERNS`
holder set), tries to preserve the previously-selected pattern across the recompute by
identity-matching into the new list, auto-selects if exactly one option, and only
actually builds a result (`setupResultSlot`, 269-288) if the banner has fewer than 6
existing pattern layers (`patterns.layers().size() >= 6` check, line 179) — the result
is `bannerStack.copyWithCount(1)` with one appended `(pattern, dyeColor)` layer via
`BannerPatternLayers.Builder`.

`quickMoveStack` (211-261) follows the standard four-region shuffle (result-slot region
[0..4), the three input slots by index, player inv [4..31), hotbar [31..40)) with
type-directed routing into whichever of banner/dye/pattern slot accepts the shift-clicked
item.

### Rust design — model directly on `stonecutter_screen_handler.rs`

- `pumpkin-inventory/src/loom_screen_handler.rs`: `LoomScreenHandler` struct with
  `behaviour: ScreenHandlerBehaviour`, three `Arc<SimpleInventory>` inputs (banner/dye/
  pattern, 1 slot each) or one 3-slot `SimpleInventory` (vanilla's `inputContainer` is a
  single 3-slot container — either representation works, but a single container more
  faithfully mirrors `slotsChanged` firing once per any-input-change), one output
  `SimpleInventory`, and an `AtomicU8`-or-similar `selected_pattern_index` (vanilla's
  `DataSlot`, needs client sync — check `pumpkin-inventory/src/window_property.rs` for
  the existing synced-property mechanism already referenced elsewhere in this crate).
- Slot type-gating: three custom `Slot` impls (banner/dye/pattern) analogous to
  `StonecutterOutputSlot` in the existing file, each checking the corresponding tag +
  data-component pair — `pumpkin-data`'s generated tag table already has
  `MINECRAFT_LOOM_DYES` and `MINECRAFT_LOOM_PATTERNS`
  (`pumpkin-data/src/generated/tag.rs`), and `data_component.rs` already has `Dye` and
  `ProvidesBannerPatterns` variants, so the gating predicates are directly
  expressible with existing generated data — no new codegen needed.
- Result computation: `on_button_click` sets the selected index (mirrors
  `StonecutterScreenHandler::select_recipe`/`on_button_click` almost exactly — same
  shape, swap "recipe" for "banner pattern"), recompute on every input-slot mutation via
  the handler's existing `on_slot_click` hook (see how `StonecutterScreenHandler::
  on_slot_click` calls `update_output().await` after slot 0/1 changes — Loom needs the
  same after any of its 3 slots change, plus the button-click path).
- Banner pattern append needs the banner block entity's pattern-list representation as
  a model — check `pumpkin/src/block/entities/banner.rs` (`patterns: Mutex<Option<Vec<
  NbtTag>>>`, raw NBT list, not a typed `BannerPatternLayers` struct) — the result-item
  construction in the screen handler will need to build/append to that same raw NBT
  list shape on the output `ItemStack`'s NBT/components (check how `ItemStack` carries
  component data in this codebase — likely via `pumpkin_data::item_stack::ItemStack`
  component storage, not a bespoke banner-entity path, since the result is an item in a
  slot, not a placed block).
- Block: `pumpkin/src/block/blocks/loom.rs`, `#[pumpkin_block("minecraft:loom")]`,
  `on_place` sets `FACING` from the placer's opposite horizontal direction (reuse
  whatever horizontal-facing helper other directional blocks already use — e.g. how
  `fence_gates.rs` or `signs.rs` set facing), `normal_use` opens the handler via a
  `ScreenHandlerFactory` exactly like `StonecutterScreenFactory`.

### Files to create/modify
- Create `pumpkin/src/block/blocks/loom.rs`.
- Create `pumpkin-inventory/src/loom_screen_handler.rs`.
- Register block in `pumpkin/src/block/blocks/mod.rs` /
  `pumpkin/src/block/registry.rs`; register handler export in whatever
  `pumpkin-inventory/src/lib.rs` re-export list carries `StonecutterScreenHandler`.

### Needs wiki/vanilla verification
- Whether `Stats::InteractWithLoom` (the loom-equivalent of
  `CustomStatistic::InteractWithStonecutter` used in `stonecutter.rs`) already exists in
  `pumpkin_data::statistic::CustomStatistic` — quick lookup, not done here.
- Exact client-side widget IDs/button semantics for pattern selection (grid layout) if
  this repo's client-protocol packet for "clicked button N" needs a specific N-to-slot
  mapping matching vanilla's GUI layout.

---

## 4. Grindstone

**Vanilla**: `net.minecraft.world.level.block.GrindstoneBlock` (`GrindstoneBlock.java`)
+ `net.minecraft.world.inventory.GrindstoneMenu` (`GrindstoneMenu.java`).
**Current Rust state**: `pumpkin/src/block/blocks/grindstone.rs` **already exists** and
implements:
- `on_place` (wall-mount facing via `WallMountedBlock::get_placement_face`)
- `can_place_at` (delegates to `WallMountedBlock::can_place_at`, always `true` here)
- `get_state_for_neighbor_update` (delegates to `WallMountedBlock`)
- the `WallMountedBlock` trait itself (`get_direction` maps `AttachFace::Floor/Ceiling/
  Wall` to up/down/facing direction)

**Missing**: `useWithoutItem`/`normal_use` to open a `GrindstoneMenu`, and the menu
itself — nothing else. This is purely additive to the existing file plus one new
screen-handler file; do not regenerate `grindstone.rs` from scratch.

### Block (`GrindstoneBlock.java:73-91`)

```java
protected InteractionResult useWithoutItem(...) {
   if (!level.isClientSide()) {
      player.openMenu(state.getMenuProvider(level, pos));
      player.awardStat(Stats.INTERACT_WITH_GRINDSTONE);
   }
   return InteractionResult.SUCCESS;
}
```

Same shape as Loom/Stonecutter — add a `normal_use` impl to the existing
`GrindstoneBlock`'s `BlockBehaviour` block, following `stonecutter.rs`'s pattern
exactly (`increment_stat` + `open_handled_screen`).

### Menu (`GrindstoneMenu.java`) — the real work

Two repair-input slots (49,19 and 49,40), one output slot (129,34). Core logic is
`computeResult` (117-139) called from `slotsChanged`→`createResult` whenever either
input changes:

- Both empty → empty result.
- Both stack sizes ≤ 1: if only one slot filled, result is that item **only if it has
  enchantments** (`EnchantmentHelper.hasAnyEnchantments`), copied with curses-only
  filtering applied (`removeNonCursesFrom`, see below) — this is the
  "grindstone strips a single enchanted item's non-curse enchants" case. If both
  filled, `mergeItems`.
- Otherwise (either stack size > 1) → empty result (grindstone repair/merge only
  operates on single items).

`mergeItems` (141-167): requires same item type; combines damage (`durability =
max(maxDamage1, maxDamage2)`, `remaining = remaining1 + remaining2 + durability*5/100`
— i.e. vanilla's classic "sum of remaining durability plus a 5% bonus" repair formula,
clamped to `durability` via `max(durability - remaining, 0)` on the final damage value);
for **non-damageable** items (e.g. enchanted books), instead requires
`getMaxStackSize() >= 2 && ItemStack.matches(input, additional)` and produces a
**stack of 2** merged copies; then merges enchantments from the second item into the
first (`mergeEnchantsFrom`, 169-180 — via `EnchantmentHelper.updateEnchantments`,
upgrading/adding each of the source's enchantments unless it's a curse already present
at any level on the target — i.e. curses on the target are never touched by the merge,
new curses aren't added if the target already carries any level of that curse); finally
strips non-curse enchantments is **not** applied to the merge result... actually it is:
`mergeItems` ends by calling `removeNonCursesFrom(newItem)` too (line 166) — same
final step both branches.

`removeNonCursesFrom` (182-198): removes every enchantment not tagged
`#minecraft:curse`; if the item is `ENCHANTED_BOOK` and now has zero enchantments, it
transmutes to a plain `BOOK`; recomputes `REPAIR_COST` (the anvil-cost-escalation
data component) from scratch by calling `AnvilMenu.calculateIncreasedRepairCost` once
per remaining enchantment (i.e. repair cost only reflects surviving curse
enchantments, and is **rebuilt from zero**, not carried over from either input).

On `resultSlot.onTake` (66-89): awards XP via `ExperienceOrb.award(serverLevel,
Vec3.atCenterOf(pos), amount)` where `amount` sums `getExperienceFromItem` over both
inputs (`getExperienceFromItem`, 91-104: sums `enchant.value().getMinCost(lvl)` over
all **non-curse** enchantments found via `EnchantmentHelper.getEnchantmentsForCrafting`),
then if `amount > 0` picks a random value in `[ceil(amount/2), ceil(amount/2) +
ceil(amount/2))` (`halfAmount + random.nextInt(halfAmount)`), plus fires
`level.levelEvent(1042, pos, 0)` (the grindstone-use particle/sound event), then clears
both repair slots.

### Rust design — reuse anvil's enchantment-cost machinery where possible

`pumpkin-inventory/src/anvil/anvil_screen_handler.rs` already has a `repair_cost: i16`
field with `set_repair_cost` and XP-level deduction logic
(`player.experience_level() >= self.repair_cost`,
`add_experience_levels(-(repair_cost))`) — this confirms the **XP-level-cost** side of
anvil mechanics exists, but grindstone's XP mechanic is different: grindstone
**awards** XP orbs to the world (from removed non-curse enchantments' `min_cost`), not
XP levels charged to open a menu. `pumpkin_data::generated::enchantment` already has a
`min_cost: Cost` field on every enchantment entry (confirmed via `enchantment.rs`), and
`generated::enchantments.rs` already distinguishes `MINECRAFT_VANISHING_CURSE`/
`MINECRAFT_BINDING_CURSE` (curse tag equivalents) — so the **data** needed for
`getExperienceFromItem` (sum of `min_cost` over non-curse enchantments) is already
present in generated tables; what's missing is (a) an `#minecraft:curse`-tag-equivalent
lookup on an item's enchantments (check whether `pumpkin-data`'s tag system covers
enchantment tags the way it covers block/item tags — the two curse enchantments could
also just be matched by identity/registry-key if no generic enchantment-tag lookup
exists yet), (b) the `AnvilMenu.calculateIncreasedRepairCost` formula itself (grep
`anvil_screen_handler.rs` for it — not found in the anvil grep above, meaning **this
formula also needs porting**, likely trivial: vanilla's is `cost == 0 ? 1 : cost * 2 +
1`), and (c) an `ExperienceOrb`-equivalent spawn-at-position call (check
`pumpkin/src/entity` for an existing orb-spawn helper before adding a new one).

Structure the new `pumpkin-inventory/src/grindstone_screen_handler.rs` on
`stonecutter_screen_handler.rs`'s shape:
- Two repair-input `SimpleInventory` slots (or one 2-slot container, matching
  vanilla's `repairSlots`), one output slot.
- `compute_result`/`merge_items`/`remove_non_curses_from` as free functions or
  associated methods mirroring the vanilla names 1:1 (keeps future vanilla-diffing
  easy), operating on this codebase's `ItemStack`/enchantment-component types.
- A custom output `Slot` whose `on_take`-equivalent hook (check what this codebase
  calls the analogous slot lifecycle method — `on_take_item` appears in
  `stonecutter_screen_handler.rs`'s `quick_move`) performs the XP-orb award, the level
  event, and clears the two repair slots — same shape as `StonecutterOutputSlot`.
- Block: add `normal_use` to the **existing** `GrindstoneBlock` in
  `pumpkin/src/block/blocks/grindstone.rs`, plus a `GrindstoneScreenFactory`
  (`ScreenHandlerFactory` impl) analogous to `StonecutterScreenFactory`, using
  `WindowType::Grindstone` and `translation::java::CONTAINER_GRINDSTONE_TITLE`
  (confirmed present in generated translation data).

### Files to create/modify
- Modify `pumpkin/src/block/blocks/grindstone.rs`: add `normal_use` + factory struct
  to the existing `impl BlockBehaviour for GrindstoneBlock` block; do not touch the
  existing `on_place`/`can_place_at`/`get_state_for_neighbor_update`/`WallMountedBlock`
  code.
- Create `pumpkin-inventory/src/grindstone_screen_handler.rs`.
- Possibly add an `AnvilMenu.calculateIncreasedRepairCost`-equivalent helper, shared
  between anvil and grindstone if the anvil handler doesn't already have an
  equivalent under a different name (re-check `anvil_screen_handler.rs` in full before
  assuming it's absent — the earlier grep only searched for a few literal names).
- Possibly add curse-enchantment-tag lookup support if not already generic in
  `pumpkin-data`.

### Needs wiki/vanilla verification
- Confirm current vanilla `AnvilMenu.calculateIncreasedRepairCost` formula from source
  (not memory) before porting — the task's verification-discipline rule applies
  directly here since this number directly affects repair-cost escalation, a
  frequently-misremembered formula.
- Confirm whether `ItemStack.isDamageableItem()` in this codebase's port already
  exists as a method/helper (used to distinguish the merge-durability path from the
  stack-of-2 non-damageable path).

---

## 5. Respawn Anchor

**Vanilla**: `net.minecraft.world.level.block.RespawnAnchorBlock`
(`RespawnAnchorBlock.java`).
**Current Rust state on `master`**: no file. **But**: an unmerged local branch
`fix/respawn-anchor-block` (commit `17a2849f`, one commit ahead of `master`, authored
by this user, dated today) already adds
`pumpkin/src/block/blocks/respawn_anchor.rs` (135 lines) plus registry wiring. This is
not stale/abandoned work to warn away from — it's a real, mostly-correct first pass.
This section is a **gap review against vanilla**, to hand to whoever finishes and
merges that branch, rather than a from-scratch design.

### What the branch already gets right

- `charges_to_comparator_output`: `charge * 15 / MAX_CHARGES` (integer division).
  Vanilla's `getScaledChargeLevel` (`RespawnAnchorBlock.java:214-216`) is
  `Mth.floor((charge - 0) / 4.0F * maximum)` — for `maximum=15`,
  `floor(charge/4.0*15)` produces the same 0/3/7/11/15 sequence as the branch's
  integer formula for `charge` in `0..=4`. **This is correct; do not "fix" it.**
- Basic charge-with-glowstone flow (`use_with_item`): checks item is glowstone,
  checks `charges < MAX_CHARGES`, decrements the glowstone stack unless creative,
  increments charge, writes the new state, plays the charge sound. Matches the shape
  of vanilla's `useItemOn`/`charge` (lines 78-97, 182-187) modulo the gaps below.
- Basic spawn-point flow (`normal_use`): charges==0 → pass; not usable here → break +
  explode; else set respawn point and play the set-spawn sound. Matches the shape of
  vanilla's `useWithoutItem` (99-131) modulo the gaps below.
- `get_comparator_output`: correct, matches `getAnalogOutputSignal` (218-221).

### Concrete gaps against vanilla source

1. **`canSetSpawn` is hardcoded to a single dimension, not the real vanilla attribute
   read.** Branch: `world.dimension == Dimension::THE_NETHER`. Vanilla
   (`RespawnAnchorBlock.java:178-180`):
   ```java
   public static boolean canSetSpawn(ServerLevel level, BlockPos pos) {
      return level.environmentAttributes().getValue(EnvironmentAttributes.RESPAWN_ANCHOR_WORKS, pos);
   }
   ```
   This reads a per-dimension (and, per the `pos` parameter, potentially
   per-position/datapack-conditioned) **environment attribute**, not a single
   hardcoded enum comparison. It happens to evaluate `true` only in the Nether under
   vanilla's default dimension attributes, so the branch's behavior is *currently*
   observably correct for vanilla dimensions, but wrong the moment a datapack changes
   the attribute (and the branch's own doc comment claims to mirror the attribute
   read, which it doesn't — the comment should either be corrected or the
   implementation should be). Check whether this codebase has any dimension/environment
   -attribute system at all before deciding whether to fix this now or file it as a
   known-divergence with a comment.

2. **`explode` omits vanilla's custom explosion damage calculator, damage source, and
   fire flag.** Vanilla (`RespawnAnchorBlock.java:160-176`):
   ```java
   private void explode(BlockState state, ServerLevel level, BlockPos pos) {
      level.removeBlock(pos, false);
      boolean anyWaterNeighbors = ...isWaterThatWouldFlow...;
      ExplosionDamageCalculator damageCalculator = new ExplosionDamageCalculator() {
         public Optional<Float> getBlockExplosionResistance(...) {
            return testPos.equals(pos) && inWater ? Optional.of(Blocks.WATER.getExplosionResistance()) : super...;
         }
      };
      level.explode(null, level.damageSources().badRespawnPointExplosion(boomPos), damageCalculator, boomPos, 5.0F, true, Level.ExplosionInteraction.BLOCK);
   }
   ```
   The branch calls `world.explode(position, 5.0)` (radius matches — 5.0F confirmed
   correct) but with none of: the water-neighbor check that softens the anchor's own
   block resistance when adjacent to flowing water, the `badRespawnPointExplosion`
   damage source (affects death messages/immunity interactions), or `fire=true` (this
   explosion sets fires, vanilla's other `explode`-adjacent calls in this repo default
   `fire=false` per the `explode_with_fire` vs. `explode` split already visible in
   `pumpkin/src/world/mod.rs`, lines ~3636-3645 — the plain `explode` used by the
   branch is very likely the no-fire variant, meaning this explosion currently
   produces no fire when vanilla's does). Check whether `World::explode`'s signature
   even has room for a custom damage-resistance calculator before deciding whether
   this is fixable without an API change, or should be filed as a known-divergence.

3. **`useItemOn` off-hand fallback is missing.** Vanilual (`RespawnAnchorBlock.java:
   92-96`):
   ```java
   return hand == InteractionHand.MAIN_HAND && isRespawnFuel(player.getItemInHand(InteractionHand.OFF_HAND)) && canBeCharged(state)
      ? InteractionResult.PASS
      : InteractionResult.TRY_WITH_EMPTY_HAND;
   ```
   When the main-hand item isn't glowstone (or the anchor is already full), vanilla
   checks whether the **off-hand** holds glowstone and the anchor is chargeable — if
   so it returns `PASS` (letting the game re-dispatch with the off-hand item) instead
   of falling through to empty-hand use. The branch's `use_with_item` returns
   `BlockActionResult::PassToDefaultBlockAction` unconditionally on the non-glowstone/
   full-anchor case, which does not special-case the off-hand-has-glowstone situation.
   Verify what `PassToDefaultBlockAction` actually triggers next in this codebase's
   dispatch (does it already retry with the off-hand item generically for all blocks,
   making this a non-issue? Or does vanilla's explicit off-hand check exist precisely
   because the generic retry doesn't happen without it?) before concluding this is a
   bug versus already handled elsewhere.

4. **`charge()` omits the `GameEvent.BLOCK_CHANGE` emission** (`RespawnAnchorBlock
   .java:185`, `level.gameEvent(GameEvent.BLOCK_CHANGE, pos, GameEvent.Context.of
   (sourceEntity, newState))`) — affects sculk sensors near a charging anchor. Check
   whether this codebase has a `GameEvent`/sculk-sensor-trigger system at all before
   treating this as an actionable gap; if the event system doesn't exist yet, file as
   a known-divergence rather than blocking on it.

5. **`findStandUpPosition`/`RESPAWN_OFFSETS` are entirely absent** (`RespawnAnchorBlock
   .java:51-66, 223-242`) — likely the single biggest *functional* gap, bigger than any
   of 1-4. This is the logic vanilla's actual respawn-teleport code path calls to find
   a safe standing position near the anchor (searching a fixed 25-position 3D offset
   pattern — the 8 horizontal neighbors, then the same 8 one block down, then the same
   8 one block up, then straight up — via `DismountHelper.findSafeDismountLocation`,
   preferring non-dangerous spots first). **This is not part of the block's own
   `BlockBehaviour` at all in vanilla — it's called from the player-respawn code path**
   (wherever this codebase resolves "player died, dimension entry point is a respawn
   anchor, where exactly do they materialize"), not from `respawn_anchor.rs`. Grepping
   this codebase for `find_stand_up`/`findStandUp`/`stand_up_position` found nothing —
   **respawning at a charged anchor with no clear space around it will currently place
   the player somewhere unsafe/inside blocks**, unrelated to whether the block file
   itself is "done". This needs its own investigation into wherever this codebase
   currently resolves respawn positions (bed-respawn logic, if implemented, is the
   nearest analog and likely shares this exact gap) — flag as a probably-separate,
   probably-larger ticket rather than folding into "finish the respawn anchor block".

6. **`Player::set_respawn_point`'s existing "same position" short-circuit** (already
   in this codebase, `pumpkin/src/entity/player.rs:1366-1381`) returns `false` when
   `!forced` and the new position matches the stored one exactly on dimension+block
   position — this lines up with vanilla's `RespawnConfig.isSamePosition` check
   (`RespawnAnchorBlock.java:114-127`) closely enough that the branch's `normal_use`
   handling of the `changed` bool (play sound + `SuccessServer` if changed, else
   `Consume`) is very likely already correct as written — call this one **verified
   fine**, not a gap.

### Files to create/modify (once the branch is picked up for finishing)
- `pumpkin/src/block/blocks/respawn_anchor.rs` (already exists on the branch — amend,
  don't recreate): fix `works_here` to either read a real per-dimension attribute
  system if one exists, or leave as-is with a corrected doc comment if it doesn't;
  extend `explode` call-site or `World::explode` itself if a custom resistance/damage
  source hook is feasible; add the off-hand-glowstone check to `use_with_item` if
  `PassToDefaultBlockAction` doesn't already generically retry with off-hand.
- Separately (different scope, larger, likely a different PR): wherever this codebase
  resolves post-death respawn positions, add the `RESPAWN_OFFSETS`/
  `findStandUpPosition` safe-landing-spot search.

### Needs wiki/vanilla verification
- Whether current vanilla still uses exactly this 25-offset stand-up search pattern
  (it's old, stable logic, but confirm against current source per the project's
  never-assert-from-memory rule) before implementing gap 5.
- Whether `World::explode`'s existing variants in `pumpkin/src/world/mod.rs` can be
  extended with a per-position resistance override without breaking other callers, or
  whether a bespoke respawn-anchor-only explosion path is warranted instead.
