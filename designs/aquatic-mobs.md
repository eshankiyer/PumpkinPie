# Aquatic Mobs Parity Design

Scope: Turtle, Dolphin, Squid/GlowSquid, and the schooling fish (Cod/Salmon/TropicalFish).
All seven Rust files are near-empty stubs (52-62 lines) wiring only generic AI goals
(`SwimGoal`, `WanderAroundGoal`, `LookAtEntityGoal`, `RandomLookAroundGoal`,
`EscapeDangerGoal`, `AvoidEntityGoal`, `FollowFlockLeaderGoal`, `DolphinJumpGoal`). None
have synced data, NBT persistence, or species-specific behavior beyond a couple of sound
overrides already present in `Mob`. Vanilla source is decompiled 26.2 client/server jar at
`/tmp/pumpkin-vanilla-26.2/decompiled/net/minecraft/world/entity/animal/`.

Conventions used throughout (confirmed by reading existing near-complete mobs):

- **Synced data**: `pumpkin-data::tracked_data::TrackedData` is codegen'd from the game's
  own registry dump and already contains every accessor id these species need —
  `HAS_EGG`, `LAYING_EGG`, `MOISTNESS_LEVEL`, `GOT_FISH`, `DARK_TICKS_REMAINING`,
  `ID_TYPE_VARIANT` (tropical fish), `VARIANT` (salmon) — see
  `pumpkin-data/src/generated/tracked_data.rs` lines 927, 1263, 2091, 1251, 2331, 1731,
  3819. **No pumpkin-data/codegen changes are needed**; only entity-side wiring
  (`entity.send_meta_data(&[Metadata::new(TrackedData::X, MetaDataType::Y, value)], None)`),
  following `pumpkin/src/entity/mob/shulker.rs` (`set_attach_face`/`set_raw_peek`) and
  `pumpkin/src/entity/passive/pufferfish.rs` (`send_puff_state`).
- **State fields**: plain atomics (`AtomicU8`/`AtomicI32`/`AtomicCell<Option<BlockPos>>`)
  on the entity struct, not behind a mutex — see `ShulkerEntity`, `PufferfishEntity`,
  `BeeEntity` (`hive_pos: AtomicCell<Option<BlockPos>>`).
- **NBT**: implement `NBTStorage::write_nbt`/`read_nbt_non_mut` (not the whole trait —
  defaults are no-ops), call `self.mob_entity.living_entity.write_nbt(nbt).await` /
  `read_nbt_non_mut` first, then read/write extra keys. `BlockPos` has no built-in codec;
  the Bee pattern (`pumpkin/src/entity/passive/bee.rs:542-551`,
  `block_pos_to_nbt`/`block_pos_from_nbt` using an int array) is the template for
  `Turtle::home_pos`.
- **Goals**: implement `crate::entity::ai::goal::Goal` (`can_start`, `should_continue`,
  `start`, `stop`, `tick`, `should_run_every_tick`, `controls`). For vanilla's
  `MoveToBlockGoal` subclasses (`TurtleGoToWaterGoal`, `TurtleLayEggGoal`), reuse the
  already-ported equivalent `MoveToTargetPosGoal<M: MoveToTargetPos>` in
  `pumpkin/src/entity/ai/goal/move_to_target_pos.rs`, implementing the `MoveToTargetPos`
  trait per species instead of hand-rolling a new goal type.
- **Bucket capture**: generic infrastructure already exists in
  `pumpkin/src/item/items/bucket.rs` — `mob_bucket_entity_type`/`bucket_item_for_entity_type`
  already map Cod/Salmon/TropicalFish/Pufferfish/Axolotl/Tadpole to their bucket items, and
  `FilledBucketItem`/`EmptyBucketItem` already spawn/despawn the entity. The **known gap**
  (called out in a code comment at `bucket.rs:208-213`) is that the bucket's
  `bucket_entity_data` component is an opaque marker, so species-specific NBT (Salmon
  size, TropicalFish pattern/colors) is **not** currently round-tripped through a bucket.
  Fixing that is a `pumpkin_data::item_stack::ItemStack` / bucket-component change outside
  this document's per-entity scope, but it must be listed as a dependency for
  "correct" Salmon/TropicalFish bucket parity (see the Fish section).
- **Leashing**: there is currently no `can_be_leashed`-style gate anywhere in the
  codebase. `Entity::leash_to` (`pumpkin/src/entity/mod.rs:3105`) is called
  unconditionally by `LeadItem::use_on_entity` (`pumpkin/src/item/items/lead.rs`) once a
  target is found; nothing on `EntityBase`/`Mob` is consulted first. `Mob` does have
  unrelated `should_follow_leash`/`on_short_leash_tick`/`before_leash_tick` hooks
  (`pumpkin/src/entity/mob/mod.rs:998-1010`) that gate *behavior while already leashed*,
  not whether a lead can attach at all. This is a **shared infrastructure gap**, not
  turtle-specific — Turtle is simply the first mob that needs the `false` case.

---

## 1. Turtle

### Vanilla source
`Turtle.java` (650 lines). Key pieces:

- Synced data: `HAS_EGG` (bool, line 67), `LAYING_EGG` (bool, line 68).
- Fields: `layEggCounter` (int, line 75), `homePos` (`BlockPos`, default `BlockPos.ZERO`,
  line 77), `travelPos` (`@Nullable BlockPos`, line 78), `goingHome` (bool, line 79).
- Constructor (lines 81-88): sets pathfinding malus `WATER -> 0.0`,
  `DOOR_IRON_CLOSED/DOOR_WOOD_CLOSED/DOOR_OPEN -> -1.0` (doors are impassable to turtles),
  and a custom `TurtleMoveControl`.
- `setHomePos`/`getHomePos` (90-96), `hasEgg`/`setHasEgg` (98-104),
  `isLayingEgg`/`setLayingEgg` (106-113: `setLayingEgg(true)` resets `layEggCounter = 1`).
- `defineSynchedData` (116-120), `addAdditionalSaveData`/`readAdditionalSaveData`
  (123-134: `home_pos` via `BlockPos.CODEC`, `has_egg` bool — note `LAYING_EGG` and
  `layEggCounter` are **not** persisted, matching vanilla exactly).
- `finalizeSpawn` (137-142): `setHomePos(blockPosition())`.
- `checkTurtleSpawnRules` (144-148): spawn Y < sea level + 4, on sand via
  `TurtleEggBlock.onSand`, bright enough.
- `registerGoals` (151-161), priorities: 0 `TurtlePanicGoal(1.2)`, 1 `TurtleBreedGoal(1.0)`,
  1 `TurtleLayEggGoal(1.0)`, 2 `TemptGoal(1.1, ItemTags.TURTLE_FOOD)`,
  3 `TurtleGoToWaterGoal(1.0)`, 4 `TurtleGoHomeGoal(1.0)`, 7 `TurtleTravelGoal(1.0)`,
  8 `LookAtPlayerGoal(8.0)`, 9 `TurtleRandomStrollGoal(1.0, 100)`.
- `createAttributes` (163-165): MAX_HEALTH 30, MOVEMENT_SPEED 0.25, STEP_HEIGHT 1.0.
- `isPushedByFluid` -> `false` (167-170) — turtles are NOT swept by currents.
- Sounds: `getAmbientSound` (177-180, land-only when not in water/baby),
  `playSwimSound`(*1.5 volume, 183-185), `getSwimSound` = `TURTLE_SWIM` (187-190),
  `getHurtSound`/`getDeathSound` baby vs adult variants (192-200),
  `playStepSound` = `TURTLE_SHAMBLE`/`TURTLE_SHAMBLE_BABY` at volume 0.15 (202-206).
- `canFallInLove` (208-211): false while `hasEgg()`.
- `getAgeScale` (218-221): 0.3 baby / 1.0 adult.
- `createNavigation` -> `TurtlePathNavigation` (223-226, an `AmphibiousPathNavigation`
  subclass whose `isStableDestination` treats water as stable once `travelPos != null`).
- `getBreedOffspring`, `isFood` (`ItemTags.TURTLE_FOOD`).
- `getWalkTargetValue` (238-245): water = 10.0 unless `goingHome`; sand = 10.0; else
  light-based cost.
- `aiStep` (247-257): while laying an egg, every 5th `layEggCounter` tick (starting at 1)
  fires level event 2001 (block-break particle) at the turtle's feet if standing on sand.
- `ageBoundaryReached` (259-265): on baby->adult transition, drops `TURTLE_GROW` loot
  table via gift-loot (only if adult now and `MOB_DROPS` gamerule).
- `travelInWater` (267-275): custom swim physics — `moveRelative(0.1, input)`, then damp
  velocity *0.9, and sink slightly (`-0.005` y) unless it has a target or (`goingHome` and
  within 20 blocks of home).
- `canBeLeashed` -> **`false`** (277-280).
- `thunderHit` (282-285): instant-kill via `hurtServer(lightningBolt, Float.MAX_VALUE)`.
- `getDefaultDimensions` (287-290): `BABY_DIMENSIONS` = turtle dims scaled 0.3, with a
  passenger attachment offset (`0, height, -0.25`).
- Inner goal classes (292-649): `TurtleBreedGoal` (extends vanilla `BreedGoal`, blocks
  breeding while `hasEgg()`, awards stat/criteria, sets both parents' age to 6000 (adult
  cooldown), drops 1-7 XP); `TurtleGoHomeGoal` (329-396, a bespoke `Goal` — random 1-in-700
  chance per tick once not already within 64 blocks of home to start heading home; give-up
  after 600 ticks close-to-home without arriving, or 20-radian pathing failure = stuck);
  `TurtleGoToWaterGoal` (398-431, `MoveToBlockGoal` subclass — babies always seek water,
  adults skip while `goingHome`/has egg/already in water; recompute path every 160 ticks;
  target validity = block is water); `TurtleLayEggGoal` (433-480, `MoveToBlockGoal`
  subclass — only active with egg within 9 blocks of home; on reaching a valid
  sand-with-air-above target, ticks `layEggCounter`: first tick sets `LAYING_EGG=true`; at
  `>200` ticks plays `TURTLE_LAY_EGG` sound, places a `TURTLE_EGG` block with
  `EGGS = random(1..4)`, clears has_egg/laying_egg, sets 600-tick post-lay love cooldown);
  `TurtleMoveControl` (482-525, custom buoyancy — nudges +0.005 y/tick while in water,
  halves speed floor to 0.08 while far from home, 0.06 baby-speed floor while baby, 0.06
  floor on land); `TurtlePanicGoal` (527-547, extends `PanicGoal`, prefers fleeing toward
  nearby water within 7 blocks over a random land position); `TurtlePathNavigation`
  (550-561); `TurtleRandomStrollGoal` (563-575, disabled while in water/going
  home/has egg); `TurtleTravelGoal` (577-649, a bespoke `Goal` — while swimming and not
  going home/no egg, picks a random long-range travel target within ±512 xz / ±4..+4 y
  (clamped to not exceed sea level), re-picks when path is done/stuck, requires target
  chunk-area loaded via `hasChunksAt` with a 34-block margin, cancels on love/egg/going
  home).

### Current Rust (`pumpkin/src/entity/passive/turtle.rs`, 56 lines)
Only wires `SwimGoal`, `EscapeDangerGoal(1.2)`, `BreedGoal(1.0)`, `TemptGoal(1.1,
[Item::SEAGRASS])` (should be the full `ItemTags.TURTLE_FOOD` tag — seagrass only is a
subset), `WanderAroundGoal(1.0)`, `LookAtEntityGoal(6.0)` (vanilla is 8.0), and
`RandomLookAroundGoal`. No synced data, no NBT, no egg-laying, no home tracking, no baby
dimensions, no custom sounds, no `thunderHit`, no `isPushedByFluid` override, no
`canBeLeashed` override, no pathfinding malus setup, no custom move control/navigation.

### Target Rust design

**Struct fields** (add to `TurtleEntity`):
```rust
pub struct TurtleEntity {
    pub mob_entity: MobEntity,
    has_egg: AtomicBool,
    laying_egg: AtomicBool,
    lay_egg_counter: AtomicI32,
    home_pos: AtomicCell<BlockPos>,        // default BlockPos::ZERO, set in finalize_spawn
    travel_pos: AtomicCell<Option<BlockPos>>,
    going_home: AtomicBool,
}
```

**Synced data**: `TrackedData::HAS_EGG` / `TrackedData::LAYING_EGG`, both
`MetaDataType::BOOLEAN`, sent from a `send_egg_state`-style helper and pushed once in
`mob_init_data_tracker` (mirrors `PufferfishEntity::mob_init_data_tracker` /
`send_puff_state`).

**NBT** (`write_nbt`/`read_nbt_non_mut`): `home_pos` via the Bee-pattern
`block_pos_to_nbt`/`block_pos_from_nbt` helper (int array `[x,y,z]`) under key
`"home_pos"`; `has_egg` as a bool under `"has_egg"`. Do **not** persist `laying_egg` or
`lay_egg_counter` — vanilla doesn't either.

**finalize_spawn hook**: needs a `Mob`/`EntityBase` spawn-finalization hookpoint (check
whatever `mob_finalize_spawn`/equivalent exists — if none exists yet, this is a small gap
to add to the `Mob` trait) to call `home_pos.store(self.block_pos())`.

**canBeLeashed**: add `fn can_be_leashed(&self) -> bool { true }` to `Mob` (default true,
matching vanilla `Mob.canBeLeashed` default), override to `false` on `TurtleEntity`, and
gate `LeadItem::use_on_entity` (`pumpkin/src/item/items/lead.rs`) on it before calling
`leash_to`. This is shared infra touched only lightly for this fix — see the note at the
top of this document.

**isPushedByFluid** -> `false`: check `EntityBase`/`Mob` for the existing hook (used
elsewhere for boats/other non-current-affected entities); override on Turtle.

**thunderHit**: check whatever lightning-strike dispatch exists (likely
`EntityBase::on_lightning_strike` or similar); override to deal `f32::MAX` damage via
the existing damage path (same shape as `ShulkerEntity::on_damage`).

**Ambient/hurt/death/swim/step sounds**: override the corresponding `Mob`/`LivingEntity`
sound hooks (whatever naming convention `get_ambient_sound`/`get_hurt_sound`/etc use in
this codebase — confirm against e.g. `wolf.rs`/`cat.rs` which already have baby/adult
sound branching) to select `TurtleAmbientLand` vs the inherited water-ambient default,
`TurtleSwim` with 1.5x volume, `TurtleHurt`/`TurtleHurtBaby`, `TurtleDeath`/`TurtleDeathBaby`,
`TurtleShamble`/`TurtleShambleBaby` at volume 0.15.

**Baby dimensions**: 0.3 scale; use the `entity_dimension.store(scaled)` pattern from
`slime.rs`/`sulfur_cube.rs`/`phantom.rs`, recomputed whenever baby/adult state flips.

**Pathfinding malus**: constructor-time (or `mob_init_data_tracker`-time) setup —
`WATER -> 0.0`, all three door path types `-> -1.0` (impassable). Check whether
`set_pathfinding_malus`-equivalent exists on `MobEntity`/navigator; if the navigator
doesn't yet support per-type malus at all, that's a prerequisite gap worth flagging
separately (affects any future amphibious mob too).

**Goals** (`registerGoals` order 0-9), all new files under
`pumpkin/src/entity/ai/goal/`:
- `TurtlePanicGoal` — likely can extend/wrap the existing `EscapeDangerGoal` the way
  vanilla's `TurtlePanicGoal extends PanicGoal`; needs the "prefer nearby water" override
  (`lookForWater` within 7 blocks) layered on top. If `EscapeDangerGoal` isn't structured
  for subclassing, a new small goal is fine.
- `TurtleBreedGoal` — wraps `BreedGoal`, denies while `has_egg`; on breed sets `has_egg`,
  sets both parents' age to 6000, resets love, spawns 1-7 XP orbs (gate on `MOB_DROPS`).
- `TurtleLayEggGoal` — new file `turtle_lay_egg.rs`, built on `MoveToTargetPosGoal` +
  `MoveToTargetPos` impl (target = sand with air above, matches `TurtleEggBlock.isSand`
  equivalent + emptiness check on the block above); the counter/state-machine (start
  laying at tick 1, place egg block + reset state past tick 200) lives in `tick()`.
- `TurtleGoToWaterGoal` — `MoveToTargetPosGoal` + `MoveToTargetPos` (`is_target_pos` =
  block is water), baby-always/adult-conditional `can_start`, recalculate every 160 ticks.
- `TurtleGoHomeGoal` — new bespoke `Goal` (no existing analog): random 1/700 trigger,
  ticks toward `home_pos` using whatever "random pos towards" pathfinding helper exists
  (search for `DefaultRandomPos`-equivalent under `ai/pathfinder`), give-up bookkeeping.
- `TurtleTravelGoal` — new bespoke `Goal`, random long-range swim target generation +
  chunk-loaded check.
- `TurtleRandomStrollGoal` — likely a thin wrapper around `WanderAroundGoal` with the
  extra `!in_water && !going_home && !has_egg` gate; check whether `WanderAroundGoal` is
  `Sized`/composable enough to wrap, otherwise duplicate with the gate.
- `TurtleMoveControl` — custom move-control buoyancy/speed-floor logic; check whether a
  `MoveControl` trait/hookpoint exists yet in this codebase's mob movement stack. If
  move-control isn't pluggable yet, `travel_in_water`-equivalent physics (see below) can
  absorb most of the vanilla behavior instead, with the move-control speed floors treated
  as a smaller follow-up.
- Custom navigation (`TurtlePathNavigation.isStableDestination`) — likely needs a hook on
  whatever amphibious-navigation abstraction exists; flag for wiki/source
  double-check if that hook doesn't exist cleanly yet.

**travel_in_water physics**: override the water-travel hook (same shape as whatever
`travelInWater` maps to in this codebase — check `LivingEntity`/`Mob` for an existing
override point, Dolphin needs the identical hook) implementing `moveRelative(0.1, input)`
+ velocity *0.9 damp + conditional sink.

**Tempt item fix**: replace the `[Item::SEAGRASS]` list with the full
`tag::ItemTag::TURTLE_FOOD` tag lookup (check `pumpkin_data::tag` for whether this tag
already exists — if not, it needs adding to tag data, likely already present since tags
are asset-driven).

### Files to touch
- Modify: `pumpkin/src/entity/passive/turtle.rs` (full rewrite).
- New: `pumpkin/src/entity/ai/goal/turtle_lay_egg.rs`,
  `turtle_go_home.rs`, `turtle_go_to_water.rs`, `turtle_travel.rs`,
  `turtle_random_stroll.rs` (or fold the last into an inline gate if `WanderAroundGoal`
  composes cleanly).
- Modify: `pumpkin/src/entity/mob/mod.rs` (add `can_be_leashed` default-true hook to
  `Mob`/`EntityBase`, whichever trait `LeadItem` should consult).
- Modify: `pumpkin/src/item/items/lead.rs` (consult `can_be_leashed` before `leash_to`).
- Possibly modify: navigator/move-control infra if per-type pathfinding malus or a
  pluggable `MoveControl` doesn't exist yet (verify before assuming; this may already be
  supported generically and just unused by Turtle).

### Needs wiki/source verification
- Exact current `ItemTags.TURTLE_FOOD` membership (seagrass, plus anything else added in
  later versions) — verify against the decompiled tag JSON, not memory.
- Whether `hurtServer(..., Float.MAX_VALUE)` for `thunderHit` is still exactly this in
  26.2 (cited above, but re-check line 284 against the actual checkout being ported, not
  a paraphrase).

---

## 2. Dolphin

### Vanilla source
`Dolphin.java` (588 lines).

- Synced data: `GOT_FISH` (bool, 74), `MOISTNESS_LEVEL` (int, 75, default 2400,
  `TOTAL_MOISTNESS_LEVEL`).
- Constants: `TOTAL_AIR_SUPPLY = 4800` (77), `BABY_SCALE = 0.65` (80), `BABY_DIMENSIONS`
  (81-85, eye height 0.09375, passenger attach at `(0, 0.3125, 0)`).
- `ALLOWED_ITEMS` predicate (79): item entity not on pickup-delay, alive, in water — used
  by `PlayWithItemsGoal`.
- Constructor (89-94): `SmoothSwimmingMoveControl(85, 10, 0.02, 0.1, true)`,
  `SmoothSwimmingLookControl(10)`, `setCanPickUpLoot(true)`.
- `finalizeSpawn` (96-104): full air supply, `xRot=0`, 10% twin chance
  (`AgeableMobGroupData(0.1F)`).
- `getAgeScale` 0.65 baby (110-113).
- `handleAirSupply` overridden to **no-op** (115-117) — air supply doesn't drain from the
  normal drowning path; dolphins manage air via the `tick()` moistness system instead.
- `gotFish`/`setGotFish`, `getMoistnessLevel`/`setMoisntessLevel` (119-133, note the
  vanilla method name typo `setMoisntessLevel` — irrelevant for Rust naming, just don't
  be surprised it's misspelled in source).
- `defineSynchedData`/save-data (135-154): `GotFish` bool, `Moistness` int, default 2400.
- `registerGoals` (156-172): 0 `BreathAirGoal`, 0 `TryFindWaterGoal`, 1
  `DolphinSwimToTreasureGoal`, 2 `DolphinSwimWithPlayerGoal(4.0)`, 4
  `RandomSwimmingGoal(1.0, 10)`, 4 `RandomLookAroundGoal`, 5 `LookAtPlayerGoal(6.0)`, 5
  `DolphinJumpGoal(10)`, 6 `MeleeAttackGoal(1.2, true)`, 8 `PlayWithItemsGoal`, 8
  `FollowPlayerRiddenEntityGoal(AbstractBoat)`, 8
  `FollowPlayerRiddenEntityGoal(AbstractNautilus)`, 9 `AvoidEntityGoal(Guardian, 8.0, 1.0,
  1.0)`; target selector 1 `HurtByTargetGoal(Guardian).setAlertOthers()`.
- `createAttributes` (174-176): MAX_HEALTH 10, MOVEMENT_SPEED 1.2, ATTACK_DAMAGE 3.
- `canAttack` (188-191): never while baby.
- `getMaxAirSupply` 4800, `increaseAirSupply` always resets to max (198-201) — i.e. once
  air starts refilling it snaps instantly, unlike the gradual player mechanic.
- Head rotation clamps `getMaxHeadXRot`/`getMaxHeadYRot` = 1 (204-211) — dolphins barely
  turn their heads (almost all "looking" is body rotation).
- `pickUpItem`/`canDispenserEquipIntoSlot` (213-230): manual mainhand-only pickup (doesn't
  use the generic loot-pickup path), sets `setGuaranteedDrop`.
- `tick()` (232-286) — **the core moistness/dry-out/particle logic**:
  - `isNoAi()` -> air supply forced to max (short-circuit).
  - else: `isInWaterOrRain()` -> moistness reset to 2400; otherwise moistness -= 1 per
    tick, and at `<= 0` deals 1.0 `dryOut` damage **every tick** (not once).
  - while out of water and `onGround()`: random horizontal flop impulse + 0.5 upward
    velocity, random yaw, force `onGround=false` (dolphins flop around on land).
  - client-side particle trail (`ParticleTypes.DOLPHIN`) while swimming with velocity —
    server-authoritative port can skip/adapt this to whatever this codebase's particle
    dispatch looks like, but the *dry-out damage* and *flopping* halves are gameplay and
    must be server-side.
- `handleEntityEvent(38)` (288-295) -> spawns 7 `HAPPY_VILLAGER` particles (used to
  broadcast "found treasure"/"ate fish" visual, see `DolphinSwimToTreasureGoal.start` and
  `PlayWithItemsGoal`... actually the broadcast site is inside
  `DolphinSwimToTreasureGoal.tick`/`start`, event 38).
- `mobInteract` (306-326): if held item `is(ItemTags.FISHES)`: play `DOLPHIN_EAT`; if
  `canAgeUp()` consume 1 + `ageUp`, else `setGotFish(true)` + consume 1. This is the
  fish-feeding/aging path from the subsystem brief.
- Sounds: hurt/death/swim-splash/swim as listed (328-351); ambient is water-vs-air
  variant (338-341).
- `closeToNextPos` (353-356): navigation target within 12 blocks.
- `getDefaultDimensions` baby branch (358-361).
- `travelInWater` (363-371): `moveRelative(getSpeed(), input)`, move, damp *0.9, sink
  `-0.005` unless has a target.
- `canBeLeashed` -> **true** (373-376, explicit override — matches default, but stated
  explicitly since `AgeableWaterCreature` might otherwise say no; keep parity by not
  special-casing this one, just don't accidentally inherit `false`).
- `DolphinSwimToTreasureGoal` (378-465): move+look flags, non-interruptable; requires
  `gotFish()` and air supply >= 100; on start, does
  `level.findNearestMapStructure(StructureTags.DOLPHIN_LOCATED, pos, 50, false)` — this is
  the "structure search" the subsystem brief calls out, needs whatever structure-lookup
  API this codebase's `ServerLevel`/`World` exposes (likely doesn't exist yet for
  structures at all — flag this as a probable prerequisite/stub-with-TODO if structure
  search isn't wired up anywhere else in Pumpkin yet); on success broadcasts event 38 (
  particle); ticks toward the treasure using "pos towards" pathfinding helper (same one
  Turtle needs); stops/resets `gotFish` when arrived or stuck.
- `DolphinSwimWithPlayerGoal` (467-513): finds nearest swimming player within 10 blocks
  (non-combat targeting, ignore line of sight), applies `MobEffects.DOLPHINS_GRACE`
  (100-tick = 5s) to the player on start and every ~6 ticks (`1/6` chance) while swimming
  close; this is the Dolphin's Grace goal from the brief.
- `PlayWithItemsGoal` (515-587): move-only; finds nearby non-picked-up in-water item
  entities within an 8-block-inflated box or already holds one; grabs the first one found;
  on stop, throws the held item back out with a randomized velocity (`getEyeY() - 0.3`
  origin, pitch/yaw-based direction + small random spread) and a 40-tick pickup delay +
  0-99 tick cooldown before playing again.

### Current Rust (`pumpkin/src/entity/passive/dolphin.rs`, 55 lines)
Only `SwimGoal`, `WanderAroundGoal(1.0)`, `LookAtEntityGoal(6.0)`, `RandomLookAroundGoal`,
`DolphinJumpGoal(10)`. No synced data, no moistness/air system, no feeding, no treasure
goal, no swim-with-player goal, no play-with-items goal, no attributes override, no
sounds, no custom dimensions/attack restrictions.

### Target Rust design

**Struct fields**:
```rust
pub struct DolphinEntity {
    pub mob_entity: MobEntity,
    got_fish: AtomicBool,
    moistness_level: AtomicI32,     // default 2400
    treasure_pos: AtomicCell<Option<BlockPos>>,
}
```

**Synced data**: `TrackedData::GOT_FISH` (BOOLEAN), `TrackedData::MOISTNESS_LEVEL` (INT),
pushed in `mob_init_data_tracker` per the Pufferfish pattern.

**NBT**: `GotFish` bool, `Moistness` int (default 2400 on missing).

**mob_tick** (this is where the moistness/dry-out/flop logic from vanilla `tick()`
belongs, following `PufferfishEntity::mob_tick`'s shape):
```
if in_water_or_rain: moistness = 2400
else:
    moistness -= 1
    if moistness <= 0: damage(dry_out, 1.0)   // every tick, not once
    if on_ground: random flop impulse, random yaw, on_ground = false
```
Need to confirm this codebase has a `dry_out`/equivalent `DamageType` — check
`pumpkin_data::damage::DamageType` for a drowning/dry-out variant (the Java
`damageSources().dryOut()` maps to a specific vanilla damage type id; verify the constant
name in `pumpkin-data` matches, don't assume).

**handle_air_supply override**: needs to be a no-op (air supply always full via the
moistness path instead) — find the equivalent hook on `LivingEntity`/`Mob`.

**Attributes**: MAX_HEALTH 10, MOVEMENT_SPEED 1.2, ATTACK_DAMAGE 3 (check whatever
attribute-registration pattern other mobs use, e.g. search for `create_attributes` or
similar on a comparable already-ported mob).

**canAttack baby restriction, max air supply override (4800, always-full
increase_air_supply), head-rotation clamps (1/1), mainhand-only item pickup**: straightforward
per-hook overrides, same shape as existing overrides elsewhere in `mob/mod.rs`.

**mobInteract (fish feeding)**: override the player-interact hook (check
`EntityBase`/`Mob` for whatever `mob_interact`/`interact` signature exists — Turtle
doesn't need this but e.g. `wolf.rs`/`cat.rs` interact overrides are a good template) —
if held item is tagged `Item::FISHES`-equivalent, play `DolphinEat`, then either age up
(if a baby that can age) or set `got_fish = true`, consuming 1 item either way.

**Sounds**: hurt/death/swim/swim-splash overrides, ambient water-vs-air branch.

**travel_in_water**: same physics shape as Turtle's (see Turtle section) but with
`get_speed()` instead of the fixed `0.1`.

**canBeLeashed**: explicit `true` override for clarity/parity (no functional change from
default, but keep it since the `can_be_leashed` infra is being added for Turtle anyway).

**Baby dimensions**: 0.65 scale, custom eye height.

**Goals**, new files under `ai/goal/`:
- `DolphinSwimToTreasureGoal` — **blocked on structure-search support**. Check whether
  Pumpkin's world model exposes any "find nearest structure of tag" API yet
  (`grep -rn "findNearestMapStructure\|find_nearest.*structure" pumpkin/src`); if it
  doesn't exist anywhere, this goal cannot be fully ported without first adding structure
  lookup to `World`/`ServerLevel` — flag this explicitly as a prerequisite rather than
  quietly stubbing the search to always-fail, since that would silently break dolphin
  treasure-hunting for good.
- `DolphinSwimWithPlayerGoal` — targeting: nearest swimming player within 10 blocks, apply
  `MobEffects::DOLPHINS_GRACE` (verify this potion effect exists in
  `pumpkin_data::effect::StatusEffect`) for 100 ticks on start + `1/6` chance per tick
  while continuing.
- `PlayWithItemsGoal` — needs a "list item entities in box matching predicate" world
  query (check whatever `get_nearby_entities`/`get_entities_of_class`-equivalent
  `PufferfishEntity::threat_nearby` already uses — that's the right precedent) plus
  spawning a new `ItemEntity` on `stop()` with a randomized throw velocity.
- `FollowPlayerRiddenEntityGoal` (boat/nautilus) — check if a generic version of this
  goal exists already (used by other "escort the ridden vehicle" mobs, if any); if not,
  small new goal.
- `HurtByTargetGoal(Guardian).setAlertOthers()` on the target selector — check whether
  `revenge.rs`/`owner_hurt_by_target.rs` already generalizes to "alert same-species
  neighbors on being hurt by X"; wire dolphin's target selector to it filtered to
  Guardian.
- `AvoidEntityGoal(Guardian, 8.0, 1.0, 1.0)` — `avoid_entity.rs` already exists and is
  used elsewhere (Cod/Salmon/TropicalFish avoid players); just parameterize for Guardian.
- `MeleeAttackGoal(1.2, true)` — check `melee_attack.rs` exists already (it does, per the
  goal listing) and wire it in.

### Files to touch
- Modify: `pumpkin/src/entity/passive/dolphin.rs` (full rewrite).
- New: `pumpkin/src/entity/ai/goal/dolphin_swim_to_treasure.rs` (or stub + explicit TODO
  if structure search is unavailable), `dolphin_swim_with_player.rs`,
  `dolphin_play_with_items.rs`, possibly `follow_player_riding.rs` if no generic version
  exists.
- Possibly modify: `pumpkin_data` `DamageType` if a dry-out/drowning-on-land damage type
  is missing (verify first).
- Possibly modify: `World`/`ServerLevel` for structure-tag lookup (only if genuinely
  absent — verify before scoping this in).

### Needs wiki/source verification
- Exact `dryOut` damage type id/translation key used by 26.2 (cite from
  `damagesource`/`DamageTypes` decompiled source, not memory).
- Whether `StructureTags.DOLPHIN_LOCATED` still resolves to buried-treasure-only in the
  current version (verify against decompiled tag data).

---

## 3. Squid / GlowSquid

### Vanilla source
`Squid.java` (322 lines) + `GlowSquid.java` (120 lines, extends `Squid`).

**Squid**:
- Visual/physics fields (38-49): `xBodyRot`/`zBodyRot` (+`O` previous-tick pairs),
  `tentacleMovement`/`oldTentacleMovement`, `tentacleAngle`/`oldTentacleAngle`, `speed`,
  `tentacleSpeed`, `rotateSpeed`, `movementVector` (Vec3, jet-propulsion direction/force).
- `BABY_DIMENSIONS` = 0.5x0.5, eye height 0.37 (50).
- Constructor: randomizes initial `tentacleSpeed` (54).
- `registerGoals` (58-61): 0 `SquidRandomMovementGoal`, 1 `SquidFleeGoal`.
- `createAttributes`: MAX_HEALTH 10 (63-65).
- Sounds: ambient/hurt/death (67-80), `getSquirtSound` (82-84, overridden by GlowSquid).
- `canBeLeashed` -> **true** (86-89, explicit).
- `getSoundVolume` 0.4 (91-94), `getMovementEmission` EVENTS (96-99, i.e. squids emit
  sculk-sensor-visible movement events, unlike most silent-movement mobs — worth a
  gameevent double check).
- `getDefaultGravity` 0.08 (106-109, lower than normal 0.08 vs typical air-mob gravity —
  double check this differs from the entity-generic default before assuming it needs an
  override at all).
- **`aiStep`** (111-175) — the jet-propulsion/tentacle-animation core:
  - advances `tentacleMovement` by `tentacleSpeed`/tick; on wraparound past 2π,
    server-side re-randomizes `tentacleSpeed` (1-in-10 chance) and broadcasts entity
    event 19 (client resets tentacle phase — `handleEntityEvent` 213-220).
  - **while in water**: tentacle-angle curve (`sin(t²·π)·π·0.25` while `t < π`, else 0);
    past 75% into a cycle, snaps velocity to `movementVector` (this is the actual jet
    thrust application) and resets `rotateSpeed=1`, otherwise decays `rotateSpeed *= 0.8`
    (or `*=0.99` and damps velocity *0.9 once past π); body rotation
    (`yBodyRot`/`xBodyRot`) smoothly interpolates toward the velocity direction, `zBodyRot`
    (roll) advances by `π·rotateSpeed·1.5`.
  - **out of water**: tentacle angle = `|sin(tentacleMovement)|·π·0.25`; server-side
    applies levitation-aware gravity (`0.05*(amplifier+1)` if levitating, else normal
    gravity) scaled by air drag; slow rotation toward -90° pitch (flopping on land).
- `hurtServer` (177-185): on successful damage **and** has a `lastHurtByMob`, spawns ink.
- `rotateVector`/`spawnInk` (187-202): computes a world-space "behind and below" origin
  via inverse body rotation, then fires 30 ink particles in randomized cone directions
  (baby squids get a smaller `0.1` vs `0.3` offset scale) + plays the squirt sound.
- `getInkParticle` -> `SQUID_INK` (overridden by GlowSquid -> `GLOW_SQUID_INK`).
- `travel` (208-211): **overridden to just `move(SELF, deltaMovement)`** — squids do not
  use the generic AI-driven travel/moveRelative path at all; all locomotion comes from
  `aiStep`'s jet propulsion.
- `handleEntityEvent(19)` -> resets `tentacleMovement = 0`.
- `hasMovementVector` (222-224): squared length > 1e-5.
- `finalizeSpawn` (226-232): 5% twin chance group data.
- `getDefaultDimensions` baby branch (234-237).
- `SquidFleeGoal` (239-297, inner, non-static since it reads outer `Squid.this`):
  requires in water + has a `lastHurtByMob` within 100 (sq) distance; every tick, computes
  a flee vector away from the attacker, clamped to speed 3.0 (reduced linearly past 5
  blocks distance), zeroes the y-component if the flee-destination block is air (so it
  doesn't try to flee onto land), sets `movementVector = fleeTo/20`; every 10th tick (mod
  5) spawns a `BUBBLE` particle.
- `SquidRandomMovementGoal` (299-321, static, holds a `Squid` ref): always `canUse`; each
  tick, if `noActionTime > 100` zeroes the movement vector (idle too long -> stop
  drifting); else 1-in-50 chance (or if not touching water / no movement vector) to pick
  a new random horizontal-cone `movementVector` (`cos(angle)*0.2, -0.1..+0.1, sin(angle)*0.2`).

**GlowSquid** (extends Squid):
- `DATA_DARK_TICKS_REMAINING` (int synced data, 27).
- `getInkParticle` -> `GLOW_SQUID_INK`.
- `defineSynchedData` adds the dark-ticks field.
- Distinct sounds: `GLOW_SQUID_SQUIRT`/`AMBIENT`/`HURT`/`DEATH`.
- NBT: `DarkTicksRemaining` int.
- `aiStep` (82-91): decrements dark ticks if > 0; **every tick**, spawns a `GLOW`
  particle at a random point in the bounding box (this runs regardless of dark-tick
  state — glow squids always emit the glow particle, dark ticks only gate the
  glow-*texture* dimming, per the wiki).
- `hurtServer` (93-101): on successful hit, resets dark ticks to 100 (this makes the
  squid render dark/non-glowing for ~5s after being hurt).
- `checkGlowSquidSpawnRules` (111-119): Y <= sea level - 33, zero raw brightness, on
  water block — deep, dark ocean spawning.

### Current Rust
`squid.rs` (52 lines) and `glow_squid.rs` (53 lines): both wire only `SwimGoal`,
`WanderAroundGoal` (explicitly commented as a placeholder — "Squids don't have
WanderAroundGoal in vanilla... but for now we give it to them", confirming the stub
status), `LookAtEntityGoal`, `RandomLookAroundGoal`. No jet-propulsion physics, no ink
defense, no dark-ticks synced data on glow squid, no distinct sounds beyond whatever
`Mob` defaults exist.

### Target Rust design

**Squid struct**:
```rust
pub struct SquidEntity {
    pub mob_entity: MobEntity,
    tentacle_movement: AtomicCell<f32>,
    old_tentacle_movement: AtomicCell<f32>,
    tentacle_angle: AtomicCell<f32>,
    tentacle_speed: AtomicCell<f32>,
    rotate_speed: AtomicCell<f32>,
    x_body_rot: AtomicCell<f32>,
    z_body_rot: AtomicCell<f32>,
    movement_vector: AtomicCell<Vector3<f64>>,
}
```
(Skip the "old"/previous-tick shadow fields that exist purely for vanilla's client-side
render interpolation — `xBodyRotO`, `zBodyRotO`, `oldTentacleAngle` — unless this
codebase's entity-render sync protocol needs them; check how another animated-only mob,
e.g. `warden.rs`'s attack-animation ticks, handles this before deciding, since duplicating
client-interp-only state server-side is wasted work if the client re-derives it.)

**GlowSquid** wraps/extends the same struct plus `dark_ticks_remaining: AtomicI32` and a
different ink-particle/sound set — since Rust has no inheritance, follow whatever pattern
`glow_squid.rs` already half-uses (a distinct struct with its own `MobEntity`), but
**share the jet-propulsion physics** via a free function or a small trait
(`SquidPhysics`) implemented by both, rather than copy-pasting the `aiStep` port twice.
This mirrors vanilla's actual inheritance and avoids the two implementations drifting.
Suggested shape:
```rust
trait SquidLike {
    fn squid_state(&self) -> &SquidState;   // shared physics fields
    fn ink_particle(&self) -> Particle;      // SQUID_INK vs GLOW_SQUID_INK
    fn squirt_sound(&self) -> Sound;
}
fn squid_ai_step(squid: &dyn SquidLike, mob: &MobEntity) { /* ported aiStep body */ }
```
`SquidEntity` and `GlowSquidEntity` each hold a `SquidState` and implement `SquidLike`;
`GlowSquidEntity` additionally ticks its own dark-ticks countdown/particle in its own
`mob_tick`, calling the shared `squid_ai_step` first.

**travel override**: both must override whatever the generic AI-movement/travel hook is
(check `Mob`/`LivingEntity` for a `travel`/`apply_movement_input`-equivalent) to *skip*
normal AI movement entirely and just apply raw velocity — this is the single most
important physics fact from vanilla (`travel` is neutered to `move(SELF, deltaMovement)`
only). Getting this wrong (leaving default AI travel active) would double-apply movement
and break the jet-propulsion feel entirely.

**hurtServer/ink defense**: override the damage hook, spawn ink only when
damaged-and-has-last-attacker, following `PufferfishEntity::sting`'s use of
`damage_with_context`/`get_last_hurt_by_mob`-equivalent as precedent for reading
"last attacker" state (check whether `LivingEntity` already tracks a `last_hurt_by`
field — likely yes, since Pufferfish's design doc references `getLastHurtByMob`-style
state elsewhere in this codebase already... verify by grep before assuming).

**GlowSquid additions**:
- `TrackedData::DARK_TICKS_REMAINING` (INT synced data) — id already exists in codegen.
- `mob_tick`: decrement dark ticks if >0 (no floor beyond 0), unconditionally spawn a
  `Particle::Glow`-equivalent each tick at a random point in the bounding box.
- `hurtServer` override: on damage, reset dark ticks to 100 (send new synced value).
- NBT: `DarkTicksRemaining` int, default 0.
- Distinct sounds + ink particle (already half-present as override points in `Mob`; just
  needs the actual `Sound::EntityGlowSquid*`/`Particle::GlowSquidInk` values wired in).

**Goals**:
- `SquidRandomMovementGoal` — new file, straightforward port (no-action-time check, 1-in-50
  reroll of a random horizontal jet direction).
- `SquidFleeGoal` — new file, needs "last hurt by" living-entity access + a
  block/air check at the flee destination (reuse whatever world block-query
  `PufferfishEntity`/others already use).
- Drop the placeholder `WanderAroundGoal` entirely for both — vanilla squids have no
  such goal; the jet-propulsion `aiStep` fully replaces it. This also removes the
  explicit "for now we give it to them" TODO comment.

### Files to touch
- Modify: `pumpkin/src/entity/passive/squid.rs`, `pumpkin/src/entity/passive/glow_squid.rs`
  (both full rewrites).
- New: `pumpkin/src/entity/ai/goal/squid_random_movement.rs`,
  `squid_flee.rs`.
- Possibly new: a small shared module (e.g.
  `pumpkin/src/entity/passive/squid_physics.rs`) holding `SquidState`/`SquidLike`/
  `squid_ai_step` if the trait-sharing approach above is adopted — recommended over
  duplicating the ~65-line `aiStep` port twice.

### Needs wiki/source verification
- Confirm `getDefaultGravity() = 0.08` for Squid is actually a deviation from this
  codebase's generic mob gravity default (if the generic default already is 0.08, no
  override is needed at all — check before adding dead code).
- Confirm `getMovementEmission() = EVENTS` semantics (sculk sensors) against current
  wiki/source rather than assuming it's decorative.

---

## 4. Salmon / TropicalFish / Cod

### Vanilla source
- `Cod.java` (40 lines) — trivial: bucket item, 4 sounds, nothing else. All behavior
  comes from its `AbstractSchoolingFish`/`AbstractFish` superclasses (not read in full
  here since Cod itself adds nothing beyond sounds+bucket; the generic
  schooling/fleeing/flopping behavior already has a Rust analog via the goals already
  wired: `AvoidEntityGoal`, `FollowFlockLeaderGoal`).
- `Salmon.java` (184 lines) — adds a **Variant** enum (`SMALL`/`MEDIUM`/`LARGE`, ids 0/1/2,
  bounding-box scales 0.5/1.0/1.5, default `MEDIUM`) as synced data `DATA_TYPE` (int, 37).
  `getMaxSchoolSize()` -> 5 (44-47, vs some default elsewhere — check the default before
  assuming this needs an override). `onSyncedDataUpdated` triggers `refreshDimensions()`
  when the variant changes (80-86) — dimension recompute must be reactive to variant
  change, not just at spawn. `finalizeSpawn` (135-145) uses a **weighted list**: SMALL 30,
  MEDIUM 50, LARGE 15 (not a flat 90/10 split — that distribution belongs to
  TropicalFish, not Salmon; don't conflate the two subsystems' random-variant rules).
  `getSalmonScale`/`getDefaultDimensions` (147-154) scale the base dimensions by the
  variant's `boundingBoxScale`. `saveToBucketTag` copies the `SALMON_SIZE` data
  component onto the bucket item (101-104) — this is the "variant survives bucket
  capture" mechanic the subsystem brief calls out, and it depends on
  `DataComponents.SALMON_SIZE`, i.e. the bucket-entity-data gap flagged at the top of
  this document.
- `TropicalFish.java` (374 lines) — the most complex of the three:
  - `Variant` record (`pattern: Pattern, baseColor: DyeColor, patternColor: DyeColor`,
    363-373) packed into a single int via `packVariant` (84-86:
    `pattern.packedId & 0xFFFF | (baseColor.id & 0xFF) << 16 | (patternColor.id & 0xFF) << 24`)
    stored as synced data `DATA_ID_TYPE_VARIANT` (int, 49).
  - `Pattern` enum (281-352): 12 patterns, each tagged `Base.SMALL` or `Base.LARGE`
    (270-279, just an id 0/1 used as the low byte of the pattern's own packed id:
    `packedId = base.id | index << 8`) — i.e. **the "small/large" distinction for
    tropical fish is a pattern-body-shape flag, not a bounding-box scale** like Salmon;
    don't conflate this with Salmon's `Variant.boundingBoxScale` mechanic even though
    both are called "size"-ish concepts in vanilla.
  - `COMMON_VARIANTS` (50-73): a fixed list of 22 exact (pattern, baseColor,
    patternColor) combos — these are the "predefined" named tropical fish varieties
    (used for tooltip naming via `getPredefinedName`/`addToTooltip`, 332-350).
  - `getBaseColor`/`getPatternColor`/`getPattern` (88-98): unpack from the int via shifts
    matching the pack function exactly — must port bit-for-bit, not approximately.
  - `finalizeSpawn` (234-257): **90% chance** picks a random entry from `COMMON_VARIANTS`
    and marks the whole spawn group with that shared variant (`TropicalFishGroupData`,
    354-361, extends `SchoolSpawnGroupData` so the whole school matches); **10% chance**
    picks a fully random `(pattern, baseColor, patternColor)` triple independently per
    fish **and sets `isSchool = false`** (i.e. rare/"non-common" tropical fish don't
    school together) — this is exactly the "90%-common/10%-random" split the subsystem
    brief describes, confirmed at the source level.
  - `isMaxGroupSizeReached` (123-126): returns `!isSchool` — a non-common fish is
    immediately "at max group size" (i.e. refuses to group at all).
  - Data components: `TROPICAL_FISH_PATTERN`/`_BASE_COLOR`/`_PATTERN_COLOR` get/apply
    (165-198) plus `saveToBucketTag` (200-206) copying all three components onto the
    bucket — same bucket-entity-data dependency as Salmon.
  - `getBucketItemStack` -> plain `TROPICAL_FISH_BUCKET` (208-211, the variant rides on
    the data component, not the base item id).
  - `checkTropicalFishSpawnRules` (259-268): fluid below is water, block above is water,
    and (biome allows tropical spawns at any height OR passes the generic surface-water
    spawn check) — a biome-gated spawn rule, distinct from Cod/Salmon's presumably
    simpler rule (not shown here since `AbstractFish`/superclass spawn rules weren't
    read; check them if spawn-rule parity for Cod/Salmon is also in scope, since this
    doc's brief only calls out TropicalFish/Salmon variant behavior specifically).

### Current Rust
`cod.rs` (62 lines), `salmon.rs` (59 lines), `tropical_fish.rs` (59 lines) — **all three
are near-identical copies** of the same goal-wiring boilerplate (`SwimGoal`,
`EscapeDangerGoal(1.25)`, `AvoidEntityGoal(PLAYER, 8.0, 1.6, 1.4)`, `WanderAroundGoal(1.0)`,
`FollowFlockLeaderGoal`, `LookAtEntityGoal(6.0)`, `RandomLookAroundGoal`) with no
per-species differentiation at all beyond the struct/type names. No bucket item override
(relies entirely on the generic `bucket.rs` mapping, which is already correct for the
*base* item but loses variant data), no synced variant data, no dimension scaling.

### Shared "fish variant" helper — recommendation

**Recommend a shared helper module, not per-species duplication**, but scoped narrowly:
Salmon and TropicalFish need genuinely different packing schemes (Salmon: a single
0/1/2 enum id with an associated bounding-box scale constant; TropicalFish: a 3-field
packed int with independent bit-shift accessors) and Cod needs *none* of this at all — so
a single "one-size-fits-all FishVariant trait" would either be too generic to save code
or too specific to fit both. Instead:

1. A small shared module `pumpkin/src/entity/passive/fish_variant.rs` holding only the
   **mechanically identical** parts:
   - The weighted-random-pick helper (Salmon's 30/50/15 weighted list and, if any other
     species reuses the *shape* of "roll a weighted enum on finalize_spawn", they call
     the same generic `pick_weighted<T>(rng, &[(T, u32)])` utility) — this is a genuine
     no-species-knowledge utility, not fish-specific at all really, and could even live
     in a more general RNG-utils module instead of a fish-specific one. Check for an
     existing weighted-pick helper elsewhere in the codebase (e.g. used by loot tables)
     before adding a new one.
   - A `pack_u8_triple`/`unpack_u8_triple` bit-packing helper (generic `(u16, u8, u8) ->
     i32` and back) that both Salmon-if-it-ever-needed-it and TropicalFish's
     `packVariant`/`getPattern`/`getBaseColor`/`getPatternColor` build on, since the
     packing *shape* (16 bits + 8 bits + 8 bits into an i32) is identical even though the
     field semantics differ.
   - The **bucket-variant-persistence gap**: since both Salmon and TropicalFish need
     "variant survives capture", and that gap is actually in `bucket.rs`'s opaque
     `bucket_entity_data` handling (not per-species), the *fix* belongs in one place
     (`pumpkin/src/item/items/bucket.rs` plus whatever `ItemStack` component storage is
     added) rather than being duplicated per fish. This is the strongest argument for
     "shared", but the shared part is the *bucket* infrastructure, not a fish trait.
2. **Do not** force Cod into any shared variant trait — it has no variant. Keep
   `cod.rs` as just the sound/bucket overrides it already trivially has.
3. Each of `salmon.rs`/`tropical_fish.rs` keeps its own synced-data field, its own
   `defineSynchedData`/NBT read-write, and its own `finalizeSpawn` variant-roll logic —
   these are genuinely different enough (weighted enum vs. 90/10 common-or-random packed
   triple) that forcing a shared "roll variant" trait would just be an indirection layer
   over two unrelated algorithms.

### Target Rust design — Cod
Minimal: verify/confirm the 4 sound overrides and bucket item are already correct (or add
them if genuinely missing — the current stub's goal wiring already looks complete for
Cod specifically, since Cod has no variant/extra state in vanilla either). Low effort,
likely near-parity already modulo whatever `AbstractFish`/`AbstractSchoolingFish` generic
behavior (flopping when out of water, generic school-following) is still missing at the
shared-fish level — that shared level wasn't in this document's scope per the subsystem
list, but flag it if `follow_flock_leader.rs`/flopping isn't already handled generically.

### Target Rust design — Salmon
**Struct field**: `variant: AtomicU8` (0/1/2, default 1 = MEDIUM).

**Synced data**: `TrackedData::VARIANT` (INT — check whether vanilla's `DATA_TYPE` int
accessor id matches the codegen'd `VARIANT` id at line 3819, or whether Salmon actually
needs its own differently-named tracked-data slot; the id existing doesn't guarantee it's
*this* field — verify against the tracked-data JSON's per-entity assignment, not just
that a same-named constant exists).

**on_synced_data_updated**: when the variant changes, recompute `entity_dimension` (the
`refreshDimensions()` reactive behavior) — check whatever hook this codebase's
`send_meta_data`/synced-data-set call site provides for post-update reactions (may need
to just call the recompute inline right after the `.set()` call if there's no generic
"on data updated" hook, which is simpler and arguably fine given only one field needs it).

**finalize_spawn**: weighted pick SMALL(30)/MEDIUM(50)/LARGE(15) — implement as a plain
match on `random_range(0..95)` if no generic weighted-list utility exists yet, rather than
building new weighted-list infrastructure just for this one call site.

**Dimensions**: scale factor 0.5/1.0/1.5 applied via the same `entity_dimension.store`
pattern used elsewhere (slime/phantom/sulfur_cube), driven off the variant.

**Bucket**: `getBucketItemStack` = `SALMON_BUCKET` (already correct via generic
`bucket.rs` mapping); **variant-survives-capture** depends on the bucket-entity-data gap
— note it, don't block this species' core variant behavior on fixing that gap first,
since variant is visible/functional in normal gameplay (rendering, spawning, breeding)
independent of whether a captured-then-released salmon remembers its size.

**NBT**: `type` key, string-or-int matching vanilla's `Variant.CODEC`
(`StringRepresentable.fromEnum`) — check whether this codebase's NBT convention for enums
elsewhere (e.g. any other variant-having mob already ported) uses string names or raw
ints, and match that convention rather than inventing a third scheme.

### Target Rust design — TropicalFish
**Struct field**: `packed_variant: AtomicI32` (default = `KOB` pattern (packed id
`0 | 0<<8 = 0`), `DyeColor::WHITE` (id?) both base/pattern colors — verify `DyeColor`
id-to-`0` mapping for WHITE against `pumpkin_data` before hardcoding the default
constant), plus a non-persisted `is_school: AtomicBool` (default true) mirroring
vanilla's per-instance (not synced, not saved) `isSchool` field, which only affects
`isMaxGroupSizeReached` at runtime.

**Bit-packing helpers** (in the shared module or inline, per the recommendation above):
```rust
fn pack_variant(pattern_packed_id: u16, base: u8, pattern_color: u8) -> i32 {
    i32::from(pattern_packed_id) & 0xFFFF
        | (i32::from(base) & 0xFF) << 16
        | (i32::from(pattern_color) & 0xFF) << 24
}
fn unpack_base_color(packed: i32) -> u8 { ((packed >> 16) & 0xFF) as u8 }
fn unpack_pattern_color(packed: i32) -> u8 { ((packed >> 24) & 0xFF) as u8 }
fn unpack_pattern(packed: i32) -> u16 { (packed & 0xFFFF) as u16 }
```
Port the 12-entry `Pattern` enum (name, `Base` small/large, index) as a Rust enum with a
`packed_id()` method (`base.id() | index << 8`), and the 22-entry `COMMON_VARIANTS` table
verbatim (pattern, base color, pattern color triples) — this table must be copied
**exactly** from source since it defines named/"predefined" fish that players recognize;
do not paraphrase or re-derive it.

**Synced data**: `TrackedData::ID_TYPE_VARIANT` (INT, codegen id at line 1731 — verify
this is indeed TropicalFish's slot and not shared/misattributed, same caution as Salmon's
`VARIANT`).

**finalize_spawn** (the 90/10 split):
```rust
if rng.random::<f32>() < 0.9 {
    let variant = *COMMON_VARIANTS.choose(rng);
    // whole spawn group shares this variant (school group data)
} else {
    is_school = false;
    let variant = fully_random_pattern_basecolor_patterncolor(rng);
}
```
The "whole spawn group shares this variant" half requires whatever group-spawn-data
mechanism this codebase has for other schooling/group mobs (check `FollowFlockLeaderGoal`
or whatever spawn-group-data equivalent already exists for e.g. wolves/villagers) — if no
generic "group spawn data" concept exists yet at all, this may need to fall back to each
fish in a batch rolling independently with the same seed-adjacent call, which is an
acceptable interim approximation but should be flagged as a known deviation, not silently
treated as done.

**getBucketItemStack**: `TROPICAL_FISH_BUCKET` (already correct via generic mapping);
same bucket-entity-data caveat as Salmon, doubly so here since 3 fields must round-trip
instead of 1.

**Sounds/flop sound**: 4 straightforward overrides.

**Spawn rule** (`checkTropicalFishSpawnRules`): biome-tag-gated — needs
`BiomeTags.ALLOWS_TROPICAL_FISH_SPAWNS_AT_ANY_HEIGHT`-equivalent tag lookup (check
`pumpkin_data::tag::Biome` for this specific tag before assuming it needs adding) plus
whatever the generic surface-water-animal spawn check already is for Cod/Salmon.

**NBT**: `Variant` key storing the packed int directly (vanilla's `Variant.CODEC` is
`Codec.INT.xmap(...)`, i.e. literally stores the raw packed int, not a compound) — this
is simpler than Salmon's named-enum codec, port it as a plain `put_int`/`get_int`.

### Files to touch
- Modify: `pumpkin/src/entity/passive/cod.rs` (verify/finish sounds+bucket, likely
  minimal diff).
- Modify: `pumpkin/src/entity/passive/salmon.rs` (add variant field, synced data, NBT,
  weighted finalize_spawn, dimension scaling).
- Modify: `pumpkin/src/entity/passive/tropical_fish.rs` (add packed-variant field, synced
  data, NBT, pattern enum + common-variants table, 90/10 finalize_spawn, biome-gated spawn
  rule).
- New (recommended): `pumpkin/src/entity/passive/fish_variant.rs` for the shared
  bit-packing helper and (if it doesn't already exist elsewhere) a generic weighted-pick
  utility — explicitly **not** a shared "FishVariant" trait implemented by all three
  species, per the recommendation above.
- Out-of-scope-but-blocking for full bucket parity: `pumpkin/src/item/items/bucket.rs` +
  `pumpkin_data`/`ItemStack` component storage, to actually persist Salmon
  size/TropicalFish pattern+colors through a bucket round-trip (currently the comment at
  `bucket.rs:208-213` documents this as unimplemented). Recommend filing this as its own
  ticket/PR rather than folding it into the per-species work, per this repo's PR-hygiene
  rule (one logical fix per PR) — but call it out loudly since two of the four subsystems
  in this document depend on it for full parity.

### Needs wiki/source verification
- Exact `DyeColor` id ordering (used both for `TropicalFish.DEFAULT_VARIANT` and for the
  `COMMON_VARIANTS` table's color references) — port from the decompiled `DyeColor` enum,
  not from memory/wiki color names, since the packed ints depend on exact ordinal values.
- Whether `AbstractSchoolingFish`/`AbstractFish` (not read in this pass) already cover
  generic flee/flop/school behavior identically to what Cod/Salmon/TropicalFish's current
  Rust goal-wiring provides — if the current `AvoidEntityGoal`/`FollowFlockLeaderGoal`
  wiring already matches those superclasses' `registerGoals`, no changes are needed there;
  this document only found species-*specific* gaps (variants, bucket data), not
  necessarily gaps in the shared base-fish goal wiring, which should be double-checked
  against `AbstractFish.java`/`AbstractSchoolingFish.java` before assuming it's already
  correct.

---

## Summary of prerequisite/shared infrastructure gaps surfaced

These affect more than one subsystem above and are worth resolving (or at least
explicitly scoping out) before or alongside the per-species work:

1. **`can_be_leashed` gate** — doesn't exist anywhere; needed for Turtle (`false`),
   touches `Mob`/`EntityBase` + `lead.rs`. Small, low-risk, single shared fix.
2. **Structure-tag lookup on `World`/`ServerLevel`** — needed for Dolphin's treasure goal;
   verify it doesn't already exist elsewhere before assuming it's missing.
3. **Bucket-entity-data / component persistence** — needed for Salmon size and
   TropicalFish pattern/colors to survive a bucket round-trip; already flagged as a known
   gap in existing code comments, affects 2 of the 4 subsystems.
4. **Per-type pathfinding malus / pluggable `MoveControl`** — needed for Turtle's water
   preference and buoyancy; verify whether the navigator already supports this generically
   (used by any other amphibious/aquatic mob already ported) before assuming it's absent.
5. **Group/school spawn data** ("the whole pack shares one randomly-rolled variant") —
   needed for TropicalFish's common-variant sharing across a spawn group; check for an
   existing analog (any other group-spawning mob) before treating this as novel.
