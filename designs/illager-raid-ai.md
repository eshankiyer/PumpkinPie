# Illager/Raid AI design: Illusioner, Vindicator, Pillager, Ravager, Witch, Vex

Vanilla source: `/tmp/pumpkin-vanilla-26.2/decompiled/net/minecraft/world/entity/monster/{illager/Illusioner,illager/Vindicator,illager/Pillager,Ravager,Witch,Vex}.java`,
plus `illager/AbstractIllager.java`, `illager/SpellcasterIllager.java`, `raid/Raider.java`, `raid/Raid.java`.

Rust source under audit: `pumpkin/src/entity/mob/{illusioner,vindicator,pillager,ravager,witch,vex}.rs`,
`pumpkin/src/entity/ai/goal/witch_attack.rs`, `pumpkin/src/entity/ai/goal/evoker_spell.rs` and
`spellcaster.rs` (reference implementations to copy the pattern from), `pumpkin/src/world/raid.rs`
and `pumpkin/src/entity/living.rs` (existing raid infra).

Every one of the six current Rust files follows the same shape: a `struct FooEntity { mob_entity:
MobEntity }`, an empty `impl NBTStorage for FooEntity {}`, and a `registerGoals`-equivalent
constructor that wires only `SwimGoal`/`WanderAroundGoal`/`LookAtEntityGoal`/`RandomLookAroundGoal`
plus (for some) a single generic attack goal. None of the six overrides `Mob::can_be_raid_leader`
except Ravager (`false`, already correct), and **no code path anywhere calls
`Raider::applyRaidBuffs`** — `Raid::spawn_group` in `pumpkin/src/world/raid.rs` only equips the
wave leader's banner, never the per-wave weapon-enchant buff every one of these six mobs is
supposed to receive. That gap is called out once here and inherited by every subsystem section
below rather than re-described per mob.

## Shared raid infrastructure (prerequisite for Vindicator, Pillager, Witch sections)

**This already exists** — `pumpkin/src/world/raid.rs` (`Raid`, `RaidManager`) plus
`LivingEntity::raid_membership : AtomicCell<Option<RaidMembership>>` and
`LivingEntity::can_join_raid : AtomicBool` in `pumpkin/src/entity/living.rs` (lines ~182-187).
`RaidMembership { raid_id: i32, wave: i32, is_patrol_leader: bool }` is set in `Raid::join_raid`
(`raid.rs:450-471`) and cleared on death (`living.rs:1502-1515`). This is a full wave-spawning,
bossbar, omen-absorption, victory/loss engine — not a stub. Nothing further needs to be invented;
what's missing is **wiring individual mobs' AI up to state that already exists**, plus two small
additions to `Raid`/`RaidManager`:

1. **`Raid::apply_raid_buffs` is never called.** `Raid::spawn_group` (`raid.rs:368-448`) must call
   `mob.apply_raid_buffs(wave, is_captain)` on each freshly spawned raider immediately after
   `join_raid`, mirroring vanilla `Raid.joinRaid` → `entity.applyRaidBuffs(this.groupsSpawned,
   isPatrolLeader)` — `Raid.java` around `joinRaid`. This requires adding an `apply_raid_buffs`
   method to the `Mob` trait (default no-op) that Vindicator/Pillager override; Illusioner/Ravager/
   Witch vanilla overrides are empty methods, so their Rust default no-op is already correct parity
   and needs no override.
2. **A cheap "is there an active raid at this position, and what's its enchant-odds/wave" query**
   is needed by goals that run every tick and can't take the `RaidManager` mutex from inside a
   `Goal::can_start`/`tick` future that already holds other locks. Recommend exposing it as a
   method directly on `LivingEntity` backed by the already-stored `raid_membership`, no new state:
   ```rust
   impl LivingEntity {
       /// Vanilla: Raider::hasActiveRaid (getCurrentRaid() != null && raid.isActive()).
       /// Reads the cached RaidMembership captured at spawn time instead of re-querying
       /// RaidManager; membership is cleared on death (see living.rs:1502) so `is_some()`
       /// here is a reasonable proxy for `hasActiveRaid` for a live mob.
       pub fn raid_wave(&self) -> Option<i32> {
           self.raid_membership.load().map(|m| m.wave)
       }
       pub fn has_active_raid(&self) -> bool {
           self.raid_membership.load().is_some()
       }
   }
   ```
   Note this is an approximation: vanilla's `hasActiveRaid` also requires `raid.isActive()`
   (`Raid.java`), i.e. it goes false once the raid is stopped/over even before the entity's
   membership is cleared. Since `Raid::remove_from_raid`/death-time clearing already tracks the
   common case, and a per-tick goal query into `RaidManager` (async mutex) from inside synchronous
   `Goal` trait methods is awkward given the current `Goal` trait's `can_start`/`tick` are already
   `async`-via-`GoalFuture`, the alternative — actually taking the `world.raids` lock and looking up
   `is_active()`/`is_over()` per tick — is also viable and more exactly matches vanilla; call this
   out for whichever implementer picks up Vindicator/Pillager/Witch to decide, since it is a small
   correctness/perf tradeoff, not an architecture one. Either way, all three mobs (and any future
   raid-gated goal) should consume the *same* helper, not reinvent it.
3. `Raid::should_spawn_bonus_group`/wave number and `Raid::get_enchant_odds` (vanilla
   `Raid.getEnchantOdds`, called from every `applyRaidBuffs` override) do not exist on the Rust
   `Raid` struct yet and must be added — `Raid.java`'s `getEnchantOdds()` returns
   `0.3f + (float)Math.min(this.numGroups, this.groupsSpawned) * 0.02f` roughly scaled by omen
   level (verify exact formula against `Raid.java` at implementation time; it wasn't reproduced
   above from memory per the "cite current vanilla source" rule — re-open `Raid.java` and copy the
   real body of `getEnchantOdds`). `Raid` needs a `pub fn enchant_odds(&self) -> f32` and
   `pub fn num_groups(&self, difficulty: Difficulty) -> i32` (already effectively
   `num_groups_for_difficulty`, just needs a public accessor bound to `self.num_groups`/the
   difficulty-parameterized vanilla overload used by wave-5 gating) exposed for the
   Vindicator/Pillager `applyRaidBuffs` enchant-provider selection below.

Files to modify for this section: `pumpkin/src/world/raid.rs` (add `apply_raid_buffs` call site in
`spawn_group`, add `enchant_odds`/`num_groups` accessors), `pumpkin/src/entity/living.rs` (add
`raid_wave`/`has_active_raid` convenience methods), `pumpkin/src/entity/mob/mod.rs` (add
`apply_raid_buffs` to the `Mob` trait with a no-op default).

---

## Illusioner

Vanilla: `Illusioner extends SpellcasterIllager implements RangedAttackMob`. Key members:
- `registerGoals`: `FloatGoal(0)`, `SpellcasterCastingSpellGoal(1)`, `AvoidEntityGoal<Creaking>(3,
  8f, 1.0, 1.2)`, `IllusionerMirrorSpellGoal(4)`, `IllusionerBlindnessSpellGoal(5)`,
  `RangedBowAttackGoal(6, 0.5, 20, 15f)`, `RandomStrollGoal(8, 0.6)`, `LookAtPlayerGoal(9/10)`.
  Target selector: `HurtByTargetGoal(Raider).setAlertOthers()`, `NearestAttackableTargetGoal<Player>`,
  `<AbstractVillager>`, `<IronGolem>`.
- `finalizeSpawn`: gives `MAINHAND = ItemStack(Items.BOW)`.
- `performRangedAttack(LivingEntity, float)`: builds an arrow via `ProjectileUtil.getMobArrow`,
  computes lead the same way `AbstractSkeleton` does, spawns with power 1.6 and inaccuracy
  `14 - difficulty*4`, plays `SoundEvents.SKELETON_SHOOT`.
- `getArmPose()`: `SPELLCASTING` while casting, else `BOW_AND_ARROW` if aggressive else `CROSSED`.
- `aiStep`: **entirely client-side** (`this.level().isClientSide()` guard) — computes
  `clientSideIllusionOffsets`/`clientSideIllusionTicks` used only by the renderer to draw four fake
  mirror-image illusioner copies while invisible. `getIllusionOffsets(float)` interpolates them for
  render.
- `IllusionerMirrorSpellGoal extends SpellcasterUseSpellGoal`: `canUse` gated on not already having
  `INVISIBILITY`; casting time 20, interval 340; `performSpellCasting` applies
  `MobEffectInstance(INVISIBILITY, 1200)` to self; prepare sound `ILLUSIONER_PREPARE_MIRROR`; spell
  id `DISAPPEAR`.
- `IllusionerBlindnessSpellGoal extends SpellcasterUseSpellGoal`: `canUse` additionally requires a
  target, requires the target changed since last cast (`lastTargetId`), and requires local
  difficulty `> NORMAL` (i.e. HARD only); casting time 20, interval 180; `performSpellCasting`
  applies `MobEffectInstance(BLINDNESS, 400)` to the target; prepare sound
  `ILLUSIONER_PREPARE_BLINDNESS`; spell id `BLINDNESS`.
- `applyRaidBuffs` is an empty override (no-op — matches the Rust default once the trait method
  exists, see shared section).

### Target Rust design

`pumpkin/src/entity/mob/illusioner.rs` should mirror `evoker.rs`'s structure exactly, since Evoker
is the one other `SpellcasterIllager` already ported (`spellcaster::SpellcasterState`,
`ai::goal::evoker_spell::{EvokerCastingSpellGoal, ...}` are the template). Concretely:

- `IllusionerEntity` gains `pub spellcaster: SpellcasterState` (reuse
  `crate::entity::ai::goal::spellcaster::SpellcasterState` verbatim — it's already
  mob-type-agnostic) and `last_blindness_target_id: AtomicCell<Option<i32>>` (mirrors
  `IllusionerBlindnessSpellGoal.lastTargetId`).
- New file `pumpkin/src/entity/ai/goal/illusioner_spell.rs` (sibling of `evoker_spell.rs`) with:
  - `IllusionerCastingSpellGoal` — near-identical copy of `EvokerCastingSpellGoal` (stop
    navigation while casting, look at target). No `wololo_target` fallback needed (Illusioner has
    none).
  - `IllusionerMirrorSpellGoal` — uses `SpellCastTimer`, gate: `!has_effect(INVISIBILITY)`,
    `perform_spell_casting` calls `living_entity.add_effect(Effect { INVISIBILITY, duration:
    1200, .. })`.
  - `IllusionerBlindnessSpellGoal` — gate: target exists, target id differs from
    `last_blindness_target_id`, **and** local difficulty at the illusioner's block position is
    `Difficulty::Hard` (Pumpkin's `Difficulty` enum has no direct `Normal.ordinal()+1` comparison —
    use `matches!(difficulty, Difficulty::Hard)`, since the only difficulty "harder than Normal" is
    Hard; verify this is the only case against `Difficulty::isHarderThan` — cite `Difficulty.java`'s
    `ordinal()` order if unsure, `PEACEFUL < EASY < NORMAL < HARD`). `perform_spell_casting` applies
    `BLINDNESS` for 400 ticks to the target and stores its id in `last_blindness_target_id`.
- `RangedBowAttackGoal` **already exists** at `pumpkin/src/entity/ai/goal/ranged_bow_attack.rs` and
  is used by skeleton-family mobs with the exact vanilla timing/lead-calculation Illusioner needs
  (`RangedBowAttackGoal::new(20, 15.0)` matches `new RangedBowAttackGoal<>(this, 0.5, 20, 15.0F)`
  modulo the move-speed parameter Pumpkin's goal doesn't take — check whether Pumpkin's version
  needs a speed parameter added, since vanilla's ctor takes `moveSpeedAmp=0.5` for the
  approach-navigation call and the current `RangedBowAttackGoal::tick` hardcodes `speed: 1.0` in its
  `NavigatorGoal` — this is a latent bug in the shared skeleton goal, not Illusioner-specific; either
  add a `speed` field to `RangedBowAttackGoal` (breaking every current caller) or accept the
  discrepancy and note it, since fixing it is out of this design's scope per the "one logical
  fix per branch" project rule).
- `finalize_spawn`/equipment: give `MAINHAND = ItemStack(Item::BOW)` at construction (same pattern
  Pillager already uses inline in `PillagerEntity::new` for its crossbow).
- `get_arm_pose`: **no client rendering exists in Pumpkin at all** (server-only; entity pose is
  purely a client renderer concern in vanilla, driven by `getArmPose()` feeding
  `IllagerRenderer`). There is currently no `arm_pose`/`ArmPose` concept anywhere in the codebase.
  Recommend **not** porting `getArmPose`/`getIllusionOffsets`/the client-side `aiStep` mirror-image
  math at all — they have zero server-observable effect (no gameplay hook reads them), and nothing
  in Pumpkin's protocol layer currently sends whatever synced byte would drive it. Document this as
  an explicit scope cut rather than silently dropping it.
- Goal registration in `IllusionerEntity::new` (priorities matching vanilla's `registerGoals`):
  `0: SwimGoal`, `1: IllusionerCastingSpellGoal`, `3: AvoidEntityGoal` (already exists generically —
  needs a `&EntityType::CREAKING` instantiation, `AvoidEntityGoal::new(&EntityType::CREAKING, 8.0,
  1.0, 1.2)`), `4: IllusionerMirrorSpellGoal`, `5: IllusionerBlindnessSpellGoal`,
  `6: RangedBowAttackGoal::new(20, 15.0)`, `8: WanderAroundGoal::new(0.6)`,
  `9/10: LookAtEntityGoal`/`RandomLookAroundGoal` (already present).
- Target selector: current file already has `RevengeGoal`, `ActiveTargetGoal<Player>`,
  `<Villager>`, `<IronGolem>` wired at priorities that don't quite match vanilla's 1/2/3/3 — leave
  as-is unless doing a dedicated priority audit (out of scope here); vanilla's
  `HurtByTargetGoal(Raider).setAlertOthers()` (alerting other raiders) vs. Rust's
  `RevengeGoal::new(true)` — check `revenge.rs` supports the "alert others" flag; if it doesn't,
  that's a separate pre-existing gap common to all raiders, not Illusioner-specific.

**Files to create**: `pumpkin/src/entity/ai/goal/illusioner_spell.rs`.
**Files to modify**: `pumpkin/src/entity/mob/illusioner.rs`, `pumpkin/src/entity/ai/goal/mod.rs`
(register new module).

**Needs wiki/vanilla-source verification**: exact `Difficulty.isHarderThan(NORMAL.ordinal())`
semantics (confirm Hard-only); whether `RevengeGoal`'s `setAlertOthers` equivalent exists.

---

## Vindicator

Vanilla: `Vindicator extends AbstractIllager`. Key members:
- Fields: `TAG_JOHNNY = "Johnny"`, `DOOR_BREAKING_PREDICATE = d -> NORMAL || HARD`,
  `isJohnny: boolean = false`.
- `registerGoals`: `FloatGoal(0)`, `AvoidEntityGoal<Creaking>(1, 8f, 1.0, 1.2)`,
  `VindicatorBreakDoorGoal(2)`, `RaiderOpenDoorGoal(3)`, `HoldGroundAttackGoal(4, 10f)`,
  `MeleeAttackGoal(5, 1.0, false)`, `RandomStrollGoal(8, 0.6)`, `LookAtPlayerGoal(9/10)`. Target
  selector: `HurtByTargetGoal(Raider).setAlertOthers()(1)`,
  `NearestAttackableTargetGoal<Player>(2)`, `<AbstractVillager>(3)`, `<IronGolem>(3)`,
  `VindicatorJohnnyAttackGoal(4)`.
- `customServerAiStep`: `getNavigation().setCanOpenDoors(level.isRaided(blockPosition()))` each
  tick, i.e. **door-opening is gated on "any raid at this position"**, not specifically *this
  mob's* raid.
- `VindicatorBreakDoorGoal extends BreakDoorGoal(mob, 6, DOOR_BREAKING_PREDICATE)`: `canUse`/
  `canContinueToUse` both additionally require `vindicator.hasActiveRaid()`; the difficulty
  predicate (`NORMAL || HARD`) is the `BreakDoorGoal` base class's own gate (checks the *current*
  local difficulty against the predicate), i.e. door-breaking never happens on Easy/Peaceful
  regardless of raid state.
- `populateDefaultEquipmentSlots`: **only** gives `MAINHAND = IRON_AXE` `if (getCurrentRaid() ==
  null)` — i.e. natural/non-raid-spawned vindicators get a plain axe; raid-spawned ones get their
  axe from `applyRaidBuffs` instead (see below), never from this method.
- `setCustomName(Component name)`: override that, once, sets `isJohnny = true` if
  `name.getString().equals("Johnny")` and it wasn't already true (idempotent one-way latch).
  `addAdditionalSaveData`/`readAdditionalSaveData`: persist `"Johnny"` boolean tag only if true.
- `VindicatorJohnnyAttackGoal extends NearestAttackableTargetGoal<LivingEntity>(mob,
  LivingEntity.class, 0, true, true, (target, level) -> target.attackable())`: `canUse` additionally
  requires `isJohnny`. This is a target-*any*-`LivingEntity` goal, unlike the other
  `NearestAttackableTargetGoal`s in this file which are type-specific.
- `applyRaidBuffs(level, wave, isCaptain)`: builds a fresh `ItemStack(IRON_AXE)`, rolls
  `random.nextFloat() <= raid.getEnchantOdds()`, and if it hits, enchants from
  `RAID_VINDICATOR_POST_WAVE_5` (if `wave > raid.getNumGroups(NORMAL)`) or `RAID_VINDICATOR`
  otherwise, then sets it as `MAINHAND` unconditionally (enchanted or not).
- `getArmPose`: `ATTACKING` if aggressive, else `CELEBRATING` if celebrating, else `CROSSED`.

### Target Rust design

- `VindicatorEntity` gains `is_johnny: AtomicBool` (default `false`).
- **Johnny detection**: there is no per-mob "on custom name changed" hook in Pumpkin (`Entity::
  set_custom_name` at `pumpkin/src/entity/mod.rs:1064` is a plain setter with no virtual-dispatch
  callback into `Mob`). Rather than adding a new hook to the `Entity`/`Mob` trait surface just for
  this, use the existing per-tick `Mob::mob_tick` hook (already used by `Vex`/`Evoker`, see
  `vex.rs`/`evoker.rs`) to lazily check-and-latch:
  ```rust
  fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
      Box::pin(async move {
          if !self.is_johnny.load(Relaxed)
              && let Some(name) = &**self.mob_entity.living_entity.entity.custom_name.load()
              && name.get_string() == "Johnny" // verify TextComponent's plain-string accessor name
          {
              self.is_johnny.store(true, Relaxed);
          }
      })
  }
  ```
  This is semantically equivalent to vanilla's setter-time latch (both are idempotent one-way
  flags; the only observable difference is up-to-one-tick latency, which is unobservable to a
  player). Needs verification: `TextComponent`'s plain-string extraction method name in
  `pumpkin-util` (vanilla compares `Component.getString()`, i.e. the flattened literal text).
- NBT persistence: implement `NBTStorage for VindicatorEntity` (currently the empty default) with
  `write_nbt` putting `"Johnny"` only if true, `read_nbt_non_mut` reading it back — same pattern as
  `creeper.rs`'s `"ignited"`/`"powered"` booleans shown above.
- **Door breaking**: replace the current always-on
  `InteractWithDoorGoal::new(false)` at priority 3 with two goals matching vanilla's split:
  - priority 2: a new `VindicatorBreakDoorGoal` (or a raid-gated wrapper around whatever "break
    door" primitive exists — note `interact_with_door.rs`'s `InteractWithDoorGoal` only *opens/
    closes* doors, it has no block-breaking logic at all; vanilla's `BreakDoorGoal` actually
    destroys the door block over time when it can't be opened. **This block-breaking primitive
    does not exist anywhere in Pumpkin's goal set** and must be authored from scratch, porting
    `net.minecraft.world.entity.ai.goal.BreakDoorGoal` (not in the files read for this survey —
    fetch and cite it before implementing), gated additionally on `hasActiveRaid()` (use the
    shared `has_active_raid()` helper from the raid-infra section) and on local difficulty being
    `Normal`/`Hard`.
  - priority 3: keep `InteractWithDoorGoal::new(false)` but wrap/gate it so it only fires
    `if hasActiveRaid()` per vanilla's `RaiderOpenDoorGoal.canUse() = super.canUse() &&
    hasActiveRaid()` — currently Vindicator's door-opening is unconditional, which is the parity
    bug named in the task brief. Simplest fix: add an `only_during_raid: bool` constructor flag to
    `InteractWithDoorGoal` (default `false`, Vindicator passes `true`) rather than a wrapper type,
    to avoid a second goal-dispatch layer.
- `customServerAiStep`-equivalent (`Mob::mob_tick`): set navigator's "can open doors" flag from
  `has_active_raid()` each tick — check whether Pumpkin's navigator even models "can open doors" as
  a distinct capability (`grep can_open_doors` came back empty in this survey; if absent, this is a
  navigator-level gap and should be filed as a prerequisite sub-task rather than silently
  approximated inside Vindicator).
- `Johnny` target goal: needs a **new "target any LivingEntity" goal**, since `ActiveTargetGoal`
  (`pumpkin/src/entity/ai/goal/active_target.rs`) is parameterized on one concrete
  `&'static EntityType` and its `find_closest_target` branches only on
  `target_type == &EntityType::PLAYER` vs. one other type — it cannot express "any type". Add
  `VindicatorJohnnyAttackGoal` in a new file, copying `ActiveTargetGoal`'s structure but iterating
  `world.get_nearby_entities(pos, follow_range)` unfiltered by type (already what the non-Player
  branch does internally minus the type filter), additionally gated on `self.is_johnny.load()`.
- `populate_default_equipment_slots`/spawn-time axe: only equip `IRON_AXE` at construction
  `if raid_membership is None` (mirrors vanilla's `getCurrentRaid() == null` check) — since
  `VindicatorEntity::new` runs before the entity has any chance to join a raid, and raid-spawned
  vindicators are constructed directly by `Raid::spawn_group` (`raid.rs:381-443`) which should call
  `apply_raid_buffs` right after `join_raid` per the shared-infra section, the axe-then-immediately-
  overwritten-by-raid-buff sequencing matches vanilla's `finalizeSpawn` → later `joinRaid` →
  `applyRaidBuffs` ordering as long as `join_raid`+`apply_raid_buffs` happens after
  `VindicatorEntity::new`, which `raid.rs`'s current call order already guarantees.
- `apply_raid_buffs(&self, wave: i32, _is_captain: bool)` on `VindicatorEntity`: build
  `ItemStack::new(1, &Item::IRON_AXE)`, roll `rand::random::<f32>() <= raid.enchant_odds()` (needs
  the `Raid` accessor from the shared-infra section — note this requires passing `&Raid` or the
  enchant-odds value into `apply_raid_buffs`, i.e. the `Mob::apply_raid_buffs` trait signature
  should take `(&self, wave: i32, is_captain: bool, enchant_odds: f32)` or similar, since a `Mob`
  impl has no direct handle back to the `Raid`/`RaidManager` it belongs to), enchant from
  `RAID_VINDICATOR_POST_WAVE_5` or `RAID_VINDICATOR` depending on `wave > num_groups(Normal)`, set
  `MAINHAND`. **Needs verification**: whether Pumpkin's enchantment system has equivalents of
  `VanillaEnchantmentProviders.RAID_VINDICATOR{,_POST_WAVE_5}` / `EnchantmentHelper
  .enchantItemFromProvider` — this survey did not check `pumpkin-data`'s enchantment-provider
  registry; if absent, the enchant step is a separate prerequisite (probably shared with Pillager's
  identical mechanism below).
- `get_arm_pose`: skip per the "no client rendering" note in the Illusioner section — same
  reasoning applies uniformly to all four illager-family mobs here.

**Files to create**: `pumpkin/src/entity/ai/goal/break_door.rs` (or extend
`interact_with_door.rs`), `pumpkin/src/entity/ai/goal/johnny_attack.rs` (or fold into a generalized
"any-type" target goal shared with Vex's copy-owner-target goal, see Vex section).
**Files to modify**: `pumpkin/src/entity/mob/vindicator.rs`,
`pumpkin/src/entity/ai/goal/interact_with_door.rs` (raid-gating flag),
`pumpkin/src/entity/mob/mod.rs` (`Mob::apply_raid_buffs` default).

**Needs wiki/vanilla-source verification**: `BreakDoorGoal`'s exact break-progress/timing (not
read in this survey — it lives in `ai/goal/BreakDoorGoal.java`, fetch before implementing);
`VanillaEnchantmentProviders`/enchant-from-provider availability in `pumpkin-data`.

---

## Pillager

Vanilla: `Pillager extends AbstractIllager implements CrossbowAttackMob, InventoryCarrier`. Key
members:
- `IS_CHARGING_CROSSBOW: EntityDataAccessor<Boolean>` synced data; `INVENTORY_SIZE = 5`,
  `SLOT_OFFSET = 300`; `inventory: SimpleContainer(5)`.
- `registerGoals`: `FloatGoal(0)`, `AvoidEntityGoal<Creaking>(1)`, `HoldGroundAttackGoal(2, 10f)`,
  `RangedCrossbowAttackGoal(3, 1.0, 8f)`, `RandomStrollGoal(8, 0.6)`, `LookAtPlayerGoal(9, 15f)`,
  `LookAtPlayerGoal(10, Mob, 15f)`. Target selector as usual four goals.
- `isChargingCrossbow`/`setChargingCrossbow` drive `getArmPose()`'s `CROSSBOW_CHARGE` state — this
  is the "IS_CHARGING_CROSSBOW synced data" the task brief calls out; it must be readable by
  `RangedCrossbowAttackGoal`/`CrossbowAttackMob`'s shared charge/aim/fire state machine (not shown
  in `Pillager.java` itself — it's on the `CrossbowAttackMob` interface default methods, not read in
  this survey; the *actual* charge-tracking logic to port is `CrossbowAttackMob`'s default
  `performCrossbowAttack`, not anything Pillager-specific — fetch that interface's source before
  implementing).
- `getWalkTargetValue` returns `0.0F` unconditionally (vanilla comment-less; likely means "no
  preference," i.e. doesn't avoid/prefer any block for wandering).
- `getMaxSpawnClusterSize() = 1` (natural-spawn cluster cap, a world-gen/mob-spawner concern, not
  an AI goal — verify whether Pumpkin's spawner even reads per-type cluster size before treating
  this as in-scope).
- `pickUpItem(ServerLevel, ItemEntity)` override: banners always go through
  `super.pickUpItem` (the generic Raider ominous-banner-leader-pickup path); everything else only
  picked up `if wantsItem(itemStack)`, where `wantsItem = hasActiveRaid() &&
  itemStack.is(Items.BANNER.white())` — i.e. **pillagers only pick up plain white banners, and only
  during an active raid**, storing them in the 5-slot `inventory`, not equipment slots. This is the
  "5-slot banner inventory" the task brief names; currently `impl NBTStorage for PillagerEntity {}`
  is empty (confirmed) and there is no equivalent inventory field on `PillagerEntity` at all.
- `getSlot(int slot)`: exposes the internal inventory at protocol slot indices `300..305` (container
  UI mapping — likely low-priority/no-op in a headless-inventory-viewer sense unless Pumpkin already
  models "extra mob inventory slots" elsewhere; check `InventoryCarrier` equivalents before
  reinventing).
- `applyRaidBuffs`: rolls `enchantOdds`; if hit, builds fresh `ItemStack(CROSSBOW)`, picks
  `RAID_PILLAGER_POST_WAVE_5` (wave > numGroups(NORMAL)) / `RAID_PILLAGER_POST_WAVE_3` (wave >
  numGroups(EASY)) / **no enchant at all** (`provider = null`, item never gets set) otherwise —
  note this is subtly different from Vindicator's: if neither wave threshold is met, Pillager's
  `MAINHAND` is **not overwritten** at all (early-wave raiders keep their spawn crossbow), whereas
  the enchant-roll gate wraps the *entire* wave-selection, not just the enchant step.
- `enchantSpawnedWeapon` override (non-raid path): 1-in-300 chance (`random.nextInt(300) == 0`) to
  enchant the spawn crossbow from `PILLAGER_SPAWN_CROSSBOW` provider, called from `finalizeSpawn`'s
  `populateDefaultEquipmentEnchantments` step (the base-class hook), independent of raids entirely.
- `getArmPose`: `CROSSBOW_CHARGE` if charging, else `CROSSBOW_HOLD` if holding a crossbow, else
  `ATTACKING`/`NEUTRAL`.

### Target Rust design

- Add `is_charging_crossbow: AtomicBool` synced-data-backed field to `PillagerEntity`, following the
  `creeper.rs`/`mob/mod.rs` `Metadata`/`TrackedData` pattern shown for creeper's `powered`/`ignited`
  flags (`pumpkin_data::tracked_data::TrackedData`, `Metadata::new` broadcast on change) — this is
  the concrete mechanism to use for the "IS_CHARGING_CROSSBOW synced data" requirement; it is not a
  new pattern, just an unused-so-far one for Pillager specifically.
- Fetch and port `CrossbowAttackMob`'s default interface methods (not read in this survey) before
  writing the charge/hold/fire state machine — `RangedCrossbowAttackGoal` in Pumpkin
  (`ranged_crossbow_attack.rs`, read above) currently does an immediate cooldown-gated shoot with
  no separate "charging" phase or synced flag at all; this is the actual gap, more than anything
  Pillager-specific. Recommend implementing the charge/hold state machine once in
  `ranged_crossbow_attack.rs` (shared by Pillager and any future crossbow-user) rather than
  duplicating it per mob, setting `is_charging_crossbow` via a new `Mob`-trait callback or a
  Pillager-specific hook the goal calls if present.
- Add `banner_inventory: tokio::sync::Mutex<[Option<ItemStack>; 5]>` (or reuse whatever generic
  small-inventory type Pumpkin already has for mobs, e.g. check `InventoryCarrier`-equivalent before
  inventing a new shape) to `PillagerEntity`, replacing the currently-empty
  `impl NBTStorage for PillagerEntity {}` with real `write_nbt`/`read_nbt_non_mut` for it (pattern:
  `creeper.rs`'s bool/short/byte fields, generalized to a small item-stack list — check how
  `pumpkin-inventory`/other mobs with inventories, e.g. villagers or horses, serialize theirs, since
  a from-scratch NBT item-list format should match an existing convention rather than invent a
  third).
- Override `Mob`'s item-pickup hook (need to locate the generic pickup hook Pumpkin mobs use —
  not read in this survey, search `pick_up_item`/`on_item_pickup` in `mob/mod.rs`/`living.rs` before
  implementing) so Pillager only accepts `Item::WHITE_BANNER` and only
  `if living_entity.has_active_raid()` (shared helper), depositing into `banner_inventory` instead
  of an equipment slot, mirroring vanilla's early-return-to-`super.pickUpItem` for the ominous-banner
  case (that path already exists generically on `Raider`/base pickup and should not be
  reimplemented — only the "else" branch is Pillager-specific).
- `apply_raid_buffs`: same shape as Vindicator's, but note the "no-op if wave too low" branch —
  implement as an `Option<EnchantProvider>` selection that, if `None`, skips the `set_item_slot`
  call entirely (unlike Vindicator, which always sets the axe).
- `enchant_spawned_weapon`-equivalent: a 1-in-300 roll at spawn time (non-raid path only) enchanting
  the crossbow from a `PILLAGER_SPAWN_CROSSBOW` provider — wire into `PillagerEntity::new` or
  whatever "finalize spawn" hook Pumpkin's mob factory (`entity/type.rs::from_type`) calls, if one
  exists; if Pumpkin's spawn pipeline currently has no `finalize_spawn` hook distinct from
  `EntityType::new`, this becomes a new hook needed by any mob wanting spawn-time-random equipment
  (a second small shared prerequisite, worth flagging to whoever implements this row).
- `get_walk_target_value`/`get_max_spawn_cluster_size`: low-priority world-gen/pathfinding-weight
  hooks; only port if the corresponding trait methods already exist on `Mob`/`PathAwareEntity` (this
  survey did not check) — otherwise treat as out-of-scope stretch goals, not core to the "5 subsystems
  missing" list.
- `get_arm_pose`: skip per Illusioner-section reasoning (no client rendering exists).

**Files to create**: none strictly required beyond extending `ranged_crossbow_attack.rs` in place.
**Files to modify**: `pumpkin/src/entity/mob/pillager.rs` (inventory field, NBT, pickup override,
`apply_raid_buffs`, spawn-time enchant roll, charging flag), `pumpkin/src/entity/ai/goal/
ranged_crossbow_attack.rs` (charge/hold state machine + synced-flag callback).

**Needs wiki/vanilla-source verification**: `CrossbowAttackMob`'s default methods (not read in this
survey — fetch `net/minecraft/world/entity/monster/CrossbowAttackMob.java` before implementing the
charge state machine); whether Pumpkin has any existing small-mob-inventory NBT convention to reuse;
whether a `finalize_spawn`-style hook exists for spawn-time-random enchants.

---

## Ravager

Vanilla: `Ravager extends Raider` (not `AbstractIllager` — no arm pose / spellcasting concerns).
Key members (`attackTick`, `stunnedTick`, `roarTick` state machine):
- `registerGoals`: `FloatGoal(0)`, `MeleeAttackGoal(4, 1.0, true)`,
  `WaterAvoidingRandomStrollGoal(5, 0.4)` — **current Rust uses `WanderAroundGoal::new(1.0)`,
  confirmed wrong speed (should be 0.4) and wrong goal type (should avoid water, not the generic
  wander)** — `LookAtPlayerGoal(6, Player, 6.0F)` — **current Rust uses `8.0` radius, confirmed
  wrong per task brief; vanilla is `6.0F`** — `LookAtPlayerGoal(10, Mob, 8.0F)` (this second one is
  already correct at `8.0`).
- `aiStep` (every tick, after `super.aiStep()`):
  1. Movement-speed lerp: if immobile, base value forced to `0.0`; else lerps the
     `MOVEMENT_SPEED` attribute's base value toward `0.35` (if has a target) or `0.3` (idle) at rate
     `Mth.lerp(0.1, current, max)` — i.e. **the "attack speed boost" is a continuous per-tick lerp
     of the attribute base value, not a flat instantaneous swap.**
  2. Leaf-destroying: only `if horizontalCollision && MOB_GRIEFING gamerule`, scans a
     `getBoundingBox().inflate(0.2)` block volume for `LeavesBlock` and calls
     `serverLevel.destroyBlock(pos, true, this)` on each; if nothing destroyed and `onGround()`,
     calls `jumpFromGround()` (i.e. it jumps to try to get over an obstacle it couldn't eat through).
  3. `roarTick` countdown: decrements; fires `roar()` at the exact tick it reaches `10` (not `0`) —
     this pre-fires the AoE 10 ticks before the roar animation/state actually ends.
  4. `attackTick` countdown: simple decrement, gates `isImmobile()`/`hasLineOfSight` while > 0.
  5. `stunnedTick` countdown: decrements, calls `stunEffect()` (client-facing tinted particle
     spawn) each tick while stunned; on the exact tick it reaches `0`, plays `RAVAGER_ROAR` sound and
     sets `roarTick = 20` (i.e. **being stunned always chains into a roar** once the stun ends).
- `roar()`: only runs `if isAlive() && level is ServerLevel`; picks
  `ROAR_TARGET_WITH_GRIEFING`/`_WITHOUT_GRIEFING` predicate by gamerule (excludes other Ravagers
  always; additionally excludes `ArmorStand` when griefing is off); for every `LivingEntity` in
  `getBoundingBox().inflate(4.0)` matching the predicate: deals `6.0F` mob-attack damage **unless**
  `entity instanceof AbstractIllager` (ravagers never hurt illagers with the roar — friendly fire
  immunity for illager-family only, not raiders broadly since Witch/other Ravagers aren't
  `AbstractIllager`), and applies `strongKnockback` to everyone **except Players**. Broadcasts
  entity event `69` (client roar-particle + knockback replay) and a `GameEvent.ENTITY_ACTION`.
- `blockedByItem` (vanilla's shield-block response hook): `if roarTick == 0`, 50/50 either stuns
  self for 40 ticks (+ sound + entity event `39` + push the shield-blocker) or applies
  `strongKnockback` to the blocker instead; either way sets `defender.hurtMarked = true`.
- `handleEntityEvent(byte)`: client/shared event dispatch — `4` → `attackTick=10` + play attack
  sound (this is also set directly in `doHurtTarget`, so the event exists for the *client* replay,
  but server-side both `doHurtTarget` and the event handler set the same field — Pumpkin, being
  server-authoritative with its own client-independent tick, likely only needs the direct
  `doHurtTarget` path, not the broadcast-event round-trip, **unless** Pumpkin's protocol needs the
  same "entity event" broadcast to trigger client-side animation — check whether Pumpkin already has
  an entity-event broadcast mechanism (`broadcastEntityEvent`) before deciding whether to model
  `handleEntityEvent` as a second code path or fold both into one server-side state transition).
  `39` → `stunnedTick=40`. `69` → client roar-particle spawn + `applyRoarKnockbackClient` (a
  *second*, client-only knockback pass over `ROAR_TARGET_ON_CLIENT`-filtered entities — since
  Pumpkin's world is authoritative server-side, this client-replay path can likely be skipped
  entirely; the server-side `roar()` already does the real knockback).
- `isImmobile()`: `super.isImmobile() || attackTick>0 || stunnedTick>0 || roarTick>0`.
- `hasLineOfSight`: `false` while `stunnedTick>0 || roarTick>0` (can't see/target while stunned or
  roaring).
- `doHurtTarget`: sets `attackTick=10`, broadcasts event `4`, plays attack sound, then does normal
  melee damage via `super.doHurtTarget`.
- `getAttackBoundingBox`: deflates the normal attack AABB by `(0.05, 0, 0.05)` — a minor
  precision tweak, low priority.
- `applyRaidBuffs`: empty override (no-op, correct already once the trait default exists).
- `canBeLeader() = false` — **already correctly overridden in the current Rust
  (`can_be_raid_leader` returns `false`)**.

### Target Rust design

This is the biggest single-mob chunk of new state/logic among the six. Recommend a dedicated
`RavagerState` struct (mirroring how `spellcaster::SpellcasterState` centralizes Illusioner/Evoker's
timer logic) holding the three counters, to keep `mob_tick` readable:

```rust
pub struct RavagerState {
    attack_tick: AtomicI32,
    stunned_tick: AtomicI32,
    roar_tick: AtomicI32,
}
```

with a `tick(&self, ravager: &RavagerEntity, world: &Arc<World>)` method run from
`Mob::mob_tick` that ports vanilla `aiStep`'s five numbered steps above, in order, using
`world.get_game_rule(GameRules::MOB_GRIEFING)` (verify exact API name),
`world.destroy_block`/leaf-detection over the inflated bounding box (check whether Pumpkin already
exposes a "get blocks in AABB" iteration helper — `interact_with_door.rs`'s door-scan and
`ranged_bow_attack.rs`'s raycast both suggest block-position iteration utilities exist; reuse
whichever one is idiomatic rather than hand-rolling `BlockPos::betweenClosed`).

- `MOVEMENT_SPEED` per-tick lerp needs `LivingEntity`/`Mob` access to mutate an attribute's *base
  value* continuously — confirm Pumpkin's attribute system supports `set_base_value`/`get_base_value`
  distinct from the *modified* value (not checked in this survey) before assuming this is a direct
  port; if attributes are immutable-after-spawn in Pumpkin's current model, this lerp becomes a
  larger prerequisite of its own.
- `roar()`: needs `world.get_entities_of_class`/bounding-box-inflate query (pattern already used by
  `HoldGroundAttackGoal`'s vanilla equivalent and Pumpkin's `active_target.rs`'s
  `get_nearby_entities`) filtering out other Ravagers (and `ArmorStand` when griefing off), skipping
  damage to `AbstractIllager`-equivalent (Pumpkin doesn't have an `AbstractIllager` marker type —
  needs either a `EntityTypeTags`-style tag check or a small `is_illager(&EntityType) -> bool`
  helper covering Vindicator/Pillager/Illusioner/Evoker/Vex — **confirm against
  `EntityTypeTags.ILLAGER_FRIENDS`/whatever vanilla actually tests here — the class check is
  `instanceof AbstractIllager`, not a tag, so the Rust equivalent should be an explicit type-id match
  list, not a tag lookup, unless Pumpkin's `pumpkin-data` already has an equivalent tag it uses
  elsewhere for the same distinction**), and knockback everyone but players.
- Shield-block response (`blockedByItem`): needs to hook into whatever "defender blocked with
  shield" combat code path already exists in Pumpkin (not located in this survey — search
  `blocked_by_item`/shield-block handling in `living.rs`/combat code before wiring this in; if no
  such hook exists yet, that is a bigger prerequisite than Ravager itself and should be flagged, not
  silently stubbed).
- `handle_entity_event`/broadcast events `4`/`39`/`69`: check whether Pumpkin's protocol layer has
  an "entity event" broadcast primitive (`CEntityStatus`/similar) — likely yes, given Pumpkin already
  broadcasts entity metadata (`Metadata::new` seen in `creeper.rs`); if so, model `attack_tick`/
  `stunned_tick` sets as direct field writes plus an outbound status-broadcast call at the same
  call site (`do_hurt_target`, `blocked_by_item`) rather than a separate `handle_entity_event`
  dispatcher, since Pumpkin's server is authoritative and doesn't need a receive-side handler for
  its own broadcasts.
- Movement/look-goal fixes (the two named tuning bugs): change `WanderAroundGoal::new(1.0)` at
  priority 5 to a water-avoiding variant at speed `0.4` — **check whether a
  `WaterAvoidingRandomStrollGoal` already exists anywhere in the goal list** (this survey's
  `ai/goal/` directory listing did not show one under that name — likely needs porting as a thin
  wrapper around `WanderAroundGoal` that additionally rejects water target positions, or, if no
  water-avoidance primitive exists at all, filing that as a small separate prerequisite reused by
  any other water-avoiding vanilla mob); change the `6.0F` player-look-radius (currently `8.0` in
  Rust) to match vanilla exactly.
- `is_immobile`/`has_line_of_sight` overrides: straightforward boolean combinators once the three
  counters exist; these already have direct analogues on whatever base "is immobile"/"can see
  target" trait methods `MobEntity`/`Mob` expose (not located in this survey — grep before
  implementing to confirm override points exist).
- `get_attack_bounding_box`: low-priority 0.05-deflate tweak, port last if time allows.

**Files to create**: `pumpkin/src/entity/ai/goal/ravager_state.rs` (or embed `RavagerState` directly
in `pumpkin/src/entity/mob/ravager.rs` if it's simple enough not to warrant its own module — Vex's
`VexEntity` embeds its own atomics directly rather than delegating to a separate state module, which
is the closer precedent here since `RavagerState` isn't shared across mob types the way
`SpellcasterState` is).
**Files to modify**: `pumpkin/src/entity/mob/ravager.rs` (goal-priority/tuning fixes, state
machine, `roar`, `blockedByItem`, entity-event wiring), possibly `pumpkin/src/entity/ai/goal/
wander_around.rs` or a new water-avoiding variant, possibly attribute system if base-value mutation
isn't supported yet.

**Needs wiki/vanilla-source verification**: exact `roarTick == 10` pre-fire timing (confirm it's
not `== 0`, double-checked above from the decompile directly, so this one's already source-cited);
whether Pumpkin's attribute system distinguishes base value from modified value; whether a
shield-block-response hook already exists; whether an `AbstractIllager`-equivalent type-id list or
tag exists anywhere in `pumpkin-data`.

---

## Witch

Vanilla: `Witch extends Raider implements RangedAttackMob` (not `AbstractIllager` — no arm pose).
Key members:
- `registerGoals`: `FloatGoal(1)`, `RangedAttackGoal(2, 1.0, 60, 10f)`,
  `WaterAvoidingRandomStrollGoal(2, 1.0)`, `LookAtPlayerGoal(3, 8f)`, `RandomLookAroundGoal(3)`.
  Target selector: `HurtByTargetGoal(Raider)(1)`, `healRaidersGoal (2)`, `attackPlayersGoal (3)` —
  **current Rust registers only `WitchAttackGoal` (a hand-rolled potion-throw attack, already
  present and reasonably faithful per the earlier read) and `WanderAroundGoal`/generic
  `ActiveTargetGoal<Player>`; the heal-raiders and gated-attack-players target goals do not exist
  at all.**
- `healRaidersGoal: NearestHealableRaiderTargetGoal<Raider>` — predicate:
  `hasActiveRaid() && !target.is(WITCH)` (a witch heals any hurt raid-mate except other witches).
  `attackPlayersGoal: NearestAttackableWitchTargetGoal<Player>(10, true, false, null)` — this is
  gated on/off every tick from `aiStep` (see below), it isn't just "always active."
- `aiStep`: (non-client only)
  1. `healRaidersGoal.decrementCooldown()`; if cooldown `<= 0`, `attackPlayersGoal.setCanAttack(true)`
     else `false` — i.e. **the witch can only start attacking players while its heal-goal is on
     cooldown** (so it prioritizes healing raid allies over attacking).
  2. Potion-drinking state machine: if `isDrinkingPotion()`, count down `usingTime`; at zero, clear
     the potion item, apply its effects to self via `PotionContents.forEachEffect`, emit
     `GameEvent.DRINK`, remove the speed-debuff modifier. Else (not drinking), roll four candidate
     self-potion triggers **in vanilla's exact `if/else if` priority order** (only one can fire per
     tick):
     - 15% chance, if in water and no `WATER_BREATHING` → drink `WATER_BREATHING`.
     - else 15% chance, if on fire or last damage was fire-tagged and no `FIRE_RESISTANCE` → drink
       `FIRE_RESISTANCE`.
     - else 5% chance, if `health < maxHealth` → drink `HEALING`.
     - else 50% chance, if has a target, no `SPEED` effect, and target is `> 11` blocks away
       (`distSqr > 121.0`) → drink `SWIFTNESS`.
     If any fired: sets `MAINHAND` to the potion item, `usingTime =
     getMainHandItem().getUseDuration(this)`, `setUsingItem(true)`, plays `WITCH_DRINK` sound, and
     applies a `-0.25` `MOVEMENT_SPEED` transient modifier for the drink duration.
  3. 1/1333 chance (`7.5E-4F`) per tick to broadcast entity event `15` (ambient ground-particle
     puff, ornamental only).
- `getDamageAfterMagicAbsorb`: **self-damage always zeroed** (`damageSource.getEntity() == this` →
  `0`, i.e. a witch's own splash potions/AoE never hurt itself), and any damage tagged
  `WITCH_RESISTANT_TO` (a `DamageTypeTags` entry — check its exact membership, not fetched in this
  survey) is reduced to 15% (`damage *= 0.15F`). This is layered **on top of** the base-class magic
  absorb calc via `super.getDamageAfterMagicAbsorb`, i.e. it's an additive override, not a
  replacement — the "85% magic damage reduction" language in the task brief maps to this
  `*0.15F` line specifically (0.15 = keep 15%, i.e. 85% reduced), scoped to the tagged damage types
  only, not all magic damage broadly.
- `performRangedAttack`: only runs `if !isDrinkingPotion()` — a witch mid-drink cannot also throw a
  potion that tick. Potion choice: `Raider` targets get healed/regenerated (and the witch clears its
  own target right after, `setTarget(null)` — i.e. **a witch that decides to help a hurt ally
  immediately stops treating it as an attack target**); otherwise slowness/poison/weakness/harming
  by distance+health+random-roll, in the exact priority already captured faithfully by the existing
  `WitchAttackGoal::choose_potion` (verified against the decompile: matches).
- `applyRaidBuffs`: empty no-op (Witches get no raid-buff item, unlike Vindicator/Pillager).
- `canBeLeader() = false`.

### Target Rust design

- **Priority order is the key thing missing**, more than any single mechanic: the "heal cooldown
  gates attack-player capability" relationship (`aiStep` step 1) needs the healing goal to exist
  first — it doesn't yet at all. Recommend:
  - New goal `NearestHealableRaiderTargetGoal` (new file
    `pumpkin/src/entity/ai/goal/nearest_healable_raider_target.rs`): a target-selector goal whose
    `can_start` finds the nearest hurt `Raider`-tagged living entity within some range excluding
    Witches, gated on `has_active_raid()` (shared helper). Needs a `cooldown: AtomicI32` field
    exposed via `decrement_cooldown()`/`get_cooldown()` methods so `WitchEntity::mob_tick` (step 1
    above) can read/drive it externally — this cross-goal coupling (one goal reads another's
    internal cooldown) is unusual for the codebase's existing goals (which are self-contained), so
    it likely wants the cooldown to live on `WitchEntity` itself (an `AtomicI32`) rather than
    inside the goal struct, with the goal just reading it — simpler and avoids needing mutable
    cross-goal references from `mob_tick`.
  - New goal `NearestAttackableWitchTargetGoal` (or extend `ActiveTargetGoal` with a
    `can_attack: Arc<AtomicBool>` gate parameter) implementing the on/off `setCanAttack` toggle —
    likely the simplest approach is adding an optional `enabled: Option<Arc<AtomicBool>>` field to
    the existing generic `ActiveTargetGoal` (checked in `can_start`) rather than a bespoke type,
    since the only difference from vanilla `NearestAttackableTargetGoal` is this external on/off
    switch.
  - `WitchEntity::mob_tick`: port `aiStep` steps 1-3 verbatim, reading/writing the cooldown/enabled
    atomics described above, running the drinking-state-machine (needs `LivingEntity::add_effect`,
    `get_use_duration` for the potion item — check this exists on `ItemStack`/`Item` data, e.g.
    `pumpkin_data::item::Item::POTION`'s use-duration constant — and a transient attribute-modifier
    API, `AttributeInstance::add_transient_modifier`/`remove_modifier` by id, matching the
    `SPEED_MODIFIER_DRINKING_ID` pattern; confirm this API shape exists in Pumpkin's attribute
    system before assuming it, since Ravager's design section above already flags base-value
    mutation as unverified — transient *modifiers* (as opposed to base value) are more likely to
    already be supported since combat/potion effects elsewhere presumably use them, but verify).
- `get_damage_after_magic_absorb`-equivalent: check whether `LivingEntity`/`Mob` has a
  `modify_incoming_damage`-style hook already (the `Mob` trait excerpt read above shows
  `fn modify_incoming_damage(&self, amount: f32, _damage_type: DamageType) -> f32` **already
  exists** as a trait default) — implement `WitchEntity::modify_incoming_damage` calling
  `super`-equivalent-if-any then applying the self-damage-zero and `WITCH_RESISTANT_TO`-tag 85%
  reduction. Needs the damage-source's "was this witch itself the source" info threaded through
  the call, plus a `DamageType`-or-tag check for `witch_resistant_to` — confirm this tag exists in
  `pumpkin-data::tag::DamageType` (the vanilla data file `data/minecraft/tags/damage_type/
  witch_resistant_to.json` exists per this survey's file listing, so the tag itself is real vanilla
  data; check it's been generated into `pumpkin-data`).
- `performRangedAttack`/`WitchAttackGoal`: add the `!isDrinkingPotion()` gate to
  `WitchAttackGoal::can_start`/`tick` (needs a way to read the witch's drinking-state atomic from
  the goal — same cross-goal-reads-entity-state pattern as above, natural since `WitchAttackGoal`
  already takes `mob: &dyn Mob` and could downcast or the drinking flag could live directly on
  `WitchEntity` read via a new `Mob`-trait accessor), and add the `Raider`-target
  heal/regen-and-clear-target branch to `WitchAttackGoal::shoot`/`choose_potion` (currently only
  handles the generic hostile-potion-selection branch, confirmed by the earlier read — the
  `Raider`-ally-heal branch is entirely absent).
- `apply_raid_buffs`: no-op default is already correct once the shared trait exists — no Witch-
  specific override needed.

**Files to create**: `pumpkin/src/entity/ai/goal/nearest_healable_raider_target.rs`,
`pumpkin/src/entity/ai/goal/nearest_attackable_witch_target.rs` (or the `ActiveTargetGoal` gate-flag
extension in place of a new file).
**Files to modify**: `pumpkin/src/entity/mob/witch.rs` (drinking state machine, goal registration,
`modify_incoming_damage`), `pumpkin/src/entity/ai/goal/witch_attack.rs` (drinking gate, ally-heal
branch), possibly `pumpkin/src/entity/ai/goal/active_target.rs` (gate-flag parameter).

**Needs wiki/vanilla-source verification**: exact `WITCH_RESISTANT_TO` tag membership (cited file
exists but contents weren't read in this survey); whether Pumpkin's attribute system supports
transient modifiers with removable ids; `ItemStack`/potion "use duration" source.

---

## Vex

Vanilla: `Vex extends Monster implements TraceableEntity, OwnableEntity`. Key members:
- `moveControl = new VexMoveControl(this)` set in the constructor — **replaces the default move
  control entirely**, not an additional goal.
- `tick()`: forces `noPhysics=true` for the duration of `super.tick()`, then restores
  `noPhysics=false` and calls `setNoGravity(true)` — i.e. **every tick**, physics collision is
  disabled just for the base entity-tick call (letting it fly through blocks that tick), then
  gravity is (re-)disabled persistently afterward. Also drives the pre-existing (already-ported)
  limited-life starvation-damage countdown.
- `registerGoals`: `FloatGoal(0)`, `VexChargeAttackGoal(4)`, `VexRandomMoveGoal(8)`,
  `LookAtPlayerGoal(9)`, `LookAtPlayerGoal(10)`. Target selector:
  `HurtByTargetGoal(Raider).setAlertOthers()(1)`, `VexCopyOwnerTargetGoal(2)`,
  `NearestAttackableTargetGoal<Player>(3)`.
- `VexChargeAttackGoal extends Goal` (flags `MOVE`): `canUse` — has a live target, no wanted
  move-control position pending, `random.nextInt(7) == 0`, and target is `> 4.0` distance-squared
  away; `canContinueToUse` — move control still has a wanted position, `isCharging()`, target still
  alive; `start` — sets move-control wanted position to target's eye position at speed `1.0`, sets
  `isCharging=true`, plays `VEX_CHARGE`; `tick` (runs every tick,
  `requiresUpdateEveryTick()=true`) — if bounding boxes intersect, calls `doHurtTarget` and clears
  charging; else if within `distanceToSqr < 9.0`, re-aims the wanted position at the target's
  current eye position (mid-charge homing correction); `stop` clears `isCharging`.
- `VexRandomMoveGoal extends Goal` (flags `MOVE`): `canUse` — no wanted move-control position
  pending, `random.nextInt(7) == 0`; `canContinueToUse` always `false` (i.e. this goal never
  "continues," it just fires its one-shot `tick()` once per activation — actually **note**: since
  vanilla `Goal` re-evaluates `canUse` every time the previous instance stops, and this goal's `tick`
  runs synchronously inside the single tick it's active, this behaves as "roll dice every eligible
  tick, and on a hit, pick one wander destination immediately" rather than a goal that persists
  across many ticks); `tick` — up to 3 attempts, picks a random offset from `boundOrigin` (or current
  position if unset) within `±7 x, ±5 y, ±7 z` roughly (exact ranges: `nextInt(15)-7` for x/z,
  `nextInt(11)-5` for y), checks `isEmptyBlock`, sets move-control wanted position at speed `0.25`
  and (if no target) looks at the chosen point.
- `VexCopyOwnerTargetGoal extends TargetGoal`: `canUse` — owner exists, implements `Targeting`, has
  a non-null target, and `canAttack(ownerTarget, copyOwnerTargeting)` passes (targeting conditions:
  non-combat, ignore line of sight, ignore invisibility); `start` copies the owner's target onto
  self.
- `VexMoveControl extends MoveControl<T>`: custom `tick()` — while `operation == MOVE_TO`, computes
  a delta vector to the wanted position; if delta length `<` bounding-box size, switches to `WAIT`
  and halves current velocity (arrival damping); else adds a scaled delta to velocity (`speedModifier
  * 0.05 / deltaLength` — **note this is much gentler acceleration than ground-based move controls**,
  consistent with floaty flight) and sets yaw facing either the movement direction (no target) or
  directly at the target (has target) — **this yaw-facing-target-while-moving-anywhere behavior is
  distinctive**: a normal ground mob faces its direction of travel, but Vex always faces its combat
  target regardless of where it's actually flying.
- `finalizeSpawn`/`populateDefaultEquipmentSlots`: gives `MAINHAND = IRON_SWORD`, drop chance `0.0`
  (already present per the read file's `set_owner`/`set_bound_origin`/`set_limited_life` methods
  existing but not yet wired to equipment — confirm current Rust already sets the sword; the earlier
  read of `vex.rs` didn't show equipment setup at all, so this may also be missing).
- `isFlapping`/`getLightLevelDependentMagicValue`/`isAffectedByBlocks`: client-render/lighting
  concerns, low priority.

### Target Rust design

The current `pumpkin/src/entity/mob/vex.rs` already has the right skeleton for the *non-goal* state
(`owner_id`, `bound_origin`, `has_limited_life`/`limited_life_ticks`, with `set_owner`/
`set_bound_origin`/`set_limited_life` accessors, and the limited-life starvation-tick logic already
correctly ported in `mob_tick`) — its own doc comment even names exactly what's missing (move
control, random-move goal, charge-attack goal, copy-owner-target goal), matching the task brief
precisely. This is the most "just wire it up" of the six mobs, since the state already exists.

- **Move control**: Pumpkin's `MobEntity` presumably has some generic `move_control`/navigator
  concept already (referenced throughout other goals as `mob.get_mob_entity().navigator`) — check
  whether there's a pluggable "move control" trait distinct from the navigator/pathfinder (vanilla's
  `MoveControl` is a lower-level velocity-integration step than `PathNavigation`) before assuming
  Vex needs its own bespoke one; if Pumpkin's movement model doesn't separate these the way vanilla
  does, `VexMoveControl`'s behavior (gentle acceleration toward a raw 3D point, ignoring
  pathfinding entirely, no-physics flight) may need to be implemented as an override of whatever
  Pumpkin's equivalent "apply velocity toward wanted position" hook is, rather than a literal new
  `MoveControl` type — this is a design decision to make when implementing, not something to
  pre-guess further here without reading Pumpkin's navigator/move-control internals (out of scope
  for this survey pass; flag as the first thing to check).
- `noPhysics=true` during `tick()`/`setNoGravity(true)` persistently: confirm `Entity` exposes
  `no_physics`/`no_gravity` fields (used by other flying mobs, e.g. `ghast.rs`, `phantom.rs`,
  `blaze.rs` — check one of those for the idiom) and replicate the same toggle-during-`tick`
  pattern in `VexEntity`'s `mob_tick` or a new `Mob::tick` override (verify whether `Mob` trait
  actually exposes a full-tick override point distinct from `mob_tick`'s current per-AI-tick hook —
  vanilla's `tick()` wraps `super.tick()` including physics/collision resolution, which might live
  outside wherever `mob_tick` is called from in Pumpkin's tick loop; if `mob_tick` runs after
  physics for this tick already resolved, the no-physics toggle needs to move to wherever physics
  actually runs).
- New file `pumpkin/src/entity/ai/goal/vex_charge_attack.rs`: port `VexChargeAttackGoal` using
  `VexEntity`'s existing `is_charging`-style flag (task brief calls for a synced `isCharging` flag
  — check if one exists yet; the earlier read of `vex.rs` shows no charging flag at all, so add
  `is_charging: AtomicBool` following the same `Metadata`/`TrackedData` synced-flag pattern flagged
  for Pillager's `IS_CHARGING_CROSSBOW` above — these two mobs need the identical mechanism, worth
  implementing once and reusing the idiom, not the code, since the two flags are unrelated data).
- New file `pumpkin/src/entity/ai/goal/vex_random_move.rs`: port `VexRandomMoveGoal`, reading
  `bound_origin` (already exists) and testing `world.is_air`/`is_empty_block`-equivalent (check
  exact API name) at up to 3 random offsets.
- New file `pumpkin/src/entity/ai/goal/vex_copy_owner_target.rs` (or generalize into the same
  "target-copying" shape as Vindicator's Johnny goal, since both ultimately need a
  "target any `LivingEntity` matching a runtime-supplied predicate" primitive distinct from the
  fixed-`EntityType` `ActiveTargetGoal`) — port `VexCopyOwnerTargetGoal`, reading `owner_id`
  (already exists, resolved via whatever "look up entity by id" world method other code uses) and
  copying its target if the owner is currently a `Mob` with a live target. **This shares real
  structure with Vindicator's `VindicatorJohnnyAttackGoal`** (both are "target-selector goals that
  don't fit `ActiveTargetGoal`'s fixed-type model") — recommend factoring a small shared
  `TargetPredicate`-based "any-type target goal" base once both are being implemented, rather than
  writing two independent one-off goal types, per the project's simplification/reuse habits (see
  the `simplify` skill's remit) — but this is an implementation-time call, not a blocking
  dependency between the two.
- Registration: replace the current `MeleeAttackGoal`/`WanderAroundGoal` pair in
  `VexEntity::new` (priority 4/5) with `VexChargeAttackGoal`(4)/`VexRandomMoveGoal`(8), keep
  `LookAtEntityGoal`/`RandomLookAroundGoal` as-is (already correct), and swap the target-selector's
  generic `ActiveTargetGoal<Player>` at priority 1 for `VexCopyOwnerTargetGoal`(2) +
  `ActiveTargetGoal<Player>`(3) (matching vanilla's three-goal target selector exactly, current Rust
  is missing the copy-owner-target goal and has the Player goal at the wrong priority slot).
- `finalize_spawn`/equipment: verify current Rust sets `IRON_SWORD` with 0 drop chance — the earlier
  file read didn't show any equipment setup in `VexEntity::new` at all, so this is likely also
  missing and should be added (`Pillager`'s inline equipment-insert-at-construction pattern in
  `pillager.rs:28-37` is the template to copy).

**Files to create**: `pumpkin/src/entity/ai/goal/vex_charge_attack.rs`,
`pumpkin/src/entity/ai/goal/vex_random_move.rs`,
`pumpkin/src/entity/ai/goal/vex_copy_owner_target.rs`.
**Files to modify**: `pumpkin/src/entity/mob/vex.rs` (move control, no-physics/no-gravity tick
behavior, `is_charging` flag, equipment, goal registration).

**Needs wiki/vanilla-source verification**: whether Pumpkin's movement model has a `MoveControl`/
navigator split matching vanilla's, or needs one introduced; whether `Entity` already exposes
`no_physics`/`no_gravity` toggles (check an existing flying mob like `ghast.rs`/`phantom.rs` before
assuming); exact "is this block passable" API name for `VexRandomMoveGoal`'s destination check.

---

## Cross-cutting scope notes

- **`get_arm_pose`/`IllagerArmPose` and all client-only rendering** (Illusioner's mirror-illusion
  offsets, `getIllusionOffsets`, the client-side branch of `Illusioner.aiStep`) are recommended as
  an explicit, uniform scope cut across every subsystem above: Pumpkin currently has no client-arm-
  pose concept anywhere in the codebase (confirmed by a repo-wide grep turning up nothing), nothing
  reads it server-side, and porting it would require inventing a new synced-data channel with no
  consumer. If a future renderer-parity pass wants this, it should be scoped as its own subsystem
  across all illager-family mobs at once, not bolted onto this batch piecemeal.
- **Enchantment providers** (`RAID_VINDICATOR`, `RAID_VINDICATOR_POST_WAVE_5`,
  `RAID_PILLAGER_POST_WAVE_3/5`, `PILLAGER_SPAWN_CROSSBOW`) are referenced by both Vindicator's and
  Pillager's `applyRaidBuffs`/`enchantSpawnedWeapon`. Whether `pumpkin-data` already generates
  equivalents of `VanillaEnchantmentProviders`/`EnchantmentHelper.enchantItemFromProvider` was not
  verified in this survey and should be checked once, shared by both mobs' implementers, rather than
  independently discovered twice.
- **"Any LivingEntity" target-selector goals** are needed by Vindicator (Johnny) and Vex
  (copy-owner-target), and `ActiveTargetGoal`'s current fixed-`&'static EntityType` design can't
  express them — this is a second small shared prerequisite (distinct from the raid infrastructure)
  worth designing once if both mobs are picked up together.
- **A synced "charging" boolean flag with client broadcast** is needed by both Pillager
  (`IS_CHARGING_CROSSBOW`) and Vex (`isCharging`) — mechanically identical to `creeper.rs`'s
  existing `ignited`/`charged` `TrackedData` fields, just applied to two new mobs; no new mechanism,
  just two more call sites of an existing one.
- This survey did not verify: the exact body of vanilla `Raid.getEnchantOdds()`, `BreakDoorGoal`,
  or `CrossbowAttackMob`'s default methods (none were opened in this pass — their call sites were
  inferred from `Vindicator`/`Pillager`/`Ravager`'s own source only). Whoever implements those rows
  must fetch and cite those three files directly before writing code, per the project's
  no-assertions-from-memory rule.
