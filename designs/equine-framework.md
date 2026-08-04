# Equine (horse-family) framework — design document

Scope: Horse, Donkey, Mule, ZombieHorse, SkeletonHorse, Llama, TraderLlama, Camel.
Today every one of these is a bare `MobEntity` wrapper with 3-5 AI goals and nothing
else — no taming, no chest inventory, no saddling/riding, no `randomizeAttributes`.
This is the single biggest shared-infrastructure gap found in the census: five of the
eight species (Horse/Donkey/Mule/ZombieHorse/SkeletonHorse) are one Java superclass
(`AbstractHorse`, plus `AbstractChestedHorse` for two of those), so the fix should be
one shared framework, not five copy-pasted taming+riding implementations.

Vanilla source root: `/tmp/pumpkin-vanilla-26.2/decompiled/net/minecraft/world/entity/animal/equine/`
(`AbstractHorse.java`, `AbstractChestedHorse.java`, `Horse.java`, `Donkey.java`, `Mule.java`,
`ZombieHorse.java`, `SkeletonHorse.java`, `Llama.java`, `TraderLlama.java`) and
`.../animal/camel/Camel.java`.

Current Rust: `pumpkin/src/entity/passive/{horse,donkey,mule,zombie_horse,skeleton_horse,llama,trader_llama,camel}.rs`,
all following the same shape: a struct wrapping `MobEntity`, a `new()` that registers a
handful of `ai::goal` entries, `NBTStorage`/`Mob` impls with almost nothing overridden.

## What already exists and can be reused (do not reinvent)

- `MobEntity` (`pumpkin/src/entity/mob/mod.rs`) already has generic taming primitives:
  `owner: AtomicCell<Option<Uuid>>`, `is_tamed()` (`owner.is_some()`), `set_owner(Uuid)`.
  `WolfEntity` (`pumpkin/src/entity/passive/wolf.rs`) is the existing example of a tamed
  animal built on top of this. Vanilla `AbstractHorse` keeps a *separate* `FLAG_TAME` bit
  from the owner reference (`isTamed()` reads the byte flag, `tameWithName` sets both
  together) — in every vanilla code path they're set atomically together, so reusing
  `MobEntity::is_tamed()`/`set_owner()` as the single source of truth is safe and avoids
  a duplicate bool. Note this equivalence explicitly in the horse code so a future reader
  doesn't "fix" it into two flags that can desync.
- `pumpkin-data` already defines the tracked-data (entity metadata) slots this needs:
  `TrackedData::HORSE_FLAGS`, `ID_CHEST`, `ID_TYPE_VARIANT`, `STRENGTH`, `DASH`,
  `LAST_POSE_TICK`, `SADDLED` (`pumpkin-data/src/generated/tracked_data.rs`). Nothing
  needs to be added there — only wired up from entity code via `entity.send_meta_data`
  the same way `WolfEntity::mob_init_data_tracker` does it for `COLLAR_COLOR`/`WOLF_VARIANT_ID`.
- `pumpkin-data`'s `EquipmentSlot::SADDLE` and `EquipmentSlot::BODY` already exist
  (`pumpkin-data/src/data_component_impl/mod.rs:335,342`) but nothing currently uses them
  on the equine entities.
- `pumpkin-inventory::generic_container_screen_handler::create_generic_9x3/9x6/3x3` plus
  `ScreenHandlerFactory` (see `pumpkin/src/block/blocks/chests.rs` for the canonical
  pattern: a small `struct XScreenFactory(Arc<dyn Inventory>)` implementing
  `ScreenHandlerFactory`, opened via `player.open_handled_screen(&factory, pos)`) is the
  exact mechanism to reuse for "open horse inventory screen" — vanilla's
  `AbstractHorse.openCustomInventoryScreen` → `player.openHorseInventory(this, this.inventory)`
  is functionally the same "open a screen backed by an `Inventory`" call. Horse
  inventories need a **new** screen handler shape (saddle+armor slot at index 0/1, then
  chest columns) — `HorseInventoryMenu.java` (`net/minecraft/world/inventory/HorseInventoryMenu.java`)
  is the exact vanilla layout to port; it isn't identical to the generic container.
- `pumpkin/src/entity/ai/goal/horse_breed.rs` (`HorseBreedGoal`) already implements
  generic breed-search/approach/timer/spawn-offspring logic parameterized by a
  `compatible: &[&EntityType]` list and a `horse_family_offspring` free function — reuse
  this unchanged for Donkey/Mule breeding as instructed; do not write a second breed goal.
- `pumpkin/src/entity/passive/animal.rs` (`Animal` trait) already has `animal_interact`
  (generic "is this food → breed/age-up" dispatch) that every horse-family `mob_interact`
  will still call into, same as today.

## What's genuinely new (the shared framework)

Nothing today models: (a) a synced tame/owner/temper/eating/standing bit-flags byte,
(b) an entity-backed `Inventory` (saddle/armor/chest slots) distinct from a *block*
inventory, (c) rider-controlled movement/jump-charge (no `PlayerRideableJumping`
equivalent trait exists anywhere in Pumpkin — boats/minecarts are client-paddled, not
server rider-input-driven, so there is no existing "shape" to copy for jump charging).
This is the real net-new work and it's genuinely shared across all 6 saddleable species
(5 AbstractHorse + Camel).

### 1. `AbstractHorseData` struct + `AbstractHorse` trait (new file: `pumpkin/src/entity/passive/equine/mod.rs`)

Mirrors vanilla's `AbstractHorse` fields as a plain data struct embedded by value in each
species struct (same pattern `MobEntity` uses today — composition, not inheritance):

```rust
pub struct AbstractHorseData {
    pub flags: AtomicU8,           // FLAG_BRED=8, FLAG_EATING=16, FLAG_STANDING=32, FLAG_OPEN_MOUTH=64
                                    // (FLAG_TAME=2 folded into MobEntity::owner, see above)
    pub temper: AtomicI32,
    pub inventory: Arc<dyn Inventory>,   // SimpleContainer equivalent; size = getInventorySize()
    pub player_jump_pending_scale: AtomicCell<f32>, // set by onPlayerJump, consumed in tickRidden
    pub allow_stand_sliding: AtomicBool,
    pub can_gallop: AtomicBool,      // false for AbstractChestedHorse subtypes
    // eat/stand/mouth animation counters are client-visual-only in vanilla; skip unless
    // Pumpkin already ports idle animation state generally (it doesn't) — note as
    // out-of-scope polish, not a functional gap.
}
```

`AbstractHorse` trait (extends `Mob`, mirrors `Animal`'s pattern of default-method
sharing):

```rust
pub trait AbstractHorse: Mob {
    fn horse_data(&self) -> &AbstractHorseData;

    fn get_inventory_columns(&self) -> u8 { 0 }               // Horse: 0; overridden by chested horses
    fn can_perform_rearing(&self) -> bool { true }             // Llama/Camel: false
    fn max_temper(&self) -> i32 { 100 }                        // Llama: 30
    fn eating_sound(&self) -> Option<Sound> { None }
    fn angry_sound(&self) -> Option<Sound> { None }

    fn is_tamed(&self) -> bool { self.get_mob_entity().is_tamed() }
    fn set_tamed(&self, player: Uuid) { self.get_mob_entity().set_owner(player); /* + broadcast entity event 7, spawn_taming_particles(true) */ }

    fn is_saddled(&self) -> bool; // reads EquipmentSlot::SADDLE via LivingEntity equipment map

    // Vanilla `AbstractHorse.handleEating` — species override this (Llama has a different
    // food table with different heal/ageUp/temper numbers; see Llama.java:179-234).
    fn handle_eating(&self, player: &Player, item_stack: &mut ItemStack) -> bool { .. default wheat/sugar/hay/apple/mushroom/carrot/golden-carrot/golden-apple table per AbstractHorse.java:423-493 .. }

    fn fed_food(&self, player: &Player, item_stack: &mut ItemStack) -> bool { .. AbstractHorse.java:414-421 .. }

    fn make_mad(&self);       // AbstractHorse.java:703-708: stand + angry sound
    fn stand_if_possible(&self);
    fn do_player_ride(&self, player: &Arc<Player>);  // start_riding + clear eating/standing

    // Default mobInteract shared by Horse/ZombieHorse (chested horses override in
    // AbstractChestedHorse instead, see below)
    fn abstract_horse_mob_interact<'a>(&'a self, player: &'a Arc<Player>, item_stack: &'a mut ItemStack)
        -> EntityBaseFuture<'a, bool> { .. AbstractHorse.java:643-669 port .. }

    fn randomize_attributes(&self, random: &mut impl RngExt) {} // no-op default; species override
}
```

Rider-jump mechanics (`PlayerRideableJumping` in vanilla) is the piece with **no existing
shape in Pumpkin at all** — there's no server-side "rider is charging a jump by holding
space while mounted" concept anywhere (boats are client-paddled; minecarts don't jump).
This needs new plumbing, shared by AbstractHorse *and* Camel (per the task's note that
Camel doesn't share the chest/taming side but does share the jump-charge shape):

```rust
pub trait RideableJumping: Mob {
    fn can_jump(&self) -> bool;                       // AbstractHorse: is_saddled(); Camel: !refuse_to_move() && saddled
    fn on_player_jump(&self, jump_amount: u8);         // stores pending scale (0..=1.0 from jump_amount/100)
    fn handle_start_jump(&self, jump_strength_scale: i32); // play jump sound, set standing (horse) / dashing (camel)
    fn handle_stop_jump(&self) {}
    fn execute_riders_jump(&self, amount: f32, input: Vector3<f64>); // apply the actual velocity impulse
}
```

This requires (new, not just refactor):
- A server-side notion of "the mounted player is holding jump" arriving over the network.
  Today `SPlayerInput`/`Action::StartSneaking` is the only rider-relevant input forwarded
  (see `pumpkin/src/net/java/play.rs:1171-1195`); jump/forward/sideways axes for a
  *mounted* player are not read anywhere. This is the actual new-infrastructure line item —
  budget it as its own sub-task, not a one-line addition. Needs: reading the client's jump
  flag + forward/strafe axes while `has_vehicle()` is true, translating them into
  `get_ridden_input`/`tickRidden`-equivalent per-tick calls into the vehicle
  (`AbstractHorse.tickRidden`, `AbstractHorse.getRiddenInput`, `AbstractHorse.getRiddenSpeed`,
  `AbstractHorse.executeRidersJump` — `AbstractHorse.java:722-782`).
- A generic "vehicle drives its own `travel()`" tick hook analogous to vanilla's
  `LivingEntity.travel` dispatching through `getControllingPassenger()` — Pumpkin's
  `VehicleEntity` (`pumpkin/src/entity/vehicle/vehicle.rs`) currently only tracks
  hurt/damage wobble, nothing about passenger-driven movement. Horse/Camel movement
  under a rider needs this; it's the largest single new piece of infrastructure in this
  whole design.

Given the size of that gap, recommend phasing: **Phase 1** ships taming, feeding,
chest inventory, breeding, variants/attributes, sounds — i.e., everything except actual
player-controlled riding movement (a saddled/mounted horse exists, can be sat on, but
doesn't yet respond to WASD+space the way vanilla does). **Phase 2** adds the
rider-input plumbing described above and wires `RideableJumping` for both AbstractHorse
and Camel. Ship Phase 1 as its own PR — it's already the large win and is independently
testable; don't block it on the rider-movement rewrite.

### 2. `AbstractChestedHorseData` + `AbstractChestedHorse` trait (same module)

```rust
pub trait AbstractChestedHorse: AbstractHorse {
    fn has_chest(&self) -> bool;               // reads TrackedData::ID_CHEST
    fn set_chest(&self, value: bool);           // also recreates inventory (AbstractChestedHorse.java:63-65,102-108)
    fn play_chest_equips_sound(&self);          // species-specific sound; default DONKEY_CHEST

    fn get_inventory_columns(&self) -> u8 { if self.has_chest() { 5 } else { 0 } } // Llama overrides -> strength

    // AbstractChestedHorse.java:143-167 — shared mobInteract for Donkey/Mule/Llama/TraderLlama:
    // baby+golden-dandelion passthrough, secondary-use+tamed -> open inventory, food -> fedFood,
    // untamed -> makeMad, chest-equip-with-item, else fall through to AbstractHorse's ride/armor logic.
    fn chested_mob_interact<'a>(&'a self, player: &'a Arc<Player>, item_stack: &'a mut ItemStack)
        -> EntityBaseFuture<'a, bool> { .. port directly .. }
}
```

`createBaseChestedHorseAttributes` (speed 0.175, jump 0.5 on top of the horse base) is a
constant table, not behavior — put it alongside the other attribute tables already used
for entity spawn (wherever Pumpkin currently defines base attribute suppliers per
species; follow existing convention, don't invent a new location).

### 3. Horse inventory screen (new: `pumpkin-inventory/src/horse_screen_handler.rs`)

Vanilla `HorseInventoryMenu` has a fixed layout: slot 0 = saddle, slot 1 = armor/chest
item (species-dependent), then `columns * 3` chest slots if `hasChest()`, then the
player's own inventory. This is genuinely a new screen handler (not reducible to
`create_generic_9x3`), because of the saddle/armor slots and the horse-specific slot
predicates (`Slot.mayPlace` gating saddle slot to saddle items only, etc.). Port
`HorseInventoryMenu.java` directly; it's a bounded, well-scoped single file.

### 4. Files to create

- `pumpkin/src/entity/passive/equine/mod.rs` — `AbstractHorseData`, `AbstractHorse` trait,
  `AbstractChestedHorseData`, `AbstractChestedHorse` trait, `RideableJumping` trait,
  shared `handle_eating` default table, shared `mobInteract` defaults, `randomize_attributes`
  helper functions (`generate_max_health`, `generate_jump_strength`, `generate_speed`,
  `create_offspring_attribute` — straight ports of `AbstractHorse.java:938-947,841-863`,
  these are pure functions, trivially portable and testable).
- `pumpkin-inventory/src/horse_screen_handler.rs` — `HorseInventoryMenu` port + factory.
- (Phase 2 only) a rider-input module, e.g. `pumpkin/src/entity/ai/rideable_jumping.rs` or
  folded into `vehicle.rs`, plus changes to `net/java/play.rs` input handling and
  `Entity`/`VehicleEntity` to route mounted-player input into `travel`/`tickRidden`.

### 5. Files to modify

- `pumpkin/src/entity/passive/mod.rs` — add `pub mod equine;`.
- All eight `pumpkin/src/entity/passive/{horse,donkey,mule,zombie_horse,skeleton_horse,
  llama,trader_llama}.rs` (Camel is separate, see below) — embed `AbstractHorseData`
  (and `AbstractChestedHorseData` for donkey/mule/llama/trader_llama), implement the
  traits, replace the stub `Mob::mob_interact` with the shared dispatch, add
  `NBTStorage` read/write for temper/tame/owner/bred/eating (`AbstractHorse.java:788-806`)
  and, for chested species, `ChestedHorse`+`Items` NBT (`AbstractChestedHorse.java:81-109`).

## Per-species sections

### Horse (`pumpkin/src/entity/passive/horse.rs`)

- Implements `AbstractHorse` (not chested).
- `DATA_ID_TYPE_VARIANT` (`Horse.java:41,60-95`): single packed int, low byte = `Variant`
  (7 coat colors: white/creamy/chestnut/brown/black/gray/darkbrown per the wiki — verify
  exact ordinal-to-name mapping against the wiki before hardcoding, vanilla source only
  has numeric ids here), high byte = `Markings` (5: none/white/whitefield/whitedots/blackdots
  — also verify against wiki). Persist as `"Variant"` NBT int (`Horse.java:66-75`), synced
  via `TrackedData::ID_TYPE_VARIANT`.
- `mobInteract` (`Horse.java:156-175`): same shared `AbstractHorse` dispatch as
  `ZombieHorse`, layered with `canMate`/`getBreedOffspring` for Horse+Donkey→Mule and
  Horse+Horse→Horse-with-inherited-variant/markings (`Horse.java:177-225`) — reuse
  `HorseBreedGoal`'s `horse_family_offspring` for the *type* decision, but the
  variant/markings inheritance roll (`Horse.java:199-217`, 4/9 self, 4/9 mate, 1/9 random
  for variant; 2/5 self, 2/5 mate, 1/5 random for markings) is new logic specific to
  Horse×Horse breeding — put it in `Horse::get_breed_offspring`, not in the shared goal.
- `randomizeAttributes` (`Horse.java:52-57`): rolls max-health, speed, AND jump-strength
  (unlike the chested horses, which only roll max-health).
- `canUseSlot`/`hurtArmor` overrides (`Horse.java:227-235`) — Horse can wear body armor
  in any slot state and its armor absorbs damage; port only if body-armor damage
  absorption is otherwise implemented for equipment generally, else file as a follow-up.

### Donkey / Mule (`donkey.rs`, `mule.rs`)

- Both implement `AbstractChestedHorse`. Reuse `HorseBreedGoal` with compatible-types
  list already present in `donkey.rs`/absent in `mule.rs` today (Mule can't breed at all
  in vanilla — `Mule.getBreedOffspring` always returns a fresh Mule but there is no
  `BreedGoal` registered for Mule in vanilla; verify this against `Mule.java`/wiki, the
  file here has no `registerGoals` override so it inherits `AbstractHorse`'s
  `BreedGoal(this, 1.0, AbstractHorse.class)` — meaning mules DO enter the breed goal but
  `canMate` is never overridden so it inherits `AbstractHorse.canMate` which returns
  `false` unconditionally; net effect: mules never breed. Keep `HorseBreedGoal` off the
  Mule's goal list, or gate `canMate` to always return false — pick whichever is less
  code, note the vanilla source rationale in a comment either way).
- Donkey sounds: AMBIENT/ANGRY/DEATH/EAT/HURT + `playJumpSound` = `DONKEY_JUMP`
  (`Donkey.java:20-57`). Mule sounds: same shape with `MULE_*` + `playChestEquipsSound`
  override → `MULE_CHEST` (`Mule.java:19-52`).
- `getBreedOffspring`: Donkey × {Horse→Mule, else→Donkey} (`Donkey.java:59-68`); Mule ×
  anything → Mule (`Mule.java:54-57`). Per the task, this breeding logic is described as
  "already ported" in `horse_breed.rs`'s `horse_family_offspring` — confirmed correct,
  reuse as-is, do not reimplement per-species.

### ZombieHorse (`zombie_horse.rs`)

- Implements `AbstractHorse` (not chested). New file needs:
  - `createAttributes` base (25 max health) — `ZombieHorse.java:53-55`.
  - `mobInteract` (`ZombieHorse.java:149-168`) — same AbstractHorse dispatch, no armor
    branch (matches base, no chest).
  - `isFood`/`ZOMBIE_HORSE_FOOD` tag gate (`ZombieHorse.java:181-183`) — check this tag
    exists in `pumpkin-data::tag::Item` already (likely does, given `tag::Item::MINECRAFT_HORSE_FOOD`
    is already referenced elsewhere in the current stub files for the wrong tag — Horse/Donkey
    currently wrongly use `MINECRAFT_HORSE_FOOD` inside `is_food`, which is *correct* for
    Horse/Donkey but ZombieHorse needs the distinct `ZOMBIE_HORSE_FOOD` tag).
  - `randomizeAttributes` uses ZombieHorse-specific jump/speed formulas
    (`generateZombieHorseJumpStrength`/`generateZombieHorseSpeed`, `ZombieHorse.java:74-88`)
    — different constants from the base `AbstractHorse` formula, port as written, don't
    reuse the generic one.
  - `canFallInLove()=false`, `canAgeUp()=false` (`ZombieHorse.java:121-123,206-208`) — check
    whether `Animal`/`MobEntity` already exposes these as overridable hooks; if not, this
    is a small trait-default addition to `Animal`.
  - Zombie-jockey spawn on natural spawn (`ZombieHorse.java:132-146`): spawn a `Zombie`,
    equip `IRON_SPEAR` (verify this is the correct 26.2 item name against the decompile —
    it reads `Items.IRON_SPEAR` not `Items.IRON_SWORD`, that's a notable recent vanilla
    change worth double-checking against the wiki before porting), `startRiding` on the
    horse. This needs Pumpkin's mob-spawn finalization hook (`finalizeSpawn` equivalent) —
    check whether Pumpkin's `EntityType`→spawn path has a matching "on natural spawn,
    inject a jockey" hook already used elsewhere (chicken jockeys / spider jockeys are
    the closest existing precedent — search for those before adding new plumbing).
  - `canUseSlot` returns true unconditionally (`ZombieHorse.java:171-173`) — armor always
    equippable regardless of tame state, unlike base `AbstractHorse`.

### SkeletonHorse (`skeleton_horse.rs`)

Smallest change in this whole batch — per the task, the trap-despawn timer is *already*
correctly ported (`is_trap`/`trap_time`/`TRAP_MAX_LIFE` in the current file). The only
gap is the `mobInteract` gate:

```java
// SkeletonHorse.java:174-176
public InteractionResult mobInteract(Player player, InteractionHand hand) {
   return !this.isTamed() ? InteractionResult.PASS : super.mobInteract(player, hand);
}
```

Add a `Mob::mob_interact` override on `SkeletonHorseEntity` that checks
`self.mob_entity.is_tamed()` and returns "pass through" (no interaction at all) when
untamed — an untamed skeleton horse (the ones spawned by lightning-near-lightning-rod,
already correctly modeled by `is_trap`) simply cannot be fed/ridden/opened until tamed
by some other mechanism. This is the one-line species override once the shared
`AbstractHorse` framework exists; it does not need the chest trait.

### Llama / TraderLlama (`llama.rs`, `trader_llama.rs`)

Llama implements `AbstractChestedHorse` (chest capacity = `strength`, not fixed 5) but
diverges from the shared defaults in several places — port these as explicit overrides,
not by forcing them through the generic path:

- `DATA_STRENGTH_ID` (1-5, random-rolled on spawn: 4% chance of 5, else 1-3,
  `Llama.java:93-96`) synced via `TrackedData::STRENGTH`; `DATA_VARIANT_ID` (creamy/
  white/brown/gray, `Llama.java:490-522`) synced via a Llama-specific variant tracked-id
  — check `pumpkin-data` for an existing `LLAMA_VARIANT`-shaped tracked id alongside the
  generic `STRENGTH`/`DASH` ones found already; if absent, it needs adding to
  `pumpkin-data` (out of scope for this doc's "don't touch pumpkin-data" instruction —
  flag as a small prerequisite PR).
- `handleEating` is fully overridden, not the base table (wheat/hay only, different heal/
  ageUp/temper numbers, `Llama.java:179-234`) — implement as an override of the
  `AbstractHorse::handle_eating` default, do not try to parametrize the generic table to
  cover it.
- `getMaxTemper() = 30` (vs. 100 default), `canPerformRearing() = false`,
  `canEatGrass() = false`, `isImmobile` overridden to drop the "eating grass" case.
- `getBreedOffspring` returns a `Llama` (not `AgeableMob` supertype signature) with its
  own strength/variant inheritance roll (`Llama.java:319-334`) — same shape as Horse's
  variant-inheritance override, implement directly, not through `HorseBreedGoal`'s
  generic offspring-type helper (Llama doesn't use `horse_family_offspring` at all;
  `makeNewLlama()` is what Donkey/Mule/Horse's shared goal calls into via
  `EntityTypes.X.create`, but the strength/variant assignment on the *resulting* baby is
  llama-specific post-processing the shared goal doesn't do today — either extend
  `HorseBreedGoal::breed` with an optional post-spawn hook, or give Llama its own thin
  breed goal wrapper that calls the shared approach/timer logic but a custom `breed()`).
- Caravan mechanic (`joinCaravan`/`leaveCaravan`/`hasCaravanTail`/`inCaravan`/
  `followMommy` override, `Llama.java:387-427`): new `caravan_head`/`caravan_tail`
  fields (each an `Weak<LlamaEntity>` or entity-id reference — check how Pumpkin
  generally references other live entities without holding a strong `Arc` cycle;
  likely an entity-id lookup through the world rather than a raw weak pointer, follow
  whatever pattern `owner: AtomicCell<Option<Uuid>>` on `MobEntity` establishes) plus
  `LlamaFollowCaravanGoal` port (`ai/goal/LlamaFollowCaravanGoal.java` — new goal file,
  `pumpkin/src/entity/ai/goal/llama_follow_caravan.rs`).
- Spit ranged attack (`Llama.java:340-365,435-437`): needs a `LlamaSpit` projectile
  entity (`net/minecraft/world/entity/projectile/LlamaSpit.java` — check whether
  Pumpkin's projectile module already has a llama-spit-shaped simple projectile to
  extend, or whether this is a new projectile type: new file
  `pumpkin/src/entity/projectile/llama_spit.rs` most likely) plus `RangedAttackGoal`
  (does Pumpkin already have a generic `RangedAttackGoal`? `ai/goal/ranged_bow_attack.rs`,
  `ranged_crossbow_attack.rs`, `ranged_snowball_attack.rs` exist — the pattern is clearly
  established, a `ranged_llama_spit.rs` or a parametrized generic ranged-attack goal
  fits the existing convention).
- `LlamaHurtByTargetGoal`/`LlamaAttackWolfGoal`/`TraderLlamaDefendWanderingTraderGoal`
  (`Llama.java:454-488`, `TraderLlama.java:132-165`) — small custom `target::TargetGoal`
  subclasses; port directly, each is under 30 lines in vanilla.
- TraderLlama additionally: despawn timer tied to its `WanderingTrader` leash-holder
  (`TraderLlama.java:71-107`), `PanicGoal` extra copy, `doPlayerRide` blocked while
  leashed to a wandering trader. All independent of the shared framework, port as
  species-specific overrides on top of `Llama`'s (Rust: compose `LlamaEntity` inside
  `TraderLlamaEntity` the same way the current stub does, or make `TraderLlamaEntity`
  hold an `AbstractChestedHorseData`+extra despawn-delay field directly — whichever
  matches how Pumpkin generally handles "subclass of a subclass" elsewhere, e.g. check
  `WanderingTrader`/`Villager` relationship for precedent since that's the closest
  existing case of an NPC "variant" species).

### Camel (`camel.rs`)

Explicitly **not** built on `AbstractChestedHorse`/`AbstractHorse`'s chest+taming side in
vanilla (`Camel extends AbstractHorse` directly but overrides `isTamed()` to always
return `true` — `Camel.java:653-655` — and has no chest at all). Design independently:

- `isTamed()` always `true` (`Camel.java:653-655`) — Camel can always be fed/ridden/have
  its inventory opened without ever being tamed by a player. If `AbstractHorse::is_tamed`
  is implemented as a default method calling `MobEntity::is_tamed()`, Camel's impl simply
  overrides it to return `true` unconditionally — cheap to support once the trait exists.
- `openCustomInventoryScreen` (`Camel.java:657-662`) — no vehicle/passenger gate at all
  (unlike base `AbstractHorse`, which requires `!isVehicle() || hasPassenger(player)`).
  Camel's inventory is 1 saddle slot only in practice (no chest ever) — confirm against
  `HorseInventoryMenu`/wiki whether Camel gets *any* extra slots; from the source here it
  looks like plain `this.inventory` sized by `getInventoryColumns()` which Camel never
  overrides, so it inherits `AbstractHorse`'s default `0` columns → saddle-only.
- Dashing (`isDashing`/`setDashing`, `TrackedData::DASH`, `Camel.java:319-336`) +
  `dashCooldown`/`getJumpCooldown` (55-tick cooldown, `Camel.java:64,294,347-349`) — this
  **is** the `RideableJumping`-shape piece the task calls out as shared with the horse
  framework: `canJump`/`onPlayerJump`/`handleStartJump`/`handleStopJump`/
  `executeRidersJump` (`Camel.java:288-297,305-333,343-349`) map directly onto the
  `RideableJumping` trait proposed above, just with camel-specific formulas (dash impulse
  uses `getLookAngle()` horizontal direction × movement speed rather than horse's
  yaw-sin/cos jump-forward nudge) and camel-specific side effects (sets `dashing=true` +
  55-tick cooldown instead of "stand" animation).
- Sitting/pose state machine (`isCamelSitting`/`isCamelVisuallySitting`/
  `isInPoseTransition`/`sitDown`/`standUp`/`standUpInstantly`, `Camel.java:572-628`,
  backed by `TrackedData::LAST_POSE_TICK` encoding sit-state as a signed game-tick value)
  — entirely camel-specific, no shared framework piece; needed for `CamelSitGoal`
  (already exists per `pumpkin/src/entity/ai/goal/camel_sit.rs` — verify it currently
  reads/writes this pose state correctly or is a stub; the task description doesn't flag
  sitting as broken, so check before assuming it needs rework, but the sit state clearly
  needs to interact with `refuseToMove()`/rider-input gating below which currently has no
  wiring at all since Camel has no `getRiddenInput` override in the Rust code today).
- `getRiddenInput`/`getRiddenSpeed`/`getRiddenRotation`/`tickRidden` overrides
  (`Camel.java:250-285`) all gate on `refuseToMove()` (sitting or mid pose-transition) —
  these depend on the same Phase-2 rider-input plumbing flagged above for
  `AbstractHorse`; Camel and Horse should share that plumbing's plumbing (the "read
  mounted-player WASD+jump, call into `travel`" mechanism) even though the two species'
  actual travel/jump math stay separate per-species overrides.
- `mobInteract` (`Camel.java:381-402`): sneaking → open inventory (no tame check, since
  always tamed); food → `fedFood`; else if passenger count < 2 and not baby → ride
  (camel supports 2 passengers, `canAddPassenger` caps at `<= 2`, `Camel.java:397,568-570`);
  baby + golden dandelion → passthrough. Two-passenger seating math
  (`getPassengerAttachmentPoint`, `Camel.java:496-512`) is cosmetic/positional, lower
  priority than the interaction gating itself.
- `handleEating` is a full override (not the base table): heals 2, sets in-love,
  ages up by 10 (fixed, not itemstack-dependent), on `CAMEL_FOOD` tag
  (`Camel.java:376-378,422-468`).
- `canPerformRearing() = false` (no "random stand" idle goal, matches Llama).

## Verification needed against the wiki (not just source-reading)

- Horse `Variant`/`Markings` name-to-id mapping (coat colors and marking patterns) —
  the decompile only has numeric ids and enum ordinals, no display names; needed to get
  breed-offspring inheritance and NBT persistence right for a player-visible feature.
- `ZombieHorse.java`'s jockey mainhand item: confirm `IRON_SPEAR` is correct for the
  current game version rather than assuming it's a decompiler artifact of `IRON_SWORD`.
- Whether Mule actually breeds in live vanilla despite inheriting `AbstractHorse`'s
  `BreedGoal` registration (source suggests no, since `canMate` returns `false` and is
  never overridden by `Mule`, but this is exactly the kind of claim this campaign's rules
  say must be checked against current behavior, not inferred from source reading alone).
- Camel's saddle-only inventory (no chest slots ever) — confirm no other item slot
  exists (e.g. some armor-body-slot equivalent) before treating it as saddle-only.
- Llama variant tracked-data id existing (or not) in `pumpkin-data`'s generated tables —
  checked `HORSE_FLAGS`/`ID_CHEST`/`ID_TYPE_VARIANT`/`STRENGTH`/`DASH`/`LAST_POSE_TICK`/
  `SADDLED` exist; did not confirm a distinct Llama-variant id exists under a different
  name — search before assuming it needs to be added.

## Summary of file list

New:
- `pumpkin/src/entity/passive/equine/mod.rs` (shared `AbstractHorseData`/`AbstractHorse`,
  `AbstractChestedHorseData`/`AbstractChestedHorse`, `RideableJumping` traits + pure
  attribute-roll helper functions)
- `pumpkin-inventory/src/horse_screen_handler.rs` (`HorseInventoryMenu` port)
- `pumpkin/src/entity/ai/goal/llama_follow_caravan.rs`
- `pumpkin/src/entity/projectile/llama_spit.rs` (+ a ranged-attack goal file following
  the existing `ranged_*_attack.rs` convention)
- Phase 2 only: a rider-input routing module (exact location TBD — smallest diff is
  probably extending `entity/vehicle/vehicle.rs` plus `net/java/play.rs`'s input handler)

Modified:
- `pumpkin/src/entity/passive/mod.rs` (module registration)
- `pumpkin/src/entity/passive/{horse,donkey,mule,zombie_horse,skeleton_horse,llama,
  trader_llama,camel}.rs` (all eight — embed shared data, implement traits, port
  species-specific overrides and NBT fields)
- `pumpkin/src/entity/passive/animal.rs` (only if `canFallInLove`/`canAgeUp` hooks need
  adding as trait defaults — check first, may already be expressible)
- Possibly `pumpkin-data` (Llama variant tracked-id, if confirmed missing) — flag as a
  small separate prerequisite PR, not part of this framework's own diff.
