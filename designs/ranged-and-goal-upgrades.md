# Design: RangedBowAttackGoal, RangedCrossbowAttackGoal, MeleeAttackGoal, TemptGoal, FollowOwnerGoal

Scope: bring five AI goals from flat-cooldown/partial approximations to vanilla-accurate
state machines. This document describes target design only; no source under
`pumpkin/`, `pumpkin-data/`, `pumpkin-world/`, or `pumpkin-inventory/` was modified while
writing it.

Vanilla source: `/tmp/pumpkin-vanilla-26.2/decompiled/net/minecraft/world/entity/ai/goal/`.
Current Rust: `/home/eshanki/Pumpkin/pumpkin/src/entity/ai/goal/`.

Shared infrastructure findings that apply to more than one goal:

- `Goal::can_start`/`should_continue`/`tick` are already `async fn` (return
  `Pin<Box<dyn Future>>`), so anything vanilla does synchronously in `canUse()` — including
  calling into the pathfinder — is not blocked by Rust's sync/async split. The blocker,
  where one exists, is that the underlying method isn't public yet, not that the call
  can't be made from a `Goal`.
- `crate::entity::ai::pathfinder::Navigator` (`pumpkin/src/entity/ai/pathfinder/mod.rs`)
  has a private `async fn compute_path(&mut self, entity: &LivingEntity, destination:
  Vector3<f64>) -> Option<Path>` (line ~139) that already implements vanilla's A*
  algorithm end-to-end (open/closed sets, follow-range cutoff, malus). It is only reachable
  today through `set_progress` + the per-tick `tick()` drive loop; there is no equivalent of
  Java's `createPath(target, 0)` that a goal can call once from `can_start` and get a `Path`
  back synchronously. Exposing a `pub async fn create_path(...)` wrapper is a small,
  self-contained addition (see MeleeAttackGoal below) — this is not missing pathfinding
  capability, just a missing public entry point.
- `Navigator::set_pathfinding_malus` exists and inserts into a private
  `path_type_overrides: HashMap<PathType, f32>`, but there is no getter, so a caller cannot
  read back the previous malus before overwriting it (needed by FollowOwnerGoal). Also
  self-contained to add.
- `pumpkin_data::attributes::Attributes::TEMPT_RANGE` already exists as a registered
  attribute (`pumpkin-data/src/generated/attributes.rs:201`) and is in the default
  attribute list. TemptGoal's hardcoded `10.0` is a Rust-side omission, not a missing
  data-registration prerequisite as the census guess suggested — confirmed by reading the
  generated file directly.
- Item-use state (`isUsingItem`/`startUsingItem`/`stopUsingItem`/`getTicksUsingItem`)
  *does* exist generically on `LivingEntity` (`pumpkin/src/entity/living.rs`): fields
  `item_use_time: AtomicI32`, `active_hand: Mutex<Option<Hand>>`, `item_in_use: Mutex<Option<ItemStack>>`,
  methods `set_active_hand`/`clear_active_hand` (lines 323–382), and a per-tick countdown at
  line ~2977 that fires when `item_use_time` reaches 0. Since `MobEntity` embeds
  `LivingEntity`, mobs already have this state for free.
- What mobs do *not* have: the release/consume hook that turns "use time expired" into
  "shoot an arrow". `ItemBehaviour::on_stopped_using` (`pumpkin/src/item/mod.rs`) and its
  dispatcher `ItemRegistry::on_stopped_using` (`pumpkin/src/item/registry.rs:52`) are typed
  to take `&Player`, and are only invoked from the two client packet handlers
  (`net/java/play.rs:2306`, `net/bedrock/play.rs:888`) — i.e. only ever in response to a
  player releasing the use-item button. `BowItem::release_bow`/`fire_arrow`
  (`pumpkin/src/item/items/bow.rs`) and `CrossbowItem::fire_projectiles`
  (`pumpkin/src/item/items/crossbow.rs`) are likewise `&Player`-only: they read from
  `player.inventory()`, call `player.find_arrow()`, `player.consume_arrow()`, etc. This is
  the real, concrete blocker for both ranged-attack goals: there is no mob-facing "release
  the drawn bow/crossbow and spawn the projectile(s), honoring `ChargedProjectiles`/enchantments"
  entry point. The two options are (a) generalize these two item behaviors to operate over
  an entity-agnostic capability instead of `&Player`, or (b) keep the goals' current
  approach of duplicating a simplified shoot path against `dyn Mob`. Given the size of the
  generalization (arrow slot selection, ammo consumption, and item damage are all
  inventory-specific concepts that don't exist for a mob), recommend (b) for now, but reuse
  the *charge-duration* and *charged-projectile-reading* logic where it's not
  inventory-coupled (see RangedCrossbowAttackGoal below) rather than re-deriving it.

---

## 1. RangedBowAttackGoal

**Vanilla**: `RangedBowAttackGoal.java` (146 lines). Constructor takes
`(mob, speedModifier, attackIntervalMin, attackRadius)`. Fields: `attackTime = -1`,
`seeTime`, `strafingClockwise`, `strafingBackwards`, `strafingTime = -1`.

- `canUse()` (37): target non-null **and** `isHoldingBow()` (`mob.isHolding(Items.BOW)`).
- `canContinueToUse()` (45): `(canUse() || !navigation.isDone()) && isHoldingBow()`.
- `stop()` (56): clears aggressive, `seeTime = 0`, `attackTime = -1`, **`stopUsingItem()`**.
- `tick()` (70–137): the hysteresis/strafing/draw state machine:
  - line-of-sight flip resets `seeTime` to 0, then increments/decrements it every tick.
  - if in range (`targetDistSqr <= attackRadiusSqr`) and `seeTime >= 20`: stop navigating,
    increment `strafingTime`; else navigate to target, reset `strafingTime = -1`.
  - every 20 strafing ticks, 30% chance each to flip `strafingClockwise` /
    `strafingBackwards`; distance thresholds (`0.75x`/`0.25x` of `attackRadiusSqr`) force
    backing off or closing in.
  - while strafing (`strafingTime > -1`): `moveControl.strafe(...)` plus `lookAt` (not
    `lookControl.setLookAt`); otherwise plain `lookControl.setLookAt`.
  - if `mob.isUsingItem()`: abort draw if LOS lost and `seeTime < -60`; else if
    `getTicksUsingItem() >= 20`, `stopUsingItem()` + `performRangedAttack(target,
    BowItem.getPowerForTime(pullTime))` + reset `attackTime`.
  - else: `if (--attackTime <= 0 && seeTime >= -60) startUsingItem(...)`.

**Current Rust** (`ranged_bow_attack.rs`): the doc comment on the struct (lines 20–25)
already says it "follows vanilla's timing while Pumpkin does not yet model item-use
state." Concretely: `can_start`/`should_continue` only check that the target is alive (no
`isHoldingBow` gate at all — a bow-less skeleton with a target would still run this goal
if selected); `tick` has one scalar `attack_cooldown` counter, no `seeTime`, no strafing, no
draw-time gating — it shoots immediately once in range and off cooldown with LOS, then
resets a flat interval. `shoot()` builds the arrow itself rather than deferring to
`BowItem`.

**Target design**:

- Struct gains: `speed_modifier: f64` (missing entirely today — vanilla's `moveTo(target,
  speedModifier)` argument is hardcoded to `1.0` in the current `set_progress` call at line
  199), `see_time: i32`, `strafing_clockwise: bool`, `strafing_backwards: bool`,
  `strafing_time: i32` (init `-1`), `attack_time: i32` (init `-1`; keep the field name
  distinct from the existing `attack_interval`/`attack_cooldown` which map to vanilla's
  `attackIntervalMin` and are otherwise fine).
- Add `is_holding_bow(mob) -> bool` using the same pattern already proven in
  `ranged_crossbow_attack.rs::has_crossbow` (`mob.get_mob_entity().living_entity.held_item(mob).await`,
  compare `item.id == Item::BOW.id`). Gate both `can_start` and `should_continue` on it, matching
  vanilla lines 37/46.
- `stop()` must call a mob-side `stop_using_item()` (see below) in addition to the existing
  `set_attacking(false)`.
- Rewrite `tick()` to the vanilla state machine 1:1: LOS-flip resets `see_time`; increment/decrement;
  branch on `distance <= range && see_time >= 20` for stop-vs-navigate; strafing timer and
  the two 30%-chance flips (reuse `mob.get_random()`, already available on `Mob`); the two
  distance-fraction thresholds; and the `moveControl.strafe` call. `MoveControl::strafe` does
  not currently exist in `pumpkin/src/entity/ai/control/move_control.rs` (only
  `look_control.rs` was inspected in depth; confirm the current `MoveControl` shape before
  wiring — if it lacks a `strafe(forward, sideways)` entry point, that's a control-layer
  addition needed alongside this goal, not blocked pathfinding infra).
- Item-use gating: since `LivingEntity` already exposes `item_use_time`/`active_hand`, add
  thin mob-facing wrappers (e.g. on `Mob` or `MobEntity`): `start_using_item(hand)` (calls
  `living_entity.set_active_hand(hand, held_stack, get_use_duration())`), `is_using_item()`
  (`active_hand.lock().await.is_some()`), `ticks_using_item()` (`get_use_duration() -
  item_use_time.load()`), `stop_using_item()` (mirrors `clear_active_hand` but does **not**
  run `ItemBehaviour::on_stopped_using` since that's player/inventory-coupled — the goal
  itself performs the vanilla `performRangedAttack` equivalent by calling its own `shoot()`,
  matching what the goal already does today, just gated by the new draw-time check instead
  of a flat cooldown).
- Compute `BowItem::get_power_for_time(pull_time)` — this function is a pure `i32 -> f32`
  calculation with no `Player` dependency (`pumpkin/src/item/items/bow.rs:143`), so it can be
  called directly from the goal without any generalization; use it in place of the hardcoded
  velocity-magnitude constant currently in `shoot()`.

**Files to modify**: `pumpkin/src/entity/ai/goal/ranged_bow_attack.rs` (full rewrite of
struct/tick), `pumpkin/src/entity/mob/mod.rs` or `living.rs` (mob-facing
start/stop/is-using/ticks-using wrappers), possibly
`pumpkin/src/entity/ai/control/move_control.rs` (add `strafe` if absent — verify first).
Call site `pumpkin/src/entity/mob/skeleton/mod.rs:97` needs an added `speed_modifier`
argument to `with_arrow_effects`/`new`.

**Wiki verification needed**: none beyond what the decompiled source already gives —
values (20-tick draw, `seeTime` thresholds of 20/-60, 0.75/0.25 distance fractions, 30%
strafe-flip chance) are all literal constants in the vanilla method above; no derived/wiki-only
behavior.

---

## 2. RangedCrossbowAttackGoal

**Vanilla**: `RangedCrossbowAttackGoal.java` (141 lines). Explicit 4-state machine:
`UNCHARGED -> CHARGING -> CHARGED -> READY_TO_ATTACK -> UNCHARGED`.

- `canUse`/`canContinueToUse` gate on `isHoldingCrossbow()` (`mob.isHolding(Items.CROSSBOW)`)
  and `isValidTarget()` (target non-null and alive).
- `stop()` (53–63): if `mob.isUsingItem()`, `stopUsingItem()` + `setChargingCrossbow(false)`
  **+ clears `ChargedProjectiles` on the use-item stack to `EMPTY`** — vanilla explicitly
  discards a half-charged shot rather than leaving stale charged data.
  `PATHFINDING_DELAY_RANGE` = `UniformInt` over 1–2 seconds, used to throttle repathing
  (`updatePathDelay`), not recompute every tick.
- `tick()` (70–129):
  - `needsToMove = (distSqr > radiusSqr || seeTime < 5) && attackDelay == 0`; only repaths
    when `updatePathDelay` counts down to 0, using `speedModifier` or half that while
    charging (`canRun()` == state is `UNCHARGED`).
  - state `UNCHARGED`: if not `needsToMove`, `startUsingItem` + `setChargingCrossbow(true)`
    + transition to `CHARGING`.
  - state `CHARGING`: if the mob stopped using the item externally, revert to
    `UNCHARGED`. Else compare `getTicksUsingItem()` against
    **`CrossbowItem.getChargeDuration(useItem, mob)`** (enchantment-aware — Quick Charge
    reduces it); once reached, `releaseUsingItem()` (not `stopUsingItem` — vanilla
    distinguishes "let the use-item system finish naturally, invoking
    `onStopUsing`/`releaseUsing` where the charge actually gets written to
    `ChargedProjectiles`"), set `attackDelay = 20 + random.nextInt(20)`, uncharge the
    charging flag.
  - state `CHARGED`: counts `attackDelay` down to 0, then transitions to
    `READY_TO_ATTACK`.
  - state `READY_TO_ATTACK` + LOS: `performRangedAttack(target, 1.0F)` (this reads
    `ChargedProjectiles` off the mob's use-item stack and fires every entry, applying
    Multishot/Piercing as normal item enchantments), back to `UNCHARGED`.

**Current Rust** (`ranged_crossbow_attack.rs`): no `CrossbowState` enum at all. `can_start`
does check `has_crossbow` (unlike the bow goal, this one already gates correctly). But
`tick()` is a single `attack_cooldown` counter: navigate-or-stop by distance, then if
cooldown is 0 and LOS, calls its own `shoot()` which hardcodes a plain `Item::ARROW`
`ArrowEntity` — never reads `ChargedProjectiles`, never applies enchantments, never
triggers the client-visible charging animation (`setChargingCrossbow`), and the mob's
actual held-crossbow stack is never touched (so Multishot/Piercing bought via the mob's
loot-table-equipped crossbow are silently dropped).

**Target design**:

- Add `CrossbowState` enum (`Uncharged`, `Charging`, `Charged`, `ReadyToAttack`) matching
  vanilla; struct gains `crossbow_state`, `see_time`, `attack_delay: i32`,
  `update_path_delay: i32`, and a `speed_modifier: f64` field (also currently missing, same
  gap as the bow goal — `set_progress` hardcodes speed `1.0` at line 146 today).
- `stop()`: must clear `ChargedProjectiles` on the mob's held-crossbow stack back to empty
  when interrupted mid-charge, mirroring vanilla lines 58–61. This requires read/write
  access to the mob's currently-held `ItemStack`, which the existing `held_item()` helper
  already provides (`Arc<Mutex<ItemStack>>` — same handle already used by `has_crossbow`).
- Reuse rather than re-derive: `CrossbowItem::fire_projectiles` in
  `pumpkin/src/item/items/crossbow.rs` already implements exactly the READY_TO_ATTACK-time
  behavior — read `ChargedProjectilesImpl` off the stack, spawn one arrow per stored
  projectile (times three angled shots if the stack has Multishot), decrement the stack's
  durability, clear `ChargedProjectiles` after firing. It is written against `&Player`
  only because it pulls `player.world()`, `player.position()`, `player.rotation()`, and
  `player.damage_held_item()` — all of which have `dyn Mob`/`dyn EntityBase` equivalents
  available already (`mob.get_entity().world`, `.pos`, yaw/pitch fields, and an
  item-damage helper likely exists near `try_attack`'s enchantment handling — verify exact
  name). Recommend factoring the body of `fire_projectiles` into a helper generic over
  "world + position + rotation + stack handle + damage-item callback" so both the player
  packet path and this goal call the same enchantment-aware firing logic instead of the
  goal maintaining its own arrow-spawn duplicate (which is the bug the census flagged).
  This is a moderate refactor of `crossbow.rs`, technically outside the
  `pumpkin/src/item/` exclusion only in the sense that this document must flag it as an
  additional file that the eventual fix PR will need to touch — it is not something this
  goal can fully fix in isolation without it.
- `CrossbowItem::getChargeDuration` needs a Rust equivalent: today the charge-time
  computation lives inline in `CrossbowItem::on_stopped_using` (`crossbow.rs:68–86`, base 25
  ticks + Quick Charge enchantment reduction) rather than as a standalone queryable
  function. Extract it into a `pub fn charge_duration(stack: &ItemStack) -> i32` so the
  goal's `CHARGING` branch can call it directly instead of hardcoding `20 +
  random.nextInt(20)`-style guesses.
- Must set/clear a `charging_crossbow` visual flag equivalent to vanilla's
  `setChargingCrossbow` (drives the client animation) — check whether `MobEntity` already
  has a comparable data-tracker flag (`CrossbowAttackMob#setChargingCrossbow` sets a boolean
  entity-data field on the mob); if absent, this is a small data-tracker addition alongside
  the goal, not a blocker.

**Files to modify**: `pumpkin/src/entity/ai/goal/ranged_crossbow_attack.rs` (rewrite),
`pumpkin/src/item/items/crossbow.rs` (extract `charge_duration` and a
mob-callable firing helper), `pumpkin/src/entity/mob/mod.rs` (charging-crossbow flag if
missing). Call site `pumpkin/src/entity/mob/pillager.rs:47` unaffected in shape (still
`RangedCrossbowAttackGoal::new(15.0)`) unless a speed-modifier parameter is added, in which
case it must be updated too, along with any other crossbow-wielding mob (illusioner/pillager family — grep for other call sites before landing).

**Wiki verification needed**: exact base Quick Charge reduction and whether Piercing
additionally needs separate goal-side handling beyond what `ChargedProjectiles` + normal
enchantment application already provides — cross-check against the current wiki
"Crossbow" mechanics page rather than assuming the 25-tick/Quick-Charge-per-level numbers
in `crossbow.rs` are already final (they're pre-existing code, not verified as part of this
task).

---

## 3. MeleeAttackGoal

**Vanilla**: `MeleeAttackGoal.java` (154 lines).

- `canUse()` (33–51): 20-tick throttle on repeated calls (`lastCanUseCheck`), then
  `this.path = mob.getNavigation().createPath(target, 0)`; returns `true` if a path was
  found, **else falls back to `mob.isWithinMeleeAttackRange(target)`** — i.e. even with no
  path, a mob already standing next to its target can still start attacking.
- `canContinueToUse()` (54–65): if `followingTargetEvenIfNotSeen`, continues based on
  `isWithinHome` and target not being a creative/spectator player; else simply
  `!navigation.isDone()`.
- `start()` (68–73): `navigation.moveTo(this.path, speedModifier)` — starts navigation
  from the **already-computed** path object, not a fresh `moveTo(target, speed)` call.
- `tick()` repath heuristic (96–120): recomputes only when
  `pathedTarget == (0,0,0)` (first tick) **or** target moved `>= 1.0` sq blocks since last
  pathed position **or** 5% random chance; extra delay scaled by distance (`>1024` -> `+10`,
  `>256` -> `+5`); `moveTo(target, speed)` failing adds `+15` more delay.
- `canPerformAttack` (143–145): `isTimeToAttack() && isWithinMeleeAttackRange(target) &&
  sensing.hasLineOfSight(target)`.

**Current Rust** (`melee_attack.rs`): the `can_start` TODO at line 98–99 states the exact
gap: `true //TODO: modify that because if a path to the target not exists then call
mob.is_in_attack_range(target)`. It never calls into the navigator at all during
`can_start`; the path/attack-range fallback simply doesn't exist, so **every** target
that's alive and off-cooldown starts the goal, even one the mob can never reach (e.g.
separated by an ocean the pathfinder would refuse). The 20-tick throttle (`last_update_time`
vs `MAX_ATTACK_TIME`) and the repath heuristic in `tick()` (lines 193–217) are otherwise
already faithful ports of vanilla's timing.

**Target design**: this goal is the most self-contained of the five, because the
underlying A* implementation (`Navigator::compute_path`, confirmed above) already exists
and only needs a public entry point:

- Add to `Navigator` (`pumpkin/src/entity/ai/pathfinder/mod.rs`):
  ```rust
  pub async fn create_path(&mut self, entity: &LivingEntity, destination: Vector3<f64>) -> Option<Path> {
      self.compute_path(entity, destination).await
  }
  ```
  (or rename `compute_path` to `pub(crate)`/`pub` directly — either is fine; keep it
  side-effect-free with respect to `current_goal`/`current_path`, i.e. don't let a
  `can_start` probe clobber an in-flight path the way `set_progress` does).
- In `MeleeAttackGoal::can_start`, after the existing throttle/target-alive checks, call
  `mob.get_mob_entity().navigator.lock()...create_path(&mob.get_mob_entity().living_entity,
  target_pos).await` (note: `Navigator` is behind a `std::sync::Mutex`, not `tokio::sync`,
  per the field declaration in `mob/mod.rs` — an async call while holding a `std::sync`
  `MutexGuard` across an `.await` is a live bug risk; either make the lock scope
  synchronous around a blocking call, or check whether `compute_path`'s internal awaits are
  the concern — since `compute_path` itself awaits (`self.evaluator.get_start().await` etc.),
  holding a `std::sync::Mutex` guard across that is unsound for a multi-threaded runtime.
  This means the fix likely needs the goal to briefly release the `std::sync::Mutex`, clone
  what's needed, or the `Navigator`'s `create_path` needs to be restructured so the lock is
  only held for the synchronous parts — flag this concurrency shape explicitly for whoever
  implements it, per this repo's rule that concurrency-sensitive changes need minimal,
  careful handling).
- Store the resulting `Option<Path>` in a new `path: Option<Path>` field (the existing
  commented-out field at line 43 becomes real); if `Some`, `can_start` returns `true`
  unconditionally; if `None`, fall back to `mob.get_mob_entity().is_in_attack_range(target)`
  (the method already exists and is already used elsewhere in this same file at line 224).
- `start()` should use the stored `path` (via a `Navigator::move_along(path, speed)`-shaped
  call) instead of recomputing from target position — check whether `Navigator` currently
  exposes a way to seed `current_path` directly from an already-computed `Path` rather than
  only from a `NavigatorGoal { destination, .. }` that triggers its own `compute_path` on
  the next `tick()`. If not, add one; otherwise `start()` can stay as-is (recompute) as a
  smaller, lower-risk first step, at the cost of one redundant pathfind per goal start.
- No change needed to `canContinueToUse`/`tick()` repath heuristic — already correct.

**Files to modify**: `pumpkin/src/entity/ai/pathfinder/mod.rs` (expose `create_path`, and
possibly a "start from existing path" entry point), `pumpkin/src/entity/ai/goal/melee_attack.rs`
(`can_start` rewrite, new `path` field wired for real).

**Wiki verification needed**: none — this is pure engine-internal control flow with no
externally observable game-rule numbers beyond what's already in the decompiled source.

---

## 4. TemptGoal

**Vanilla**: `TemptGoal.java` (148 lines). Constructor takes `(mob, speedModifier, items,
canScare)` or `(..., stopDistance)`; default `stopDistance = 2.5`. Uses
`TargetingConditions.forNonCombat().ignoreLineOfSight()` with `.range(mob.getAttributeValue(Attributes.TEMPT_RANGE))`
— i.e. **the search range comes from the mob's own attribute instance**, which varies per
mob (Sniffer registers a larger `TEMPT_RANGE` base value than the default 10.0).

- `canUse()` (52–60): `calmDown` cooldown counter (100-tick, set in `stop()`), then
  `getServerLevel(mob).getNearestPlayer(targetingConditions.range(attrValue), mob)`.
- `canContinueToUse()` (67–88) is the anti-flee logic the census flags: only when
  `canScare()` is true, and only once `mob.distanceToSqr(player) < 36.0` (i.e. within 6
  blocks) does it start comparing the player's current position/rotation against the
  values captured on the previous tick — `player.distanceToSqr(px,py,pz) > 0.01` (i.e.
  moved more than 0.1 blocks) or `abs(xRot - pRotX) > 5.0 || abs(yRot - pRotY) > 5.0`
  aborts the goal (`return false`) *without* updating `px/py/pz` first, so it only takes
  effect once the mob is already close. Outside 6 blocks, `px/py/pz` are refreshed every
  tick to the player's current position (so the "did they move" check can't falsely trigger
  from stale far-away coordinates once the mob closes in). `pRotX/pRotY` are updated every
  tick regardless of distance.
- `tick()` (111–118): `stopDistance` (constructor param, default 2.5, not always the same
  as the goal's search cutoff) stops navigation once within `stopDistance^2`.
- Also has a `ForNonPathfinders` subclass using `moveControl.setWantedPosition` for mobs
  without ground pathfinding (not relevant unless Pumpkin has non-pathfinding tempt-able
  mobs — check before deciding whether to port it).

**Current Rust** (`tempt.rs`): `TEMPT_RANGE: f64 = 10.0` is a module constant, not read
from the mob's attribute instance — but as noted above, `Attributes::TEMPT_RANGE` already
exists in `pumpkin-data`, so this is purely a Rust-side lookup fix, not a data-registration
gap. `should_continue`/`is_player_still_tempting` only checks distance and held-item;
there is no `canScare`/anti-flee logic at all — a scareable mob (villager, fox, etc.) in
this codebase will currently follow a player who is running away or spinning in place,
which vanilla explicitly prevents.

**Target design**:

- Read range via `mob.get_mob_entity().living_entity.get_attribute_value(&Attributes::TEMPT_RANGE)`
  (the pattern already used elsewhere, e.g. `pumpkin/src/entity/ai/pathfinder/mod.rs`'s
  `FOLLOW_RANGE` lookup at line ~178) instead of the module constant. Drop `TEMPT_RANGE`
  const; keep `STOP_DISTANCE` as a constructor parameter (defaulting to 2.5) rather than a
  fixed const, matching vanilla's `stopDistance` field — some callers may want a
  non-default value.
- Add `can_scare: bool` (constructor parameter, since vanilla mobs pass this explicitly —
  e.g. most animals pass `true`).
- Add tracked state: `last_player_pos: Option<Vector3<f64>>`, `last_player_rot: Option<(f32,
  f32)>` (yaw/pitch — check `Player`/`Entity`'s actual rotation field names, likely
  `entity.yaw`/`entity.pitch` `AtomicCell<f32>`, matching the `x_rot`/`y_rot` naming in
  vanilla, which is pitch/yaw despite the "X/Y" naming — confirm the exact
  vanilla-to-Rust field mapping before porting the 5-degree comparison, since a yaw/pitch
  swap would silently invert the check).
- `should_continue` gains the vanilla branch: only when `can_scare` and within 36.0
  (6 blocks) squared distance, compare current player pos/rot against the last-seen values;
  return `false` (abort) if position moved > 0.01 sq (i.e. > 0.1 blocks) or rotation
  changed by > 5 degrees on either axis; otherwise (or when far away) refresh the stored
  pos/rot every tick, matching vanilla's exact order of operations (position check first,
  rotation always refreshed after, but only the position is refreshed conditionally on
  distance).
- No changes needed to `find_tempting_player`'s use of `get_nearby_players` shape — just
  feed it the attribute-derived range instead of the constant.

**Files to modify**: `pumpkin/src/entity/ai/goal/tempt.rs` only. Check whether any
existing call sites (grep shows none currently instantiate `TemptGoal` outside doc/tests at
the time of this audit — confirm again before landing, since a mob may have been wired up
between this audit and implementation) need a new `can_scare`/`stop_distance` argument
added.

**Wiki verification needed**: the per-mob `TEMPT_RANGE` base values (e.g. Sniffer's
non-default base) — verify against current wiki attribute tables or the decompiled
`Sniffer.java`'s attribute-supplier registration before assuming 10.0 is the universal
default; it's vanilla's *default* `Attributes.TEMPT_RANGE` base but individual mobs can
override it via their `createAttributes()` builder.

---

## 5. FollowOwnerGoal

**Vanilla**: `FollowOwnerGoal.java` (93 lines). Constructor validates the mob's navigation
is `GroundPathNavigation` or `FlyingPathNavigation` (throws otherwise).

- `canUse()` (35–51): `owner == null` -> false; **`tamable.unableToMoveToOwner()`** -> false;
  distance-to-owner below `startDistance^2` -> false. `unableToMoveToOwner()` itself (not
  shown in this file, defined on `TamableAnimal`) is
  `isOrderedToSit() || isPassenger() || mayBeLeashed() || (owner is Player && owner.isSpectator())` —
  this is the exact set the census flags as missing; current Rust only checks
  `!mob.is_sitting()`.
- `start()`/`stop()` (63–74): **temporarily zero the mob's `PathType.WATER` pathfinding
  malus** for the duration of the goal, restoring the pre-goal value (`oldWaterCost`) on
  stop — so a tamed mob will path straight through water while following its owner, but
  reverts to normal water-avoidance once the goal ends.
- `tick()` (77–91): if `shouldTryTeleportToOwner()` (owner far away), skip the look-at and
  call `tryToTeleportToOwner()` on the 10-tick recalc cadence; else normal
  `lookControl.setLookAt` + `navigation.moveTo(owner, speedModifier)`.

**Current Rust** (`follow_owner.rs`): `can_follow` is `!mob.is_sitting()` only (line
33–35) — misses passenger/leash/owner-spectator checks entirely. Separately worth noting:
`Mob::is_sitting()`'s *default* trait implementation in `pumpkin/src/entity/mob/mod.rs`
(line ~663) is hardcoded `false` — grepping `wolf.rs`/`cat.rs` shows **neither currently
overrides `is_sitting()`**, and both have their `FollowOwnerGoal::new(...)` call sites
commented out entirely (`wolf.rs:68`, `cat.rs:115`) — this goal is not wired into any mob
yet in this codebase, so the fix here is greenfield rather than a behavior-preserving
patch to a live path. Water-malus save/restore in `start()`/`stop()` is entirely absent —
current `start()` only resets `update_countdown`.

**Target design**:

- Rewrite `can_follow`/`unable_to_move_to_owner(mob, owner) -> bool` as the conjunction:
  `mob.get_mob_entity().is_ordered_to_sit()` (the underlying `MobEntity` field already
  exists — `ordered_to_sit: AtomicBool`, with `is_ordered_to_sit()`/`set_ordered_to_sit()`
  accessors at `mob/mod.rs:222–228` — so this is just wiring the goal to the *right*
  existing accessor instead of the always-`false` `is_sitting()` trait default) ||
  `mob.get_entity().has_vehicle().await` (the `is_passenger()` equivalent — `EntityBase`
  already exposes this at `entity/mod.rs:356–361`, just needs awaiting since the current
  synchronous `can_follow` isn't async — but `can_start`/`should_continue` already are, so
  this only requires making the internal `can_follow`/`unable_to_move_to_owner` helper
  `async` too) || `mob.get_entity().leashed_to.lock().await.is_some()` (approximates
  `mayBeLeashed()` — vanilla's actual `mayBeLeashed()` is `isAlive() && !isLeashed()`
  inverted contextually; re-derive the exact boolean sense from
  `Mob.java`/`Leashable.java` rather than guessing, since getting the polarity backwards
  would silently break every leashed tamed mob) || (owner is a `Player` and
  `owner.is_spectator()` — already available via `Player::is_spectator()` at
  `player.rs:4937`, or the shared `EntityBase::is_spectator` default at `entity/mod.rs:255`).
- Add `set_pathfinding_malus`'s missing getter to `Navigator`:
  ```rust
  pub fn get_pathfinding_malus(&self, path_type: PathType) -> Option<f32> {
      self.path_type_overrides.get(&path_type).copied()
  }
  ```
  so `start()` can snapshot the pre-goal value (vanilla's `oldWaterCost` — note vanilla
  calls `tamable.getPathfindingMalus(PathType.WATER)`, which returns the mob's *effective*
  default malus if no override is set, not `None`; if this codebase's default `Water` malus
  isn't otherwise tracked outside `path_type_overrides` — recall `compute_path` hardcodes
  `mob_data.set_pathfinding_malus(PathType::Water, 8.0)` as a baseline before applying
  overrides — then `get_pathfinding_malus` returning `None` should be treated by the goal as
  "restore to no override" (i.e. call `Navigator::clear_pathfinding_malus` on stop, a new
  method to add alongside the getter) rather than restoring a synthetic default value).
- `start()`: snapshot old water malus, call `set_pathfinding_malus(PathType::Water, 0.0)`.
  `stop()`: restore snapshot (or clear override if none existed).
- `tick()` is otherwise already close to correct (teleport-vs-follow branch, 10-tick
  recalc cadence via `to_goal_ticks`); no changes needed there beyond whatever the
  `unable_to_move_to_owner` rename requires at call sites within this file.

**Files to modify**: `pumpkin/src/entity/ai/goal/follow_owner.rs` (the `can_follow`
rewrite + water-malus save/restore in `start`/`stop`), `pumpkin/src/entity/ai/pathfinder/mod.rs`
(add `get_pathfinding_malus`/`clear_pathfinding_malus`). Since the goal isn't currently
wired into any mob, also **uncomment and fix the call sites** at `pumpkin/src/entity/passive/wolf.rs:68`
and `pumpkin/src/entity/passive/cat.rs:115` as part of landing this (otherwise the fix has
no observable effect) — and audit whether `wolf.rs`/`cat.rs` need an `is_sitting()`
override wired to `MobEntity::is_ordered_to_sit()` first, since `SitGoal` (already active
in both goal selectors) presumably calls `set_ordered_to_sit`, but the `Mob::is_sitting()`
trait default staying `false` means anything else in the codebase gating on `is_sitting()`
(not just this goal) is currently reading a stale/wrong value — worth flagging as a
possibly-separate, higher-priority bug (`Mob::is_sitting()` default returning `false`
unconditionally) discovered as a side effect of this audit.

**Wiki verification needed**: exact vanilla `mayBeLeashed()` boolean polarity (cite
`Leashable.java`/`Mob.java` directly, not memory) before wiring the leash check.

---

## Summary of blockers vs. self-contained work

| Goal | Blocked on missing infra? | Notes |
|---|---|---|
| MeleeAttackGoal | No (once `create_path` is exposed) | The A* engine already exists; needs a public entry point plus a real concurrency-safe call from `can_start`. Smallest, safest change of the five, but has a genuine `std::sync::Mutex`-across-`.await` hazard to design around carefully. |
| TemptGoal | No | `Attributes::TEMPT_RANGE` already registered; anti-flee logic is pure goal-local state. Fully self-contained to `tempt.rs`. |
| FollowOwnerGoal | Mostly no | All needed accessors (`is_ordered_to_sit`, `has_vehicle`, `leashed_to`, `is_spectator`) already exist; only a small `Navigator` getter is missing. The goal also isn't wired to any mob yet, so landing it means touching `wolf.rs`/`cat.rs` too, and surfaces a separate suspect bug (`Mob::is_sitting()` default always `false`). |
| RangedBowAttackGoal | Partially | Item-use timing state (`item_use_time`/`active_hand`) already exists on `LivingEntity`; the gap is control-layer (`MoveControl::strafe`, unverified) and needs new mob-facing use-item wrapper methods, but no pathfinding/attribute prerequisite is missing. |
| RangedCrossbowAttackGoal | Yes, partially | The `CrossbowState` machine and use-item timing are portable now, but correct enchantment/`ChargedProjectiles` handling depends on refactoring `CrossbowItem::fire_projectiles`/`getChargeDuration` out of their current `&Player`-only shape in `pumpkin/src/item/items/crossbow.rs` — that refactor is a real prerequisite this goal's fix cannot avoid touching. |
