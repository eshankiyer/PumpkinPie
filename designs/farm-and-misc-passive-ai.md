# Design: farm-animal and misc passive-mob AI gaps

Scope: Sheep shearing/genetics, Rabbit jump movement + killer bunny, Fox behavior,
Creaking heart-binding lifecycle, Sniffer state machine. This is a design document only;
no `pumpkin*` crate files are modified as part of writing it.

Vanilla source: `/tmp/pumpkin-vanilla-26.2/decompiled/net/minecraft/world/entity/animal/{sheep,rabbit,fox,sniffer}/*.java`,
`net/minecraft/world/entity/monster/creaking/Creaking.java`,
`net/minecraft/world/level/block/entity/CreakingHeartBlockEntity.java`,
`net/minecraft/world/item/DyeColor.java`.

Current Rust: `pumpkin/src/entity/passive/{sheep,rabbit,fox,sniffer}.rs`,
`pumpkin/src/entity/mob/creaking.rs`, `pumpkin/src/entity/ai/goal/sniffer_dig.rs`,
`pumpkin/src/block/{blocks,entities}/creaking_heart.rs`.

Conventions used throughout (from the existing codebase, not invented here):
- A `Goal` is `Send + Sync`, implements `can_start`/`should_continue`/`start`/`stop`/`tick`
  returning boxed futures (`GoalFuture`), and is registered into a mob's
  `goal_selector`/`target_selector` (`Mutex<GoalSelector>` on `MobEntity`) at construction
  time via `add_goal(priority, Box::new(...))`.
- Synced/flag state on an entity is typically a packed `AtomicU8`/byte with getter/setter
  pairs that call `send_meta_data` (see `SheepEntity::color_and_sheared`, `Fox`'s
  `DATA_FLAGS_ID` byte in vanilla). Pumpkin mobs so far model this the same way (see
  `SheepEntity`).
- `AgeableMob`/`Animal` are traits implemented per-mob providing `get_ageable_data()`,
  `is_food()`, NBT read/write helpers; mobs compose `MobEntity` + these traits rather than
  inheriting a Java-style class hierarchy.
- Attribute modifiers exist (`pumpkin/src/entity/attributes.rs`: `AttributeInstance`,
  `Modifier`, `ModifierOperation::{Add, MultiplyBase, MultiplyTotal}`,
  `add_or_replace_modifier`), reachable via `LivingEntity::update_attribute`. This is
  sufficient for the killer-bunny armor/attack-damage modifiers below; no new attribute
  machinery is needed.
- There is **no `JumpControl`/jump-control abstraction** in Pumpkin today (only
  `MoveControlTrait`/`MoveControl`, see `pumpkin/src/entity/ai/control/`). Rabbit's
  jump-based locomotion is therefore the one piece here that needs genuinely new control
  machinery, not just a goal.
- Brain/Activity/Sensor machinery (vanilla's `Brain<T>`, `SensorType`, `MemoryModuleType`)
  does not exist in Pumpkin. Every mob that vanilla drives through a `Brain` (Creaking,
  Sniffer) is ported here as a `Goal`-based approximation, matching the precedent already
  set by `sniffer_dig.rs`'s own doc comment.

---

## 1. Sheep

### Vanilla (`Sheep.java`, 307 lines)

- `shear(ServerLevel, SoundSource, ItemStack)` (163-186): plays `SHEEP_SHEAR`, calls
  `dropFromShearingLootTable(level, BuiltInLootTables.SHEAR_SHEEP, tool, callback)` which
  rolls the `shear_sheep` loot table (color-dependent single `#wool` entry, count 1-3 via
  the table's `count` range, not hardcoded in Java) and spawns each unit of wool as a
  separate `ItemEntity` with a small randomized velocity kick; finally `setSheared(true)`.
- `readyForShearing()` (188-191): `!isSheared() && !isBaby()`.
- `mobInteract` (145-160): only on `Items.SHEARS`, only server-side, only if
  `readyForShearing()`; calls `shear`, fires `GameEvent.SHEAR`, damages the shears by 1.
  Otherwise (already sheared/baby) returns `CONSUME` (blocks default animal interact, no
  drop).
- `getColor()`/`setColor()` (227-234): color packed into low 4 bits of `DATA_WOOL_ID`,
  sheared flag in bit 4 (`isSheared`/`setSheared`, 257-268).
- `getRandomSheepColor(ServerLevelAccessor, BlockPos)` (270-273): delegates to
  `SheepColorSpawnRules.getSheepColor(biome, random)` — a biome-based weighted table
  (not itself read in this pass; treat as **wiki/source verification item**, see below).
  Called from `finalizeSpawn` (300-306) to color newly spawned sheep.
- `getBreedOffspring` (280-289): creates a new sheep, colors it via
  `DyeColor.getMixedColor(level, parent1Color, parent2Color)`.
- `ate()` (291-298): un-shears on eating grass, ages up 60s if eligible — **already ported**
  (`Mob::on_eating_grass` in `sheep.rs` calls `set_sheared(false)`, though the `canAgeUp` +
  `ageUp(60)` half is not wired there yet — worth folding into this same PR).

### Vanilla `DyeColor.getMixedColor` (`DyeColor.java` 134-170)

Not a fixed 16x16 table: it builds a **2-slot** `CraftingInput` (`CraftingInput.of(2, 1,
...)`) from one item stack per parent color and looks up a matching `CRAFTING` recipe.
Because the input is fixed at exactly 2 ingredients, only crafting recipes with exactly
two dye ingredients can ever match — recipes needing 3-4 ingredients (several `magenta`/
`light_gray` variants, see below) can never be produced this way and always fall through
to `random.nextBoolean() ? dyeColor1 : dyeColor2`. Verified directly against the
extracted vanilla data (`data/minecraft/recipe/*_dye_from_*.json` and `*_dye.json`), the
**complete set of 2-ingredient dye-mix recipes** is:

| Result | From (unordered pair) |
|---|---|
| `light_blue` | `blue` + `white` |
| `light_gray` | `gray` + `white` |
| `gray` | `black` + `white` |
| `cyan` | `blue` + `green` |
| `lime` | `green` + `white` |
| `pink` | `red` + `white` |
| `purple` | `blue` + `red` |
| `magenta` | `purple` + `pink` |
| `orange` | `red` + `yellow` |

(`black`, `blue`, `brown`, `green`, `white`, `yellow` only have single-ingredient "from a
natural item" recipes — ink sac, lapis, cocoa beans, cactus, bone meal, dandelion/etc —
none of which are dye+dye pairs, so those colors are never a *mixing result*, only
mixing *inputs*. `magenta` and `light_gray` also each have a second, 3-4-ingredient
recipe that cannot match the 2-slot input and is irrelevant here.) Any dye-color pair not
in this table (including a color mixed with itself) falls straight to the 50/50 coin
flip. Sheep-breeding color genetics are therefore **mostly a coin flip**, with these nine
pairs as the only deterministic outcomes — do not build a denser table than this.

Pumpkin currently has a `DyeColor` enum (`pumpkin/src/block/entities/sign.rs`, re-used by
`pumpkin/src/item/items/dye.rs`) with just an id/name mapping — no mixing table. Building
`DyeColor::mixed(a, b, rng)` is: a small static sparse table (the 9 pairs above, checked
both orderings since the table is symmetric) with a coin-flip default, exposed as
`pub fn mixed_color(a: DyeColor, b: DyeColor, rng: &mut impl RngExt) -> DyeColor`. Given
Pumpkin's crafting-recipe registry was not confirmed in this pass to support a reverse
output-component lookup, hardcoding this fixed, finite, already-enumerated table is both
correct and clearly the pragmatic route (no dependency on the crafting system at all).
Co-locate it with `DyeColor` in `pumpkin/src/block/entities/sign.rs`, or promote
`DyeColor` to a shared `pumpkin/src/color.rs` if sheep/rabbit/other consumers want it
outside the sign module — recommended, since "DyeColor lives in `sign.rs`" is already a
layering smell this feature would make worse.

**This DyeColor promotion/mixing-table work is a shared prerequisite** for Sheep's
`getBreedOffspring` and is out of scope to reinvent per-mob; design it once, referenced
from Sheep's design below.

### Target Rust design

Files:
- `pumpkin/src/entity/passive/sheep.rs` (modify) — add `shear()`, `ready_for_shearing()`,
  `mob_interact` shears branch, `get_random_sheep_color`, and an `Animal::create_offspring`
  override (needs the `breed.rs`/`animal.rs` prerequisite from the cross-cutting section
  landed first).
- `pumpkin/src/color.rs` (new, or extend `pumpkin/src/block/entities/sign.rs`) — promote
  `DyeColor` to a shared location with `mixed_color(a, b) -> DyeColor` and expose it from
  both sign and sheep call sites. (Prerequisite, shared with nothing else in this doc's
  scope but flagged as reusable.)
- `pumpkin-data` — verify/add a `shear_sheep` loot table entry if Pumpkin's loot-table
  data doesn't already encode per-color wool drops (check `pumpkin-data`/`assets` for an
  existing `loot_table/entities/sheep.json`-equivalent before adding one).
- A biome-based `SheepColorSpawnRules`-equivalent table (new, small): a function
  `get_random_sheep_color(world: &World, pos: BlockPos, rng: &mut impl RngExt) -> DyeColor`
  ported from vanilla's `SheepColorSpawnRules.java` (read in full; distinct from
  `DyeColor::mixed_color`). Three weighted configurations keyed by biome tag,
  `SPAWNS_WARM_VARIANT_FARM_ANIMALS` -> warm, `SPAWNS_COLD_VARIANT_FARM_ANIMALS` -> cold,
  else temperate (check these two tags exist in `pumpkin-data`'s generated biome tags the
  same way `SPAWNS_WHITE_RABBITS`/`SPAWNS_GOLD_RABBITS`/`SPAWNS_SNOW_FOXES` were
  confirmed present, below). Each configuration is a weighted pick over 5 entries (weights
  out of 100); the last entry (weight 82) is itself a nested weighted pick, 499:1 in
  favor of that configuration's dominant color over `PINK`. The three tables, verbatim
  from `SheepColorSpawnRules.java`:
  - Temperate (default): `BLACK`=5, `GRAY`=5, `LIGHT_GRAY`=5, `BROWN`=3, then 82 split
    499:1 between `WHITE` and `PINK`.
  - Warm (`SPAWNS_WARM_VARIANT_FARM_ANIMALS`): `GRAY`=5, `LIGHT_GRAY`=5, `WHITE`=5,
    `BLACK`=3, then 82 split 499:1 between `BROWN` and `PINK`.
  - Cold (`SPAWNS_COLD_VARIANT_FARM_ANIMALS`): `LIGHT_GRAY`=5, `GRAY`=5, `WHITE`=5,
    `BROWN`=3, then 82 split 499:1 between `BLACK` and `PINK`.
  Port this as a small `match` over three static weighted tables rather than a generic
  weighted-list type unless Pumpkin already has one (check `rand`/`pumpkin_util` for an
  existing weighted-pick helper before writing a new one — vanilla's `WeightedList` is a
  fairly generic utility other Pumpkin spawn-rule code may already need).

Control flow for `shear`:
```
mob_interact(player, item_stack):
  if item_stack.item == Item::SHEARS:
    if world is server && self.ready_for_shearing():
      self.shear(world, SoundCategory::Players, item_stack)
      world.fire_game_event(GameEvent::Shear, self, player)
      item_stack.hurt_and_break(1, player, hand.as_equipment_slot())
      return SUCCESS_SERVER
    else:
      return CONSUME
  else:
    super::mob_interact(...)  // existing animal_interact path (feeding/breeding)

shear(world, sound_category, tool):
  world.play_sound(Sound::EntitySheepShear, sound_category, pos, 1.0, 1.0)
  drops = loot_table::roll(SHEAR_SHEEP_TABLE, luck_from(tool), color=self.get_color())
  for drop in drops.iter():
    for _ in 0..drop.count:
      spawn ItemEntity(drop.copy_with_count(1)) at self.pos with randomized velocity kick
  self.set_sheared(true)
```
This is a straightforward addition given `SheepEntity` already has `get_color`/
`set_color`/`set_sheared` packed-byte accessors — the shearing goal itself needs no new
`Goal` type (it's a `mob_interact` hook, mirrored in `mobInteract`, not a `registerGoals`
addition).

`readyForShearing` and `getRandomSheepColor` at `finalize_spawn`/spawn time follow
directly. `getBreedOffspring`'s color-mixing logic depends on the `Animal::create_offspring`
hook described in the cross-cutting section below — `BreedGoal` does not call any such
hook today, so this is not implementable in isolation; land the `breed.rs`/`animal.rs`
prerequisite first, then implement `SheepEntity`'s `create_offspring` override.

### Needs wiki/source verification
- Whether Pumpkin's loot-table system already has a `shear_sheep`-equivalent table wired
  to `pumpkin-data`, or whether this needs a new data table.
- The full list of dye-mixing recipe outputs (16 colors -> which pairs have a defined
  mixed result) if going with route (a) above — cross-check against
  `data/minecraft/recipe/*_dye.json` in the vanilla data pack rather than reconstructing
  from memory.

---

## 2. Rabbit

### Vanilla (`Rabbit.java`, 703 lines)

Already covered by the current Rust `rabbit.rs` (grep confirmed): `EscapeDangerGoal`
(≈`RabbitPanicGoal`/`PanicGoal`), `BreedGoal`, `TemptGoal`, `FollowParentGoal`,
`RaidGardenGoal` (full port already exists, `pumpkin/src/entity/ai/goal/raid_garden.rs`),
`WanderAroundGoal`, `LookAtEntityGoal`, `ClimbOnTopOfPowderSnowGoal`. What's **missing**:

1. **Jump-based locomotion.** Vanilla drives rabbit movement through three custom
   controls plus per-tick state in `customServerAiStep` (206-252):
   - `RabbitJumpControl` (517-545): a `JumpControl` subtype exposing `wantJump()`/
     `canJump()`/`setCanJump()`; `tick()` calls `rabbit.startJumping()` and clears the
     jump flag when `jump` is requested.
   - `RabbitMoveControl<T>` (547-576): overrides `tick()` to zero the speed modifier when
     grounded/not jumping/not wanting to jump, else applies `nextJumpSpeed`; overrides
     `setWantedPosition` to force `speedModifier = 1.5` while in water and remember
     `nextJumpSpeed`.
   - `getJumpPower()` (138-158): computes a variable jump power (0.2-0.5, scaled by
     `super.getJumpPower(power/0.42)`) based on current speed modifier, path incline, and
     horizontal-collision/ascending state.
   - `jumpFromGround()` (160-174): after the vanilla jump impulse, gives a forward
     boost if nearly stationary, and broadcasts entity event `1` (triggers hop animation
     client-side, `handleEntityEvent` 446-454).
   - `customServerAiStep` (206-252): decrements `jumpDelayTicks`/`moreCarrotTicks`; on
     landing (`onGround() && !wasOnGround`) stops jumping and calls `checkLandingDelay()`
     (sets `jumpDelayTicks` = 10 normally / 3 if `speedModifier >= 2.2`, and disables the
     jump control); if evil variant and `jumpDelayTicks == 0` and has a target within 4
     blocks, faces and jumps at it; otherwise, if the jump control doesn't already want
     to jump and the move control has a destination and delay has elapsed, faces the path
     point and `startJumping()`.
   - `aiStep()` (280-290): advances `jumpTicks` toward `jumpDuration` (15 ticks), then
     resets both to 0 and un-sets jumping.

   Pumpkin has **no jump-control abstraction at all** — only `MoveControlTrait`. However,
   confirmed by reading `pumpkin/src/entity/ai/goal/climb_on_top_of_powder_snow.rs`
   (lines ~65-70), Pumpkin's `LivingEntity` already exposes a settable physics-level
   `jumping: AtomicBool` field that the movement system consumes each tick — that goal's
   entire "jump" implementation is `mob.get_mob_entity().living_entity.jumping.store(true, SeqCst)`,
   a direct Rust equivalent of vanilla's `this.mob.getJumpControl().jump()` arming the
   `jump` flag for the next physics tick. This settles the design question: **a real
   `JumpControlTrait`/`JumpControl` is not required**. Commit to the goal-only route:
   a new `RabbitHopGoal` (a `Goal` with `Controls::MOVE | Controls::JUMP`,
   `should_run_every_tick() == true`) that, every tick, replicates the
   `customServerAiStep` state machine (206-252) inline against new `RabbitEntity` fields
   (`jump_ticks`, `jump_duration`, `jump_delay_ticks`, `was_on_ground`), and a
   `RabbitEntity::start_jumping()` method that sets `jump_duration = 15`, `jump_ticks = 0`,
   plays the jump sound, and stores `true` into `living_entity.jumping` exactly like
   `ClimbOnTopOfPowderSnowGoal` does. The variable jump-power calculation
   (`getJumpPower`, 138-158) and the forward-boost-on-jump (`jumpFromGround`, 160-174)
   still need a genuine hook into the jump-impulse/velocity calculation itself (as
   opposed to just the boolean "should I jump" flag) — check where Pumpkin's physics
   layer reads `jumping` and turns it into a Y-velocity impulse (likely near wherever
   `was_in_powder_snow`/ground-collision physics lives) to see whether jump *power* is
   currently hardcoded or already parameterizable; if it's hardcoded, giving rabbits a
   variable jump height is a small, targeted physics-layer change, not a new Control
   type — scope it as such rather than reintroducing the `JumpControl` abstraction just
   for this one number.

2. **Killer bunny (`Variant::EVIL`).** `setVariant` (368-385): when set to `EVIL`,
   sets `ARMOR` base value to 8.0, adds a `MeleeAttackGoal(1.4, true)` to goal_selector
   priority 4, adds `HurtByTargetGoal(this).setAlertOthers()` (targetSelector prio 1),
   adds two `NearestAttackableTargetGoal<Player>`/`<Wolf>` (targetSelector prio 2, both
   `mustSee=true`), adds a transient `ATTACK_DAMAGE` modifier `+5.0` (`ADD_VALUE`), and
   sets a custom name to `entity.minecraft.killer_bunny` if none is set. Un-setting
   removes the attack-damage modifier only (armor/goals persist — this asymmetry is
   vanilla's actual behavior, not a bug: variant is normally set once at spawn/read and
   never reverted in practice).
   Rust design: a `RabbitEntity::set_variant(&self, variant: RabbitVariant)` mirroring
   this exactly, using:
   - `LivingEntity::update_attribute(Attributes::ARMOR, |a| a.base_value = 8.0)`
   - `LivingEntity::update_attribute(Attributes::ATTACK_DAMAGE, |a| a.add_or_replace_modifier(Modifier { id: "evil".into(), amount: 5.0, operation: ModifierOperation::Add }))`
   - goal_selector/target_selector pushes exactly like `Fox::setTargetGoals` already does
     conditionally post-construction in vanilla (i.e. these three goals are added lazily
     from `set_variant`/`finalize_spawn`/`read_nbt`, not unconditionally in `new()` —
     match that timing, since `MeleeAttackGoal` for a non-evil rabbit must never run).
   - `HurtByTargetGoal(...).setAlertOthers()` -> Pumpkin's `RevengeGoal` is the
     `HurtByTargetGoal` analog (see `pumpkin/src/entity/ai/goal/revenge.rs`) but has no
     "alert others" flag today; either extend `RevengeGoal` with an `alert_others: bool`
     field (broadcasts the target to same-type mobs in radius on hurt, vanilla's
     `Mob.alertOthers`) or accept the narrower behavior (self-targeting only, no
     pack-alerting) as a documented simplification for the first PR — flag explicitly in
     the PR body either way, this is a real behavior gap either direction.
   - Two `NearestAttackableTargetGoal` analogs already exist in spirit as
     `active_target.rs`/`non_tame_random_target.rs`; use `ActiveTargetGoal::with_default`
     against `EntityType::PLAYER` and `EntityType::WOLF` at target_selector priority 2.

3. **`RabbitAvoidEntityGoal` x3.** Vanilla registers `AvoidEntityGoal<Player>` (8.0 range),
   `<Wolf>` (10.0), `<Monster>` (4.0), all at priority 4, all speed 2.2/2.2, but wrapped
   in a rabbit-specific subclass whose only override is `canUse() = variant != EVIL &&
   super.canUse()` (492-506). Pumpkin's `AvoidEntityGoal` (`avoid_entity.rs`) already
   takes a `flee_type: &'static EntityType` (single type, not a class hierarchy) and has
   no such per-mob `can_start` gate hook — but `Goal::can_start` is already an overridable
   trait method, so the correct port is a thin wrapper struct
   `RabbitAvoidEntityGoal { inner: AvoidEntityGoal, rabbit: Weak<RabbitEntity> }` whose
   `can_start` checks `rabbit.upgrade().is_some_and(|r| r.get_variant() != EVIL)` before
   delegating every other trait method to `inner`. This needs **one wrapper type**, not
   three — parameterize by the three `(EntityType, distance)` tuples at registration:
   ```rust
   goal_selector.add_goal(4, RabbitAvoidEntityGoal::new(&EntityType::PLAYER, 8.0, 2.2, 2.2, weak.clone()));
   goal_selector.add_goal(4, RabbitAvoidEntityGoal::new(&EntityType::WOLF, 10.0, 2.2, 2.2, weak.clone()));
   goal_selector.add_goal(4, RabbitAvoidEntityGoal::new(&EntityType::MONSTER, 4.0, 2.2, 2.2, weak.clone()));
   ```
   Note: vanilla's third avoid-goal keys off the abstract `Monster` class, not a single
   `EntityType`; `AvoidEntityGoal::find_threat` in Pumpkin currently takes a single
   `flee_type`. Either extend it to accept a small type-set/predicate, or (simpler, and
   consistent with how `Fox`'s own `AvoidEntityGoal` construction in vanilla already
   takes a `Predicate<Entity>` as an additional filter) add an optional predicate
   parameter to `AvoidEntityGoal` search — this generalization is needed for Fox's own
   `AvoidEntityGoal<Player>`/`<Wolf>`/`<PolarBear>` registrations too (section 3 below),
   so implement it once and reuse.

4. **Biome-based variant + breeding genetics.** `getRandomRabbitVariant` (423-433):
   biome-tag-gated (`SPAWNS_WHITE_RABBITS` -> 80% `WHITE`/20% `WHITE_SPLOTCHED`,
   `SPAWNS_GOLD_RABBITS` -> always `GOLD`, else 50% `BROWN`/40% `SALT`/10% `BLACK`). All
   three tags (`MINECRAFT_SPAWNS_WHITE_RABBITS`, `MINECRAFT_SPAWNS_GOLD_RABBITS`, and
   `MINECRAFT_SPAWNS_SNOW_FOXES` for fox) already exist in
   `pumpkin-data/src/generated/tag.rs` — confirmed present, no data-gen work needed.
   `getBreedOffspring` (341-357): 95% chance (`random.nextInt(20) != 0`) to inherit a
   parent's variant (50/50 which parent), else roll a fresh biome-based variant. The
   logic itself is straightforward once `Variant` exists as a Rust enum with
   `DEFAULT`/`EVIL` and an `entity_data`-synced `u8`/i32 field (mirrors
   `SheepEntity::color_and_sheared` pattern, one `AtomicU8`/`AtomicI32` field on
   `RabbitEntity`) — but, same as Sheep, it can only be wired in as an
   `Animal::create_offspring` override once that hook exists (cross-cutting section
   below); it is not reachable from today's hardcoded `BreedGoal::breed`.

5. Rabbit does **not** override `spawnChildFromBreeding`/needs no state-machine work
   analogous to Sniffer — it's ordinary `AgeableMob` breeding, so no additional file
   beyond `rabbit.rs` + the new `RabbitAvoidEntityGoal` wrapper (which can live in
   `pumpkin/src/entity/ai/goal/avoid_entity.rs` itself as an additional exported type, or
   a new `rabbit_avoid_entity.rs` if keeping per-mob wrapper goals in their own files is
   preferred style — check whether other per-mob goal wrappers exist as separate files
   before deciding; `fox.rs`'s own goals are inline in this doc's section 3 design, so
   precedent leans toward a dedicated small file per wrapper goal).

### Files to create/modify
- `pumpkin/src/entity/passive/rabbit.rs` (modify): add `Variant` enum + synced field,
  `set_variant`, `get_variant`, an `Animal::create_offspring` override (post-prerequisite,
  see cross-cutting section), `get_random_rabbit_variant`,
  jump-related fields (`jump_ticks`, `jump_duration`, `jump_delay_ticks`,
  `was_on_ground`). Confirmed by reading `pumpkin/src/entity/ai/goal/raid_garden.rs`
  (line 40): it already owns an `AtomicI32` field named `more_carrot_ticks` and the
  `wants_more_food`-equivalent check/decrement logic (lines 59-67, 147) — this fully
  covers vanilla's `moreCarrotTicks`/`wantsMoreFood`, so **do not** add a duplicate
  field to `RabbitEntity`; the jump goal and any other rabbit code that needs this state
  should read it off the existing `RaidGardenGoal` instance (or, if goals are
  intentionally not exposed for other goals to query directly in this codebase's style,
  move `more_carrot_ticks` up onto `RabbitEntity` and have `RaidGardenGoal` read/write it
  there instead — check the established convention for cross-goal shared per-mob state
  before choosing, since this doc's first read of `raid_garden.rs` only confirms the
  field's current owner, not the intended pattern for goals that need to share it).
- `pumpkin/src/entity/ai/goal/rabbit_hop.rs` (new): the jump-state-machine `Goal`
  (option 1 above) or `pumpkin/src/entity/ai/control/jump_control.rs` (new, if going
  with option 2, the fuller-parity route).
- `pumpkin/src/entity/ai/goal/avoid_entity.rs` (modify): generalize `find_threat` to
  accept an optional extra predicate closure (needed by both Rabbit's evil-gate wrapper
  and Fox's three `AvoidEntityGoal` uses in section 3), and/or add the
  `RabbitAvoidEntityGoal` wrapper type here.
- `pumpkin/src/entity/ai/goal/revenge.rs` (modify, optional): add `alert_others: bool` to
  support `HurtByTargetGoal.setAlertOthers()` for killer bunnies (and reusable later for
  wolves/other pack mobs that use the same vanilla flag).

### Needs wiki/source verification
- Where Pumpkin's physics layer turns the `jumping` boolean into an actual Y-velocity
  impulse, and whether that impulse magnitude is already parameterizable (needed for
  the variable jump-power/forward-boost half of rabbit hopping — the boolean-flag half
  is now resolved, see above).
- Exact killer-bunny spawn conditions (naturally never spawns; only via
  `/summon minecraft:rabbit ~ ~ ~ {RabbitType:"evil"}` or old-name variants) — confirm
  current NBT variant string mapping (`"evil"` vs `99`) round-trips through
  `Rabbit.Variant.LEGACY_CODEC` correctly if Pumpkin's NBT variant codec is int-based.

---

## 3. Fox

By far the largest gap: vanilla `Fox.java` is 1533 lines with 20 inner goal classes,
3 custom controls, a full flag byte (7 boolean states), a two-slot trusted-entity list,
and biome-based variant. Current `fox.rs` is 108 lines with **zero** fox-specific goals
(uses only generic `SwimGoal`/`EscapeDangerGoal`/`BreedGoal`/`TemptGoal`/
`FollowParentGoal`/`WanderAroundGoal`/`LookAtEntityGoal`/`RandomLookAroundGoal` — i.e.
today's fox behaves like a generic prey animal with none of what makes a fox a fox).

### Full inventory of what vanilla Fox does that Pumpkin's does not

State (`DATA_FLAGS_ID` byte, 113-119, 462-505): `SITTING`(1), `CROUCHING`(4),
`INTERESTED`(8), `POUNCING`(16), `SLEEPING`(32), `FACEPLANTED`(64), `DEFENDING`(128).
Plus `DATA_TYPE_ID` (Variant: RED/SNOW), two `DATA_TRUSTED_ID_{0,1}` optional entity
references, and non-synced `interestedAngle(O)`, `crouchAmount(O)`, `ticksSinceEaten`.

Controls: `FoxLookControl` (1062-1078, skip tick while sleeping; `resetXRotOnTick` false
while pouncing/crouching/interested/faceplanted), `FoxMoveControl` (1106-1117, tick only
if `canMove()` i.e. not sleeping/sitting/faceplanted).

Goals (priority as registered, 180-208):
| Prio | Goal | Behavior |
|---|---|---|
| 0 | `FoxFloatGoal` | swim float + `clearStates()` on start; `canUse` = in water past 0.25 fluid height or in lava |
| 0 | `ClimbOnTopOfPowderSnowGoal` | generic, already exists |
| 1 | `FaceplantGoal` | holds fox immobile 40 ticks after a snow faceplant, then clears the flag |
| 2 | `FoxPanicGoal` | `PanicGoal` but suppressed while `isDefending()` |
| 3 | `FoxBreedGoal` | `BreedGoal` + `clearStates()` both parents on start; offspring inherits trust from love-cause player(s) |
| 4 | `AvoidEntityGoal<Player>` | 16.0 range, 1.6/1.4 speed, predicate: not discrete, not creative/spectator, not trusted, not defending |
| 4 | `AvoidEntityGoal<Wolf>` | 8.0 range, predicate: untamed wolf, not defending |
| 4 | `AvoidEntityGoal<PolarBear>` | 8.0 range, predicate: not defending |
| 5 | `StalkPreyGoal` | crouch+interest when target (chicken/rabbit) is 6-36 blocks away and path is clear |
| 6 | `FoxPounceGoal` (`JumpGoal`) | pounce leap once fully crouched, target's path is clear; snow faceplant on landing rot>0 |
| 6 | `SeekShelterGoal` (`FleeSunGoal`) | seek shelter when thundering+sky-visible, or periodically in daylight outside village |
| 7 | `FoxMeleeAttackGoal` | melee bite, gated off while sitting/sleeping/crouching/faceplanted |
| 7 | `SleepGoal` | sleep during day in a sheltered, non-alertable spot after a randomized wait |
| 8 | `FoxFollowParentGoal` | gated off while defending |
| 9 | `FoxStrollThroughVillageGoal` | gated off while sleeping/sitting/defending/has-target |
| 10 | `FoxEatBerriesGoal` (`MoveToBlockGoal`) | eats sweet berries (ages bush down) or glow berries |
| 10 | `LeapAtTargetGoal` | generic |
| 11 | `WaterAvoidingRandomStrollGoal` | generic |
| 11 | `FoxSearchForItemsGoal` | walks to nearby pickup-able items when mouth is empty and no target/last-hurt-by |
| 12 | `FoxLookAtPlayerGoal` | gated off while faceplanted/interested |
| 13 | `PerchAndSearchGoal` | idle look-around sitting behavior, rare trigger, gated by `alertable()` |
| targetSelector 3 | `DefendTrustedTargetGoal` | retaliates against whoever last hurt a trusted entity within 600 ticks |

Plus: item-in-mouth mechanics (`populateDefaultEquipmentSlots` random starting item,
`canHoldItem`/`pickUpItem`/`spitOutItem` swap-on-pickup, eating countdown in `aiStep`
214-247, particle/sound on eat via entity event 45), biome variant (`byBiome`), trust
list (`addTrustedEntity`, `trusts`, NBT round-trip), `isPathClear` static helper used by
both stalk and pounce goals, per-variant target-goal wiring (`setTargetGoals`, red foxes
hunt chickens/rabbits/turtle-eggs first then fish; snow foxes the reverse priority).

### Recommended smallest coherent first slice

Given the size, ship in at least two PRs. **First PR — "fox looks and acts like a fox at
rest and when calm"**:
1. `DATA_FLAGS_ID` byte + all 7 flag accessors (this is cheap and everything else reads
   off it — do it first regardless of scope).
2. Biome variant (`Variant::byBiome`, `get_variant`/`set_variant`, `finalize_spawn`
   wiring) — cheap, self-contained, matches this doc's Rabbit/Sheep variant pattern.
3. `SleepGoal` + `FoxFloatGoal`'s `clearStates()` + `wakeUp()`/`clearStates()` plumbing —
   this alone makes foxes visibly behave like foxes (sleep during the day) without
   touching combat AI.
4. `StalkPreyGoal` + `FoxPounceGoal` — the single most visually distinctive fox
   behavior (crouch-stalk-pounce on chickens/rabbits), and the reason this subsystem is
   called out by name in the census. Needs `isPathClear` ported as a free function.
5. Trust list + `DefendTrustedTargetGoal` can wait — breeding/ownership is lower-value
   without the stalk/pounce/sleep foundation already in place.

**Second PR — "fox economy and combat"**: item-in-mouth (hold/steal/spit/eat), melee
attack goal with its state gates, avoid-entity x3 (reusing the generalized
`AvoidEntityGoal` predicate support designed in section 2), berries-eating goal,
faceplant, defending/trust, per-variant target-goal ordering, village strolling,
perch-and-search idle animation. This second slice is genuinely large on its own; do not
try to land it in one PR either — the melee/avoid/target-goal cluster and the
item-economy cluster (`canHoldItem`/`pickUpItem`/`spitOutItem`/eating) are separable.

### Target Rust design

`FoxEntity` grows from a bare `MobEntity + AgeableData` wrapper to:
```rust
pub struct FoxEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    flags: AtomicU8,               // packed SITTING/CROUCHING/INTERESTED/POUNCING/SLEEPING/FACEPLANTED/DEFENDING
    variant: AtomicU8,              // Variant::Red | Snow
    trusted: Mutex<[Option<Uuid>; 2]>, // DATA_TRUSTED_ID_0/1 -- entity refs, resolved lazily like CreakingHeartBlockEntity's creaking_uuid
    interested_angle: AtomicU32,     // f32 bits, client-only lerp state -- may be skippable if Pumpkin doesn't need client interpolation server-side
    crouch_amount: AtomicU32,        // f32 bits, ditto
    ticks_since_eaten: AtomicI32,
}
```
Each flag gets a `pub fn is_x(&self) -> bool` / `pub fn set_x(&self, bool)` pair exactly
like `SheepEntity::{is_sheared,set_sheared}`, backed by the shared packed byte (matches
vanilla's `getFlag`/`setFlag`). `clear_states()` is a convenience method zeroing
sitting/crouching/interested/sleeping/defending/faceplanted in one packed write (mirrors
vanilla 652-659, minus 5 separate `send_meta_data` calls if the metadata protocol allows
batching — check `send_meta_data`'s signature, it already accepts a slice).

New goal files (first-slice scope): `pumpkin/src/entity/ai/goal/fox_sleep.rs`,
`fox_stalk_prey.rs`, `fox_pounce.rs` (or `fox_jump.rs` if a generic `JumpGoal` base is
worth factoring out — check whether Pumpkin has a `JumpGoal` analog before inventing
one; grep found none, so this is new). `isPathClear` becomes a free function
`pub fn fox_path_is_clear(fox: &FoxEntity, target: &dyn EntityBase) -> bool` in
`fox.rs` or a shared `fox_util.rs` alongside the goal files (mirrors the existing
`breeze_util.rs` convention for a mob with several cooperating goal files).

`FoxLookControl`/`FoxMoveControl` follow the `SlimeMoveControl` pattern: override the
`Mutex<Box<dyn MoveControlTrait>>` and (new) `Mutex<LookControl>` — check whether
`LookControl` in Pumpkin is trait-based/overridable the same way `MoveControlTrait` is
before assuming this is symmetric; if `look_control` on `MobEntity` is a concrete
`LookControl` rather than `Box<dyn LookControlTrait>`, the "skip tick while sleeping"
behavior needs a different hook (e.g. checked inside a wrapping goal, or by adding a
`LookControlTrait` mirroring `MoveControlTrait` — flag this as a design decision to make
during implementation, not resolved here since it wasn't verified in this pass).

### Files to create/modify (first-slice PR)
- `pumpkin/src/entity/passive/fox.rs` (modify): flags, variant, `clear_states`,
  `finalize_spawn` biome wiring, `SleepGoal`/`StalkPreyGoal`/`FoxPounceGoal`
  registration, `wake_up()` call sites (in water / has target / thundering).
- `pumpkin/src/entity/ai/goal/fox_sleep.rs` (new)
- `pumpkin/src/entity/ai/goal/fox_stalk_prey.rs` (new)
- `pumpkin/src/entity/ai/goal/fox_pounce.rs` (new)
- `pumpkin/src/entity/ai/goal/fox_util.rs` (new, `isPathClear` + any other shared helper)
- `pumpkin/src/entity/ai/goal/mod.rs` (modify): register the three new modules.

### Needs wiki/source verification
- Confirm `FLEE_SUN` / shelter-seeking (`hasShelter`, `canSeeSky`) primitives exist in
  Pumpkin's world API (`level.canSeeSky(pos)`) before relying on them in `SleepGoal`'s
  `hasShelter()` port (`FoxBehaviorGoal.hasShelter`, 845-848) — this is a lookup against
  the current codebase, not the wiki, but flagged since it wasn't checked in this pass.
- `isVillage(pos)` (used by `SeekShelterGoal`, second-slice item) — verify Pumpkin has a
  village/raid-center concept at all before scoping that goal's port faithfully.

---

## 4. Creaking

### Vanilla (`Creaking.java` 606 lines) vs. block-side state (already designed/built)

The block half is **already implemented** in this repo
(`pumpkin/src/block/entities/creaking_heart.rs`, `pumpkin/src/block/blocks/creaking_heart.rs`)
and its own doc comment is explicit about the boundary: `CreakingHeartBlockEntity`'s
`tick()` ports `updateCreakingState`/analog-output-signal/dormant<->awake cycling, but
**deliberately does not port** `spawnProtector`, `removeProtector`, `creakingHurt`,
`spreadResin`, or the heart<->mob binding, because those need `Creaking::setTransient`/
`tearDown`/`playerIsStuckInYou` which don't exist yet. This section designs exactly that
missing entity-side half so the block can grow into it, **not** a redesign of the block.

Full vanilla surface to port, from `Creaking.java`:
- `setTransient(BlockPos)` (95-102): binds `HOME_POS`, plus pathfinding-malus tweaks
  (DAMAGING/POWDER_SNOW/LAVA=8.0 avoid-cost, FIRE/FIRE_IN_NEIGHBOR=0.0 ignore).
- `isHeartBound()` (104-106): `getHomePos() != null`.
- `hurtServer` (151-180): if unbound or damage bypasses invulnerability, normal damage.
  Otherwise: invulnerable while `invulnerabilityAnimationRemainingTicks > 0`; on a fresh
  hit, requires either a living-entity/projectile direct source or a responsible player
  (else the hit is a no-op, `return false` — e.g. explosions/fire with no player
  attribution do nothing); sets `invulnerabilityAnimationRemainingTicks = 8`,
  broadcasts entity event 66 (client hurt-flash), fires `ENTITY_ACTION` game event, and
  — **only if the home block entity is still this creaking's live protector** — calls
  `creakingHeartBlockEntity.creakingHurt()` (drives the block's particle/resin-spread
  effect) when a player caused it, and plays the hurt sound either way. Returns `true`
  (damage "absorbed", no HP loss) whenever any of this ran.
- `tick()` (243-260): if heart-bound and the home block entity is no longer a live
  protector of this creaking (`isProtector(this)` false), instantly `setHealth(0)` — a
  creaking whose heart was broken/reassigned dies.
- `tickDeath()` (262-272): if heart-bound and `isTearingDown()`, run a custom 45-tick
  death countdown that calls `tearDown()` instead of vanilla's normal death removal.
- `tearDown()` (286-321): spawns two particle bursts (pale-oak-wood crumble + creaking-
  heart-awake-state crumble) at the bounding box center, plays the death sound, then
  removes the entity with `RemovalReason.DISCARDED` (no death drops/XP — this bypasses
  normal death handling entirely).
- `creakingDeathEffects(DamageSource)` (323-327): used when the *heart* is destroyed
  while a creaking is bound (called from `CreakingHeartBlockEntity.removeProtector`
  with a non-null damage source) — attributes the kill, calls `die(source)` (normal
  death/loot path, unlike `tearDown`), plays `CREAKING_TWITCH`.
- `checkCanMove()` (454-489): the invulnerability-and-immobility-while-watched core rule.
  Reads `NEAREST_PLAYERS` brain memory (Pumpkin equivalent: a plain per-tick nearby-
  players query, no Brain needed). If no nearby players: deactivate if active, return
  `true` (can move). Else, for each nearby player the creaking `canAttack` and is not
  allied with: if that player is looking at the creaking (`isLookingAtMe`, 0.5 fov
  threshold, checked against three candidate eye heights) and either the creaking is
  already inactive or the player isn't wearing a disguise item (carved pumpkin):
  - if already active, `return false` (frozen — a watched, already-active creaking
    can't move at all)
  - else if that player is within 144 (12²) distance, `activate(player)` and
    `return false` (freshly spotted at close range: activate AND freeze this tick)
  If no potential target existed and it was active, deactivate. Otherwise return `true`.
  This is the "immobile while any player looks at it" rule from the task description —
  note it is **conditional on distance/disguise/alliance**, not "any player looking
  freezes it unconditionally" — read the real branch structure above rather than a
  simplified restatement when implementing.
- `activate(Player)`/`deactivate()` (491-503): set `ATTACK_TARGET` memory (Pumpkin:
  becomes the mob's target directly, no Brain memory needed), fire `ENTITY_ACTION` game
  event, play `CREAKING_ACTIVATE`/`CREAKING_DEACTIVATE`, flip `IS_ACTIVE` synced bool.
- `playerIsStuckInYou()` (357-375): counts consecutive ticks a nearby player's eye
  position is inside the creaking's own bounding box; returns true past 4 consecutive
  ticks (used by the block entity to force-remove a stuck protector).
- Eyes-glowing flicker (`checkEyeBlink`, 409-415): client-tick-only random flicker timer,
  cosmetic — low priority, can be deferred past a first PR.
- `getAmbientSound`/`getHurtSound` gate on `isActive()`/`isHeartBound()` respectively;
  `fireImmune()`/`canUsePortal()` gate on `isHeartBound()`.
- `HomeNodeEvaluator` (592-605): blocks pathfinding more than 1024 (32²) blocks from
  home *and* farther from home than the creaking's current position — i.e. a soft
  leash keeping it from wandering away from its heart. This needs Pumpkin's pathfinder
  to expose a per-node-type override hook; check `pumpkin/src/entity/ai/pathfinder/`
  before assuming this is a drop-in (not verified in this pass).

### Target Rust design

`CreakingEntity` (currently a bare `MobEntity` wrapper) grows:
```rust
pub struct CreakingEntity {
    pub mob_entity: MobEntity,
    home_pos: AtomicCell<Option<BlockPos>>,       // HOME_POS, synced
    can_move: AtomicBool,                          // CAN_MOVE, synced
    is_active: AtomicBool,                         // IS_ACTIVE, synced
    is_tearing_down: AtomicBool,                    // IS_TEARING_DOWN, synced
    invulnerability_ticks: AtomicI32,
    attack_animation_ticks: AtomicI32,
    death_time: AtomicI32,                          // for the 45-tick tearDown countdown
    player_stuck_counter: AtomicI32,
}
```
Wiring:
- `set_transient(pos)`: sets `home_pos`, applies the pathfinding-malus overrides (check
  `MobEntity`/`LivingEntity` exposes a `set_pathfinding_malus(PathType, f32)`-equivalent).
- `is_heart_bound()`: `home_pos.load().is_some()`.
- Override `hurt`/`hurt_server` path (find Pumpkin's equivalent hook — likely
  `LivingEntity::damage` or a `hurt`-style trait method on `Mob`/`EntityBase`) to
  replicate the invulnerability/attribution gate above; this needs the responsible-player
  resolution helper (`blameSourceForDamage`/`resolvePlayerResponsibleForDamage`) — check
  whether Pumpkin's `DamageSource` already carries an attributed player/attacker (likely,
  given `RevengeGoal` already reads `last_attacker_id`) before adding new plumbing.
- Per-tick (`aiStep`/`tick` equivalent — check Pumpkin's per-tick mob hook naming):
  call `check_can_move()`, diff against stored `can_move`, play freeze/unfreeze sounds
  and `stop_in_place()` on transition, matching vanilla 213-240.
- `tick()`: if heart-bound, look up the block entity at `home_pos` via
  `world.get_block_entity(pos)`, downcast to `CreakingHeartBlockEntity`, and check a
  **new** method `is_protector(&self, creaking_uuid: Uuid) -> bool` on that block entity
  (compare against its existing `creaking_uuid: Mutex<Option<Uuid>>` field — this is the
  natural extension point, since the block entity already tracks the UUID but has no
  such query method yet). If not a live protector, `set_health(0)`.
- `tick_death()`/`tear_down()`/`creaking_death_effects()`: port directly; `tear_down`'s
  particle bursts need Pumpkin's particle-broadcast API (`world.send_particles` or
  equivalent — used elsewhere, e.g. in `creaking_heart.rs`'s own block-side effects, so
  a working example already exists in this codebase to copy from).
- `check_can_move`/`activate`/`deactivate`: needs a "nearby players" query (plain
  `world.get_nearby_players(pos, range)`-style call, no Brain/memory needed — Pumpkin
  goals already do ad hoc nearby-entity queries, e.g. `AvoidEntityGoal::find_threat`) and
  a "player looking at me" check (`is_looking_at_me`-equivalent — grep for existing
  "is looking at" helpers, e.g. used by `LookAtPlayerGoal`/`beg.rs`/villager gossip
  before writing a new one).

### Consuming side on the block entity (`CreakingHeartBlockEntity`)

This is the other half of the handshake and belongs in the same PR since neither side
is independently testable without the other:
- Add `is_protector(&self, uuid: Uuid) -> bool` (replaces the placeholder logic
  `compute_analog_output_signal` currently special-cases to "no live protector").
- Implement `spawn_protector`/the `serverTick` branch that spawns a `Creaking` near the
  heart when `AWAKE` + a nearby player + monster-spawning allowed, calls
  `creaking.set_transient(pos)`, and stores the new UUID via the existing
  `creaking_uuid` field.
- Implement `remove_protector(damage_source: Option<DamageSource>)`,
  `creaking_hurt()` (drives the resin-spread + particle effects already stubbed as
  "deliberately not ported" in the current file), wired from the `tick()` body's
  `if !CREAKING_ACTIVE && !is_persistence_required || distance > 34 || player_stuck`
  branch (vanilla 126-133).
- `compute_analog_output_signal` needs to actually measure distance-to-creaking once a
  live protector exists (currently hardcoded to `0`).

### Files to create/modify
- `pumpkin/src/entity/mob/creaking.rs` (modify): all state/behavior above.
- `pumpkin/src/block/entities/creaking_heart.rs` (modify): `is_protector`,
  `spawn_protector`, `remove_protector`, `creaking_hurt`, real
  `compute_analog_output_signal`, wiring these into the existing `tick()`.
- Possibly `pumpkin/src/entity/ai/pathfinder/` (modify, if a per-node malus/home-leash
  hook doesn't already exist) for the `HomeNodeEvaluator` leash behavior — lower
  priority than the invulnerability/activation core; acceptable to defer past a first PR
  if the pathfinder doesn't already expose a hook (flag as a known simplification if
  deferred, don't silently drop it).

### Needs wiki/source verification
- Whether Pumpkin's `DamageSource`/attack-attribution already resolves a responsible
  player the way `blameSourceForDamage` does (check, don't assume).
- Whether Pumpkin's pathfinder supports per-node-type malus overrides and a custom node
  evaluator hook (needed for both `setTransient`'s malus calls and `HomeNodeEvaluator`).
- Exact carved-pumpkin "disguise item" check (`PLAYER_NOT_WEARING_DISGUISE_ITEM`) —
  confirm current item/equipment-slot API naming before wiring `checkCanMove`.

---

## 5. Sniffer

### Vanilla (`Sniffer.java` 484 lines) vs. current Rust (51-line entity + `sniffer_dig.rs`)

`sniffer_dig.rs` already exists and its doc comment is explicit: it is a **simplified,
`Goal`-based approximation** of vanilla's Brain/Behavior-driven digging (no
`Sniffer.State` machine, no explored-position memory, flat 50/50 seed choice instead of
the loot table, no panic/temptation/love gating). It is a **separate, self-contained
goal** already correctly wired into `sniffer.rs`'s goal_selector at priority 1 — this
section's job is the `Sniffer.State` enum/state-machine and breeding pieces around it,
which `sniffer_dig.rs` does not attempt and should not be redesigned to attempt (keep it
as the "digs for seeds" approximation it already is; layer the state enum next to it).

Gaps against vanilla `Sniffer.java`:
1. **`State` enum** (463-483): `IDLING`(0), `FEELING_HAPPY`(1), `SCENTING`(2),
   `SNIFFING`(3), `SEARCHING`(4), `DIGGING`(5), `RISING`(6). Currently absent from
   Pumpkin entirely — no `DATA_STATE` synced field on `SnifferEntity`.
2. **`getBabyStartAge()` override** (117-120): `-48000` (twice vanilla's default
   `-24000`/`BABY_START_AGE` in `ageable.rs`) — sniffers take twice as long to grow up.
   Trivial: `impl AgeableMob for SnifferEntity { fn get_baby_start_age(&self) -> i32 { -48000 } }`
   — the trait method already exists with a default (`ageable.rs` line ~31-33) precisely
   so mobs can override it; this is a one-line fix, unrelated to the state machine.
3. **`canMate` override** (427-435): only maes if the partner is also a `Sniffer` in one
   of `{IDLING, SCENTING, FEELING_HAPPY}` *and* both are in a matching state *and* the
   generic `Animal.canMate` conditions hold. Needs the state field to exist first.
4. **`spawnChildFromBreeding` override** (346-353): unlike ordinary `Animal` breeding
   (spawn a live baby), sniffers drop a `SNIFFER_EGG` item entity instead, still call
   `finalizeSpawnChildFromBreeding(level, partner, null)` (resets love/age-cooldowns on
   both parents without an actual offspring entity — verify what this shared helper
   does versus a normal breed to make sure the "no baby, no `getBreedOffspring` call"
   path still resets parent state correctly) and plays `SNIFFER_EGG_PLOP`. Sniffer's own
   `getBreedOffspring` (422-425) is essentially unused/dead by this path but should still
   be ported for interface completeness (some other code may call it directly, e.g. egg
   spawn via dispenser/summon-from-egg flows) — it just creates a fresh default `Sniffer`.
5. **Sound overrides gated on state**: `getAmbientSound` (silence during
   `DIGGING`/`SEARCHING`), `playSearchingSound` (client-only, tickCount%20==0 while
   `SEARCHING`), `canPlayDiggingSound`, `playEatingSound`. Needs the state field.
6. **`onSyncedDataUpdated`** (171-199): drives four separate `AnimationState`s
   (feeling-happy/scenting/sniffing/digging/rising) off state transitions, plus
   `refreshDimensions()` (dimensions shrink during `DIGGING`, `getDefaultDimensions`
   123-125). Animation states are presentation/client-facing; verify how much of
   Pumpkin's animation-state system exists before committing to a full port — this may
   be mostly a no-op on a server-authoritative implementation beyond sending the right
   entity-metadata state byte for clients to animate off of.
7. **`transitionTo(State)`** (209-237): the actual state-change entry point, plays the
   state-appropriate sound and (for `SCENTING`/`DIGGING`) calls `onScentingStart`/
   `onDiggingStart` side effects. `onDiggingStart` sets `DATA_DROP_SEED_AT_TICK` and
   broadcasts entity event 63 — this **duplicates** what `sniffer_dig.rs` already
   approximates internally (its own `digging_ticks` timer). **Design decision needed**:
   either (a) keep `sniffer_dig.rs`'s self-contained timer as the "digging" implementation
   and have the new `State` field be a mostly-cosmetic/read-only reflection driven by
   that goal (simplest, least risk of the two systems fighting over who owns "am I
   digging"), or (b) do the fuller port where `SnifferAi`'s Brain-behaviors
   (`Digging`/`FinishedDigging`, not read in this pass) truly drive the state and
   `sniffer_dig.rs` is rewritten to consume `State` instead of its own `digging_ticks`.
   **Recommend (a) for the first PR**: add the `State` enum as an observable/settable
   field with the sound/animation/canMate/dimension hooks wired to it, and have
   `SnifferDigGoal::start`/`tick`/`stop` additionally call
   `sniffer.transition_to(State::Digging)` / `State::Idling` at its existing
   start/stop boundaries rather than rewriting its timer logic — this gets breeding-gate
   correctness and sound/animation correctness without destabilizing the already-working
   dig goal. `SEARCHING`/`SCENTING`/`FEELING_HAPPY`/`RISING` transitions belong to
   `SnifferAi` behaviors not yet ported at all (walking-to-dig-spot pre-roll, tempted
   "happy" reaction, post-dig rising animation) — these can be added incrementally later
   as their own small goals/state-transition call sites without re-touching this design.

### Target Rust design

```rust
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SnifferState {
    Idling = 0,
    FeelingHappy = 1,
    Scenting = 2,
    Sniffing = 3,
    Searching = 4,
    Digging = 5,
    Rising = 6,
}

pub struct SnifferEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    state: AtomicU8,   // SnifferState, synced via send_meta_data like Sheep's packed byte
}
```
- `SnifferEntity::transition_to(&self, state: SnifferState)`: stores the state, sends
  metadata, plays the state's sound (`SNIFFER_HAPPY`/`SNIFFER_SCENTING`/
  `SNIFFER_SNIFFING`/`SNIFFER_DIGGING_STOP` per vanilla's `transitionTo` switch — note
  `IDLING`/`SEARCHING`/`DIGGING` play no sound *here*, `DIGGING`'s sound is the entity
  event 63 broadcast which `sniffer_dig.rs` already emits via `play_sound_raw` at its own
  dig-start point, so don't double-play it).
  `get_state()`. `is_searching()` = `state == Searching`.
- `impl AgeableMob for SnifferEntity { fn get_baby_start_age() -> i32 { -48000 } }`.
- `impl Animal for SnifferEntity`: **confirmed** (read `pumpkin/src/entity/passive/animal.rs`
  and `pumpkin/src/entity/ai/goal/breed.rs` in full) that neither a `can_mate` nor a
  `spawn_child_from_breeding`/offspring-customization hook exists on the `Animal` trait
  today — `BreedGoal::breed` (breed.rs:67) unconditionally spawns a same-species baby via
  `from_type(...)` with no per-mob override point at all. Adding
  `fn create_offspring(&self, mate: &dyn EntityBase) -> Arc<dyn EntityBase>` and
  `fn spawn_child_from_breeding(&self, mob: &dyn Mob, mate: &dyn EntityBase)` to `Animal`
  (see the cross-cutting section below — this is a shared prerequisite for Sheep/Rabbit/
  Fox genetics too, not Sniffer-specific, so it should land as its own small PR first)
  is a **named, confirmed prerequisite**, not a maybe. Once it exists: gate `can_mate` on
  `{Idling, Scenting, FeelingHappy}` for both sides plus a downcast check that the
  partner is a `SnifferEntity` (`can_mate` needs to be added to `Animal` regardless, with
  a sensible default of "true" for mobs that don't override it, since `BreedGoal::can_start`
  currently has no such gate either — check `breed.rs`'s `can_start` for whether it
  already calls something equivalent before assuming this is purely additive); override
  `spawn_child_from_breeding` to spawn an `Item::SNIFFER_EGG` `ItemEntity` instead of
  calling `create_offspring`, matching vanilla's egg-plop sound.
- `SnifferDigGoal` (modify, minimal): call `sniffer.transition_to(State::Digging)` at
  the point it currently sets `digging_ticks = DIG_DURATION` and plays
  `EntitySnifferDigging`, and `transition_to(State::Idling)` in `stop()`. This requires
  `SnifferDigGoal` to hold a way to reach the concrete `SnifferEntity` (currently it's
  generic over `dyn Mob` — check whether `mob.cast_any().downcast_ref::<SnifferEntity>()`
  is the established pattern here, matching how `dye.rs` downcasts to `SheepEntity`).
- Sound overrides (`get_ambient_sound`, `play_eating_sound`, searching-sound tick):
  straightforward once `get_state()` exists.

### Files to create/modify
- `pumpkin/src/entity/passive/sniffer.rs` (modify): `SnifferState` enum, `state` field,
  `transition_to`/`get_state`, `AgeableMob::get_baby_start_age` override, `Animal`
  `can_mate`/`spawn_child_from_breeding` overrides, sound overrides.
- `pumpkin/src/entity/ai/goal/sniffer_dig.rs` (modify, minimal): two `transition_to`
  call sites as described; no change to its existing timer/seed-drop logic.
- `pumpkin/src/entity/passive/animal.rs` (modify — confirmed necessary, see above): add
  `create_offspring`/`spawn_child_from_breeding`/`can_mate` hooks with defaults matching
  today's behavior.
- `pumpkin/src/entity/ai/goal/breed.rs` (modify — confirmed necessary): `breed()` calls
  `mob.spawn_child_from_breeding(mate)` instead of inlining `from_type(...)` + spawn.

### Needs wiki/source verification
- Whether `finalizeSpawnChildFromBreeding(level, partner, null)` (called by Sniffer's
  egg-drop path with a null offspring) does anything beyond resetting love/age
  cooldowns on both parents that a from-scratch Pumpkin implementation must also do —
  read `Animal.java`'s definition of that helper (not opened in this pass) before
  implementing, since getting parent-state reset wrong would be a silent breeding bug.

---

## Cross-cutting items

- **`BreedGoal` has no offspring-customization hook at all — this blocks every genetics
  item in this doc, not just Sniffer.** Read in full (`pumpkin/src/entity/ai/goal/breed.rs`,
  `breed()` at line 67): the current implementation is hardcoded —
  `from_type(entity.entity_type, parent_pos, &world, Uuid::new_v4())` always spawns a
  bare same-species baby with default state, then sets its age and spawns an XP orb.
  There is **no call site at all** for anything like `Animal::get_breed_offspring` or
  `Animal::spawn_child_from_breeding`, and `pumpkin/src/entity/passive/animal.rs`'s
  `Animal` trait (read in full) has only `is_food`/`write_animal_nbt`/`read_animal_nbt`/
  `animal_interact` — no breeding-outcome hooks whatsoever. Concretely this means: Sheep
  color mixing (section 1), rabbit variant inheritance (section 2), and fox variant
  inheritance (section 3) all currently produce a baby with **no** color/variant at all
  (whatever the type's default synced value is) because nothing downstream of
  `BreedGoal::breed` ever calls a per-mob customization point — and Sniffer's
  egg-instead-of-baby override (section 5) has nowhere to plug in even in principle,
  since `breed()` unconditionally spawns and adds the baby entity before any hook could
  suppress it.
  **This is therefore a real prerequisite for four of this doc's five subsystems, not an
  optional nice-to-have**: `BreedGoal::breed` needs two new `Animal`-trait extension
  points with sensible defaults —
  `fn create_offspring(&self, mate: &dyn EntityBase) -> Arc<dyn EntityBase>` (default:
  today's `from_type(...)` call, overridden by Sheep/Rabbit/Fox to also copy/mix
  color-or-variant state onto the freshly created baby before it's returned), and
  `fn spawn_child_from_breeding(&self, mob: &dyn Mob, mate: &dyn EntityBase)` (default:
  call `create_offspring` then do what `breed()` does today — set age, spawn the baby,
  award XP; overridden by Sniffer to instead drop a `SNIFFER_EGG` item entity and skip
  spawning any baby, while still doing the shared cooldown/stat/advancement/XP-orb
  bookkeeping `breed()` already has above the baby-spawn line). Land this as its own
  small prerequisite PR (touching only `breed.rs` + `animal.rs`, no default-behavior
  change for any mob that doesn't override the new hooks) before any of Sheep/Rabbit/
  Fox/Sniffer's genetics-touching PRs, exactly the same way the DyeColor-mixing-table PR
  below is scoped as an isolated prerequisite.
- **DyeColor promotion + mixing table** (section 1) is the one true shared prerequisite
  among these five subsystems — only Sheep needs it directly, but it's worth landing as
  its own tiny PR before Sheep's genetics land, so Sheep's PR is pure sheep logic.
- **`AvoidEntityGoal` predicate generalization** (sections 2 and 3) is needed by both
  Rabbit (evil-variant gate) and Fox (trust/defending/tame gates) — implement once in
  `avoid_entity.rs`, reused by both per-mob wrapper goals.
- **Jump-control question** (section 2) should be resolved (grep the physics/movement
  layer for a settable "jumping" input) before writing any rabbit-hop code, since it
  decides between a goal-only approximation and new `Control` trait machinery.
- **Sound/animation-state plumbing** (Fox's `interestedAngle`/`crouchAmount` lerps,
  Sniffer's four `AnimationState`s) may be substantially no-ops if Pumpkin's
  server-authoritative model doesn't track client interpolation state server-side —
  worth a quick check of how any *existing* ported mob with an `AnimationState`-like
  field (if any) handles this before assuming full parity is required here.
