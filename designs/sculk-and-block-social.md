# Design: Sculk spread/charge, chest cat-blocking, Conduit, Beehive

Vanilla source: `/tmp/pumpkin-vanilla-26.2/decompiled/`. Rust codebase: `/home/eshanki/Pumpkin`.
This is a design document only — no code in this batch.

---

## 1. Sculk spread/charge system (roadmap, ordered)

### 1.0 Why this is the highest-priority design here

`pumpkin/src/block/entities/sculk_catalyst.rs`, `sculk_sensor.rs`, `sculk_shrieker.rs` are all
NBT-only shells today — `decay_delay` / `last_vibration_frequency` / `warning_level` fields with
no behavior. `pumpkin/src/block/blocks/redstone/sculk_sensor.rs` is further along than the
census assumed: it already has a working `SculkSensorListener` wired through
`pumpkin/src/world/game_event/mod.rs`'s `GameEventListener`/`register_game_event_listener`
machinery (vanilla `VibrationSystem`/`GameEventListener`), correctly activates/deactivates the
sensor and schedules the cooldown tick (`SculkSensorBlock::trigger`, `on_scheduled_tick`). So the
vibration-detection half of sculk is not the gap. The entire gap is **growth**: nothing in this
repo drives `SculkSpreader.updateCursors`, so:

- `SculkCatalystBlock`'s `CatalystListener.handleGameEvent` (mob-death XP capture, `bloom()`
  pulse, advancement trigger) has no Rust equivalent at all — killing a mob near a catalyst does
  nothing.
- Even if it did, there is no `SculkSpreader`/`ChargeCursor` to hold and spend that charge.
- Even if there were, there is no `SculkBehaviour` dispatch, so charge could never place new
  `SculkBlock`/`SculkSensorBlock`/`SculkShriekerBlock` blocks or grow `SculkVeinBlock` faces.
- There is no `MultifaceBlock`/`MultifaceSpreader` framework at all in this codebase, and
  `SculkVeinBlock` (and, confirmed by reading `GlowLichenBlock.java`, `GlowLichenBlock` too) both
  depend on it. `pumpkin/src/block/blocks/vine.rs` (regular `VineBlock`) is a **separate,
  hand-rolled, non-multiface implementation** — it does not share any code path with
  `MultifaceBlock` and must not be conflated with it or reused for sculk vein/glow lichen.

A partial port (e.g. just wiring the catalyst's game-event listener without the spreader/growth
logic) would silently consume XP orbs into nothing and never grow sculk — worse than todays
stub, which at least visibly does nothing. Hence: build bottom-up, land each PR only when it is
independently correct and inert until wired.

### 1.1 Vanilla model (what has to be ported, precisely)

**`SculkSpreader`** (`net/minecraft/world/level/block/SculkSpreader.java`, 364 lines):
- Two static factories: `createLevelSpreader()` (non-worldgen: `growthSpawnCost=10,
  noGrowthRadius=4, chargeDecayRate=10, additionalDecayRate=5`, tag `SCULK_REPLACEABLE`) and
  `createWorldGenSpreader()` (worldgen variant, not needed for this design — no worldgen feature
  work is in scope). Constants: `MAX_GROWTH_RATE_RADIUS=24`, `MAX_CHARGE=1000`, `MAX_CURSORS=32`,
  `SHRIEKER_PLACEMENT_RATE=11`, `MAX_CURSOR_DISTANCE=1024`.
- Holds `List<ChargeCursor> cursors` (max 32, extra `addCursors` charge silently dropped once at
  cap — lines 127-139).
- `updateCursors(level, originPos, random, spreadVeins)` (lines 141-188): the per-tick driver.
  For each cursor not "unreasonable" (`distChessboard(originPos) > 1024` -> dropped, line 148),
  calls `cursor.update(...)`. If a cursor's charge drops to 0 it fires level event 3006 (sculk
  charge particle) with data `0` and is removed. Surviving cursors at the same `BlockPos` are
  merged when total charge fits in 1000 (`isWorldGeneration()` disables merging — line 159);
  otherwise kept separate, keeping the lower-charge one as the "primary" for particle purposes.
  After processing, one level-event-3006 packet per distinct position is sent, with `data =
  (numParticles << 6) + MultifaceBlock.pack(faces)` where `numParticles = (int)(log1p(charge) /
  2.3) + 1` — this is charge->particle-count encoding other implementations must replicate for
  parity, not just for show.
- **`ChargeCursor.update`** (lines 258-301) — the core per-cursor step, run only if
  `shouldUpdate` (charge > 0, and non-worldgen cursors only update on chunks that
  `shouldTickBlocksAt`):
  1. If `updateDelay > 0`, decrement and stop (a per-cursor cooldown after moving).
  2. Look up `SculkBehaviour` for the current block at the cursor's position (falls back to
     `SculkBehaviour.DEFAULT` if the block doesn't implement it — DEFAULT's
     `attemptUseCharge` just holds/drops charge based on `decayDelay`, and its
     `attemptSpreadVein` calls the same-space vein spreader).
  3. If `spreadVeins`, call `sculkBehaviour.attemptSpreadVein(...)`; if it returns true, play
     `SCULK_BLOCK_SPREAD` sound, and if `canChangeBlockStateOnSpread()` (true by default, **false**
     for `SculkBlock` — see 1.1.3) re-fetch state/behaviour since the block under the cursor may
     have changed.
  4. Call `sculkBehaviour.attemptUseCharge(cursor, level, originPos, random, spreader,
     spreadVeins)` — this is where the actual charge economy and growth placement happens; result
     becomes the cursor's new charge.
  5. If charge is now `<= 0`, call `onDischarged` and stop (cursor dies next `updateCursors`
     pass, level event 3006 with data 0).
  6. Otherwise, try to move: `getValidMovementPos` picks a random *non-corner* neighbor offset
     (18 candidates: face+edge neighbors of a 3x3x3 cube, corners excluded) whose block also
     implements `SculkBehaviour` and whose path is "unobstructed" (`isMovementUnobstructed` —
     face-adjacent moves always ok; diagonal moves need at least one of the two intermediate
     faces to be non-sturdy) and prefers one with substrate access
     (`SculkVeinBlock.hasSubstrateAccess`, i.e. a neighbor face is `SCULK_REPLACEABLE`) — first
     such candidate wins, else falls back to the first movement-unobstructed
     `SculkBehaviour`-neighbor found. If a move is found, `onDischarged` is called at the *old*
     position and the cursor's `pos` updates to the new one.
  7. `facings` is recomputed from the new position's `MultifaceBlock.availableFaces` if the new
     block is a `SculkBehaviour`.
  8. `decayDelay = sculkBehaviour.updateDecayDelay(decayDelay)` (DEFAULT: `max(age-1, 0)`, i.e.
     the delay is "used up" once) and `updateDelay = sculkBehaviour.getSculkSpreadDelay()`
     (default 1 tick).

**`SculkBehaviour`** interface (73 lines) — this is the trait every sculk-family block state
implements:
```java
byte getSculkSpreadDelay() default 1
void onDischarged(level, state, pos, random) default no-op
boolean depositCharge(level, pos, random) default false   // unused by SculkSpreader itself; hook for future use
boolean attemptSpreadVein(level, pos, state, facings, postProcess)
   default: spreads SCULK_VEIN's spreader.spreadAll(...) at this pos
boolean canChangeBlockStateOnSpread() default true
int updateDecayDelay(int age) default 1
int attemptUseCharge(cursor, level, originPos, random, spreader, spreadVeins)  // no default, must implement
```
`SculkBehaviour.DEFAULT` (anonymous instance, used when the block at a cursor's position is *not*
a `SculkBehaviour` at all, e.g. after growth turned it into something else, or air):
`attemptUseCharge` returns `cursor.getCharge()` if `decayDelay > 0` else `0` (i.e. an
un-decayed cursor idles once then dies); `attemptSpreadVein` special-cases `facings == null`
(same-space spreader) vs non-empty facings (regrow) vs empty (no-op via default).

**`SculkBlock`** (107 lines) implements `SculkBehaviour` directly on the plain sculk block:
- `attemptUseCharge` (lines 27-58): only acts `1/chargeDecayRate` of the time
  (`random.nextInt(chargeDecayRate) == 0`). If not close to the catalyst
  (`!closerThan(originPos, noGrowthRadius)`) and `canPlaceGrowth` (air or water above, and at
  most 2 sensors/shriekers already in the 9x3x9 box centered above — lines 81-100), rolls
  `random.nextInt(growthSpawnCost) < charge`; if true, places a sensor or (1/11 chance, per
  `SHRIEKER_PLACEMENT_RATE`) a shrieker above (waterlogged if the target is submerged), plays the
  new block's place sound, and always subtracts `growthSpawnCost` from charge (floored at 0)
  regardless of whether the roll succeeded. If growth is blocked (too close to catalyst, or
  growth-placement conditions fail), charge only decays `1/additionalDecayRate` of the time, by 1
  if close to the catalyst, else by `getDecayPenalty` — `max(1, charge * min(1, ((sqrt(distSq) -
  noGrowthRadius)^2) / (24-noGrowthRadius)^2) * 0.5)` (quadratic falloff between
  `noGrowthRadius` and `MAX_GROWTH_RATE_RADIUS=24`).
- `canChangeBlockStateOnSpread()` returns **false** — sculk blocks never turn into something else
  via `attemptSpreadVein` (only vein blocks do).

**`SculkVeinBlock`** (197 lines) extends `MultifaceSpreadeableBlock` (i.e. is itself a
multiface block) and also implements `SculkBehaviour`:
- Two `MultifaceSpreader` instances: `veinSpreader` (normal `DEFAULT_SPREAD_ORDER`:
  SAME_POSITION -> SAME_PLANE -> WRAP_AROUND) and `sameSpaceSpreader` (SAME_POSITION only), both
  using a custom `SculkVeinSpreaderConfig` whose `stateCanBeReplaced` refuses to spread onto
  SCULK/SCULK_CATALYST/MOVING_PISTON-backed faces, refuses non-water fluids and fire, and whose
  `isOtherBlockValidAsSource` is true for any non-vein block (so a bare `SCULK_REPLACEABLE`
  surface, not just an existing vein, can be a spread source).
- `attemptUseCharge` (lines 90-103): if `spreadVeins` and `attemptPlaceSculk` succeeds, charge -1;
  else `1/chargeDecayRate` chance to halve charge (`floor(charge*0.5)`), else unchanged.
- `attemptPlaceSculk` (lines 105-137): for each face this vein currently occupies (shuffled),
  if the block *beyond that face* is in `SCULK_REPLACEABLE`, turn it into `SCULK`, push entities
  up, play spread sound, run `veinSpreader.spreadAll` on the new sculk block, then for every
  other direction around the new sculk block, if a vein block is found there, call its
  `onDischarged` (this "eats" the vein now embedded in the new sculk block, since sculk itself has
  no faces).
- `onDischarged` (lines 69-87): strips any vein face pointing at a now-`SCULK` neighbor, and if
  no faces remain, the whole block reverts to air or water (matching fluid state).
- `regrow` (static, lines 46-67): used by `SculkBehaviour.DEFAULT.attemptSpreadVein` when a
  cursor's tracked `facings` is non-empty but the block underneath is gone — rebuilds a vein
  state with only the faces that still `canAttachTo`.
- `hasSubstrateAccess` (lines 139-151): true if any vein face points at a `SCULK_REPLACEABLE`
  block — used by `SculkSpreader.ChargeCursor.getValidMovementPos` to prefer moving onto veins
  that can still grow.

**`MultifaceBlock`** (301 lines) — the generic multi-face-attachment framework, NOT
sculk-specific in vanilla. Confirmed by reading `GlowLichenBlock.java` (55 lines) that it is
*also* a `MultifaceSpreadeableBlock` consumer there (its own `MultifaceSpreader` field,
`isValidBonemealTarget`/`performBonemeal` delegate straight to `spreader.canSpreadInAnyDirection`/
`spreadFromRandomFaceTowardRandomDirection`). Also confirmed: `pumpkin/src/block/blocks/` has no
glow-lichen file at all today (`vine.rs` is the only face-attachment-shaped block, and per 1.2 it
is a separate hand-rolled implementation, not a `MultifaceBlock` port). So in *this codebase*,
right now, the framework will have exactly one real consumer (`SculkVeinBlock`, Step 2 below);
its value as shared infrastructure is realized only if/when glow lichen is ported later. Design
and PR it block-family-agnostic anyway — it is vanilla's actual shape and cheap to keep generic —
but don't justify Step 1's scope by pointing at a second consumer that doesn't exist yet in this
repo.
- One `BooleanProperty` per `Direction` (reusing vanilla's `PipeBlock.PROPERTY_BY_DIRECTION`,
  6 directions: up/down/north/south/east/west) plus `WATERLOGGED`.
- `hasFace`/`availableFaces`/`pack`/`unpack` (bitmask, `1 << direction.ordinal()`, used by the
  charge-particle level-event data field above — Rust needs the same ordinal mapping as
  `pumpkin_data`'s `BlockDirection`/`Direction` enum, must verify ordinal order matches).
- `updateShape`: removes a face if its neighbor is no longer `canAttachTo` (face-full support
  shape or collision shape on the opposite side); reverts to air if no faces remain.
- `canSurvive`/`getStateForPlacement`/`rotate`/`mirror`/shape-caching (`makeShapes`, one VoxelShape
  per state via `Shapes.rotateAll` + per-face union) — all fairly mechanical Block trait methods.

**`MultifaceSpreader`** (212 lines) — the actual spread algorithm, decoupled from any specific
block via `SpreadConfig` (interface, default impl `DefaultSpreaderConfig` wraps a
`MultifaceBlock` reference and answers `canSpreadInto`/`getStateForPlacement` by delegating to
block methods; `SculkVeinSpreaderConfig` overrides `stateCanBeReplaced`/`isOtherBlockValidAsSource`/
`getSpreadTypes`).
- `SpreadType` enum with 3 strategies computing a `SpreadPos` (target block + face) from a
  starting face + spread direction: `SAME_POSITION` (spread to a different face of the *same*
  block), `SAME_PLANE` (move to the neighbor in the spread direction, same face), `WRAP_AROUND`
  (move diagonally around a corner, face flips to the opposite of the spread direction).
  `DEFAULT_SPREAD_ORDER` tries all three in that order per candidate direction; `SculkVeinBlock`'s
  same-space spreader uses only `SAME_POSITION`.
- `spreadAll`/`spreadFromFaceTowardAllDirections`/`spreadFromRandomFaceTowardRandomDirection`/
  `canSpreadInAnyDirection` — the public entry points used by `SculkVeinBlock`,
  `SculkBehaviour.DEFAULT`, and `GlowLichenBlock` bonemeal.
- `MultifaceSpreadeableBlock` (15-line abstract class) is just the "this multiface block also
  has a `getSpreader()`" marker — needed so `SculkBehaviour.DEFAULT` and other code can grab
  `((MultifaceSpreadeableBlock) Blocks.SCULK_VEIN).getSpreader()` generically.

**`SculkCatalystBlock`** (67 lines) + **`SculkCatalystBlockEntity`** (124 lines):
- Block itself: `PULSE` boolean property (bloom animation flag), scheduled `tick` clears PULSE
  after 8 ticks, `spawnAfterBreak` drops a flat 5 XP (`xpRange = ConstantInt.of(5)`, a constant,
  not a range despite the field name) on break — separate from the mob-kill-charge economy
  below.
- `CatalystListener` (`GameEventListener`, radius 8, `DeliveryMode.BY_DISTANCE`): on
  `GameEvent.ENTITY_DIE` where the dying entity is a `LivingEntity` that hasn't already had its
  XP consumed (`!mob.wasExperienceConsumed()`): computes `experienceWouldDrop` from
  `mob.getExperienceReward(...)`, and if `shouldDropExperience() && experienceWouldDrop > 0`,
  calls `sculkSpreader.addCursors(deathPos, experienceWouldDrop)` (this is literally "the mob's
  XP orbs become sculk charge instead of dropping as orbs") and fires the "It Spreads"
  advancement if the killer was a player. Either way `mob.skipDropExperience()` is called
  (consumes the XP so normal orb drop doesn't also happen), and the catalyst always `bloom()`s
  (sets PULSE, schedules the 8-tick clear, spawns `SCULK_SOUL` particles, plays
  `SCULK_CATALYST_BLOOM`) even if this particular death contributed no charge (e.g. already
  consumed, or 0 XP) — note it blooms once per death event, not once per charge unit.
- `serverTick` simply calls `catalystListener.getSculkSpreader().updateCursors(level, pos,
  random, true)` every block-entity tick — the catalyst is the primary (only non-worldgen)
  driver of `updateCursors`, `spreadVeins=true` always.
- Persistence: `SculkSpreader.load/save` (de)serializes the cursor list (`ChargeCursor.CODEC`:
  pos, charge 0-1000, decay_delay 0-1, update_delay, optional facings set) under `"cursors"`
  inside the block entity's own NBT.

**`SculkSensorBlock`** (295 lines) / **`SculkShriekerBlock`** (145 lines) / their block entities
— vibration detection is **already ported for the sensor only** (see 1.0): confirmed
`pumpkin/src/block/blocks/redstone/sculk_sensor.rs` implements `SculkSensorListener`,
activation/cooldown, and the calibrated-sensor variant. **The shrieker has no block-level
behavior in this codebase at all** — `find pumpkin/src/block/blocks -iname "*sculk*"` returns
only `redstone/sculk_sensor.rs`; there is no `blocks/sculk_shrieker.rs`. `tryShriek`, `stepOn`,
the warden-summon/darkness-radius/warning-level logic in `SculkShriekerBlockEntity` are all
unported and are a separate, independent gap from this design (the block entity shell in
`pumpkin/src/block/entities/sculk_shrieker.rs` only round-trips `warning_level` NBT — no
behavior). This matters for the roadmap: Step 3 below ports `SculkBlock.getRandomGrowthState`,
which places shriekers 1-in-11 times growth succeeds — once Step 3/4 land, shriekers will start
appearing in the world as inert blocks with no shriek/warden behavior until that separate gap is
also closed. File it as its own follow-up; do not fold it into this roadmap's PRs.

### 1.2 Current Rust state (confirmed empty)

- `pumpkin/src/block/entities/sculk_catalyst.rs`: `SculkCatalystBlockEntity { position,
  decay_delay: Mutex<i32> }` — NBT round-trip only, no `SculkSpreader`, no `CatalystListener`, no
  `serverTick`/`BlockEntity::tick` override, no `GameEventListener` registration, no
  `attemptUseCharge`/bloom logic. `decay_delay` here doesn't even match vanilla's field (vanilla
  keeps decay_delay per-cursor inside `ChargeCursor`, not once on the block entity — this field is
  vestigial/wrong-shaped and should be replaced by the cursor list, not reused).
- `sculk_sensor.rs` / `sculk_shrieker.rs`: same shell pattern, `last_vibration_frequency` /
  `warning_level` only — these fields ARE correct and already used by the already-ported
  vibration logic in `pumpkin/src/block/blocks/redstone/sculk_sensor.rs`. No sculk-spread
  changes needed to these two entity files.
- No `pumpkin/src/block/blocks/sculk*.rs` for plain `SculkBlock` or `SculkVeinBlock` behavior at
  all (only the sensor lives under `blocks/redstone/`).
- No `MultifaceBlock`/`MultifaceSpreader` trait or struct anywhere in `pumpkin/src` or
  `pumpkin-data`.
- `pumpkin/src/block/blocks/vine.rs` is confirmed unrelated (own direction-tracking logic, not
  reusable).
- `pumpkin/src/world/game_event/mod.rs` (`GameEventListener`, `PositionSource`,
  `register_game_event_listener`) is the exact hook point `CatalystListener` should be built on —
  this part of the vanilla-parity infrastructure already exists and matches `GameEvent::EntityDie`
  firing from `pumpkin/src/entity/living.rs:on_death` (line ~1570-1576).

### 1.3 Ordered roadmap

Each step should be its own PR: land, verify inert-by-default where applicable, then build the
next step on top.

**Step 1 — `MultifaceBlock` core framework** (foundation; needed by sculk vein now, glow lichen
later if/when it's ported — check `pumpkin/src/block/blocks/` for an existing glow-lichen file
before starting, none was found in this pass but re-verify at implementation time).
- New module, e.g. `pumpkin/src/block/multiface.rs` or `pumpkin-data`-adjacent if face
  properties need generated boilerplate. Needs: a `MultifaceBlock` trait (or a shared struct
  `MultifaceState` helper) providing `has_face`/`available_faces`/`pack`/`unpack` over the 6
  `BlockDirection` face properties, `can_attach_to` (support-shape check — verify what shape
  query helper already exists on `pumpkin_world`/`pumpkin_data` block state, e.g. something like
  `is_face_sturdy`/`get_collision_shape`), `update_shape`/`can_survive` wired through the existing
  `GetStateForNeighborUpdateArgs`/`CanPlaceAtArgs` block trait methods (pattern already used in
  `vine.rs`, though vine does NOT reuse this — build fresh).
- A `MultifaceSpreader`/`SpreadConfig`/`SpreadType` port: `SAME_POSITION`/`SAME_PLANE`/
  `WRAP_AROUND` position math is pure and directly portable; `SpreadConfig` as a Rust trait with
  a default impl mirroring `DefaultSpreaderConfig`.
- Verify against wiki/vanilla source: exact face-property ordinal order (`Direction.values()`
  order in Java: DOWN, UP, NORTH, SOUTH, WEST, EAST) vs whatever order `pumpkin_data`'s
  `BlockDirection` enum uses — the `pack`/`unpack` bitmask and the `numParticles << 6 |
  pack(faces)` level-event data field depend on this matching exactly for correct client-side
  particle rendering.

**Step 2 — `SculkVeinBlock` as the simplest consumer.**
- New `pumpkin/src/block/blocks/sculk_vein.rs` (or similar) using Step 1's framework: block state
  definition with the 6 face properties + waterlogged (check whether `pumpkin_data`'s generated
  block properties for `sculk_vein` already expose these — likely yes, since
  `MultifaceLikeProperties`-style generated structs are the norm here, same pattern as
  `SculkSensorLikeProperties` seen in the existing sensor file).
  - Two spreader configs (`veinSpreader`, `sameSpaceSpreader`) per vanilla; the custom
    `stateCanBeReplaced`/`isOtherBlockValidAsSource` rules from `SculkVeinSpreaderConfig`.
  - Placement/neighbor-update/rotate/mirror plumbed through Step 1's generic multiface block
    methods.
  - This step does NOT yet implement `SculkBehaviour`/`attemptUseCharge`/`onDischarged` (those are
    Step 3) — land it as a block that can be placed/grown via `performBonemeal`-equivalent (if
    glow-lichen-style bonemeal interactions are in scope) or at minimum via direct
    `spreadAll`/`spreadFromRandomFaceTowardRandomDirection` calls, fully inert with respect to the
    catalyst until Step 3/4 land. Confirms the framework end-to-end on the cheapest real
    consumer before touching the catalyst's charge economy.

**Step 3 — `SculkBehaviour` trait + full `SculkBlock` + wire vein's behaviour impl.**
- Define a `SculkBehaviour` trait (default-method-heavy, mirroring the Java interface exactly:
  `sculk_spread_delay() -> u8 { 1 }`, `on_discharged(...)` default no-op,
  `attempt_spread_vein(...)` default (delegate to `SCULK_VEIN`'s spreader, matching the DEFAULT
  singleton's two branches on `facings`), `can_change_block_state_on_spread() -> bool { true }`,
  `update_decay_delay(age) -> i32 { 1 }`, and the mandatory `attempt_use_charge(...)`).
- Implement it for a real `SculkBlock` behavior (new `pumpkin/src/block/blocks/sculk.rs`):
  `attempt_use_charge` per 1.1.3's exact formula (decay-rate roll, no-growth-radius check,
  `canPlaceGrowth` 9x3x9 sensor/shrieker count-cap scan, `growthSpawnCost` roll and subtraction,
  quadratic `getDecayPenalty`), `can_change_block_state_on_spread() = false`.
- Implement `SculkBehaviour` for `SculkVeinBlock` from Step 2 (`attemptUseCharge`,
  `attemptPlaceSculk`, `onDischarged`, `hasSubstrateAccess`) — this is where Step 2's block
  becomes an actual charge-consumer rather than just a placeable multiface block.
- Provide the `SculkBehaviour::DEFAULT` fallback object/const for use by `ChargeCursor::update`
  when the block under a cursor doesn't implement the trait.
- Still fully inert: nothing calls `SculkSpreader::update_cursors` yet, so none of this fires in a
  running server. Should be unit-testable in isolation (charge math, decay penalty formula,
  growth-count-cap scan) without a full world.

**Step 4 — `SculkSpreader` + `ChargeCursor` + wire `SculkCatalystBlockEntity`.**
- Port `SculkSpreader`/`ChargeCursor` as a plain data structure + `update_cursors` method
  (`pumpkin/src/block/entities/sculk_catalyst.rs` or a new shared module if vein/sensor-only
  worldgen spreaders are ever needed later — not in scope now, only `createLevelSpreader`'s
  constants are needed). Cursor list capped at 32, `addCursors` splitting charge into <=1000
  chunks, `is_pos_unreasonable` (chessboard distance > 1024), the merge-by-position logic in
  `update_cursors`, `getValidMovementPos`'s 18-offset non-corner neighbor shuffle +
  `isMovementUnobstructed` face-sturdiness check, and the charge-particle level-event dispatch
  (verify what `level.levelEvent(3006, pos, data)` maps to in this codebase's world/particle API
  — likely `world.spawn_particle`/a raw level-event packet helper; grep for existing level-event
  3006/3007 usage, e.g. the shrieker's `levelEvent(3007, ...)` may already have a Rust
  equivalent worth matching).
- Replace `SculkCatalystBlockEntity`'s current `decay_delay: Mutex<i32>` field with the ported
  cursor list (persisted the same way vanilla nests it under `"cursors"`), and give the block
  entity a real `serverTick`/`BlockEntity::tick` override calling `update_cursors(...,
  spread_veins: true)` every tick, matching the existing tick-driver pattern used elsewhere in
  `pumpkin/src/block/entities/`.
- Build `CatalystListener` as a `GameEventListener` (reusing the exact
  `pumpkin/src/world/game_event` trait already proven out by
  `pumpkin/src/block/blocks/redstone/sculk_sensor.rs`'s `SculkSensorListener`): radius 8,
  `handle_game_event` on `GameEvent::EntityDie`, checking "XP not yet consumed", computing
  `experience_would_drop` from the dying `LivingEntity` (needs whatever Rust equivalent of
  `getExperienceReward`/`shouldDropExperience`/`wasExperienceConsumed`/`skipDropExperience`
  exists on `pumpkin`'s living-entity death path — verify these exist on the current
  `LivingEntity`/mob death flow in `pumpkin/src/entity/living.rs`, since XP-consumption tracking
  wasn't explicitly checked in this pass and may need a small addition itself), calling
  `sculk_spreader.add_cursors(...)`, always triggering `bloom()` (PULSE state + scheduled 8-tick
  clear + `SCULK_SOUL` particles + bloom sound), and firing the "It Spreads"
  (`kill_mob_near_sculk_catalyst`) advancement trigger if a player criteria-trigger system exists
  for it (check `pumpkin`'s advancement/criteria infrastructure — may not be wired up at all,
  in which case skip that sub-piece and file it separately rather than blocking this PR on
  advancements).
- Register/unregister this listener on `placed`/`on_state_replaced` exactly like
  `SculkSensorListener` already does.
- **This is the step where the whole system goes live.** Land it last, and only after Steps 1-3
  are independently verified, since this is what actually starts consuming mob-death XP and
  growing the world.

### 1.4 Files to create/modify (Sculk)

- New: `pumpkin/src/block/multiface.rs` (or equivalent path) — Step 1.
- New: `pumpkin/src/block/blocks/sculk_vein.rs` — Step 2, behavior added in Step 3.
- New: `pumpkin/src/block/blocks/sculk.rs` (plain `SculkBlock`) — Step 3.
- New (or extend existing shell): sculk behaviour trait module, e.g.
  `pumpkin/src/block/sculk_behaviour.rs` — Step 3.
- Modify: `pumpkin/src/block/entities/sculk_catalyst.rs` — Step 4 (replace `decay_delay` field
  with cursor list, add tick + listener).
- No changes expected to `sculk_sensor.rs`/`sculk_shrieker.rs` block-entity files or
  `pumpkin/src/block/blocks/redstone/sculk_sensor.rs` (already correct for their scope) — only
  reference their `GameEventListener` pattern as a template.
- Verify at implementation time whether `pumpkin-data`'s generated block-properties module
  already has multiface-shaped structs for `sculk_vein` (6 face bools + waterlogged) — if so,
  reuse rather than hand-roll.

### 1.5 Needs wiki/vanilla-source verification

- Exact `BlockDirection` ordinal order in `pumpkin_data` vs Java `Direction.values()` order,
  for the `MultifaceBlock.pack`/`unpack` bitmask parity (affects only particle data byte, not
  gameplay correctness, but flagged per the "verification discipline" rule).
- Whether `pumpkin`'s current mob-death path already tracks "XP already consumed" per vanilla's
  `wasExperienceConsumed`/`skipDropExperience` — if it doesn't, Step 4 needs a small addition to
  `entity/living.rs` first (out of the read-only research done in this pass, flag rather than
  assume).
- Whether an advancement/criteria-trigger system exists in this codebase at all, for the "It
  Spreads" trigger in Step 4 — not confirmed in this pass.

---

## 2. ChestBlock cat-sitting-blocks-open

Read directly from `ChestBlock.java` (lines 332-352):
```java
public static boolean isChestBlockedAt(LevelAccessor level, BlockPos pos) {
   return isBlockedChestByBlock(level, pos) || isCatSittingOnChest(level, pos);
}
private static boolean isBlockedChestByBlock(BlockGetter level, BlockPos pos) {
   BlockPos above = pos.above();
   return level.getBlockState(above).isRedstoneConductor(level, above);
}
private static boolean isCatSittingOnChest(LevelAccessor level, BlockPos pos) {
   List<Cat> cats = level.getEntitiesOfClass(Cat.class,
      new AABB(pos.getX(), pos.getY() + 1, pos.getZ(), pos.getX() + 1, pos.getY() + 2, pos.getZ() + 1));
   for (Cat cat : cats) {
      if (cat.isInSittingPose()) return true;
   }
   return false;
}
```
This is simpler than the task brief's census framing assumed: **there is no per-block "cat is
sitting on this specific block" position tracker at all** — vanilla just tests any `Cat` entity
inside the 1x1x1 AABB directly above the chest for its generic `isInSittingPose()` render-pose
flag. It is not gated on the cat's target/owner/goal being chest-specific in any way; any sitting
cat standing on top of any chest blocks it. So the fix does **not** depend on a `CatSitOnBlockGoal`
position-tracking design at all — that goal doesn't exist in vanilla's own chest-blocking check,
and any such doc is irrelevant to this fix.

What it *does* depend on: whether "is in sitting pose" is queryable at all in this codebase.
Checked `pumpkin/src/entity/passive/cat.rs` and `pumpkin/src/entity/ai/goal/sit.rs`: there is a
`SitGoal` (`SitWhenOrderedToGoal` port) exposing `mob_entity.is_ordered_to_sit()` — the AI
*intent* to sit — but `sit.rs`'s own doc comment states explicitly: *"Currently does not sync
sitting pose to clients (`TAMEABLE_FLAGS` is not actively tracked in Pumpkin for 26.1/26.2). The
mob will be stationary but render standing rather than in a sitting pose."* Vanilla's
`isInSittingPose()` reads a synced `TAMEABLE_FLAGS` data-tracker bit set once the sit animation
has actually taken hold (distinct from "ordered to sit," though in practice they track closely
once a stationary cat settles) — this bit does not exist in Pumpkin yet.

Current Rust: `pumpkin/src/block/blocks/chests.rs`, `fn is_chest_blocked` (line ~663):
```rust
fn is_chest_blocked(world: &World, block_pos: &BlockPos) -> bool {
    // TODO: Block opening when a cat is sitting on top.
    has_block_on_top(world, block_pos)
}
```
Only the solid-block-above case is implemented; the cat case is an explicit TODO no-op.
(Note: vanilla's `isBlockedChestByBlock` is a redstone-conductor check on the block above, not a
generic "is solid" check — verify `has_block_on_top`'s `is_solid_block()` is an exact match for
`isRedstoneConductor` or adjust it as part of this fix; it may currently be slightly
over/under-inclusive.)

**Target design:** add a `TAMEABLE_FLAGS`-equivalent synced sitting-pose bit to the tameable
mob base (or, as a smaller interim step, add a standalone `is_in_sitting_pose()` accessor on
`Cat` derived from `is_ordered_to_sit()` plus the existing on-ground/stationary checks already in
`SitGoal::can_start`, explicitly documented as an approximation until real pose sync lands).
Then `is_chest_blocked` scans a 1x1x1 AABB at `block_pos.up()` for any `Cat` entity and ORs
`cat.is_in_sitting_pose()` across matches with the existing `has_block_on_top` check. This is the
smallest fix in this batch and is **not** blocked on any other design doc — only on deciding
whether to land the real pose-sync bit now or take the `is_ordered_to_sit()` approximation as a
stopgap.

**Files to modify:** `pumpkin/src/block/blocks/chests.rs` (`is_chest_blocked`),
`pumpkin/src/entity/passive/cat.rs` (add `is_in_sitting_pose` accessor), and — if doing the full
fix rather than the stopgap — wherever this codebase's tameable-mob synced-data-tracker
infrastructure lives (not located in this pass; grep for `TAMEABLE_FLAGS` or existing
`DataTracker`/`EntityDataAccessor`-equivalent usage on tameable mobs before starting).

---

## 3. ConduitBlockEntity: unimplemented server tick

Vanilla: `ConduitBlockEntity.java` (266 lines), `ConduitBlock.java` (98 lines).

### 3.1 Vanilla model
- `serverTick` (lines 92-121) runs every tick, but the expensive work (`updateShape`,
  `applyEffects`, `updateAndAttackTarget`) only every 40 ticks (`gameTime % 40L == 0L`):
  1. `updateShape` (lines 127-162): the frame-detection scan. First, a strict 3x3x3 (`-1..=1`
     each axis) check that **every** block around the conduit is water (`level.isWaterAt`) — if
     any isn't, immediately not active (frame doesn't even get scanned). Then scans the "shell"
     of a 5x5x5 cube (`-2..=2`, but only positions where at least one axis has `abs > 1`, filtered
     to the specific plane-membership condition at lines 147 — i.e. only the 42 "frame" positions
     forming 6 face-plates of the shell, not the full 5x5x5 minus 3x3x3 volume) for
     PRISMARINE/PRISMARINE_BRICKS/SEA_LANTERN/DARK_PRISMARINE blocks, collecting matches into
     `effectBlocks`. Active iff `effectBlocks.size() >= 16` (`MIN_ACTIVE_SIZE`).
  2. Active-state transition plays `CONDUIT_ACTIVATE`/`CONDUIT_DEACTIVATE`.
  3. `updateHunting`: `isHunting = effectBlocks.size() >= 42` (`MIN_KILL_SIZE`, full frame).
  4. If active: `applyEffects` (lines 164-179) — `effectRange = (activeSize/7)*16` (so 16 blocks
     of frame gives 16 blocks radius, scaling with more frame blocks up to the 42-block full
     frame), builds an AABB centered on the conduit expanded by `effectRange` and the full world
     height, finds all `Player`s in it, and for any within `effectRange` (Chebyshev,
     `closerThan`) **and currently in water or rain** (`isInWaterOrRain`), applies
     `CONDUIT_POWER` for 260 ticks (13s * 20, refreshed every 40-tick cycle so it never expires
     while conditions hold), amplifier 0, ambient=true, showParticles=true.
  5. `updateAndAttackTarget` (lines 181-217), only meaningful when `isHunting`:
     `updateDestroyTarget` clears the target if not hunting; if hunting and no target, calls
     `selectNewTarget` — picks a random `LivingEntity` that `instanceof Enemy && isInWaterOrRain()`
     within an 8-block-inflated AABB (`getDestroyRangeAABB`); if a target exists, keeps it only
     while alive and within 8 blocks (`closerThan`), else drops it. If (after update) a target
     exists, plays `CONDUIT_ATTACK_TARGET` sound at the target and deals 4.0 magic damage
     (`hurtServer(damageSources().magic(), 4.0F)`) — this happens every 40-tick cycle while a
     target persists, i.e. repeated damage over time, not once.
  6. Ambient sounds: `CONDUIT_AMBIENT` every 80 ticks while active; `CONDUIT_AMBIENT_SHORT` on an
     irregular schedule (`nextAmbientSoundActivation = gameTime + 60 + random.nextInt(40)`).
- `clientTick` mirrors the shape/hunting bookkeeping (no player effects/attacks — client has no
  authority) purely to drive rotation (`activeRotation++` while active, used by
  `getActiveRotation(partialTick) = (activeRotation + partialTick) * -0.0375F`) and particle
  animation (`animationTick`, nautilus particles orbiting the frame blocks and streaming toward
  a destroy target) — **this client-only piece is out of scope for a server implementation**
  beyond tracking `isActive`/`getActiveRotation` state that a client could read from an update
  packet, if this codebase's client-visual-state sync conventions require it (check how other
  block entities like beacon expose similar "is active" visual state over the network).
- Persistence: only `destroyTarget` (`EntityReference`) is saved/loaded (`"Target"` key) —
  `effectBlocks`/`isActive`/`isHunting`/`tickCount`/`activeRotation` are all transient,
  recomputed every 40 ticks.

### 3.2 Current Rust state
`pumpkin/src/block/entities/conduit.rs`: `ConduitBlockEntity { position, active: Mutex<bool>,
target: Mutex<Option<NbtTag>> }` — has the right *shape* of persisted fields (active, target)
but `active` is never set by anything, and `target` is stored as a raw opaque `NbtTag` rather
than a typed `EntityReference`-equivalent. No `updateShape`, no `applyEffects`, no
`updateAndAttackTarget`, no tick override — `BlockEntity::tick` default no-op is inherited.
`pumpkin/src/block/blocks/conduit.rs`: `on_place` sets waterlogged from `replacing.water_source()`
and `placed` registers the block entity — no `getTicker`/scheduled behavior at all. Confirmed:
entirely unimplemented as stated in this task's brief.

### 3.3 Target Rust design
- Add periodic tick logic. Check this codebase's existing pattern for "every N ticks, else no-op"
  block-entity behavior (the sculk sensor/shrieker files use `world.schedule_block_tick` for
  discrete delayed single-shot ticks; conduit instead needs a *recurring* `% 40 == 0` check
  against `world`'s game time, similar to how vanilla does it inline in `serverTick` — check
  whether other ported block entities in `pumpkin/src/block/entities/` already have a
  `BlockEntity::tick(world)` override doing a modulo-game-time gate, e.g. beacon or similar, and
  follow that convention rather than inventing a new one).
- `ConduitBlockEntity` needs new transient fields: `effect_blocks: Mutex<Vec<BlockPos>>` (or
  plain `Mutex<Vec<_>>` recomputed each 40-tick cycle, not persisted), `is_hunting: Mutex<bool>`,
  `tick_count`, `active_rotation` (f32, for eventual client sync), `next_ambient_sound_activation:
  Mutex<i64>` (game-time tick, not persisted — recomputed with a fresh random offset on next
  activation, matches vanilla not persisting it either).
- `target: Mutex<Option<NbtTag>>` should become a typed entity reference (check what
  `EntityReference`-equivalent this codebase uses elsewhere — likely a UUID + cached
  `Weak<dyn EntityBase>` or similar; grep other block entities that reference living entities,
  e.g. beacon/lodestone tracking, for the established pattern) rather than a raw NBT tag, so
  `update_and_attack_target` can actually look up and damage the entity each cycle.
- `update_shape`: needs a `world.is_water_at`/equivalent full-block-water check (verify this
  helper exists — a grep in this pass for `is_water_at` on `World` found nothing, may need to add
  it or use `get_fluid_state(pos).is_still_water()`-equivalent) for the strict 3x3x3, then the
  42-position frame shell scan against the 4 valid blocks.
- `apply_effects`: needs the `CONDUIT_POWER` effect (confirmed present:
  `pumpkin_data::effect::Effect::CONDUIT_POWER`) applied via the existing
  `LivingEntity::add_effect`/player equivalent — check whether `add_effect` takes
  ambient/particles-visible flags matching vanilla's `MobEffectInstance(effect, duration,
  amplifier, ambient, showParticles)` 5-arg constructor, or needs a small signature extension.
  Needs an "is in water or rain" query on players — check if this predicate already exists
  (likely used by other water-related mechanics, e.g. dolphin's grace period or similar).
- `update_and_attack_target`: needs an "is Enemy" classification (vanilla's `Enemy` marker
  interface — check how hostile-mob classification is done in this codebase, e.g. an
  `EntityType`/tag-based check) and a magic damage source + `hurt`-equivalent call.
- `get_ticker`/block-side wiring: `pumpkin/src/block/blocks/conduit.rs` needs a `get_ticker`
  (or this codebase's equivalent trait hook) pointing at the new server-tick function, following
  the same registration pattern as beehive/sculk-catalyst (`createTickerHelper` equivalent).

### 3.4 Files to create/modify
- Modify: `pumpkin/src/block/entities/conduit.rs` (new fields, tick logic, typed target).
- Modify: `pumpkin/src/block/blocks/conduit.rs` (wire tick/get_ticker hook).
- Possibly modify: `pumpkin/src/world/mod.rs` if a water-check helper needs adding.
- Possibly modify: `pumpkin/src/entity/living.rs` if `add_effect` needs an ambient/particles
  parameter extension, or the "in water or rain" predicate needs adding.

### 3.5 Needs verification
- Exact 42-position frame-shell filter condition (line 147's compound boolean) — re-derive
  carefully from source at implementation time rather than approximating; getting this wrong
  changes the activation threshold silently.
- Whether this codebase's `add_effect` signature already supports ambient/no-particles-hidden
  flags, or needs extension.
- Whether an `EntityReference`-style typed-and-persisted entity pointer pattern already exists to
  copy, or needs inventing for conduit specifically.

---

## 4. Beehive full bee lifecycle

Vanilla: `BeehiveBlock.java` (364 lines), `BeehiveBlockEntity.java` (434 lines).

### 4.1 Vanilla model
- **Storage**: `stored: List<BeeData>` (max 3, `MAX_OCCUPANTS`), each wrapping an `Occupant`
  record (`entityData: TypedEntityData`, `ticksInHive`, `minTicksInHive` — 2400 if the bee had
  nectar when it entered, else 600 `MIN_OCCUPATION_TICKS_NECTARLESS`) plus a shared
  `savedFlowerPos` for the whole hive.
- **`addOccupant(bee)`** (lines 167-186): if under capacity, stops bee riding/ejects
  passengers/drops leash, snapshots it via `Occupant.of(bee)` (serializes to NBT, strips a long
  list of `IGNORED_BEE_TAGS` — position/rotation/AI-transient fields — records
  `hasNectar`-derived `minTicksInHive`), updates the hive's shared `savedFlowerPos` (bee's own
  flower pos if the bee has one and either the hive doesn't yet or a coin-flip), plays
  `BEEHIVE_ENTER`, fires `GameEvent.BLOCK_CHANGE`, then discards the live entity.
- **`tickOccupants`/`serverTick`** (lines 260-292): every stored bee's `ticksInHive` increments;
  once it exceeds `minTicksInHive`, attempt release via `releaseOccupant` with status
  `HONEY_DELIVERED` (if `hasNectar()`, checked from the snapshotted NBT's `HasNectar` boolean) or
  `BEE_RELEASED` otherwise; on success remove from `stored`. Additionally, 0.5% chance per tick
  (`random.nextDouble() < 0.005`) to play `BEEHIVE_WORK` ambient sound while non-empty.
- **`releaseOccupant`** (lines 192-254, static): refuses to release (returns false, bee stays
  queued) if `EnvironmentAttributes.BEES_STAY_IN_HIVE` is set for this position AND status isn't
  `EMERGENCY`; refuses if the hive's front (`FACING` direction) is blocked by a non-empty
  collision shape AND status isn't `EMERGENCY` (emergency releases always succeed regardless of
  blockage). Reconstructs the entity from NBT (stripping the same ignored tags), restores
  `savedFlowerPos` onto the bee (90% chance, only if the bee doesn't already have one) if the
  hive has one, and if status is `HONEY_DELIVERED`: calls `bee.dropOffNectar()` and — only if the
  current block state actually has a `HONEY_LEVEL` property (i.e. still a beehive/bee_nest, not
  something else post-break) — increments honey level by 1 (or 2 with 1% chance,
  `random.nextInt(100)==0`), clamped so it never exceeds 5 (decrements the roll-of-2 back to 1 if
  it would overflow). Positions the released bee just outside the front face (or at the hive
  center if the front is blocked, e.g. emergency release through a wall) and plays
  `BEEHIVE_EXIT` + `GameEvent.BLOCK_CHANGE`.
- **`emptyAllLivingFromHive`** (lines 125-138): calls `releaseAllOccupants` (loops all stored
  bees through `releaseOccupant` with the given reason, removing successes), then — if a
  `player` triggered this — for every released bee within 4 blocks (`distanceToSqr <= 16.0`) of
  that player: if the hive is **not** currently sedated (`isSedated()` = campfire smoke check via
  `CampfireBlock.isSmokeyPos`), sets the bee's attack target to the player (angry release);
  otherwise sets `stayOutOfHiveCountdown = 400` on the bee instead (sedated bees don't get angry,
  they just wander off for a while). This is the "angry bee release" behavior.
- **`isFireNearby`** (lines 103-115): scans the 3x3x3 around the hive for any `FireBlock`
  instance; `setChanged` override (lines 94-101) calls `emptyAllLivingFromHive(null, ...,
  EMERGENCY)` whenever the block entity is marked dirty while fire is nearby — i.e. this is
  checked opportunistically on every NBT-dirty event, not on a fixed schedule, and also
  explicitly on `updateShape` when a neighbor placement is a `FireBlock` (`BeehiveBlock.
  updateShape`, lines 338-353).
- **`BeehiveBlock.playerDestroy`** (lines 90-109): on break (unless the tool has
  `PREVENTS_BEE_SPAWNS_WHEN_MINING`), empties the hive with `EMERGENCY`, updates neighbors, and
  `angerNearbyBees` (lines 119-135) — separately finds all `Bee`s within an 8x6x8 box around the
  break position and, if any players are also in that box, assigns each targetless bee a random
  player from that set as its target (this is a *second*, broader anger mechanism than the
  proximity check in `emptyAllLivingFromHive`, triggered specifically by block breakage/explosion,
  not just by bee release).
- **`onExplosionHit`** also calls `angerNearbyBees`.
- **`useItemOn`** (lines 150-203): only acts if `HONEY_LEVEL >= 5`. Shears -> `dropHoneycomb`
  (loot table `HARVEST_BEEHIVE`), play `BEEHIVE_SHEAR`, damage the shears by 1. Glass bottle ->
  shrink stack by 1, give/drop a `HONEY_BOTTLE`, play `BOTTLE_FILL`. Either way (`hiveEmptied`):
  if **not** currently smoked (`!CampfireBlock.isSmokeyPos`), and the hive currently contains
  bees, `angerNearbyBees`, then `releaseBeesAndResetHoneyLevel` with `EMERGENCY` (so
  harvesting honey/honeycomb without a lit campfire below always kicks out and angers the bees);
  if smoked, only `resetHoneyLevel` (bees stay put, calm) — this is the campfire-smoking
  suppression of angry release.
- `animateTick`/`trySpawnDripParticles` (client-visual honey-dripping at `HONEY_LEVEL>=5`) —
  cosmetic, lower priority, include only if this codebase already does equivalent particle-tick
  work elsewhere for other blocks (check convention, e.g. does `animate_tick` exist as a trait
  hook already).
- `getDrops`/`getCloneItemStack`/`playerWillDestroy` (silk-touch-with-bees item-component
  handling, TNT/creeper/wither/minecart-TNT-triggered emergency empty before drops compute) —
  needed for full parity of "break with silk touch preserves bee state in the item", check
  whether this codebase's item/loot component system (`DataComponents.BEES`) has an equivalent
  already (the `Bees`/`Occupant` component pattern) — if not, this is its own sub-piece.

### 4.2 Current Rust state
- `pumpkin/src/block/blocks/beehive.rs`: only `get_comparator_output` reading `HONEY_LEVEL` —
  everything else (placement, use-item, playerDestroy, updateShape/fire-check, angerNearbyBees)
  is unimplemented.
- `pumpkin/src/block/entities/beehive.rs`: `BeehiveBlockEntity { position, bees:
  Mutex<Option<Vec<NbtTag>>>, flower_pos: Mutex<Option<BlockPos>> }` — NBT round-trip shell only
  (raw `NbtTag` list, no typed `Occupant`/`BeeData`, no `ticksInHive` tracking separate from
  whatever's embedded in the raw tag, no tick logic at all — `BlockEntity::tick` default inherited
  no-op). Confirmed: entirely unimplemented server tick/lifecycle as stated in this task's brief.
- `pumpkin/src/entity/passive/bee.rs` is confirmed to already carry most of the per-bee state
  vanilla's `Occupant`/`releaseOccupant` needs: `flower_pos: AtomicCell<Option<BlockPos>>`
  (vanilla's `savedFlowerPos`), `has_nectar()`, `drop_off_nectar()`, and
  `set_stay_out_of_hive_countdown(ticks)` all exist with matching NBT round-trip
  (`"flower_pos"`/`"HasNectar"` keys). This means Step 4.3's `release_occupant`/
  `empty_all_living_from_hive` port has real methods to call on the bee side already — the gap is
  entirely in the beehive block entity's storage/lifecycle, not in the bee entity itself.

### 4.3 Target Rust design
- Replace `bees: Mutex<Option<Vec<NbtTag>>>` with a typed `Vec<BeeOccupant>` struct mirroring
  vanilla's `Occupant` (serialized entity NBT/component data + `ticks_in_hive` +
  `min_ticks_in_hive`), stored via whatever this codebase's `TypedEntityData`-equivalent is for
  entity-in-item/block-entity serialization (check `pumpkin_world`/`pumpkin` for an existing
  pattern used by, e.g., shulker box or bucket-of-fish-style "entity stored as data" components,
  since `DataComponents.BEES` in vanilla is exactly this shape).
- Add `BlockEntity::tick(world)` override on `BeehiveBlockEntity` implementing
  `tick_occupants`/`serverTick`: increment each occupant's `ticks_in_hive`, release via a ported
  `release_occupant` once past `min_ticks_in_hive`, plus the 0.5%-chance `BEEHIVE_WORK` sound.
- Port `is_fire_nearby` (3x3x3 scan for fire block) and call `empty_all_living_from_hive(...,
  Emergency)` from it — wire into whatever this codebase's "block entity marked dirty" hook is
  (`set_changed`-equivalent, if one exists) AND from the beehive block's `on_neighbor_update`
  (or equivalent trait hook) when the changed neighbor is fire, matching vanilla's two call
  sites.
- Port `add_occupant`, `release_occupant` (env-attribute stay-in-hive check — verify whether this
  codebase has an `EnvironmentAttributes`-equivalent system at all, else treat as "always allowed
  to release, gate only on front-blocked/emergency" and file the missing env-attribute as a
  separate gap), `empty_all_living_from_hive` (with the proximity-based angry-vs-sedated branch),
  and `BeehiveBlock`'s `anger_nearby_bees` (8x6x8 AABB scan, separate from the proximity check).
- Wire `playerDestroy`-equivalent (`broken`/`player_will_destroy` trait hooks — check existing
  hook names in `pumpkin/src/block/mod.rs`, likely `BrokenArgs`/similar already seen in
  `chests.rs`) to call `emptyAllLivingFromHive(EMERGENCY)` + `angerNearbyBees`, respecting the
  `PREVENTS_BEE_SPAWNS_WHEN_MINING` enchantment tag check (verify this tag/enchantment-helper
  equivalent exists in this codebase's enchantment system).
- Wire `use_with_item`/`normal_use`-equivalent for shears (drop honeycomb via loot table, damage
  tool) and glass bottle (-> honey bottle), gated on `HONEY_LEVEL >= 5`, followed by the
  campfire-smoke-gated angry-release-vs-calm-reset branch (`isSedated`/`CampfireBlock.isSmokeyPos`
  — check whether this codebase already has a smokey-position helper for campfires, likely yes
  given campfire/hopper suppression is called out elsewhere in the parity campaign per
  `PARITY.md`).
- `isSedated`/campfire-smoke suppression is shared logic needed by both the tick-based release
  path and the shear/bottle-harvest path — implement once, reuse.

### 4.4 Files to create/modify
- Modify: `pumpkin/src/block/entities/beehive.rs` (typed occupant storage, tick logic, all the
  release/anger/fire helper methods).
- Modify: `pumpkin/src/block/blocks/beehive.rs` (placement/FACING, use-item shears/bottle logic,
  playerDestroy/broken hook, updateShape fire-neighbor hook, get_ticker wiring,
  getDrops TNT/creeper/wither-triggered emergency-empty, getCloneItemStack/playerWillDestroy
  silk-touch bee-preservation if the component system supports it).
- Possibly modify: `pumpkin/src/entity/passive/bee.rs` if any of `hasSavedFlowerPos`/
  `setSavedFlowerPos`/`dropOffNectar`/`setStayOutOfHiveCountdown`/`HasNectar` tracking is missing
  (not fully verified in this pass — check before starting implementation).

### 4.5 Needs verification
- Whether `pumpkin`'s enchantment-helper/tag system already has an equivalent of
  `EnchantmentTags.PREVENTS_BEE_SPAWNS_WHEN_MINING` and `EnchantmentHelper.hasTag`.
- Whether `pumpkin`'s campfire implementation already exposes an `isSmokeyPos`-equivalent (the
  task brief calls out "campfire-smoking suppression of angry release" as needing to plug into
  something — check `pumpkin/src/block/blocks/` for an existing campfire smoke-detection helper
  before writing a new one).
- Whether an `EnvironmentAttributes`/`BEES_STAY_IN_HIVE`-equivalent gamerule/attribute exists, or
  whether that check should simply be omitted (document as a known small gap) if this codebase
  has no such system at all.
- Exact silk-touch/`DataComponents.BEES` item-component preservation behavior — depends on
  whether this codebase's item component system supports storing entity snapshots on an
  `ItemStack`s at all (check `pumpkin-inventory`/`pumpkin` item component modules).

---

## Summary of scope ordering across all four

1. **Sculk** — by far the largest (four ordered PRs: multiface framework -> vein consumer ->
   behaviour trait + full sculk block -> catalyst wiring). Do not skip ahead to Step 4 without
   Steps 1-3 landed and independently verified, since Step 4 is what makes the whole system
   start consuming real mob-death XP in a live server.
2. **Beehive** — second largest, but flat (no strict ordering dependency chain the way sculk
   has) — typed occupant storage + tick loop is the core; release/anger/harvest logic can be
   layered incrementally afterward inside a single PR or split by sub-behavior if preferred.
3. **Conduit** — medium, self-contained, mostly mechanical (frame scan, effect application,
   target selection/attack) with a couple of small infra questions (typed entity reference,
   effect-instance ambient flag, water-at helper) to resolve first.
4. **Chest cat-blocking** — smallest, and (contrary to the initial framing in this task's brief)
   **not** blocked on any Cat AI position-tracking design — vanilla's own check is a simple
   AABB-above-the-chest + sitting-pose test, no per-block state involved. It only needs a sitting-
   pose accessor on `Cat` (either a real synced `TAMEABLE_FLAGS`-equivalent bit, or a documented
   `is_ordered_to_sit()`-based stopgap) and is startable independently of everything else in this
   batch.
