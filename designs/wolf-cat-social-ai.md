# Design: Wolf and Cat social AI (owner-defense, persistent anger, wolf armor, animation state, cat lying/relax, collar recolor, missing cat goals)

Scope: design only, no code changes. Vanilla source referenced from
`/tmp/pumpkin-vanilla-26.2/decompiled/net/minecraft/world/entity/animal/wolf/Wolf.java`
(706 lines) and `.../animal/feline/Cat.java` (677 lines). Current Rust state read from
`pumpkin/src/entity/passive/wolf.rs`, `pumpkin/src/entity/passive/cat.rs`, and
`pumpkin/src/entity/ai/goal/`.

Both files carry a self-documenting gap admission already:

- `wolf.rs:78-81`: "Vanilla priorities 1-3 (OwnerHurtByTarget/OwnerHurtTarget/HurtByTarget[+alert])
  and 4/7 (angry-at-player/hostile-skeleton) target goals aren't ported here."
- `wolf.rs:68`: `// goal_selector.add_goal(6, FollowOwnerGoal::new(1.0, 10.0, 2.0, false));` — commented out.
- `cat.rs:115`: `// goal_selector.add_goal(7, FollowOwnerGoal::new(1.0, 10.0, 5.0, false));` — commented out.
- `mob/zombified_piglin.rs:41-44`: "the `NeutralMob` anger goals ... need per player anger
  state Pumpkin does not track yet" — confirms no shared persistent-anger infra exists,
  independently of this wolf audit.

A key finding up front: **`OwnerHurtByTargetGoal` and `OwnerHurtTargetGoal` already exist**
as fully-implemented files (`pumpkin/src/entity/ai/goal/owner_hurt_by_target.rs`,
`owner_hurt_target.rs`) but are never registered in `wolf.rs`. Likewise `FollowOwnerGoal`
(`ai/goal/follow_owner.rs`) is fully implemented with a 3-argument constructor
(`speed, min_distance, max_distance`), but the commented-out call sites in both wolf.rs and
cat.rs pass 4 arguments — stale call signatures from before the goal was finished, not a
genuine blocker. Re-enabling `FollowOwnerGoal` and wiring the two owner-hurt goals is close
to a drop-in fix once the goal-priority registrations below are added.

---

## 1. Wolf owner-defense/anger goals, persistent anger, wolf armor, shake/animation state

### 1a. Owner-defense and targeting goals (Wolf.java:141-148)

```java
this.targetSelector.addGoal(1, new OwnerHurtByTargetGoal(this));
this.targetSelector.addGoal(2, new OwnerHurtTargetGoal(this));
this.targetSelector.addGoal(3, new HurtByTargetGoal(this).setAlertOthers());
this.targetSelector.addGoal(4, new NearestAttackableTargetGoal<>(this, Player.class, 10, true, false, this::isAngryAt));
this.targetSelector.addGoal(5, new NonTameRandomTargetGoal<>(this, Animal.class, false, PREY_SELECTOR));
this.targetSelector.addGoal(6, new NonTameRandomTargetGoal<>(this, Turtle.class, false, Turtle.BABY_ON_LAND_SELECTOR));
this.targetSelector.addGoal(7, new NearestAttackableTargetGoal<>(this, AbstractSkeleton.class, false));
this.targetSelector.addGoal(8, new ResetUniversalAngerTargetGoal<>(this, true));
```

Current Rust `wolf.rs` only registers the priority-5/6 prey goals (`NonTameRandomTargetGoal`
for sheep/rabbit/fox and turtles). Priorities 1, 2, 3, 4, 7, 8 are entirely absent.

**What to add, mapped onto existing Pumpkin infra:**

| Priority | Vanilla goal | Rust plan |
|---|---|---|
| 1 | `OwnerHurtByTargetGoal` | Register existing `owner_hurt_by_target::OwnerHurtByTargetGoal::new()` — no new code needed. |
| 2 | `OwnerHurtTargetGoal` | Register existing `owner_hurt_target::OwnerHurtTargetGoal::new()` — no new code needed. |
| 3 | `HurtByTargetGoal().setAlertOthers()` | `ai/goal/revenge.rs`'s `RevengeGoal` is Pumpkin's port of `HurtByTargetGoal`/`RevengeGoal` semantics and **already implements the "alert others of my type nearby" broadcast** (see `revenge.rs` `start()`: it iterates `get_nearby_entities` filtered by matching `entity_type` and assigns them the same target — this is exactly vanilla's `setAlertOthers()` behavior). So this priority is `RevengeGoal::new(true)` (check_visibility=true, matching `ZombifiedPiglinEntity`'s `RevengeGoal::new(true)` precedent) — no new goal type needed, just registration. |
| 4 | `NearestAttackableTargetGoal<Player>(..., this::isAngryAt)` | New goal or reuse of `ActiveTargetGoal` (`ai/goal/active_target.rs`) parameterized with `target_type = &EntityType::PLAYER`, `reciprocal_chance = 10`, `check_visibility = true`, `check_can_navigate = false`, and a predicate closure `is_angry_at(player, world)`. `ActiveTargetGoal::new` already accepts an `F: Fn(Arc<LivingEntity>, Arc<World>) -> Fut` predicate, so this is a call-site addition, not new goal-type work — **provided** the persistent-anger state (1b below) exists to answer `is_angry_at`. |
| 5, 6 | prey / baby-turtle | Already implemented in current `wolf.rs`. |
| 7 | `NearestAttackableTargetGoal<AbstractSkeleton>` | Same `ActiveTargetGoal` shape, `target_type = &EntityType::SKELETON` (also `STRAY`/`WITHER_SKELETON` — vanilla's `AbstractSkeleton` covers all three concrete skeleton subclasses; Pumpkin has no class hierarchy so this needs either 3 separate `ActiveTargetGoal` registrations at priority 7, or `ActiveTargetGoal` extended to accept a slice of target types the way `NonTameRandomTargetGoal` already does for `WOLF_PREY_TYPES`/`TURTLE_TYPES`). Recommend extending `ActiveTargetGoal` with a `target_types: &'static [&'static EntityType]` field (mirroring `non_tame_random_target.rs`'s existing pattern) rather than adding three goals — this is shared infra useful beyond wolves. |
| 8 | `ResetUniversalAngerTargetGoal<>(this, true)` | No Rust equivalent exists. This is a small per-tick goal: while the mob is "universally angry" (target is null but anger timer > 0, i.e. angry at an unresolvable/every-player target) it re-picks the nearest valid player target each tick, and separately decrements/clears the anger timer when it runs out. New file `ai/goal/reset_universal_anger_target.rs`; depends entirely on 1b's persistent-anger trait to read `persistent_anger_target`/`persistent_anger_end_time`. |

`wantsToAttack` (Wolf.java:637-648) governs whether `mob_interact`-driven targeting actually
attacks (never attacks Creeper/Ghast/ArmorStand; never attacks another tamed wolf owned by
the same owner; players are filtered by `canHarmPlayer`, i.e. PvP/team rules; never attacks a
tamed horse or other tamed animal). This corresponds to `Mob::can_attack_with_owner` in
`mob/mod.rs:627`, currently a stub returning `false` unconditionally
(`fn can_attack_with_owner(&self, _target: &dyn EntityBase, _owner: &dyn EntityBase) -> bool { false }`
— confirm exact stub body before editing). `WolfEntity` needs to override this per vanilla's
`wantsToAttack`, since `OwnerHurtByTargetGoal`/`OwnerHurtTargetGoal` both call
`mob.can_attack_with_owner(...)` as their final gate (see `owner_hurt_by_target.rs:58` and
`owner_hurt_target.rs:57`) — right now those goals are unusable for wolves even once
registered, because the default stub always vetoes the attack.

### 1b. Persistent anger system — proposed as SHARED infrastructure

Vanilla's `NeutralMob` interface (implemented by `Wolf`, `ZombifiedPiglin`, `Bee`, `Dolphin`,
`PolarBear`, others) provides:

```java
long getPersistentAngerEndTime();
void setPersistentAngerEndTime(long endTime);
void startPersistentAngerTimer();  // default: setPersistentAngerEndTime(currentTime + sample())
EntityReference<LivingEntity> getPersistentAngerTarget();
void setPersistentAngerTarget(EntityReference<LivingEntity> target);
default boolean isAngry() { return getPersistentAngerEndTime() > 0; }  // sign differs per mob; Wolf compares to level time
```

with `updatePersistentAnger(ServerLevel, boolean mobRequiresTameOwnerToAttack)` — a default
method that: decrements/clears the timer once it reaches the current game time, resolves
`persistentAngerTarget` from its `EntityReference` (which itself may need re-resolution if
the target entity reloaded under a new object identity), and calls
`startPersistentAngerTimer()`/clears state as appropriate. Wolf's override
(`Wolf.java:117`, `PERSISTENT_ANGER_TIME = TimeUtil.rangeOfSeconds(20, 39)`) samples 20–39
seconds (400–780 ticks) when a fresh anger timer starts, matching `ResetUniversalAngerTargetGoal`
and the target-selector priority-4 goal above.

**This is exactly the gap `mob/zombified_piglin.rs`'s own comment calls out**
("need per player anger state Pumpkin does not track yet") — so this should be designed
once as shared infra, not duplicated per-mob. Proposed shape, added to `mob/mod.rs`
alongside the existing `owner`/`is_tamed`/`set_owner` fields on `MobEntity` (or as a
separate optional component if not every mob needs it — Wolf and ZombifiedPiglin do,
most other `Mob` impls don't):

```rust
// New: mob/persistent_anger.rs
pub struct PersistentAnger {
    /// Ticks (world time) at which anger ends; 0 or absent = not angry.
    /// Mirrors NeutralMob.getPersistentAngerEndTime/setPersistentAngerEndTime.
    pub anger_end_time: AtomicI64,
    /// The specific entity this mob is angry at, by UUID (resolved lazily via
    /// World::get_entity_by_uuid — this method already exists in world/mod.rs:4235,
    /// so no new world-lookup infra is needed).
    pub anger_target: AtomicCell<Option<Uuid>>,
}

impl PersistentAnger {
    pub fn is_angry(&self) -> bool { self.anger_end_time.load(Relaxed) > 0 }
    pub fn start_timer(&self, world_time: i64, random: &mut impl Rng, range: RangeInclusive<i32>) { ... }
    /// Port of NeutralMob.updatePersistentAnger — called once per tick from aiStep-equivalent.
    pub fn update(&self, world: &World, world_time: i64, requires_owner_to_attack: bool) { ... }
}
```

Every mob wiring this in (Wolf now, ZombifiedPiglin later) adds a `PersistentAnger` field to
its entity struct, calls `.update(...)` from its `mob_tick`, and implements two new
default-able `Mob` trait methods (`fn persistent_anger(&self) -> Option<&PersistentAnger> { None }`
returning `Some(&self.anger)` for wolves/piglins) so that shared goals
(`ActiveTargetGoal`'s `is_angry_at` predicate, `ResetUniversalAngerTargetGoal`) can call
through the trait rather than downcasting to concrete types.

NBT persistence: vanilla's `NeutralMob.addPersistentAngerSaveData`/`readPersistentAngerSaveData`
write `AngerTime` (ticks remaining, not absolute end-time — converted at load) and
`AngryAt` (UUID). Pumpkin's `NBTStorage` trait (see `WolfEntity::write_nbt`/`read_nbt_non_mut`
in `wolf.rs:117-166` for the existing pattern) needs the same two keys added to `WolfEntity`'s
NBT read/write, converting between the stored "ticks remaining" and the in-memory absolute
`anger_end_time` the same way vanilla does (`endTime - currentTime` on write, `currentTime +
storedTicks` on read).

Wiki/source verification needed: confirm the exact `AngerTime`/`AngryAt` NBT key names and
whether Java 26.2's `ValueInput`/`ValueOutput` codec changed the tag format from older
snapshots (this decompile is from a very recent Java client based on the `ValueInput`/
`ValueOutput`/`DataComponentGetter` APIs present, which are notably newer that classic
`CompoundTag` APIs — do not assume older wiki NBT documentation for `Wolf`/`AngerTime`
still applies verbatim; check the decompiled source's exact key strings, which were not
directly visible in the `Wolf.java` file itself since `addPersistentAngerSaveData` lives in
the `NeutralMob` interface, not printed above — read `NeutralMob.java` before implementing).

### 1c. Wolf armor mechanics (Wolf.java:408-448)

- `actuallyHurt` (409-424): if `canArmorAbsorb` (body armor is `minecraft:wolf_armor` and the
  damage source isn't tagged `bypasses_wolf_armor`), damage is diverted into the armor item's
  durability via `hurtAndBreak` instead of the wolf's health; crossing a `Crackiness` threshold
  (crackiness stages computed from `damageBefore`/`maxDamage` vs. after) plays
  `WOLF_ARMOR_CRACK` and spawns armadillo-scute item particles.
- `hurtArmor` (441-443) routes to `doHurtEquipment(damageSource, damage, EquipmentSlot.BODY)`.
- `canShearEquipment` (446-448): only the owner may shear (remove) body armor.
- `getHurtSound` (290-292): plays `WOLF_ARMOR_DAMAGE` instead of the normal hurt sound while
  armored.
- Armor repair via right-click with a valid repair item while sitting (`mobInteract:466-477`):
  `repairUnit = maxDamage * 0.125F` (the `ARMOR_REPAIR_UNIT` constant), consumes 1 item, plays
  `WOLF_ARMOR_REPAIR`.
- Equipping armor via right-click (`mobInteract:460-464`): requires tamed, owned, non-baby,
  no armor already worn, item valid for `EquipmentSlot.BODY`.

None of this exists in Pumpkin's `wolf.rs` — there's no armor slot handling, no `Crackiness`
concept, no `hurtArmor`/`canShearEquipment` overrides. This depends on whatever
equipment-slot/durability infrastructure other equippable mobs already use in Pumpkin (check
`pumpkin/src/entity/living.rs` for `EquipmentSlot`, `hurt_and_break`, `is_equippable_in_slot`
— not audited in this pass; a build/fix worker should grep for an existing horse-armor or
llama-carpet equivalent, since horses/llamas already have body-slot equipment in vanilla and
may already have partial Rust support to extend rather than invent from scratch).

New file: extend `wolf.rs` with armor state (or reuse `LivingEntity`'s existing equipment
storage if `EquipmentSlot::Body` is already modeled) plus a `Crackiness`-equivalent threshold
enum (`Crackiness.WOLF_ARMOR` in vanilla is a small table of damage-fraction thresholds —
verify exact fractions from `Crackiness.java` before implementing, not guessed).

### 1d. Shake/tail/head animation state (Wolf.java:111-116, 305-397, 518-529)

Fields: `interestedAngle`/`interestedAngleO` (begging head-tilt lerp), `isWet`, `isShaking`,
`shakeAnim`/`shakeAnimO`.

- `aiStep` (305-317): sets `isShaking=true` and broadcasts entity event byte `8` (start shake)
  when wet, not already shaking, not pathfinding, and on ground.
- `tick` (320-364): lerps `interestedAngle` toward 1 or 0 by factor 0.4/tick depending on
  `isInterested()` (this is the wolf beg/food-interest head-tilt, driven by `BegGoal` setting
  `DATA_INTERESTED_ID` — Pumpkin already has `ai/goal/beg.rs` registered; need to confirm it
  sets the equivalent tracked-data flag). Tracks water/rain state; on shake, advances
  `shakeAnim` by 0.05/tick, plays `WOLF_SHAKE` sound at animation start, ends the cycle at
  `shakeAnimO >= 2.0`, and while `shakeAnim > 0.4` spawns splash particles scaled by
  `sin((shakeAnim-0.4)*PI)*7`.
- `handleEntityEvent` (518-529): byte `8` → start shake client-side, byte `56` → cancel shake
  (this pairs with server broadcasting byte `56` from `tick` when the wolf re-enters water
  mid-shake).
- `getShakeAnim`, `getHeadRollAngle`, `getWetShade`, `getTailAngle` (381-541): pure
  render-side accessors — `getTailAngle` in particular encodes tail droop as a function of
  damage ratio when tamed (`(0.55 - damageRatio*0.4) * PI`), a fixed angry angle
  (`1.5393804` rad), or a fixed default (`PI/5`) when wild.

This is client-visible animation state. Pumpkin's `WolfEntity` currently tracks none of it —
no `is_wet`/`is_shaking`/`shake_anim`/`interested_angle` fields at all. Since this is purely
animation/particle/sound state (not gameplay-affecting beyond the sound cues), it's lower
priority than the targeting/anger/armor systems, but it is observable to players (the
"wolf shakes off water" visual and sound is a well-known vanilla behavior). It requires:
- New `AtomicU8`/`AtomicBool`-style fields on `WolfEntity` for `is_wet`, `is_shaking`,
  `shake_anim` (as a fixed-point or `AtomicU32`-as-bits float), `interested_angle`.
- A per-tick hook (Pumpkin's `mob_tick` or equivalent on `Mob`/`EntityBase`) replicating the
  state machine above.
- Entity-event byte broadcasting (`world.broadcastEntityEvent`-equivalent — check whether
  Pumpkin's `World`/`Entity` already exposes a generic "broadcast entity status byte" call,
  since bytes 6/7/8/56 are already vanilla-standard tame/shake events other entities use too).
- Splash-particle spawning tied to `shakeAnim` — reuse whatever particle-spawn helper
  `mob_interact`'s existing `Particle::Heart`/`Particle::Smoke` calls use (`wolf.rs:213,215`).

---

## 2. Cat lying/relax-state, collar recolor, sound variants, missing goals

### 2a. Lying/relax-state data model (Cat.java:78-98, 179-232, 300-365)

Tracked data: `IS_LYING` (bool), `RELAX_STATE_ONE` (bool) — **both already exist in
Pumpkin's generated tracked-data table** (`pumpkin-data/src/generated/tracked_data.rs`:
`IS_LYING` at line 1935, `RELAX_STATE_ONE` at line 2703), so no codegen/data work is needed,
only wiring them into `CatEntity`.

Non-synced float state for client-side lerp/animation: `lieDownAmount`/`lieDownAmountO`,
`lieDownAmountTail`/`lieDownAmountOTail`, `relaxStateOneAmount`/`relaxStateOneAmountO`,
plus the boolean `isLyingOnTopOfSleepingPlayer` (recomputed each tick by scanning
`getEntitiesOfClass(Player.class, AABB(catPos).inflate(2,2,2))` for a sleeping player).

Per-tick update (`handleLieDown`, Cat.java:310-328, called from `tick()`):
- Every 5 ticks while lying or in relax-state-one, plays the purr sound at
  `0.6 + 0.4*(rand-rand)` volume.
- `updateLieDownAmount`: approaches 1.0 by +0.15 (body) / +0.08 (tail) per tick while lying,
  or 0.0 by −0.22 / −0.13 per tick otherwise (asymmetric ease-in/ease-out, not a symmetric
  lerp — implement as exactly these additive step constants, not `Mth.lerp`).
- `updateRelaxStateOneAmount`: same shape, +0.1 / −0.13.

None of this exists in current `cat.rs` — no `IS_LYING`/`RELAX_STATE_ONE` fields, no
`lie_down_amount` tracking, no `handle_lie_down` tick hook. New `CatEntity` fields:

```rust
is_lying: AtomicBool,
relax_state_one: AtomicBool,
lie_down_amount: AtomicU32,       // bit-pattern f32, or switch to a Mutex<f32> pair
lie_down_amount_tail: AtomicU32,
relax_state_one_amount: AtomicU32,
is_lying_on_top_of_sleeping_player: AtomicBool,
```
(follow whatever atomic-float convention the codebase already uses elsewhere — grep for an
existing `AtomicU32`-as-`f32::to_bits` pattern before inventing a new one; if none exists,
a `Mutex<(f32, f32)>` per pair, matching the `_O` vanilla naming for "previous tick" values
used for render interpolation, is the simpler idiom to match `wolf.rs`'s existing
`AtomicU8` style).

### 2b. Collar color get/set + dye recoloring (Cat.java:80, 195-201, 408-422)

Directly analogous to Wolf's existing `collar_color: AtomicU8` + `set_collar_color` pattern
in `wolf.rs:44,104-114` — this is a near copy-paste port. Vanilla's cat recolor path
(`mobInteract:410-422`) requires: tamed, owned by the interacting player, item tagged
`ItemTags.CAT_COLLAR_DYES`, and the resolved `DyeColor` differs from current — then
`setCollarColor`, consume 1, `setPersistenceRequired()` (Pumpkin equivalent: whatever marks
a tamed mob as no-despawn — check if `WolfEntity`/`MobEntity` already has this from taming,
since `removeWhenFarAway` in Cat.java:464-466 is despawn-timer logic that also needs a look
if not already ported). NBT key is the same `"CollarColor"` legacy-dye-id byte vanilla uses
for wolves, so `CatEntity::write_nbt`/`read_nbt_non_mut` can copy `wolf.rs`'s existing
`CollarColor` NBT block verbatim.

Note a subtlety: unlike Wolf, Cat's `DEFAULT_COLLAR_COLOR` is also `DyeColor.RED`
(Cat.java:85) — same default, so the constant can be shared rather than duplicated, another
small argument for a shared "tameable collar" helper (see closing section).

### 2c. Sound variants: hiss/purr/purreow ambient logic (Cat.java:254-284, 296-298, 355-361)

`getAmbientSound` (255-265): tamed+in-love → `purrSound`; tamed+not-in-love → 1-in-4 chance
`purreowSound` else normal `ambientSound`; untamed → `strayAmbientSound`.
`ambientSoundInterval` fixed at 120 ticks (line 269).
`hiss()` (272-274): public method other code paths call (e.g. when scared) — plays
`hissSound`. Need to find/add whatever caller vanilla has for this (likely from a
scare/avoid-goal interaction — check `CatAvoidEntityGoal`/scare triggers, and Ocelot's
analogous hiss trigger for a second data point, since Ocelot shares `CatSoundVariant`-style
sound-set plumbing).
`playEatingSound` (296-298) uses `eatSound` from the sound set.
`handleLieDown`'s purr-while-lying (310-313) reuses `purrSound` independent of the ambient
path.

Pumpkin's `CatEntity` currently has **no sound-variant plumbing at all** — no
`CatSoundVariant` equivalent, no `getAmbientSound` override, silence on all these paths.
This needs:
- A `CatSoundVariant`/`CatSoundSet`-equivalent data table (check whether
  `pumpkin-data` already generates cat/wolf sound-variant tables the way it generates
  `CAT_VARIANT`/`WOLF_VARIANT_ID` tracked-data enums — `WolfSoundVariants`/`CatSoundVariants`
  are registry-backed in vanilla 26.2, meaning there may already be a data-driven registry
  Pumpkin's codegen pulls from; check `pumpkin-data/src/generated/` for anything named
  `*sound_variant*` before hand-rolling one).
- A `sound_variant: AtomicU8` field on `CatEntity` (mirrors `WolfEntity::variant`), picked
  randomly at spawn the way `WolfEntity`/`CatEntity` already pick their appearance variant.
- Override of whatever `Mob`/`LivingEntity` ambient-sound hook Pumpkin uses (need to locate
  the trait method — `wolf.rs` doesn't currently override an ambient-sound hook either, so
  this is new plumbing shared by both mobs, another shared-infra candidate).

### 2d. Missing goals

| Vanilla goal | Priority | Status in Pumpkin |
|---|---|---|
| `CatLieOnBedGoal` | 5 | Missing entirely. |
| `CatSitOnBlockGoal` | 7 | Missing entirely. |
| `CatRelaxOnOwnerGoal` (private inner class, Cat.java:519-646) | 3 | Missing entirely — this is the most complex of the four, ~130 lines including the sleeping-owner detection, bed-space-occupied check, morning-gift loot drop, and the tick-based `IS_LYING`/`RELAX_STATE_ONE` transition (16-tick threshold before transitioning from "settling" to "lying," Cat.java:632-640). |
| `OcelotAttackGoal` | 9 | Missing (shared with actual Ocelot mob — check `ai/goal/ocelot_attack.rs`, which **already exists** per the goal directory listing; likely already used by `passive/ocelot.rs` — this is a registration-only gap for Cat, reusing the existing goal file, not new goal-authoring work). |
| `LeapAtTargetGoal` | 8 | No dedicated file found in `ai/goal/` — check whether this is folded into `melee_attack.rs` or genuinely absent; Wolf.java also uses `LeapAtTargetGoal` (priority 4) so this is shared infra needed by both mobs regardless of Cat. |
| `Cat.CatAvoidEntityGoal<Player>` | 4 (only while untamed, added/removed via `reassessTameGoals`) | Missing. Needs `avoid_entity.rs`'s existing `AvoidEntityGoal` (used by Wolf's own `WolfAvoidEntityGoal<Llama>` inner class) parameterized for `Player` + `EntitySelector.NO_CREATIVE_OR_SPECTATOR` (i.e. don't flee creative/spectator players), plus the dynamic add/remove-on-tame-state-change behavior `reassessTameGoals` implements (Cat.java:474-483) — this dynamic goal-list mutation on taming is a pattern Pumpkin's goal selector needs to support if it doesn't already (check `goal_selector.rs`'s `remove_goal` — referenced in `SitGoal`'s sibling design nowhere obviously, needs to be confirmed present). |
| `FollowOwnerGoal` | 6 | **Already implemented and functional** (`ai/goal/follow_owner.rs`) — just commented out with a stale 4-arg call. Fix is: `goal_selector.add_goal(6, FollowOwnerGoal::new(1.0, 10.0, 5.0));` (3 args, matching the real signature). |

`CatTemptGoal` (Cat.java:648-676, the private subclass with `selectedPlayer` scare-suppression
logic) is a thin override of vanilla `TemptGoal` that Pumpkin's existing `tempt.rs` doesn't
appear to special-case for cats (current `cat.rs` uses a bare `TemptGoal::new(0.6,
TEMPT_ITEMS)`), so the scare-immunity-to-one-remembered-player nuance is also currently
unported — worth a line item even though it's small.

---

## 3. Shared infrastructure to design once, not per-mob

1. **`PersistentAnger` (NeutralMob equivalent)** — see §1b. Needed now for Wolf, and already
   flagged as needed for `ZombifiedPiglinEntity` by that file's own comment. Should live as
   a standalone struct (`mob/persistent_anger.rs`) composed into any `Mob` impl that needs
   it, with two new optional `Mob` trait methods to expose it polymorphically to shared
   goals (`ActiveTargetGoal`'s anger predicate, a new `ResetUniversalAngerTargetGoal`).
   Do **not** conflate this with `mob/warden_anger.rs`'s `AngerManagement` — that's a
   different vanilla system (`Warden`'s multi-suspect `AngerLevel`/`AngerManagement`) with
   different semantics (accumulating anger score vs. a single end-time + single target);
   they should not share a struct even though the domain both "anger" sounds similar.

2. **Tameable collar-color helper** — Wolf and Cat both have byte-identical
   `DATA_COLLAR_COLOR` tracked data, identical `DyeColor.RED` default, identical
   `"CollarColor"` NBT key/legacy-id codec, and near-identical `mobInteract` recolor gating
   (tamed + owned + dye-tag item + color changed). Wolf's `set_collar_color` in `wolf.rs:104`
   is a fine template; extracting a small shared helper (e.g. a `CollaredPet` mixin struct
   with `collar_color: AtomicU8`, `set_collar_color`, and shared NBT read/write functions)
   avoids duplicating this a third time when Cat is ported, and a fourth time if any future
   tameable (there are only these two in vanilla today, but the duplication is already 2x
   with zero abstraction).

3. **Ambient-sound-variant plumbing** — both Wolf and Cat have registry-backed
   `WolfSoundVariant`/`CatSoundVariant` holders with baby/adult sound sets and near-identical
   `getSoundSet()` (baby vs adult dispatch) shape. If `pumpkin-data` doesn't already generate
   these tables, this is a second shared piece of infra (a generic `SoundVariantSet<T>` with
   `.babySounds()`/`.adultSounds()` accessors) worth building once rather than twice.

4. **Dynamic goal-list mutation on state change** — Cat's `reassessTameGoals` adds/removes
   `CatAvoidEntityGoal` from the goal selector at runtime when tame state flips. Confirm
   `goal_selector.rs` exposes a `remove_goal`/re-`add_goal` pair usable from outside
   construction time (i.e. from a `set_tame`-equivalent call), since this pattern will
   recur for other tameables' avoid-goals.

5. **`ActiveTargetGoal` multi-type support** — needed for Wolf's `AbstractSkeleton` target
   (regular/stray/wither skeleton) at priority 7; `NonTameRandomTargetGoal` already has this
   shape (`WOLF_PREY_TYPES`, `TURTLE_TYPES` slices) so extending `ActiveTargetGoal` similarly
   is a small, mechanical change to an existing struct rather than new design.

---

## Files to create or modify

**New files:**
- `pumpkin/src/entity/mob/persistent_anger.rs` — shared `PersistentAnger` struct (§1b, §3.1).
- `pumpkin/src/entity/ai/goal/reset_universal_anger_target.rs` — port of
  `ResetUniversalAngerTargetGoal` (§1a priority 8).
- Possibly `pumpkin/src/entity/mob/collared_pet.rs` — shared collar-color helper (§3.2), if
  the team wants to de-duplicate rather than copy `wolf.rs`'s pattern into `cat.rs` verbatim.
- Possibly `pumpkin/src/entity/mob/sound_variant.rs` — shared ambient sound-variant
  plumbing (§3.3), pending confirmation of what `pumpkin-data` already generates.
- `pumpkin/src/entity/ai/goal/cat_lie_on_bed.rs` — `CatLieOnBedGoal` port (§2d).
- `pumpkin/src/entity/ai/goal/cat_sit_on_block.rs` — `CatSitOnBlockGoal` port (§2d).
- `pumpkin/src/entity/ai/goal/cat_relax_on_owner.rs` — `CatRelaxOnOwnerGoal` port (§2d),
  the largest single new goal in this batch.
- Possibly `pumpkin/src/entity/ai/goal/leap_at_target.rs` if genuinely absent (shared by
  Wolf and Cat both) — verify first.

**Modified files:**
- `pumpkin/src/entity/passive/wolf.rs` — register priorities 1/2/3/4/7/8 on the target
  selector; uncomment/fix `FollowOwnerGoal` at priority 6 with the correct 3-arg call; add
  `PersistentAnger` field + NBT persistence (`AngerTime`/`AngryAt` keys — verify exact names
  from `NeutralMob.java`, not this file, before implementing); add wolf-armor fields/overrides
  (§1c); add shake/animation-state fields and per-tick state machine (§1d).
- `pumpkin/src/entity/passive/cat.rs` — uncomment/fix `FollowOwnerGoal` at priority 7 (wait:
  vanilla priority 6, double-check against the registerGoals listing above before wiring);
  register `CatLieOnBedGoal`/`CatSitOnBlockGoal`/`CatRelaxOnOwnerGoal`/`OcelotAttackGoal`/
  `LeapAtTargetGoal`/`CatAvoidEntityGoal<Player>`; add `IS_LYING`/`RELAX_STATE_ONE` tracked
  data + lie-down/relax float state + per-tick `handle_lie_down`; add collar color field +
  NBT + recolor `mob_interact` branch; add sound-variant field + ambient sound override.
- `pumpkin/src/entity/mob/mod.rs` — flesh out `can_attack_with_owner` stub with vanilla's
  `wantsToAttack` logic (currently an always-`false` default, per grep above — confirm exact
  stub body before editing since only the signature was located in this pass, not the body
  text); add `fn persistent_anger(&self) -> Option<&PersistentAnger> { None }` default method
  if the shared-trait approach (§3.1) is adopted; confirm/extend `remove_goal` support in
  the goal selector if needed for §3.4.
- `pumpkin/src/entity/ai/goal/active_target.rs` — extend to accept multiple target types
  (§3.5), needed for the skeleton-family target at Wolf priority 7.
- `pumpkin/src/entity/ai/goal/mod.rs` — add `pub mod` lines for each new goal file.
- `pumpkin/src/entity/ai/goal/tempt.rs` (or a cat-specific subclass file) — add the
  `CatTemptGoal` scare-immunity-to-remembered-player override if pursued.
- `pumpkin/src/entity/mob/zombified_piglin.rs` — once `PersistentAnger` exists, this is the
  natural second consumer (out of scope for this specific wolf/cat design doc's file list,
  but flagged since the shared-infra decision directly unblocks it).

## Needs wiki/source verification before implementation (do not guess from memory)

- Exact `NeutralMob.addPersistentAngerSaveData`/`readPersistentAngerSaveData` NBT key names
  and whether the "ticks remaining vs. absolute end time" conversion changed in this
  decompile's newer `ValueInput`/`ValueOutput` codec API vs. older wiki documentation.
- `Crackiness.WOLF_ARMOR`'s exact damage-fraction thresholds (referenced but not printed by
  `Wolf.java` itself — lives in `Crackiness.java`).
- Whether `pumpkin-data` already generates `WolfSoundVariant`/`CatSoundVariant` registry
  tables (grep for `*sound_variant*` in `pumpkin-data/src/generated/` before assuming absent).
- The precise vanilla priority number for Cat's `FollowOwnerGoal` — this doc's §2d table
  says priority 6 per the `registerGoals` listing at the top of §2, but double-check against
  the exact `Cat.java:113` line (`this.goalSelector.addGoal(6, new FollowOwnerGoal(...))`)
  before wiring, since a transcription slip here would silently reorder cat AI priority
  relative to `CatSitOnBlockGoal` (7) and `LeapAtTargetGoal` (8).
- `EntitySelector.NO_CREATIVE_OR_SPECTATOR`'s exact semantics for the `CatAvoidEntityGoal`
  port, to confirm it's just "not creative and not spectator" and not something narrower.
