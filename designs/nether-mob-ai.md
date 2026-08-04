# Nether mob AI: Piglin, Piglin Brute, Hoglin, Strider

Design document, not code. Scope: bring the four nether mobs up to gameplay parity with
current Pumpkin infrastructure. Vanilla source is
`/tmp/pumpkin-vanilla-26.2/decompiled/net/minecraft/world/entity/monster/{piglin,hoglin}/*`
and `.../monster/Strider.java`. Current Pumpkin state:
`pumpkin/src/entity/mob/piglin.rs`, `piglin_brute.rs`, `hoglin.rs`,
`pumpkin/src/entity/passive/strider.rs`.

## Cross-cutting finding: no Brain/memory system, no conversion helper, no jockey spawning

Confirmed by reading `pumpkin/src/entity/ai/goal/*` (32 files, all single-purpose
`Goal`/`Controls` structs driven by `GoalSelector`, `active_target::ActiveTargetGoal` for
targeting) and `pumpkin/src/entity/mob/mod.rs`: Pumpkin has **no** equivalent of vanilla's
`Brain<T>` / `MemoryModuleType` / `Sensor` / `Activity` system that `PiglinAi`, `HoglinAi`,
`PiglinBruteAi` are built on (861, 255, 163 lines respectively, almost entirely
Brain-behavior wiring). Porting the Brain system itself is out of scope — it would mean
building a generic memory/sensor/activity scheduler used nowhere else in the codebase, for
a payoff of four mobs. Every section below designs a **Goal-based approximation**:
individual vanilla behaviors become individual `Goal` impls added to the existing
`goals_selector` / `target_selector`, prioritized the way vanilla's `ActivityData` priority
and `RunOne` weighting suggests, without a shared blackboard. Cross-mob signaling that
vanilla does via memory broadcast (anger propagation, retreat propagation, hunt
coordination) has no `Brain` to piggyback on; each such case is called out individually
with a concrete alternative (usually: a world query for nearby same-type mobs plus a
direct method call, matching how `AvoidEntityGoal` already does `world.get_closest_entity_where`).

Two more pieces of shared infrastructure were checked and are **absent**, not merely
under-used:

- **Entity-type conversion** (piglin→zombified piglin, hoglin→zoglin, "TimeInOverworld"
  counters). There is no generic `convert_to`. The one existing precedent is
  `pumpkin/src/entity/mob/zombie/zombie_villager.rs` (villager↔zombie-villager cure/infect):
  it manually constructs the new entity, copies `pos`/`custom_name`/active effects, adds a
  200-tick nausea effect (matching `MobEffects.NAUSEA` in `AbstractPiglin.finishConversion`
  and `Hoglin.finishConversion`), calls `world.spawn_entity(new)`, fires a `sync_world_event`
  sound, then `old_entity.remove()`. This is the pattern to copy for both piglin→zombified
  piglin and hoglin→zoglin conversion; there is no reusable helper function today, so each
  mob currently reimplements it inline (as zombie_villager.rs does) unless a shared
  `entity/conversion.rs` helper is factored out (recommended — see Shared Infrastructure).
- **Jockey spawning** (`grep -rln jockey pumpkin/src/entity/` found nothing but a match
  inside warden.rs comments, and `EntitySpawnReason`/spawn-reason plumbing does not exist —
  confirmed by `pumpkin/src/entity/mob/mod.rs:978` (`// TODO: missing SpawnReason
  attribute`) and `pumpkin/src/world/natural_spawner.rs:804`). Strider's zombified-piglin
  jockey and baby-strider jockey, and any other jockey pattern in the codebase, would be the
  *first* jockey-spawning implementation in Pumpkin, not a reuse of an existing one. Riding
  itself (mount/dismount, passenger list) is implemented generically on `Entity`
  (`pumpkin/src/entity/mod.rs`: `passengers: Mutex<Vec<Arc<dyn EntityBase>>>`, `add_passenger`
  around line 3216, recursive passenger teleport at `teleport_passengers_recursive`), so
  jockey spawning only needs to construct the rider entity and call the existing mount path
  — it does not need new riding primitives.

---

## 1. Piglin

### Vanilla source
- `AbstractPiglin.java` (140 lines): shared piglin/brute base — `CONVERSION_TIME = 300`,
  `timeInOverworld`, `isConverting()` (gated on `!isImmuneToZombification() && !isNoAi() &&
  environmentAttributes().getValue(EnvironmentAttributes.PIGLINS_ZOMBIFY, position())`),
  `finishConversion` (`convertTo(ZOMBIFIED_PIGLIN, ConversionParams.single(this, true, true),
  z -> z.addEffect(NAUSEA, 200, 0))`), `customServerAiStep` (increments `timeInOverworld`,
  triggers conversion past 300 ticks), `getArmPose()` abstract, `isHoldingMeleeWeapon`,
  `playAmbientSound` gated on `PiglinAi.isIdle`.
- `Piglin.java` (441 lines): brain provider (memories: `UNIVERSAL_ANGER`, `ATE_RECENTLY`,
  `SPEAR_FLEEING_TIME/POSITION`, `SPEAR_CHARGE_POSITION`, `SPEAR_ENGAGE_TIME`; sensors:
  `NEAREST_LIVING_ENTITIES`, `NEAREST_PLAYERS`, `NEAREST_ITEMS`, `HURT_BY`,
  `PIGLIN_SPECIFIC_SENSOR`), 8-slot `SimpleContainer` inventory (`INVENTORY_SLOT_OFFSET =
  300`), `dropCustomDeathLoot` (dumps inventory), `canAddToInventory`/`addToInventory`,
  `populateDefaultEquipmentSlots` (10% chance each of gold helmet/chestplate/leggings/boots,
  adults only), `finalizeSpawn` (20% baby, else 50/50 crossbow vs 10%-golden-spear/90%-
  golden-sword mainhand via `createSpawnWeapon`), `checkPiglinSpawnRules` (not on nether wart
  block), `mobInteract`→`PiglinAi.mobInteract`, `setBaby` (adds `SPEED_MODIFIER_BABY`
  `+0.2F ADD_MULTIPLIED_BASE` attribute modifier), `canHunt()`/`cannotHunt` field,
  `finishConversion` override (cancels admiring, drops inventory, calls super),
  `getPreferredWeaponType` (null for babies), `onCrossbowAttackPerformed`,
  `getArmPose()` (dancing > admiring > attacking-with-melee > crossbow-charge >
  crossbow-hold > default), `isDancing`/`setDancing`, `hurtServer`→`PiglinAi.wasHurtBy`,
  `performRangedAttack`→`performCrossbowAttack(this, 1.6F)`, `canUseNonMeleeWeapon`
  (crossbow or `KINETIC_WEAPON` component — i.e. spear), `holdInMainHand`/`holdInOffHand`,
  `wantsToPickUp`→`PiglinAi.wantsToPickup` gated on `MOB_GRIEFING` gamerule and
  `canPickUpLoot()`, `canReplaceCurrentItem` (loved/preferred-weapon priority logic),
  `pickUpItem`→`PiglinAi.pickUpItem`, `startRiding` (babies auto-ride topmost baby hoglin
  passenger stack, max 3 via `getTopPassenger(vehicle, 3)`), sound overrides.
- `PiglinAi.java` (861 lines, static helpers driving the Brain): `getActivities` (CORE,
  IDLE, ADMIRE_ITEM, FIGHT, CELEBRATE, AVOID, RIDE activities); `wasHurtBy` (line 556-587:
  skip if attacker is a piglin; cancel offhand-hold; erase celebrate/dancing/admiring
  memories; 400-tick `ADMIRING_DISABLED` if attacker is a player; erase `AVOID_TARGET` if
  type differs; baby → flee attacker 100 ticks + maybe broadcast anger; hoglin attacker that
  outnumbers → retreat + broadcast retreat; else `maybeRetaliate`); `maybeRetaliate` (skip
  if already in AVOID activity; check `isEntityAttackableIgnoringLineOfSight`; check target
  isn't much farther than current attack target; universal-anger gamerule branch vs direct
  `setAngerTarget` + `broadcastAngerTarget`); `canAdmire`/`isLovedItem` (`PIGLIN_LOVED` tag,
  not just gold ingot — Pumpkin's current `piglin_admire.rs` module only reacts to gold
  ingot, per its own doc comment); `wantsToPickup` (baby-ignore tag, repellent tag,
  admiring-disabled-while-fighting exception, barter-currency check, gold-nugget/food/loved
  item/preferred-weapon branches); `pickUpItem` (gold nugget takes whole stack, else takes
  1; loved→admire, food→eat (200-tick cooldown), else equip-or-inventory);
  `getBarterResponseItems` (rolls `BuiltInLootTables.PIGLIN_BARTERING` loot table);
  `angerNearbyPiglins` (public entry point used by e.g. player-attacks-piglin-structure
  logic — 16-block radius, filters idle piglins, filters by visibility if requested);
  `broadcastAngerTarget`/`broadcastRetreat`/`broadcastUniversalAnger` (iterate
  `NEARBY_ADULT_PIGLINS`/`NEAREST_VISIBLE_ADULT_PIGLINS` memories and propagate);
  `findNearestValidAttackTarget` (near-zombified suppression > `ANGRY_AT` UUID memory >
  universal-anger nearest-attackable-player > nemesis memory > not-wearing-gold player);
  `getSoundForCurrentActivity` (FIGHT→angry, converting/AVOID-near-target→retreat,
  ADMIRE_ITEM→admiring, CELEBRATE→celebrate, sees-player-with-loved-item→jealous,
  near-repellent→retreat, else ambient); `isWearingSafeArmor` (checks `PIGLIN_SAFE_ARMOR` tag
  across armor slots — governs whether players anger piglins by proximity); `isZombified`
  (`ZOMBIFIED_PIGLIN` or `ZOGLIN`).

### Current Rust state (`pumpkin/src/entity/mob/piglin.rs`, 216 lines)
Implemented: `SwimGoal`, a bespoke `PiglinAdmireGoal` (gold-ingot-only, doc-comments admit
it's narrower than the `PIGLIN_LOVED` tag), `MeleeAttackGoal` (doc comment: "Piglins use
crossbows or swords, but for now we give them melee"), `WanderAroundGoal`,
`LookAtEntityGoal`, `RandomLookAroundGoal`, `ActiveTargetGoal` for player/wither
skeleton/wither. `mob_interact` implements the gold-ingot admire handshake directly
(bypassing `PIGLIN_LOVED`). `on_damage` cancels admiring and sets a 400-tick
admiring-disabled lockout on player attackers, explicitly *skipping* the
anger-propagation/avoid-broadcasting half of `wasHurtBy` per its own comment. `mob_tick`
decrements the lockout. Missing entirely: baby state (no `is_baby`/dimension/speed-modifier
handling beyond an `is_adult()` age check), inventory, bartering beyond the single-item
admire, crossbow/spear combat and `getArmPose`/dancing, zombification/conversion,
`checkPiglinSpawnRules`, `populateDefaultEquipmentSlots`, `dropCustomDeathLoot`, hoglin-baby
riding, sound-activity mapping.

### Target Rust design

**New goals** (each a `Goal` impl under `pumpkin/src/entity/ai/goal/`, added to
`goals_selector`/`target_selector` in `piglin.rs`'s constructor with priorities mirroring
vanilla's `ActivityData` ordering — CORE-equivalent behaviors run every tick outside the
selector list as part of `mob_tick`, the rest are ordinary prioritized goals):

- `piglin_hunt_hoglin.rs` — port of `StartHuntingHoglin`: triggers when `canHunt()` and a
  nearby huntable adult hoglin exists and `HUNTED_RECENTLY` isn't set; sets attack target
  and a "hunted recently" cooldown (`TIME_BETWEEN_HUNTS` = 30–120s) instead of a Brain
  memory-with-expiry — use a plain `AtomicI32` cooldown field on `PiglinEntity`, same
  pattern as `admiring_disabled_ticks` already on `PiglinEntity`.
- Extend `piglin_admire.rs` (or add `piglin_barter.rs`) to cover the full bartering flow:
  `canAddToInventory`/`addToInventory` (backing store: a small fixed-size `Vec<ItemStack>`
  or reuse whatever inventory abstraction backs villagers/hoppers — check
  `SimpleContainer`-equivalent in pumpkin-inventory before inventing one),
  `wantsToPickUp`/`pickUpItem` full logic (gold nugget vs `PIGLIN_LOVED` vs `PIGLIN_FOOD` vs
  preferred-weapon-replacement, per `PiglinAi.wantsToPickup`/`pickUpItem` above), and
  `getBarterResponseItems` via the existing loot-table machinery (Pumpkin already has loot
  tables for mob drops — reuse that resolver against `piglin_bartering.json`, do not write a
  new loot roller).
- `piglin_combat.rs` (or fold into existing `MeleeAttackGoal`/`RangedCrossbowAttackGoal`):
  since Pumpkin's `MeleeAttackGoal` is generic and `RangedCrossbowAttackGoal` already exists
  (`pumpkin/src/entity/ai/goal/ranged_crossbow_attack.rs`) and is used by other crossbow
  mobs, wire the piglin to pick melee vs `RangedCrossbowAttackGoal` based on held item
  (mirrors `canUseNonMeleeWeapon`), instead of hardcoding `MeleeAttackGoal` unconditionally
  as today. Spear combat (`SpearApproach`/`SpearAttack`/`SpearRetreat`) can reuse
  `pumpkin/src/entity/ai/goal/spear_use.rs`, which already exists as generic infra (doc
  comments cite `SpearUseGoal` constants directly) — confirm it's usable by non-warden mobs
  and wire it in rather than writing a piglin-specific spear goal.
- `piglin_avoid_zombified.rs` — flee nearby zombified piglins/zoglins (`avoidZombified`
  CopyMemoryWithExpiry equivalent): a small `Goal` that scans for nearby zombified
  entities within a fixed radius and sets a flee target for a few seconds, approximating
  `AVOID_ZOMBIFIED_DURATION` (5-7s). Can likely share code with `AvoidEntityGoal`
  (`pumpkin/src/entity/ai/goal/avoid_entity.rs`) by adding a second flee-type or a small
  variant, rather than duplicating its pathing logic.
- Baby-specific: `piglin_baby_avoid_nemesis.rs` (flee `NEAREST_VISIBLE_NEMESIS` — zombified
  entities are the nemesis type per vanilla) and `piglin_baby_ride_hoglin.rs`
  (opportunistically mount a nearby baby hoglin — vanilla's `babySometimesRideBabyHoglin` +
  `RIDE` activity + `startRiding`'s `getTopPassenger(vehicle, 3)` stacking cap). Both are
  baby-only; gate on the existing `is_adult()` check already in `piglin.rs`.
- Dancing/celebrating: a `piglin_celebrate.rs` goal triggering on a kill (10% chance if the
  killed target was a hoglin, matching `wantsToDance`), setting `is_dancing` (needs a new
  synced-data-equivalent bool on `PiglinEntity`, check how `is_baby` or similar booleans are
  synced to the client elsewhere in `mob/mod.rs`/`TrackedData` before inventing a new
  channel) for up to 300 ticks (`CELEBRATION_TIME`).

**Anger propagation** (`wasHurtBy`, `maybeRetaliate`, `broadcastAngerTarget`,
`angerNearbyPiglins`): no Brain memory bus exists, so implement as a direct world query:
on `on_damage`, if attacker isn't a piglin, do the existing admiring-cancel/lockout (already
present), then additionally query `world.get_entities_of_type_in_radius(EntityType::PIGLIN,
pos, 16.0)` (equivalent to `AvoidEntityGoal`'s `get_closest_entity_where`, but "all within
radius" rather than "closest") filtering adult piglins, and call a new `set_anger_target`
method on each directly — this is the Goal-based approximation of `broadcastAngerTarget`.
`angerNearbyPiglins`'s public entry point (used when a player breaks bastion-guarded blocks,
per `guarded_by_piglins` block tag) should become a free function in `piglin.rs` or a new
`pumpkin/src/entity/ai/anger/piglin.rs` module, called from wherever Pumpkin handles
`guarded_by_piglins` block breaks (check block-break event plumbing before assuming it needs
to be added — search for the tag's current usage first).

**Conversion**: implement `PiglinEntity::finish_conversion` following the
`zombie_villager.rs` pattern (construct `ZombifiedPiglinEntity`, copy pos/effects, add
200-tick nausea, `world.spawn_entity`, drop the piglin's bartering inventory via
`spawn_at_location`-equivalent, `old_entity.remove()`), driven by a `time_in_overworld:
AtomicI32` field incremented once per tick in `mob_tick` while `is_converting()` is true.
`is_converting()` needs the vanilla `EnvironmentAttributes.PIGLINS_ZOMBIFY` predicate —
**needs wiki/source verification**: confirm the exact dimension/light rule (piglins
zombify anywhere outside the nether/nether-associated biomes/structures, gated by
`piglin_safe` block tag proximity and dimension check) against current pumpkin-world
dimension-tag support, which does not appear to have an equivalent flag today (searched,
found none) — this may need a new `pumpkin-world` predicate, which is out of scope for this
doc's Goal-focused design and should be flagged as a dependency.

**Equipment/spawn**: `populate_default_equipment_slots` (10%-per-slot gold armor, adults
only) and `create_spawn_weapon` (50% crossbow / 45% sword / 5% spear) belong in
`PiglinEntity::new` or a `finalize_spawn` hook — check whether Pumpkin has any
per-mob-type finalize-spawn hook today (equivalent to vanilla's `finalizeSpawn`) before
adding one; if not, this needs to piggyback on wherever mob spawning currently calls
`EntityType::PIGLIN`'s constructor.

### Files to create/modify
- Modify: `pumpkin/src/entity/mob/piglin.rs` (baby state, inventory, arm pose/dancing,
  conversion, equipment, hunt cooldown, anger propagation entry points).
- New: `pumpkin/src/entity/ai/goal/piglin_hunt_hoglin.rs`,
  `piglin_avoid_zombified.rs`, `piglin_baby_avoid_nemesis.rs`,
  `piglin_baby_ride_hoglin.rs`, `piglin_celebrate.rs`.
- Extend: `pumpkin/src/entity/ai/goal/piglin_admire.rs` (full bartering) or split into a new
  `piglin_barter.rs`.
- Possibly new: `pumpkin/src/entity/conversion.rs` (shared entity-morph helper factored out
  of the `zombie_villager.rs` pattern — see Shared Infrastructure).
- Data: confirm `PIGLIN_LOVED`, `PIGLIN_FOOD`, `PIGLIN_REPELLENTS`, `PIGLIN_SAFE_ARMOR`,
  `PIGLIN_PREFERRED_WEAPONS`, `IGNORED_BY_PIGLIN_BABIES`, `GUARDED_BY_PIGLINS` tags exist in
  `pumpkin-data`; add if missing.

---

## 2. Piglin Brute

### Vanilla source
`PiglinBrute.java` (145 lines) + `PiglinBruteAi.java` (163 lines). Simpler than Piglin: no
inventory, no bartering, no crossbow. Key facts: `MAX_HEALTH=50`, `ATTACK_DAMAGE=7`,
`FOLLOW_RANGE=12`; `canHunt()` hardcoded `false`; `wantsToPickUp` restricted to
`Items.GOLDEN_AXE` only (falls through to `super.wantsToPickUp` — i.e. still gated by
`MOB_GRIEFING`/`canPickUpLoot`/`PiglinAi.wantsToPickup`, just filtered to golden axe first);
`populateDefaultEquipmentSlots` unconditionally sets mainhand to a golden axe (no random
roll, no armor); `getArmPose` (aggressive+melee-weapon → attacking pose, else default — no
crossbow/admiring/dancing states at all); `hurtServer`→`PiglinBruteAi.wasHurtBy` which, if
attacker isn't any `AbstractPiglin`, calls the *same* `PiglinAi.maybeRetaliate` piglin uses
(shared retaliation logic, brute-specific: no baby-flee branch, no hoglin-outnumber branch —
bastion brutes always just retaliate); `initMemories` sets a `HOME` GlobalPos memory used by
`StrollToPoi`/`StrollAroundPoi` idle-movement behaviors (keeps brutes tethered near their
bastion); `maybePlayActivitySound`/`playActivitySound` (1.25%/tick chance of an angry sound
while in FIGHT activity — separate from vanilla's per-activity-change sound-on-transition
elsewhere).

### Current Rust state (`pumpkin/src/entity/mob/piglin_brute.rs`, 71 lines)
`SwimGoal`, `MeleeAttackGoal`, `WanderAroundGoal`, `LookAtEntityGoal`,
`RandomLookAroundGoal`, `ActiveTargetGoal` (player/wither skeleton/wither) — essentially
identical goal set to the plain piglin, no brute-specific behavior at all: no mandatory
golden axe, no `canHunt()->false` (brutes can't hunt hoglins at all — currently
indistinguishable from a piglin in this regard since `piglin_brute.rs` has no hunt goal to
begin with, so this one happens to be accidentally correct only because the hunt goal
doesn't exist yet on either mob), no home tether, no angry-sound hook.

### Target Rust design
- Equipment: on construction (or a spawn hook, see Piglin section), unconditionally equip
  `Item::GOLDEN_AXE` in the mainhand slot — trivial, no new goal needed.
- `wantsToPickUp` override: golden-axe-only filter before falling through to the shared
  `wantsToPickup` logic designed for Piglin above — this should be a shared free function
  (`piglin_shared::wants_to_pick_up`) called by both `piglin.rs` and `piglin_brute.rs` with
  a brute-specific pre-filter, not two independent copies.
- Home tether: a new `stroll_to_poi.rs`/`stroll_around_poi.rs` goal pair, or a single
  `HomeTetherGoal` recording the brute's spawn position as "home" and wandering back toward
  it once far away (`HOME_TOO_FAR_DISTANCE=100`) vs. loitering near it
  (`HOME_STROLL_AROUND_DISTANCE=5`) — this is new infra, no existing Pumpkin goal does
  home-position tethering (checked `pumpkin/src/entity/ai/goal/`, nothing matches).
- Retaliation: brute's `on_damage` should call the *same* shared retaliation helper the
  Piglin anger-propagation design introduces (see section 1's "anger propagation" bullet) —
  don't fork a second copy of `maybeRetaliate`'s logic. No hoglin-outnumber or baby-flee
  special cases apply to brutes (they're never babies).
- `canHunt()` returns `false` — once the Piglin hunt-cooldown goal exists (section 1), the
  brute simply never gets it added to its goal selector, rather than needing its own
  false-returning method (there's no shared `AbstractPiglin` trait today; whether to
  introduce one for `canHunt`/`getArmPose`/`isImmuneToZombification`/`finishConversion`
  commonality is a judgment call — given only two piglin variants exist, plain duplication
  of the handful of shared fields may be less churn than introducing a new base trait; either
  is acceptable, but if a trait is introduced, model it after how `Mob` itself is structured
  as a trait over `MobEntity`).
- Conversion: brutes convert to zombified piglins too (`AbstractPiglin.finishConversion` is
  inherited, brute has no override) — reuse the same conversion helper as Piglin.
- Sound hook: a `maybe_play_activity_sound` call each tick with a flat probability check
  (1.25%) while an aggressive/fight state is active — straightforward, no goal needed, just
  a `mob_tick` addition alongside the existing `admiring_disabled_ticks`-style per-tick
  logic pattern already used in `piglin.rs`.

### Files to create/modify
- Modify: `pumpkin/src/entity/mob/piglin_brute.rs` (equipment, wantsToPickUp filter, home
  tether wiring, retaliation hookup, sound tick).
- New: a home-tether goal (name TBD, e.g. `pumpkin/src/entity/ai/goal/home_tether.rs`).
- Shared: whatever free function/trait is chosen for cross-piglin-variant logic (see above).

---

## 3. Hoglin

### Vanilla source
- `Hoglin.java` (364 lines): `MAX_HEALTH=40`, `MOVEMENT_SPEED_WHEN_FIGHTING=0.3`,
  `KNOCKBACK_RESISTANCE=0.6`, `ATTACK_KNOCKBACK=1`, `ATTACK_DAMAGE=6`/baby `0.5`; brain
  provider (sensors: `NEAREST_LIVING_ENTITIES`, `NEAREST_PLAYERS`, `NEAREST_ADULT`,
  `HOGLIN_SPECIFIC_SENSOR`; no custom memory list beyond sensor-populated ones);
  `doHurtTarget` (10-tick attack animation, byte-event 4 broadcast, `HOGLIN_ATTACK` sound,
  `HoglinAi.onHitTarget`, `HoglinBase.hurtAndThrowTarget`); `blockedByItem` (adults only,
  `HoglinBase.throwTarget` on shield-block); `hurtServer`→`HoglinAi.wasHurtBy`;
  `customServerAiStep` (brain tick, `HoglinAi.updateActivity`, conversion-time increment +
  `finishConversion` past 300 ticks with `HOGLIN_CONVERTED_TO_ZOMBIFIED` sound);
  `ageBoundaryReached` (swaps `ATTACK_DAMAGE` base value and `xpReward` between baby/adult
  values on growth); `checkHoglinSpawnRules` (not on nether wart block);
  `finalizeSpawn` (20% baby); `removeWhenFarAway` → always true; `getWalkTargetValue`
  (repellent-avoidance −1, crimson nylium +10 preference); `mobInteract` (sets
  `persistenceRequired` on any consuming interaction — keeps tamed/leashed/interacted
  hoglins from despawning); `getAttackAnimationRemainingTicks`/`handleEntityEvent(4)` (client
  animation replay); `finishConversion` (→ZOGLIN, 200-tick nausea); `isFood` (`HOGLIN_FOOD`
  tag); `checkHoglinSpawnRules`/`canBeHunted` (`isAdult() && !cannotBeHunted`, backing
  `CannotBeHunted` NBT field); `getBreedOffspring`/`canFallInLove` (blocked while
  `HoglinAi.isPacified`); sound overrides (ambient depends on current activity via
  `HoglinAi.getSoundForCurrentActivity`).
- `HoglinAi.java` (255 lines): activities CORE/IDLE/FIGHT/AVOID. IDLE:
  `BecomePassiveIfMemoryPresent(NEAREST_REPELLENT, 200)` (200-tick pacify near repellent
  blocks — gold blocks/warped fungus etc, `HOGLIN_REPELLENTS` tag),
  `AnimalMakeLove(HOGLIN, 0.6, 2)` (breeding), flee-from-repellent walk target,
  `StartAttacking`, adult-only flee-from-visible-adult-piglin (8-block desired distance,
  speed 0.4), `SetEntityLookTargetSometimes`, `BabyFollowAdult` (5-16 block follow range,
  speed 0.6), idle-movement `RunOne`. FIGHT: repellent-pacify check persists even mid-fight,
  breeding persists, `SetWalkTargetFromAttackTargetIfTargetOutOfReach`, adult `MeleeAttack`
  cooldown 40 vs baby cooldown 15, `StopAttackingIfTargetInvalid`, erase attack target if
  breeding started. AVOID (retreat): flee attack-target-turned-avoid-target at 15-block
  desired distance, speed 1.3 (`SPEED_MULTIPLIER_WHEN_RETREATING`), erase avoid-target once
  `wantsToStopFleeing` (adult hoglins stop fleeing once they no longer outnumbered by
  piglins). `onHitTarget` (non-baby only: if target is a piglin and piglins currently
  outnumber hoglins → retreat + broadcast retreat to nearby adult hoglins instead of
  fighting; else broadcast the attack target to nearby adult hoglins so they pile on).
  `wasHurtBy` (erase pacified/breed-target memories; baby → immediate retreat from attacker;
  adult → `maybeRetaliate`, which itself skips retaliating against a piglin while already in
  AVOID, and never retaliates against another hoglin). `piglinsOutnumberHoglins`/
  `isPosNearNearestRepellent`/`isPacified` are the three cross-cutting predicate helpers.
- `HoglinBase.java` (interface, 54 lines): `hurtAndThrowTarget` (adult damage = base/2 +
  random(base) — i.e. randomized 50-150%ish of attack damage, babies deal flat base damage;
  applies enchantment post-attack effects; adults additionally knock back via `throwTarget`)
  and `throwTarget` (knockback vector computed from attack-knockback minus target's
  knockback-resistance attribute, ±10° randomized horizontal angle, partial vertical
  push) — this is a generic "gore" attack usable by any `LivingEntity`, not hoglin-specific
  in its math, useful to port as a free function rather than an inherent method.

### Current Rust state (`pumpkin/src/entity/mob/hoglin.rs`, 71 lines)
`SwimGoal`, plain `MeleeAttackGoal`, a single `AvoidEntityGoal` merging vanilla's two
distinct piglin-avoidance distances/speeds into one (doc comment explicitly flags this
simplification and the missing adult/baby + `isPacified`/repellent gating),
`WanderAroundGoal`, `LookAtEntityGoal`, `RandomLookAroundGoal`, `ActiveTargetGoal` (player
only). Missing entirely: baby dimensions/attributes/`ageBoundaryReached` damage swap,
repellent pacification, breeding (`AnimalMakeLove`/`BabyFollowAdult`/`canFallInLove` gating),
`hurtAndThrowTarget` knockback attack (currently presumably falls back to generic melee
damage with no knockback-throw or randomized damage), attack-animation byte-event,
zombification-to-zoglin conversion, `isFood`/`HOGLIN_FOOD`, NBT fields
(`ImmuneToZombification`/`TimeInOverworld`/`CannotBeHunted`), outnumbered-retreat/broadcast
logic, custom sounds tied to activity.

### Target Rust design
- **Attack**: replace the generic `MeleeAttackGoal` damage path for hoglins with a
  `hoglin_gore_attack.rs` goal (or keep `MeleeAttackGoal` for targeting/reach and add a
  post-hit hook) implementing `HoglinBase::hurt_and_throw_target`/`throw_target` as free
  functions in a shared location (e.g. `pumpkin/src/entity/combat.rs` if one exists, else
  `pumpkin/src/entity/mob/hoglin.rs` initially) since the math is generic knockback, not
  Brain-dependent — this is a pure port, no Goal-system redesign needed. Must add the
  10-tick attack-animation counter + byte-event-4 broadcast + `handle_entity_event`
  override, matching the `attack_animation_remaining_ticks`/`aiStep` decrement pattern.
- **Repellent pacification**: new `hoglin_repellent_pacify.rs` — each tick, check for a
  nearby `HOGLIN_REPELLENTS`-tagged block within the 8-horizontal/4-vertical detection
  range (matches Piglin's identical `REPELLENT_DETECTION_RANGE_HORIZONTAL/VERTICAL`
  constants — factor the block-scan into one shared helper both mobs call, since the
  numbers and semantics are identical); while pacified, suppress the attack-target goal
  (`is_pacified()` gate) for 200 ticks after last proximity, and suppress `canFallInLove`.
  Model as an `AtomicI32` decrementing-cooldown field on `HoglinEntity`, same shape as
  Piglin's `admiring_disabled_ticks`.
- **Piglin avoidance**: split the current single `AvoidEntityGoal` call into two goals or
  parameterize by hoglin state at construction time — adults flee visible piglins at
  8 blocks/speed 0.4 while idle, and flee at 15 blocks/speed 1.3 once actually attacked
  (retreat state) with retaliation-outnumber logic (`onHitTarget`/`piglinsOutnumberHoglins`)
  deciding whether to fight or flee at all. Babies never fight piglins directly (`onHitTarget`
  is a no-op for babies) — gate the whole flee-vs-fight decision on adult/baby, and gate
  `AvoidEntityGoal`'s existing single-speed-pair design by adding a variant that supports a
  runtime-selectable "provoked" distance/speed pair, or add a second `AvoidEntityGoal`
  instance active only while an internal "recently hurt by piglin" flag/cooldown is set —
  prefer extending `AvoidEntityGoal` with an optional second (distance, speed) override over
  forking the whole goal.
- **Broadcast retreat/attack-target on hit**: same "no Brain, so direct world query + method
  call" pattern as Piglin's anger propagation — on `on_damage`/`onHitTarget`-equivalent, scan
  nearby adult hoglins and call a `set_avoid_target`/`set_attack_target` method on each
  directly instead of a memory broadcast.
- **Breeding**: reuse whatever `BreedGoal`/`AnimalMakeLove`-equivalent Pumpkin already has for
  other passive-breeding animals (check `pumpkin/src/entity/ai/goal/breed.rs` — it exists in
  the goal listing already) rather than writing hoglin-specific breeding logic; gate
  `can_fall_in_love` on `!is_pacified()`.
- **Conversion**: same helper as Piglin, target `EntityType::ZOGLIN`, 300-tick
  `time_in_overworld` counter, `HOGLIN_CONVERTED_TO_ZOMBIFIED` sound instead of the piglin
  sound. Needs the same `PIGLINS_ZOMBIFY`-equivalent predicate flagged above (shared
  dependency between this and section 1 — implement once, use twice).
- **Baby dimensions/attributes**: `ageBoundaryReached`-equivalent hook swapping
  `ATTACK_DAMAGE` base value (6.0 adult / 0.5 baby) and xp reward (5/3) on growth —
  check how Pumpkin currently handles baby-transition hooks for other `Animal`-like mobs
  (breeding growth) before adding a new hook point.

### Files to create/modify
- Modify: `pumpkin/src/entity/mob/hoglin.rs` (all the above wiring, NBT fields, sounds,
  conversion, baby handling).
- New: `pumpkin/src/entity/ai/goal/hoglin_gore_attack.rs` (or fold into a shared combat
  helper), `hoglin_repellent_pacify.rs`.
- Extend: `pumpkin/src/entity/ai/goal/avoid_entity.rs` with an optional provoked-state
  distance/speed override (shared with the retreat use case), rather than a hoglin-only
  fork.
- Shared: repellent-block-scan helper usable by both Piglin and Hoglin (identical detection
  range constants).
- Data: confirm `HOGLIN_FOOD`, `HOGLIN_REPELLENTS`, `NETHER_WART_BLOCK` spawn-rule check
  exist/are wired in `pumpkin-data`/`pumpkin-world`.

---

## 4. Strider

### Vanilla source
`Strider.java` (364 lines, `extends Animal implements ItemSteerable`). Key facts:
`checkStriderSpawnRules` (must be under lava with air above the lava surface);
`canUseSlot(SADDLE)` gated on alive+non-baby; `registerGoals` (`PanicGoal(1.65)`,
`BreedGoal(1.0)`, `TemptGoal(1.4, STRIDER_TEMPT_ITEMS, false)` stored in a field for
`isBeingTempted()` checks, `StriderGoToLavaGoal(1.0)` — a private inner `MoveToBlockGoal`
subclass seeking exposed lava with pathfindable air above, re-evaluating its path every 20
ticks (`shouldRecalculatePath`) and blocked entirely while already in lava,
`FollowParentGoal(1.0)`, `RandomStrollGoal(1.0, 60)`, two `LookAtPlayerGoal`s (players at
8 blocks, other striders at 8 blocks), `RandomLookAroundGoal`); `setSuffocating`/
`isSuffocating` (adds/removes a `-0.34F ADD_MULTIPLIED_BASE` `MOVEMENT_SPEED` modifier);
`canStandOnFluid` (lava only); baby dimensions; `getPassengerAttachmentPoint` (client-side
bob animation offset from walk-animation phase); `getControllingPassenger` (rider must be
holding `WARPED_FUNGUS_ON_A_STICK` while saddled to steer); `getDismountLocationForPassenger`
(searches 5 candidate horizontal escape angles × vertical column for a non-lava, valid-floor
dismount spot); `tickRidden` (locks rotation to controller, ticks the `ItemBasedSteering`
boost timer); `getRiddenInput` → always `(0,0,1)` (forward-only, no strafing);
`getRiddenSpeed` → `MOVEMENT_SPEED attr * (isSuffocating() ? 0.35F : 0.55F) *
steering.boostFactor()`; `boost()` → `steering.boost(random)` (consumes warped-fungus-stick
durability-equivalent for a temporary speed burst, standard `ItemBasedSteering`
saddle-item-boost pattern shared with pigs/camels — check if Pumpkin already has an
`ItemBasedSteering`-equivalent for pigs before writing a strider-specific one);
`checkFallDamage` → suppressed entirely while in lava; `tick()` (happy sound while tempted,
retreat sound while panicking, `setSuffocating` recomputed every tick from
`STRIDER_WARM_BLOCKS` tag / lava-fluid-height check / riding-a-non-suffocating-strider
check, then `floatStrider()`); `floatStrider` (while in lava: if there's exposed-to-air
lava surface directly above and the block above isn't also lava, snap `onGround=true`, else
apply a gentle upward + damping velocity nudge — this is the "ride the lava surface like a
boat" buoyancy); custom `StriderPathNavigation` (treats lava as a stable/walkable
destination, in addition to normal ground); `getWalkTargetValue` (lava tile = 10 preference,
in-lava-already = `NEGATIVE_INFINITY` i.e. don't path-plan while already there — pathing
targets are picked from outside); `mobInteract` (non-food + saddled + not already ridden +
not sneaking → mount; else default interact, including saddle-equip via
`isEquippableInSlot`+`interactLivingEntity`; food interaction plays `STRIDER_EAT`);
`finalizeSpawn` (non-baby: 1/30 chance spawn a zombified-piglin jockey holding a
warped-fungus-on-a-stick + force-saddle the strider + guarantee the saddle drops on death;
else 1/10 chance spawn a baby-strider jockey (`setAge(-24000)`); else default group data) —
`spawnJockey` helper snaps the jockey to the strider's position/rotation, finalizes its
spawn, and calls `startRiding(this, true, false)`.

### Current Rust state (`pumpkin/src/entity/passive/strider.rs`, 52 lines)
`SwimGoal`, `EscapeDangerGoal(1.65)` (panic-equivalent), `WanderAroundGoal(1.0)`,
`LookAtEntityGoal` (player, 6.0 — vanilla uses 8.0, minor discrepancy), `RandomLookAroundGoal`.
Missing entirely: `TemptGoal`+`isBeingTempted` tracking (note: a generic `TemptGoal` already
exists at `pumpkin/src/entity/ai/goal/tempt.rs` and takes an item allowlist — reuse it,
don't write a new one, just need `STRIDER_TEMPT_ITEMS` tag data and a way to query
"is this goal currently running" the way vanilla's `isBeingTempted()` does), breeding
(`BreedGoal`), `StriderGoToLavaGoal`, `FollowParentGoal` (also already exists per the goal
file listing — reuse), lava floating/buoyancy, suffocation speed penalty, saddle/steering,
fall-damage immunity in lava, custom lava-aware pathing, jockey spawning, all sound overrides
beyond whatever default `Animal`/`Mob` provides.

### Target Rust design
- **Lava floating** (`floatStrider`): a per-tick hook in the Strider's `mob_tick` (or
  wherever Pumpkin runs per-entity physics-adjacent logic each tick — check the existing
  `mob_tick`/`aiStep`-equivalent call site used by e.g. Hoglin's attack-animation decrement)
  replicating the buoyancy check against `CollisionContext`/liquid-collision-shape
  equivalents in `pumpkin-world`. **Needs verification** that Pumpkin's fluid/collision
  APIs expose an equivalent to `context.isAbove(liquidCollisionShape, pos, true)` and
  `getFluidState(pos.above())` before assuming a 1:1 port is mechanical.
- **Suffocation**: `set_suffocating`/`is_suffocating` with a `-0.34` `ADD_MULTIPLIED_BASE`
  movement-speed attribute modifier, recomputed every tick from `STRIDER_WARM_BLOCKS` tag +
  lava-fluid-height + mounted-on-non-suffocating-strider. Straightforward port once the
  attribute-modifier API (already used elsewhere, e.g. Piglin's baby speed modifier) is
  confirmed to support add/remove-by-id semantics for striders too.
- **Steering**: needs an `ItemBasedSteering`-equivalent (boost timer + `boostFactor()`) —
  **check whether pigs (`saddle`+steering) already have this in Pumpkin** before writing a
  new one; if pigs don't ride yet either, this is genuinely new shared infra and should live
  somewhere generic (e.g. `pumpkin/src/entity/ai/steering.rs`) rather than inside
  `strider.rs`, since vanilla itself shares `ItemBasedSteering` across pig/strider/camel.
  `getControllingPassenger`/`getRiddenInput`/`tickRidden`/`getRiddenSpeed`/`boost()` are thin
  wrappers around that shared steering state plus the `WARPED_FUNGUS_ON_A_STICK` check and
  the forward-only `(0,0,1)` input — port directly once steering infra exists.
- **Fall damage immunity in lava**: a one-line override in the fall-damage check path
  (`checkFallDamage`-equivalent) — trivial once that hook is located.
- **`StriderGoToLavaGoal`**: new `pumpkin/src/entity/ai/goal/strider_go_to_lava.rs`, built on
  whatever `MoveToBlockGoal`-equivalent Pumpkin has (check `move_to_target_pos.rs` in the
  goal listing — likely the right base to extend/wrap) with the lava-specific
  `isValidTarget`/`canUse`/`canContinueToUse`/`shouldRecalculatePath` overrides.
- **Custom pathing** (`StriderPathNavigation`: lava counts as a stable destination): check
  how Pumpkin's `Navigator`/pathfinder (`pumpkin/src/entity/ai/pathfinder.rs`,
  `NavigatorGoal` referenced throughout the goal files) exposes per-mob path-type malus —
  Piglin already calls `setPathfindingMalus(PathType.FIRE_IN_NEIGHBOR, ...)` in vanilla, so
  Pumpkin likely has *some* malus-table mechanism; confirm it can express "lava is walkable"
  (not just "less penalized") since that's a stronger statement than a malus adjustment —
  may need the same kind of navigator subclass vanilla uses (`isStableDestination`
  override), not just a malus tweak.
- **Jockey spawning** (zombified-piglin-on-strider, baby-strider-on-strider): this is the
  first jockey implementation in the codebase (see Cross-cutting section). Design as a
  `finalize_spawn`-time hook (wherever that hook ends up existing, shared with Piglin's
  equipment-on-spawn needs — strongly consider building the `finalize_spawn` hook and the
  jockey helper together, since both this doc's Piglin section and this section need it) that:
  rolls 1/30 for a zombified-piglin rider (holding warped-fungus-on-a-stick, forces a saddle
  onto the strider, marks the saddle as a guaranteed death-drop — check whether "guaranteed
  drop" tracking exists in Pumpkin's item/equipment model, might not), else 1/10 for a
  baby-strider rider (age forced to the "just spawned, long time to grow up" value), then
  calls the existing `add_passenger`/mount path on `Entity` to seat the jockey. This reuses
  riding primitives but must build jockey-spawn-selection logic from scratch.

### Files to create/modify
- Modify: `pumpkin/src/entity/passive/strider.rs` (goal set, suffocation, lava floating,
  fall-damage immunity, sounds, spawn rules if not already correct, jockey hook wiring).
- New: `pumpkin/src/entity/ai/goal/strider_go_to_lava.rs`.
- Possibly new (shared, verify pigs first): `pumpkin/src/entity/ai/steering.rs`
  (`ItemBasedSteering`-equivalent).
- Possibly new (shared with Piglin): a `finalize_spawn` hook mechanism and a jockey-spawn
  helper.
- Data: confirm `STRIDER_TEMPT_ITEMS`, `STRIDER_FOOD`, `STRIDER_WARM_BLOCKS` tags exist.

---

## Shared infrastructure summary

| Need | Exists today? | Where | Action |
|---|---|---|---|
| Goal/GoalSelector framework | Yes | `pumpkin/src/entity/ai/goal/` | Reuse as-is |
| Brain/memory/sensor/activity system | **No** | — | Do not port; Goal-based approximations per-mob (this doc) |
| Entity-type conversion (morph to another mob type) | Partial (one hand-rolled instance) | `zombie_villager.rs` | Factor into a shared helper, reused by Piglin→Zombified Piglin and Hoglin→Zoglin |
| Riding/mount/passenger primitives | Yes | `pumpkin/src/entity/mod.rs` (`passengers`, `add_passenger`, recursive teleport) | Reuse as-is |
| Jockey spawn-selection (roll + seat rider at spawn time) | **No** | — | New; first use will be Strider's zombified-piglin/baby-strider jockeys; needs a `finalize_spawn`-equivalent hook too (also missing) |
| `EntitySpawnReason`/spawn-reason plumbing | **No** | flagged in `mob/mod.rs`, `natural_spawner.rs` | Out of scope to fully port; jockey spawning can proceed without it if jockey creation is triggered directly rather than through a spawn-reason-aware pipeline |
| Generic melee/ranged attack goals | Yes | `melee_attack.rs`, `ranged_crossbow_attack.rs`, `spear_use.rs` | Reuse for Piglin combat |
| Avoid/flee goal | Yes, single-speed-pair only | `avoid_entity.rs` | Extend with a provoked-state override rather than forking, for Hoglin's two-tier flee |
| Tempt/breed/follow-parent goals | Yes | `tempt.rs`, `breed.rs`, `follow_parent.rs` (present in goal listing) | Reuse for Strider |
| Item-based steering (boost via held item while ridden) | Unknown — check pigs | — | Verify before writing; if absent, this is new shared infra, not strider-specific |
| Attribute modifiers (transient, by id) | Yes (used by Piglin baby speed) | pattern in `piglin.rs`/vanilla parity | Reuse for Strider suffocation and Piglin baby speed |
| Repellent-block detection (8H/4V radius scan) | **No** | — | New; shared by Piglin and Hoglin, write once |
| `finalize_spawn`-equivalent per-mob-type spawn hook | Unknown — likely absent | — | Verify; needed by Piglin equipment/weapon roll, Hoglin baby roll, Strider jockeys |
| Nether/zombification-environment predicate (`PIGLINS_ZOMBIFY`) | **No** | — | Needs wiki/source verification of exact rule, then a new `pumpkin-world` predicate; blocks both Piglin and Hoglin conversion |

## Items needing wiki/source verification before implementation

1. Exact `EnvironmentAttributes.PIGLINS_ZOMBIFY` predicate (which dimensions/biomes/light
   conditions suppress zombification) — affects both Piglin and Hoglin conversion timers.
2. Whether Pumpkin's fluid/collision APIs can express `floatStrider`'s buoyancy check
   (`CollisionContext.isAbove`, liquid collision shape) without new pumpkin-world plumbing.
3. Whether pigs (or any existing Pumpkin mob) already implement `ItemBasedSteering`-style
   boost riding — determines whether Strider steering is a port or new shared infra.
4. Whether a `finalize_spawn`/per-type spawn-hook mechanism exists anywhere in Pumpkin's
   mob-spawning path — determines how much new plumbing the equipment-roll and jockey-spawn
   designs above actually require.
5. Whether Pumpkin's navigator/pathfinder can express "fluid X is a stable destination" (not
   just a malus discount) for Strider's lava-walking pathing.
6. Confirm exact current contents of `PIGLIN_LOVED` (item tag, wider than gold ingot) since
   `piglin_admire.rs`'s doc comments only reference gold ingot — needs the tag's current
   Pumpkin-data JSON contents cross-checked against `data/minecraft/tags/item/piglin_loved.json`.

## Rough complexity summary

- **Piglin**: highest complexity of the four — bartering inventory, crossbow/spear combat
  branching, dancing/celebration, baby-riding, anger propagation, and conversion all land on
  one mob. Most of the individual pieces have Goal-level precedent to build from
  (`RangedCrossbowAttackGoal`, `spear_use.rs`, `AvoidEntityGoal`) but wiring them together
  correctly (priority order, mutual exclusion between admire/fight/celebrate/avoid states)
  is the hard part without a Brain/Activity scheduler to arbitrate.
- **Piglin Brute**: low-to-medium complexity — mostly a restricted Piglin (no bartering, no
  ranged combat, no dancing) plus one new primitive (home tether) and shared retaliation
  logic with Piglin.
- **Hoglin**: medium complexity — attack/knockback math and repellent pacification are
  straightforward ports, but the adult/baby + outnumbered-piglins flee/fight decision tree
  and the two-tier avoid-goal (idle-distance vs retreat-distance) require real design work
  since `AvoidEntityGoal` only supports one distance/speed pair today.
- **Strider**: medium complexity concentrated in a few deep mechanics (lava buoyancy,
  item-based steering, jockey spawning) rather than breadth — three of those four are either
  entirely new infra or need verification that supporting APIs exist at all, making the
  actual risk here more about "does pumpkin-world/pumpkin-inventory support this" than
  "is the vanilla logic complex."
