# Design: unimplemented / stubbed items

Scope: `ShearsItem`, `CompassItem` (lodestone lock), `SpyglassItem`, `BrushItem` (+ its
block-entity `brush()`), `BundleItem` stacked-click overrides, `ShieldItem` dye-color
name, `CrossbowItem` firework-rocket ammo. Vanilla source is the decompiled 26.2 tree at
`/tmp/pumpkin-vanilla-26.2/decompiled/`; all line numbers below refer to that tree.

Read-only research doc. No files under `pumpkin/`, `pumpkin-data/`, `pumpkin-world/`, or
`pumpkin-inventory/` were modified while producing this document.

## Shared prerequisite: a per-tick "use" hook is missing

`pumpkin/src/item/items/mod.rs`'s `ItemBehaviour` trait (pumpkin/src/item/mod.rs) has
`normal_use`, `use_on_block`, `use_on_entity`, `on_stopped_using`, and `get_use_duration`,
but **no `onUseTick` equivalent**. The only per-tick logic for an item held in the "using"
state lives inline in `LivingEntity` (pumpkin/src/entity/living.rs, the `tick_in_void`-
adjacent block around lines 2973-3138): it hardcodes food-eating, `Consumable` sound/
game-event, potion-effect application, and stack decrement/cooldown-start when
`item_use_time` reaches 0. There is no dispatch point that calls into an
`ItemBehaviour` impl once per tick while the item is in use, matching vanilla's
`Item#onUseTick(Level, LivingEntity, ItemStack, int ticksRemaining)`.

This blocks three of the seven items designed below:
- `BrushItem.onUseTick` (particles + brush-block progression every 5 ticks — vanilla
  BrushItem.java:58-96)
- `CrossbowItem.onUseTick` (progressive loading-start/loading-mid sounds — vanilla
  CrossbowItem.java:202-238; Pumpkin's crossbow.rs currently only plays the loading-end
  sound, in `on_stopped_using`)
- `BundleItem.onUseTick` (repeated single-item drops while holding right-click — vanilla
  BundleItem.java:219-228) — lower priority since Pumpkin's bundle already offers a
  single-shot "drop one" via `normal_use`.

**Recommended fix, once, shared by all three:** add
```rust
fn on_use_tick<'a>(
    &'a self,
    _player: &'a Player,
    _stack: &'a ItemStack,
    _ticks_remaining: i32,
) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
    Box::pin(async {})
}
```
to `ItemBehaviour`, and call it once per tick from the same block in `living.rs` that
currently does `self.item_use_time.fetch_sub(1, ...)` — call it *before* the `<= 0`
branch fires, every tick the item is active, passing
`self.item_use_time.load(Ordering::Relaxed)` as `ticks_remaining` (vanilla calls
`onUseTick` every tick regardless of whether `useItem` is about to finish this tick).
This requires locating the registered `ItemBehaviour` for the currently-used item's
`Item` (the registry lookup pattern already used elsewhere via `ItemRegistry`).

This is called out once here so BrushItem's and CrossbowItem's per-file sections below
can assume it exists; it is a genuine new-code item in the file list, not decoration.

---

## 1. ShearsItem

### Vanilla source
`net/minecraft/world/item/ShearsItem.java` (86 lines):
- `createToolProperties()` (32-45): builds a `Tool` data component —
  `Tool.Rule.minesAndDrops(#COBWEB, 15.0F)`, then three `overrideSpeed` rules keyed off
  `#minecraft:shears_extreme_breaking_speed` (15.0F), `#minecraft:shears_major_breaking_speed`
  (5.0F), `#minecraft:shears_minor_breaking_speed` (2.0F). These tags already exist as
  data files (`data/minecraft/tags/block/shears_{extreme,major,minor}_breaking_speed.json`)
  and are exposed on the Rust side as `tag::Block::MINECRAFT_SHEARS_EXTREME_BREAKING_SPEED`
  etc. (confirmed present in `pumpkin-data/src/generated/tag.rs`).
- `mineBlock` (47-59): server-side only, damages the shears by `tool.damagePerBlock()`
  (1, from the `Tool` component's 4th ctor arg) unless the broken block `is(BlockTags.FIRE)`.
- `useOn` (61-85): if the clicked block is a `GrowingPlantHeadBlock` (kelp, twisting
  vines, weeping vines, cave vines — anything not already at max age) and not already at
  max age: fires `CriteriaTriggers.ITEM_USED_ON_BLOCK`, plays `GROWING_PLANT_CROP` sound,
  sets the block to its max-age state, fires `GameEvent.BLOCK_CHANGE`, damages the shears
  by 1, returns `SUCCESS`. Otherwise falls through to `super.useOn` (does nothing, `PASS`).

Related but out of `ShearsItem.java` itself, all still driven by "is this stack shears":
- `Sheep.mobInteract` (`net/minecraft/world/entity/animal/sheep/Sheep.java:146-191`):
  `itemStack.is(Items.SHEARS)` + `readyForShearing()` (`!isSheared() && !isBaby()`) →
  `shear()`: plays `SHEEP_SHEAR`, drops from loot table `SHEAR_SHEEP` (colored wool,
  1-3 stacks per vanilla loot table, with velocity jitter), `setSheared(true)`; caller
  (`mobInteract`) then does `itemStack.hurtAndBreak(1, player, hand)` and returns
  `SUCCESS_SERVER`. If not ready-for-shearing, returns `CONSUME` (still swallows the
  interaction, no durability loss).
- `MushroomCow.mobInteract` (MushroomCow.java:125-132) + `.shear()` (179-190):
  `readyForShearing()` is `!isBaby()` (sheared mooshrooms don't exist as a concept —
  shearing *converts* the mob). Plays `MOOSHROOM_SHEAR`, `convertTo(EntityTypes.COW, ...)`,
  spawns an `EXPLOSION` particle, drops from `SHEAR_MOOSHROOM` loot table as individual
  `ItemEntity`s at eye height, `itemStack.hurtAndBreak(1, ...)`.
- `SnowGolem.java:139-160`: `readyForShearing()` = has a pumpkin head (checked via
  `!isBaby()`-equivalent state, effectively `hasPumpkin()`); `shear()` drops from
  `SHEAR_SNOW_GOLEM` at eye height and removes the pumpkin head (renders bare from then on).
- `Bogged.java:73-138`: `readyForShearing()` = `!isSheared()`; `shear()` calls
  `spawnShearedMushrooms` (drops from `BOGGED_SHEAR` loot table at bounding-box height),
  `setSheared(true)` (persisted via `"sheared"` NBT boolean and an entity-data byte).
- `PumpkinBlock.useItemOn` (PumpkinBlock.java:36-77): `itemStack.is(Items.SHEARS)` only;
  computes carve direction (block face clicked, or player's opposite facing if clicked on
  top/bottom), drops from `CARVE_PUMPKIN` block-interact loot table as a single
  `ItemEntity` kicked outward from the carve face, plays `PUMPKIN_CARVE`, replaces the
  block with `CARVED_PUMPKIN` facing that direction (flags `11`), `hurtAndBreak(1, ...)`,
  `GameEvent.SHEAR`, `player.awardStat(Stats.ITEM_USED.get(Items.SHEARS))`.
- Beehive/nest shearing (`BeehiveBlock`/`BeehiveBlockEntity`) is **out of scope here** —
  it is covered by the separate sculk-and-block-social design doc
  (`designs/wolf-cat-social-ai.md`'s sibling doc for hive/nest interactions); this doc
  only cross-references it, it does not redesign it.

### Current Rust state
No `pumpkin/src/item/items/shears.rs` exists at all, and `Item::SHEARS` is never
registered in `default_registry()` (`pumpkin/src/item/items/mod.rs`). So right now a
shears stack has zero item-side behavior: no efficient-block-breaking bonus beyond
whatever the generic `Tool` data component in `pumpkin-data` already encodes (need to
verify `Item::SHEARS`'s generated `Tool` component matches vanilla's `createToolProperties`
— likely yes, since Tool rules are data-driven per-item and shears' `Tool` component is
almost certainly already baked into `pumpkin-data/src/generated/item.rs`'s definition;
this doc's scope is the *item-code* gap, not re-deriving the data component), no
`useOn`/carve-pumpkin/grow-plant behavior, and no shear-an-entity behavior at all.

On the entity side, the Rust models are further along than the item side:
- `pumpkin/src/entity/passive/sheep.rs` already has `color_and_sheared: AtomicU8`,
  `is_sheared()`, `set_sheared()`, and NBT round-trip for `"Sheared"` — the state model
  exists, but nothing calls `set_sheared(true)` in response to a shears interaction; no
  wool-drop loot table hookup.
- `pumpkin/src/entity/mob/skeleton/bogged.rs` — not inspected in depth here, but by the
  same pattern this doc expects a `sheared` flag placeholder at most, no shear() method.
- `pumpkin/src/entity/passive/mooshroom.rs` — **no variant/mushroom-type field at all**
  yet (`MooshroomEntity` is just a bare `MobEntity` wrapper with 3 goals). Shearing a
  mooshroom (convert to `Cow` + drop mushrooms) needs the conversion machinery (probably
  a `convert_to`-style helper elsewhere in Pumpkin for zombie villager cures /
  zombie-to-drowned, worth checking for reuse) *and* a variant field before it can even
  be represented, which is a larger prerequisite than the other three shearable mobs.
- `pumpkin/src/entity/passive/snow_golem.rs` — not inspected; pumpkin-head removal on
  shear needs a "has pumpkin head" flag if not already present.
- `pumpkin/src/entity/mod.rs`'s `EntityBase::interact()` (line ~327) is the right hook:
  `fn interact<'a>(&'a self, _player: &'a Arc<Player>, _item_stack: &'a mut ItemStack) ->
  EntityBaseFuture<'a, bool>` already exists and defaults to `false`/no-op. Each
  shearable mob's `interact` override is where `itemStack.is(Items.SHEARS)` +
  `readyForShearing()` dispatch belongs, matching vanilla's per-mob `mobInteract`.
- `pumpkin/src/block/blocks/plant/{twisting_vines,weeping_vines,cave_vines}.rs` and
  `pumpkin/src/block/blocks/plant/kelp.rs` exist; `ShearsItem::use_on_block` needs to
  detect "is this a GrowingPlantHeadBlock not at max age" — check each of these blocks'
  age/height property name and add a small helper (`fn max_age_state(...)`) analogous to
  vanilla's `GrowingPlantHeadBlock.getMaxAgeState`.
- `pumpkin/src/block/blocks/tripwire.rs`'s `broken()` hook (around line ~103) already
  has a **direct, non-shared** shears check:
  `main_hand_item_stack.lock().await.get_item().eq(&Item::SHEARS)`. There is no
  `ItemStack::is_shears()` helper today (unlike `is_axe`/`is_hoe`/`is_sword`/etc. in
  `pumpkin-data/src/item_stack/categories.rs`, which are all tag-based since those tool
  categories have multiple materials). Shears has exactly one item id, so a tag isn't
  strictly required, but for consistency with the existing `categories.rs` convention
  and to de-duplicate the two shears-identity checks that would otherwise exist
  (tripwire.rs's inline one and ShearsItem's own registration predicate), add:
  ```rust
  #[inline]
  #[must_use]
  pub fn is_shears(&self) -> bool {
      self.item == &crate::item::Item::SHEARS // or `self.item.id == Item::SHEARS.id`
  }
  ```
  to `categories.rs`, and change tripwire.rs's `broken()` to call
  `main_hand_item_stack.lock().await.is_shears()` instead of the direct `Item::SHEARS`
  comparison. This is a one-line simplification riding along with the ShearsItem work,
  not a separate project.

### Target design
New file `pumpkin/src/item/items/shears.rs`, following the `axe.rs`/`hoe.rs` convention
(a unit struct implementing `ItemMetadata` + `ItemBehaviour`, registered by exact id
since there's only one shears item — `fn ids() -> Box<[u16]> { Box::new([Item::SHEARS.id]) }`,
mirroring `crossbow.rs`'s single-id pattern rather than axe/hoe's tag-based pattern):

- `ItemBehaviour::use_on_block`: replicate vanilla's `useOn` — if `block` is one of the
  twisting/weeping/cave-vines/kelp-head blocks (check each block's own "am I at max
  age/height" predicate) and not already maxed, play `Sound::GrowingPlantCrop` (verify
  exact Rust sound enum name), set the block to its max-age state via
  `world.set_block_state(..., BlockFlags::NOTIFY_ALL)`, damage the shears by 1 via
  `player.damage_held_item(1)` (mirrors axe.rs's pattern), fire whatever
  `CriteriaTriggers.ITEM_USED_ON_BLOCK` maps to in Pumpkin's advancement trigger system
  (`pumpkin/src/entity/player/advancement/trigger.rs`) if such a generic "used item on
  block" trigger already exists — else note as a follow-up, not a blocker.
  - Then handle `PumpkinBlock`/`CarvedPumpkinBlock` carving: if `block == &Block::PUMPKIN`,
    compute the carve direction (`hitResult` face, or player's opposite horizontal facing
    if the clicked face is up/down — check what cursor/face info `use_on_block`'s
    `_face: BlockDirection` parameter already gives you, it should be exactly
    `hitResult.getDirection()`), drop a pumpkin-seeds item entity kicked outward along
    that direction with the same velocity jitter formula as vanilla (lines 58-64), replace
    the block with `Block::CARVED_PUMPKIN` at that facing, play the carve sound, damage
    the shears by 1, and award the `ItemUsed` statistic for `Item::SHEARS` (check
    `pumpkin/src/entity/player.rs`'s stat-increment helper, used elsewhere for
    `Stats.ITEM_USED`-equivalent bookkeeping).
  - Cobweb/leaves/vine "efficient tool" bonus mining speed is data-driven via the `Tool`
    component already baked into `Item::SHEARS`'s generated data (verify by grepping
    `pumpkin-data/src/generated/item.rs` for `SHEARS` and its `Tool` fields) — if present,
    no Rust code is needed for that part; only `mineBlock`'s durability-loss-on-break
    needs `ItemBehaviour` support. Pumpkin's mining-speed/durability-on-break dispatch for
    generic tools should already exist (used by axe/pickaxe/shovel); reuse whatever hook
    those use rather than reinventing block-break durability loss here. If no such hook
    exists yet for non-axe/hoe tools, that's a larger prerequisite worth flagging back to
    whoever picks this up rather than solving inside this design.
- `ItemBehaviour::use_on_entity`: dispatch to `entity.interact(player, item_stack)`ish —
  actually per the `EntityBase::interact` signature already in `entity/mod.rs`, the more
  natural placement per Pumpkin's existing architecture is for `ShearsItem::use_on_entity`
  to just forward into the entity via whatever generic interact-dispatch code already
  calls `EntityBase::interact()` for other interactions (check `player.rs`'s handling of
  right-click-on-entity packets) — then each of `sheep.rs`, `mushroom.rs` /
  `mooshroom.rs`, `snow_golem.rs`, `bogged.rs` overrides `interact()` to check
  `item_stack.is_shears()` (the new helper above) + its own `ready_for_shearing()`
  equivalent, and implements the four per-mob `shear()` bodies described above. This is
  4 separate, mostly-independent sub-tasks; Mooshroom's is the largest since it also
  needs a variant field and mob-conversion plumbing that doesn't exist yet.

### Files to create/modify
- Create `pumpkin/src/item/items/shears.rs`.
- Modify `pumpkin/src/item/items/mod.rs`: add `pub mod shears;`, `use shears::ShearsItem;`,
  `manager.register(ShearsItem);`.
- Modify `pumpkin-data/src/item_stack/categories.rs`: add `is_shears()`.
- Modify `pumpkin/src/block/blocks/tripwire.rs`: use `.is_shears()` instead of the inline
  `Item::SHEARS` comparison.
- Modify `pumpkin/src/entity/passive/sheep.rs`: `interact()` override + wool-drop loot.
- Modify `pumpkin/src/entity/mob/skeleton/bogged.rs`: `interact()` override + mushroom drop.
- Modify `pumpkin/src/entity/passive/snow_golem.rs`: `interact()` override + pumpkin
  removal (needs a "has pumpkin" flag if none exists — verify).
- Modify `pumpkin/src/entity/passive/mooshroom.rs`: needs variant field + conversion +
  `interact()` override — largest sub-item, budget separately.
- Modify whichever of `pumpkin/src/block/blocks/plant/{kelp,twisting_vines,
  weeping_vines,cave_vines}.rs` exposes an age/max-age helper (add one if missing).
- Modify `pumpkin/src/block/blocks/pumpkin.rs` (or wherever `Block::PUMPKIN`'s behaviour
  lives — verify exact filename) if pumpkin-carving needs a block-side hook rather than
  living entirely in the item's `use_on_block`.

### Needs wiki/vanilla verification
- Exact Rust `Sound` enum variants for `GROWING_PLANT_CROP`, `SHEEP_SHEAR`,
  `MOOSHROOM_SHEAR`, `PUMPKIN_CARVE` — check `pumpkin-data/src/generated/sound.rs`, not
  yet confirmed present in this pass.
- Whether Pumpkin already has a generic "mob conversion" helper (zombie→drowned,
  zombie villager cure, etc.) that Mooshroom→Cow shearing could reuse — worth a targeted
  search before writing Mooshroom's shear() from scratch.
- Whether `Item::SHEARS`'s generated `Tool` data component already matches vanilla's
  `createToolProperties()` rule set (extreme/major/minor speed tags + cobweb rule) —
  this doc assumes yes based on the census's framing ("efficient tool" determination
  already exists elsewhere) but was not independently re-verified byte-for-byte here.

---

## 2. CompassItem (lodestone lock)

### Vanilla source
`net/minecraft/world/item/CompassItem.java` (75 lines):
- `isFoil` (28-31): `true` if `DataComponents.LODESTONE_TRACKER` is present (glint
  override), else falls through to default `isFoil` (enchantment-based).
- `inventoryTick` (33-42): every server tick this stack is anywhere in an entity's
  inventory, if it has a `LodestoneTracker`, call `tracker.tick(level)` and only
  `itemStack.set(...)` if the returned tracker differs (avoids needless component
  churn/hashing). `LodestoneTracker.tick` (not read here, but by the type name)
  presumably re-validates the target chunk is still loaded / the lodestone block still
  exists, invalidating the tracked position if not (`tracked: true` → `false`, or
  clearing target) — **this doc does not re-derive that method's body**; note as needing
  its own read pass when implementing.
- `useOn` (44-69): only fires if the clicked block `is(Blocks.LODESTONE)`. Plays
  `LODESTONE_COMPASS_LOCK` at volume/pitch 1.0 (played to *all* players, `null` source).
  `replaceExistingStack` = true iff the player doesn't have infinite materials AND the
  stack count is exactly 1 (i.e., a single non-creative compass just becomes a lodestone
  compass in place). Otherwise (count > 1, or creative infinite materials): `transmuteCopy`
  a fresh single compass, `consume(1, player)` from the original stack (decrements it by
  one, respecting creative), sets the `LodestoneTracker` component on the *new* single
  stack, then tries `player.getInventory().add(...)` and drops it on the ground if the
  inventory is full. Target is
  `new LodestoneTracker(Optional.of(GlobalPos.of(level.dimension(), blockPos)), true)`
  — `tracked = true`.
- `getName` (71-74): returns the static `"item.minecraft.lodestone_compass"` translatable
  component whenever `LODESTONE_TRACKER` is present, otherwise default name.

### Current Rust state
No `pumpkin/src/item/items/compass.rs` file exists, and no registration for
`Item::COMPASS` in `default_registry()`. However, the underlying data component is
already modeled: `pumpkin-data/src/data_component_impl/utility.rs` has
`LodestoneTarget { dimension: String, x: i32, y: i32, z: i32 }` and
`LodestoneTrackerImpl { target: Option<LodestoneTarget>, tracked: bool }`, with NBT
read (`read_data`, parses `"tracked"` bool default `true` and `"target"` compound with
`"dimension"` string + `"pos"` int array) and `write_data` already implemented, plus
`default_impl!(LodestoneTracker)` for standard hashing/component-id plumbing. So the
component storage/serialization side is done; only the *item behavior* that creates,
locks, and ticks it is missing.

Per the task framing, `EntityEquipment.tick`'s absence (a per-tick equipped-item hook,
separate from the on-use-tick gap discussed above — this one is about ticking items an
entity is merely *wearing/holding*, not actively using) is a genuine prerequisite for
`inventoryTick`-style behavior while a compass sits in a hotbar slot or armor slot. This
doc treats compass **use** (the `useOn` lock action) as independently designable and
shippable without that prerequisite; the tick-based invalidation
(`inventoryTick`/`LodestoneTracker.tick`) should be filed as depending on that hook and
implemented alongside/after it, not blocked on it for the lock feature itself.

### Target design
New file `pumpkin/src/item/items/compass.rs`:
- `ItemMetadata::ids() -> Box<[u16]>` — likely just `Item::COMPASS.id` and
  `Item::RECOVERY_COMPASS.id` if the latter shares `CompassItem` behavior in vanilla
  (check `Items.java`/registration — recovery compass typically does *not* support
  lodestone-locking since it always points to death location instead; verify before
  including it).
- `ItemBehaviour::use_on_block`: check `block == &Block::LODESTONE`; if not, no-op
  (`PASS`, matching vanilla's fallthrough). If yes: play the lock sound broadcast to all
  nearby players (Pumpkin's `world.play_sound` variants already used elsewhere, e.g.
  `Sound::ItemAxeStrip` usage in axe.rs, take a `SoundCategory` and position — use
  `Players` category per vanilla `SoundSource.PLAYERS`); compute
  `replace_existing_stack = !player.has_infinite_materials() && item.item_count == 1`;
  if true, set the `LodestoneTrackerImpl` data component directly on the held stack; else
  clone a fresh single-count compass stack, decrement the original by 1 (respecting
  creative), set the component on the new stack, and try to insert it into the player's
  inventory, falling back to `player.drop_item(...)` if full (mirrors bundle.rs's
  existing `player.drop_item` usage pattern for the "no room" case).
- `isFoil` isn't in the current `ItemBehaviour` trait at all (no override point exists
  for "does this specific stack glow with enchant glint independent of actual
  enchantments"). Search shows Pumpkin's enchant-glint rendering is likely computed
  client-side from the `Enchantments`/`StoredEnchantments` components only today; adding
  a per-item `isFoil` override hook to `ItemBehaviour` (defaulting to whatever the
  existing enchantment-based check does) is a second small trait addition needed here,
  parallel in spirit to the `on_use_tick` gap above but much smaller in scope (one bool
  method, no tick loop wiring).
- `getName` override: same situation — no `ItemBehaviour::get_name`/display-name-override
  hook was found anywhere in the codebase during this pass (see ShieldItem section below,
  which needs the identical hook). This is a **second shared prerequisite**: add
  ```rust
  fn get_custom_name(&self, _stack: &ItemStack) -> Option<TextComponent> { None }
  ```
  (or similar) to `ItemBehaviour`, call it from wherever item display names are currently
  resolved for tooltips/chat/death messages (this lookup site was not located in this
  pass — needs a follow-up grep for wherever `item.registry_key`-derived translation
  keys are turned into client-visible names, e.g. tooltip packet construction) before
  either Compass's lodestone-compass name or Shield's dye-colored name can be wired up.

### Files to create/modify
- Create `pumpkin/src/item/items/compass.rs`.
- Modify `pumpkin/src/item/items/mod.rs`: register it.
- Modify `pumpkin/src/item/mod.rs` (`ItemBehaviour` trait): add `is_foil`-override hook
  and a display-name-override hook (shared with ShieldItem, section 6).
- Wherever the display-name resolution site turns out to live (follow-up grep needed).
- Depends on (but does not require finishing first) the `EntityEquipment.tick` /
  per-tick-equipped-item hook for the `inventoryTick` invalidation half of this feature.

### Needs wiki/vanilla verification
- Whether `RECOVERY_COMPASS` shares `CompassItem`'s class (and thus this `useOn`
  behavior) or has its own item class in vanilla — not checked in this pass, only
  `CompassItem.java` was read.
- `LodestoneTracker.tick(ServerLevel)`'s exact body (not read in this pass) — needed
  before implementing the tick-invalidation half.

---

## 3. SpyglassItem

### Vanilla source
`net/minecraft/world/item/SpyglassItem.java` (52 lines) — this is genuinely small:
- `getUseDuration` → constant `1200` ticks (60 seconds; `USE_DURATION` static).
- `getUseAnimation` → `ItemUseAnimation.SPYGLASS` (a client-rendering hint; the zoom
  effect itself is 100% client-side FOV/rendering, out of server scope).
- `use` (29-34): plays `SPYGLASS_USE` (volume/pitch 1.0), `player.awardStat(Stats.
  ITEM_USED.get(this))`, then `ItemUtils.startUsingInstantly(level, player, hand)` — the
  "instantly" variant sets the item as in-use without the usual wind-up, i.e. the zoom
  starts immediately on right-click-down rather than after some delay.
- `finishUsingItem` (36-40): only called if the full 1200-tick duration elapses without
  the player releasing early — just calls `stopUsing` (plays the stop sound) and returns
  the stack unchanged (no consumption, no durability, no NBT change).
- `releaseUsing` (42-46): called when the player releases early (right-click let go
  before 1200 ticks); also calls `stopUsing`, returns `true` (successfully handled).
- `stopUsing` (48-50): plays `SPYGLASS_STOP_USING` (volume/pitch 1.0) *on the entity*
  (`entity.playSound`, not `level.playSound` — meaning only that entity hears it locally,
  unlike the broadcast-style sounds elsewhere in this doc).

There is no `onUseTick` override — the client handles the actual zoom rendering every
frame independently; the server-side contract is just "am I in the SPYGLASS-use-animation
state for up to 1200 ticks" plus start/stop sound + stat bookkeeping.

### Current Rust state
No `pumpkin/src/item/items/spyglass.rs` exists; `Item::SPYGLASS` is unregistered.
Because `getUseAnimation` doesn't correspond to any hook in the current
`ItemBehaviour` trait either (see the `on_use_tick`/foil/name gaps above — this is a
fourth spot where the trait is missing a vanilla `Item#` override point), and because the
use-duration mechanism (`item_use_time` + `set_active_hand`/`clear_active_hand` in
`living.rs`) already exists and is exercised by `crossbow.rs`'s `normal_use` (which calls
`player.living_entity.set_active_hand(Hand::Right, stack, 72000)`), Spyglass is actually
one of the *simplest* of the seven to implement once the general use-animation gap is
acknowledged, because it needs no new per-tick hook — only start (`normal_use`) and
release/finish (`on_stopped_using`, which the trait already has) logic.

### Target design
New file `pumpkin/src/item/items/spyglass.rs`:
- `ItemMetadata::ids()` → `Box::new([Item::SPYGLASS.id])`.
- `ItemBehaviour::normal_use`: play `Sound::ItemSpyglassUse` at the player's position
  (entity-local sound — check whether Pumpkin's `player.playSound`-style helper exists
  separately from `world.play_sound`/`play_sound_fine`, since vanilla's `use()` calls
  `player.playSound` not `level.playSound`; crossbow.rs and others in this codebase
  mostly use `world.play_sound_fine`/`world.play_sound`, which broadcast — verify there's
  a "local only" variant, or confirm broadcasting is an acceptable approximation given
  volume 1.0 sounds are audible to everyone nearby anyway in practice), increment the
  `ItemUsed` statistic for `Item::SPYGLASS` (same helper Compass/Shears would use),
  then call `player.living_entity.set_active_hand(Hand::Right-or-whichever-hand-was-used,
  stack, 1200)` — matching the crossbow pattern but with the 1200-tick duration and no
  "instantly" wind-up distinction needed server-side (the "instantly" part of
  `ItemUtils.startUsingInstantly` is specifically about skipping a client-side windup
  animation delay before the use-duration countdown begins, which the server doesn't
  otherwise model as a separate concept in Pumpkin's `item_use_time` scheme — confirm
  this by checking whether any other Pumpkin item distinguishes "instant start" from
  normal start; if not, plain `set_active_hand` is the right target).
- `ItemBehaviour::on_stopped_using`: play `Sound::ItemSpyglassStopUsing`, no stack
  mutation (no durability, no consumption) — this covers both vanilla's
  `finishUsingItem` (full duration elapsed) and `releaseUsing` (early release) paths,
  since both just call `stopUsing()`; Pumpkin's `on_stopped_using` hook already fires for
  both cases uniformly per the crossbow.rs precedent, so a single implementation
  suffices — verify by checking exactly when `on_stopped_using` fires relative to
  `item_use_time` reaching 0 vs. the player manually releasing right-click, to be sure
  both vanilla code paths map onto this one Pumpkin hook.
- `get_use_duration` → `1200`.
- No `use_on_block`/`use_on_entity`/`mine_block` overrides needed.

### Files to create/modify
- Create `pumpkin/src/item/items/spyglass.rs`.
- Modify `pumpkin/src/item/items/mod.rs`: register it.
- No trait changes strictly required (assuming the ItemUsed-stat helper and a
  per-player-only sound helper already exist somewhere reusable) — this is the item in
  this batch with the smallest new-trait footprint.

### Needs wiki/vanilla verification
- Confirm `Sound::ItemSpyglassUse` / `Sound::ItemSpyglassStopUsing` exist in
  `pumpkin-data/src/generated/sound.rs` under those or similar names.
- Confirm whether Pumpkin has (or needs) a "play sound to this player only" primitive
  distinct from the broadcast-to-nearby-players helpers used elsewhere, to match
  vanilla's `entity.playSound`/`player.playSound` call sites exactly.

---

## 4. BrushItem + BrushableBlockEntity::brush()

### Vanilla source
Item side, `net/minecraft/world/item/BrushItem.java` (143 lines):
- Constants: `ANIMATION_DURATION = 10` (ticks per brush stroke/particle burst),
  `USE_DURATION = 200` (total ticks the brushing animation can run before auto-stopping).
- `useOn` (37-45): if a raycast (`calculateHitResult`, using
  `ProjectileUtil.getHitResultOnViewVector` against `player.blockInteractionRange()`)
  hits a block, `player.startUsingItem(hand)`. Always returns `CONSUME` regardless (so
  the client always sees *some* interaction feedback even on a miss, but only starts
  the use-animation on an actual block hit).
- `getUseAnimation` → `ItemUseAnimation.BRUSH`; `getUseDuration` → `200`.
- `onUseTick` (57-96) — the heart of the mechanic, called every tick while brushing:
  - Re-raycasts every tick (`calculateHitResult`); if the result stops being a block hit,
    `livingEntity.releaseUsingItem()` (auto-cancel — e.g. player looked away).
  - `timeElapsed = getUseDuration - ticksRemaining + 1`; `isLastTickBeforeBackswing =
    timeElapsed % 10 == 5` — i.e., **exactly once every 10 ticks**, at the tick 5 ticks
    into each 10-tick cycle, the actual brush effect fires (this is the "swing" landing
    partway through each repeat of the brushing animation, not every tick).
  - On that tick: picks the brushing arm (main arm, or its opposite if off-hand was
    used), spawns dust particles if
    `state.shouldSpawnTerrainParticles() && renderShape != INVISIBLE`
    (`spawnDustParticles`, 102-124: 7-11 particles per burst, direction-dependent offset
    formula via the `DustParticlesDelta` record keyed off hit face — DOWN/UP derive from
    the view vector's horizontal components, the four horizontal faces use fixed
    `(±1.0, 0, ∓0.1)`-style deltas), plays a brush sound (block-specific
    `BrushableBlock.getBrushSound()` if the target is a `BrushableBlock`, else generic
    `BRUSH_GENERIC`), and — only if the level is a `ServerLevel` and the target block
    entity is a `BrushableBlockEntity` — calls `brushableBlockEntity.brush(gameTime,
    serverLevel, player, hitDirection, itemStack)`, and if that returned `true`
    (brushing *just completed*, i.e. this stroke was the 10th), damages the brush by 1
    durability (`itemStack.hurtAndBreak(1, player, equippedHand)`), picking OFFHAND vs
    MAINHAND by identity comparison against the player's off-hand stack.
  - If the raycast never hit a block at all this call, also `releaseUsingItem()`.

Block-entity side, `net/minecraft/world/level/block/entity/BrushableBlockEntity.java`
(247 lines):
- Fields: `brushCount: int`, `brushCountResetsAtTick: long`, `coolDownEndsAtTick: long`,
  `item: ItemStack` (the currently-unpacked-but-not-yet-dropped loot item),
  `hitDirection: Direction?` (locked to the *first* brush direction, used later to
  choose which face the final item pops out of), `lootTable: ResourceKey<LootTable>?`,
  `lootTableSeed: long`. Constants: `BRUSH_COOLDOWN_TICKS = 10`, `BRUSH_RESET_TICKS = 40`,
  `REQUIRED_BRUSHES_TO_BREAK = 10`.
- `brush(gameTime, level, user, direction, brush)` (59-86): locks `hitDirection` on
  first call; always bumps `brushCountResetsAtTick = gameTime + 40`; **if still on
  cooldown** (`gameTime < coolDownEndsAtTick`) returns `false` immediately (no progress,
  no sound-suppression logic needed here since the item side already gated the call to
  once per 10-tick cycle, so this cooldown check is mostly a defense-in-depth/anti-spam
  guard against multiple brush sources). Otherwise sets a new 10-tick cooldown,
  `unpackLootTable(...)` (lazily rolls the loot table into `this.item` the *first* time
  brushing starts, so the block always knows exactly what it's about to yield),
  increments `brushCount`; if `brushCount >= 10`: `brushingCompleted(...)`, return `true`.
  Else: schedules a block tick 2 ticks out (drives `checkReset`), and if the *completion
  state* (a 0-3 tier derived from `brushCount`, see below) changed, updates the
  `DUSTED` block-state property (this is what drives the client-visible progressive
  dust/crack overlay) and returns `false`.
- `unpackLootTable` (88-114): rolls the loot table exactly once (guarded by
  `lootTable != null`, then nulls it out so it never re-rolls), using
  `LootContextParamSets.ARCHAEOLOGY` params (origin = block center, luck = user's luck,
  `THIS_ENTITY` = user, `TOOL` = the brush stack), expects **at most 1** item back
  (warns and takes the first if the loot table misbehaves and returns more).
- `brushingCompleted` (116-128): drops the unpacked item (`dropContent`), triggers level
  event `3008` (block-break-ish particle burst, keyed to the *old* block state id) at
  the block, and replaces the block with `BrushableBlock.getTurnsInto()` (e.g. suspicious
  sand → sand, suspicious gravel → gravel).
- `dropContent` (130-146): re-unpacks defensively (idempotent no-op if already unpacked),
  then if `item` isn't empty: picks a `dropPos` one block along `hitDirection` (defaulting
  `UP` if somehow null), computes a centered-but-jittered spawn position, splits off
  `random.nextInt(21) + 10` (10-30) of the stack (note: this is a *quantity* jitter on
  top of whatever count the loot table rolled — only up to 10-30 units of the rolled
  stack get dropped per call, though in practice a loot table roll is far smaller so
  this effectively just says "drop the whole thing", the split-cap rarely binds), zero
  velocity, spawns as an `ItemEntity`, clears `this.item`.
- `checkReset(level)` (148-168): called from a scheduled block tick (see `brush`'s
  `scheduleTick(pos, block, 2)` — a 2-tick delay, this is the decay/backslide
  mechanism). If `brushCount != 0` and `gameTime >= brushCountResetsAtTick` (i.e. no
  brush stroke landed within the last `BRUSH_RESET_TICKS = 40` ticks): decays
  `brushCount` by 2 (floor 0), updates `DUSTED` if the completion tier changed, and
  reschedules its own decay 4 ticks later. If `brushCount` has decayed all the way to 0:
  clears `hitDirection`/`brushCountResetsAtTick`/`coolDownEndsAtTick` entirely (full
  reset, ready for a fresh unrelated brush attempt); otherwise (still >0 either from
  ongoing brushing or partial decay) reschedules another 2-tick check.
- `getCompletionState()` (230-238): `brushCount == 0 → 0`; `< 3 → 1`; `< 6 → 2`; else
  `→ 3` — this 0-3 tier is what's written to the `DUSTED` block-state property.
- NBT: `"LootTable"`/`"LootTableSeed"` (only present pre-unpack), `"hit_direction"`
  (legacy direction id codec), `"item"` (only present post-unpack, pre-drop — i.e. these
  two are mutually exclusive on disk, matching `tryLoadLootTable`/`trySaveLootTable`'s
  either/or logic at load/save time).

### Current Rust state
`pumpkin/src/item/items/mod.rs` has a literal `// TODO: Register BrushItem with
per-stroke durability cost.` comment — the item is entirely unimplemented, no file
exists. The block-entity side is further along but still incomplete:
`pumpkin/src/block/entities/brushable_block.rs` (86 lines) has
`BrushableBlockBlockEntity { position, item: Mutex<Option<ItemStack>>, hits: Mutex<i32>,
direction: Mutex<u8> }` with NBT round-trip for `item`/`hits`/`direction` (note: Rust's
field is named `hits`, mapping to vanilla's `brushCount`; `direction` is stored as a raw
`u8` rather than an `Option<Direction>`, so "no direction locked yet" isn't currently
representable — needs to change to `Option<Direction>` or an equivalent sentinel to
match vanilla's `null`-until-first-brush semantics) — but there is **no `brush()`
method, no `checkReset`, no cooldown/reset-tick fields at all**
(`brushCountResetsAtTick`/`coolDownEndsAtTick` don't exist), no loot-table
unpack/lazy-roll state (`lootTable`/`lootTableSeed`/the unpack-once guard), and no
`DUSTED` block-state-property write-back. This needs substantially more new code than
the "just add a method" framing might suggest — it's closer to a from-scratch port of
the whole vanilla class's state machine on top of the existing struct's skeleton.

### Target design
This is the largest of the seven items; item side and block-entity side must land
together since neither is independently useful.

**Item side** — new `pumpkin/src/item/items/brush.rs`:
- `ItemMetadata::ids()` → `Box::new([Item::BRUSH.id])`.
- `ItemBehaviour::use_on_block`: raycast via whatever Pumpkin equivalent of
  `ProjectileUtil.getHitResultOnViewVector`/`player.blockInteractionRange()` exists
  (check `get_start_and_end_pos`, already on the trait, or a dedicated raycast helper
  used by other block-interaction items); if it lands on a block,
  `player.living_entity.set_active_hand(hand, stack, 200)` (mirrors crossbow's/would-be
  spyglass's pattern). Return value doesn't need to encode `CONSUME` specially if
  Pumpkin's dispatch layer already treats "handled" uniformly — verify against how
  `use_on_block`'s return type/side-effects are consumed by callers.
- `ItemBehaviour::on_use_tick` (**depends on the shared prerequisite at the top of this
  doc**): re-raycast every tick; if no longer hitting a block, cancel the use (whatever
  Pumpkin's equivalent of `releaseUsingItem()` is — likely `clear_active_hand()`).
  Compute `time_elapsed = 200 - ticks_remaining + 1`; only act when
  `time_elapsed % 10 == 5`. On that tick: resolve the brushing arm, spawn dust particles
  (port `spawnDustParticles`'s per-face delta table verbatim — it's a small, exact,
  already-fully-specified formula, low risk), play the brush sound (block-specific via a
  new `BrushableBlock`-equivalent trait/method returning its sound, or generic
  `Sound::BrushGeneric`), and if the block entity at that position is a
  `BrushableBlockBlockEntity`, call its new `brush(...)` method (see below); if that
  returns `true`, damage the brush by 1 (`item.damage_item(1)`/`hurtAndBreak`-equivalent,
  picking OFFHAND vs MAINHAND by stack identity against the player's off-hand, same
  pattern as vanilla).
- `get_use_duration` → `200`.

**Block-entity side** — extend `pumpkin/src/block/entities/brushable_block.rs`:
- Change `direction: Mutex<u8>` to `Mutex<Option<BlockDirection>>` (or keep a raw byte
  but add an explicit "unset" sentinel/bool) so "not yet brushed from any direction" is
  representable, matching vanilla's `hitDirection == null` checks.
- Add fields: `brush_count_resets_at_tick: Mutex<i64>`, `cooldown_ends_at_tick:
  Mutex<i64>`, `loot_table: Mutex<Option<ResourceKey-equivalent>>`,
  `loot_table_seed: Mutex<i64>`. Rename/repurpose `hits` to line up with `brush_count`
  semantics (or keep the field name for NBT-compat and just document the mapping).
- Add `pub async fn brush(&self, game_time: i64, world: &World, user: &Player,
  direction: BlockDirection, brush_stack: &ItemStack) -> bool` implementing the full
  vanilla `brush()` body: lock hit_direction on first call, bump the reset deadline,
  cooldown short-circuit, lazy loot unpack, increment count, completion check (10 →
  `brushing_completed`), else schedule a 2-tick follow-up and update `DUSTED` if the
  completion tier changed.
- Add `unpack_loot_table`, `brushing_completed`, `drop_content`, `check_reset`,
  `get_completion_state` as private/pub(crate) helpers, porting the exact vanilla logic
  (tick constants: 10-tick cooldown, 40-tick reset window, 10 brushes to break, 4-tick
  decay re-check interval, decay-by-2-per-check).
- `check_reset` needs to be invoked from wherever Pumpkin's scheduled-block-tick
  machinery calls into block entities — check how other scheduled-tick block entities
  (e.g. furnace, hopper) wire this up in Pumpkin's tick dispatch, and register
  `BrushableBlock`'s scheduled tick handler to call it (vanilla drives this via
  `Block#tick`/`BlockEntityTicker`, not the item; Pumpkin's equivalent scheduled-tick
  registration point needs identifying — likely near
  `pumpkin/src/block/blocks/brushable_block.rs`'s *block* (not block-entity) definition,
  which wasn't located/read in this pass and should be checked for existence before
  starting implementation).
- Loot-table integration: `LootContextParamSets.ARCHAEOLOGY` and the `data/minecraft/
  loot_table/brush/*` (suspicious sand/gravel loot tables) already exist as data;
  Pumpkin's loot-table evaluation API (used elsewhere for block drops) should be
  reusable here with an archaeology-specific param set — verify Pumpkin's loot context
  builder supports arbitrary/custom param sets or if archaeology needs its own small
  addition.

### Files to create/modify
- Create `pumpkin/src/item/items/brush.rs`.
- Modify `pumpkin/src/item/items/mod.rs`: remove the TODO comment, register `BrushItem`.
- Modify `pumpkin/src/block/entities/brushable_block.rs`: substantially extend as above.
- Modify `pumpkin/src/item/mod.rs`: add the shared `on_use_tick` hook (see top of doc).
- Modify `pumpkin/src/entity/living.rs`: wire `on_use_tick` into the per-tick use loop.
- Locate and modify the block (not block-entity) definition file for suspicious
  sand/gravel (likely `pumpkin/src/block/blocks/*.rs`, not found in this pass) to wire
  in `get_brush_sound()` and the scheduled-tick → `check_reset` call, and to register
  `DUSTED` state-property writes.

### Needs wiki/vanilla verification
- Exact loot tables at `data/minecraft/loot_table/brush/*` were not opened in this pass
  — confirm which specific loot table each suspicious block variant uses.
- Confirm Pumpkin's scheduled-block-tick API shape before wiring `check_reset`/the
  10-brush-count block-tick reschedule into it.
- Confirm `BrushableBlock.getTurnsInto()`'s mapping (suspicious sand → sand, suspicious
  gravel → gravel) against `BrushableBlock.java` (not read in this pass, only the block
  *entity* was read) before hardcoding the post-completion block swap.

---

## 5. BundleItem stacked-click overrides

### Vanilla source
`net/minecraft/world/item/BundleItem.java` (281 lines), the two override points:
- `overrideStackedOnOther(self, slot, clickAction, player)` (58-94): fires when the
  player is holding the bundle *on the cursor* and clicks a slot. `self` = the bundle
  stack (cursor), `slot.getItem()` = `other`.
  - `clickAction == PRIMARY (left-click) && !other.isEmpty()`: try `tryTransfer(slot,
    player)` (attempt to insert the slot's stack into the bundle); if it moved > 0 items,
    play `BUNDLE_INSERT`, else `BUNDLE_INSERT_FAIL`; writes back `contents` regardless;
    broadcasts container-menu slot changes; returns `true` (handled).
  - `clickAction == SECONDARY (right-click) && other.isEmpty()`: `removeOne()` from the
    bundle, `slot.safeInsert(itemStack)` into the (empty) slot; if any remainder didn't
    fit, `tryInsert` it back into the bundle (defensive, shouldn't normally happen against
    an empty slot); if it fully placed, play `BUNDLE_REMOVE_ONE`; writes back, broadcasts,
    returns `true`.
  - Anything else (e.g. left-click on an empty slot, right-click on a non-empty slot):
    returns `false` (not handled — falls through to normal slot-click behavior).
- `overrideOtherStackedOnMe(self, other, slot, clickAction, player, carriedItem)`
  (96-137): fires when the bundle is *in the slot* and the cursor clicks it. `self` =
  slot's bundle, `other` = cursor stack, `carriedItem` = a `SlotAccess` for what ends up
  back on the cursor.
  - `PRIMARY && other.isEmpty()`: `toggleSelectedItem(self, -1)` (clears the
    GUI-highlighted "selected" item index — a pure rendering-state toggle, not an
    inventory mutation) and returns `false` (**not handled** — despite doing something,
    vanilla still lets normal pickup logic proceed afterward for this specific case).
  - `PRIMARY && !other.isEmpty()`: if `slot.allowModification(player)` and
    `tryInsert(other) > 0`, play `BUNDLE_INSERT`, else `BUNDLE_INSERT_FAIL`; writes back;
    broadcasts; returns `true`.
  - `SECONDARY && other.isEmpty()`: if `slot.allowModification(player)`, `removeOne()`
    and if non-null, play `BUNDLE_REMOVE_ONE` and put it on `carriedItem` (the cursor);
    writes back; broadcasts; returns `true`.
  - Anything else: `toggleSelectedItem(self, -1)`, returns `false`.

So the vanilla left/right split is: **left-click transfers a full stack in/out; right-
click transfers exactly one item in/out** — this is the opposite of a naive "right-click
= single item" intuition being universal, since here right-click *is* the single-item
path but left-click is the bulk path (consistent framing, just worth stating precisely).

Also relevant: `onDestroyed` (248-255) — if a bundle item entity is destroyed (e.g. burns
in lava/fire), its contents spill out via `ItemUtils.onContainerDestroyed` rather than
vanishing silently with the bundle.

### Current Rust state
Two separate, disconnected pieces of bundle logic exist today:
1. `pumpkin/src/item/items/bundle.rs`'s `normal_use` — handles right-click *without* an
   open inventory screen (hotbar-select right-click in the world): pops one item via
   `BundleContentsImpl::try_extract()` and drops it. This corresponds to vanilla's
   `BundleItem.use()`/`dropContent()` path (200-tick hold-to-repeat-drop via
   `onUseTick`), not to the stacked-click overrides at all — it's a different vanilla
   entry point (`use()`, not `overrideStackedOnOther`/`overrideOtherStackedOnMe`).
2. `pumpkin-inventory/src/screen_handler.rs`, inside the generic `Pickup`/`QuickMove`
   slot-click handler (around lines 1060-1114), **already has inline,
   hardcoded-to-`BundleContentsImpl` logic** for right-click (`MouseClick::Right`) only:
   - cursor non-empty + slot has a bundle → `bundle.try_insert(&mut cursor_stack)`.
   - cursor has a bundle + slot non-empty → `bundle.try_insert(&mut inner_slot_stack)`.
   - cursor empty + slot has a bundle → `bundle.try_extract()` onto the cursor.
   - slot empty + cursor has a bundle → `bundle.try_extract()` into the slot.

   This is a genuine, non-trivial existing implementation — **but it only covers the
   right-click path**, and it does so unconditionally on `MouseClick::Right` for *all
   four* insert/extract cases, whereas vanilla's actual split is: left-click = full-stack
   transfer (`tryTransfer`/insert-other-into-self), right-click = single-item transfer
   (`removeOne`/single insert). The current Rust code has, in effect, implemented
   something closer to "single-item transfer on right-click in all four
   directions" and has **no left-click (full-stack-into-bundle) behavior at all** — so
   dragging a whole stack from a slot onto a bundle currently does nothing bundle-
   specific (falls through to normal swap-with-cursor behavior instead of inserting into
   the bundle), which is the main missing piece, not "there is no click handling."

   Architecturally, this logic is also hardcoded inline in the generic slot-click
   dispatcher rather than routed through any per-item override point — there is no
   `ItemBehaviour::override_stacked_on_other`/`override_other_stacked_on_me` today, so
   this doesn't generalize to any future stackable-container item (shulker boxes are a
   block-entity-backed container, a different mechanism, so this is bundle-specific for
   now, but the *pattern* vanilla uses is explicitly item-level and reusable).

### Target design
Two-part fix:
1. **Correct the click-type mapping** to match vanilla exactly: left-click
   (`MouseClick::Left`) should attempt a *full stack* transfer in the relevant direction
   (slot→bundle when cursor holds the bundle and slot is non-empty and cursor is
   clicking onto it; or bundle-in-slot + cursor non-empty inserting the cursor stack into
   the slot's bundle) with insert/insert-fail sound feedback; right-click keeps today's
   single-item semantics but should also respect vanilla's exact preconditions (`other.
   isEmpty()` gating the extract directions, not just "click type is right"). This means
   duplicating the existing four-branch structure once more for `MouseClick::Left` with
   `try_insert`-of-the-whole-stack semantics (which `BundleContentsImpl::try_insert`
   already supports per-item, per its existing signature taking `&mut ItemStack` and
   looping while it can absorb items — need to verify it fully drains a multi-count
   stack rather than just one unit, since `tryInsert`/`tryTransfer` in vanilla move as
   many as fit, which the existing Rust `try_insert` implementation in
   `data_component_impl/utility.rs` appears to already do via its `while stack.item_count
   > 0 && ...` loop — so the *building block* exists, only the call site's click-type
   gating needs correcting).
2. **Consider (lower priority, larger refactor) generalizing** the hardcoded
   `BundleContentsImpl` checks in `screen_handler.rs` into two new `ItemBehaviour`
   methods (`override_stacked_on_other`, `override_other_stacked_on_me`, matching
   vanilla's signatures as closely as Rust ergonomics allow) dispatched from the same
   call site, with `BundleItem`'s implementation containing today's logic. This isn't
   strictly required to fix the left-click gap (item 1 above can be done as a pure
   bugfix inside `screen_handler.rs` without a new trait), but it's the architecturally
   faithful mirror of vanilla and is what the task's framing points at
   ("check how screen-handler slot-click dispatch works ... to design where this
   stacked-click override would hook in generically"). Recommend doing (1) as the
   correctness fix and proposing (2) as a follow-up refactor, since (2) touches a
   1388-line file's core dispatch loop and is riskier to land in one patch alongside
   the actual bundle-behavior fix.
3. `toggleSelectedItem`'s GUI-selection-index behavior (vanilla's PRIMARY-with-empty-
   other and fallback branches) affects only which bundle item is client-rendered as
   "selected" in the tooltip grid — check whether `BundleContentsImpl` already has a
   selected-index concept (`getSelectedItemIndex`/`getSelectedItem` exist in vanilla;
   grep `BundleContentsImpl` in Rust for an equivalent — not found in the excerpt read
   in this pass, so likely also missing and would need its own small field addition if
   pursued; treat as a cosmetic nice-to-have, not core gameplay parity).

### Files to create/modify
- Modify `pumpkin-inventory/src/screen_handler.rs`: add the left-click full-stack-
  transfer branches alongside the existing right-click single-item branches (minimum
  fix); optionally refactor both into `ItemBehaviour` dispatch calls (larger fix).
- If doing the trait refactor: modify `pumpkin/src/item/mod.rs` (new methods) and
  `pumpkin/src/item/items/bundle.rs` (move the logic there from screen_handler.rs).
- Possibly `pumpkin-data/src/data_component_impl/utility.rs`'s `BundleContentsImpl` if a
  selected-index field is added for the toggle behavior.

### Needs wiki/vanilla verification
- Confirm `BundleContentsImpl::try_insert`'s existing loop fully drains a stack (moves
  as many units as fit) rather than one unit per call, before assuming it's directly
  reusable for the left-click full-stack path without modification.
- `ClickAction.PRIMARY`/`SECONDARY`'s exact mapping to Pumpkin's `MouseClick::Left`/
  `Right` was inferred from `SlotActionType::Pickup`'s `button == 0 || button == 1`
  branch in `screen_handler.rs`; confirm this mapping holds for all client versions
  Pumpkin targets (Java + Bedrock, given this is a multi-protocol server) before
  assuming Bedrock's click packets map onto the same button-index convention.

---

## 6. ShieldItem dye-color name

### Vanilla source
`net/minecraft/world/item/ShieldItem.java` (16 lines) — the entire class:
```java
public Component getName(final ItemStack itemStack) {
   DyeColor baseColor = itemStack.get(DataComponents.BASE_COLOR);
   return baseColor != null
      ? Component.translatable(this.descriptionId + "." + baseColor.getName())
      : super.getName(itemStack);
}
```
`this.descriptionId` for `ShieldItem` is `"item.minecraft.shield"`, so a red-based-
color shield's name becomes `"item.minecraft.shield.red"` (a real localization key that
exists in `en_us.json` etc., distinct from generic un-dyed `"item.minecraft.shield"`).

### Current Rust state
No `pumpkin/src/item/items/shield.rs` exists at all (not found in the earlier
directory listing or `find -iname "*shield*"` under `pumpkin/src`), and no
`ItemBehaviour::get_name`/display-name-override hook exists on the trait (same gap
identified under CompassItem above — this doc treats it as one shared trait addition
serving both items, see section 2's "second shared prerequisite").

### Target design
This is indeed the smallest, most localized item in this batch once the shared
display-name-override hook exists (see section 2):
- New `pumpkin/src/item/items/shield.rs`, or — if Pumpkin's item-name resolution
  doesn't strictly need a full `ItemBehaviour` file per item and instead has a smaller,
  centralized per-item-id name-override table/match — that may be an equally valid,
  possibly simpler, landing spot; this depends entirely on where the shared name-
  resolution call site (to be located per section 2's follow-up) actually lives. If it's
  naturally per-`ItemBehaviour`, add:
  ```rust
  fn get_custom_name(&self, stack: &ItemStack) -> Option<TranslatableText> {
      stack.get_data_component::<DyedColorImpl-or-BaseColorImpl>()
          .map(|color| translatable(format!("item.minecraft.shield.{}", color.name())))
  }
  ```
  — note vanilla keys off `DataComponents.BASE_COLOR` (a `DyeColor` enum component used
  for banner-pattern-bearing items), **not** `DYED_COLOR` (an arbitrary RGB int used for
  leather armor dyeing) — `pumpkin-data/src/data_component_impl/utility.rs` currently
  only shows `DyedColorImpl { rgb: i32 }` in the excerpt read in this pass; confirm
  whether a separate `BaseColorImpl`(`DyeColor` enum, not raw RGB) component already
  exists elsewhere in that file or module before assuming it needs to be added — shields
  and banners share `BASE_COLOR`, so if banners already work in Pumpkin, this component
  likely already exists and just needs a consumer for shields.
- `ItemMetadata::ids()` → `Box::new([Item::SHIELD.id])`.

### Files to create/modify
- Create `pumpkin/src/item/items/shield.rs` (or add to whatever centralized name-
  resolution site is found).
- Modify `pumpkin/src/item/items/mod.rs`: register, if implemented as a full
  `ItemBehaviour`.
- Depends on the shared display-name-override hook from section 2.

### Needs wiki/vanilla verification
- Confirm `BASE_COLOR` (not `DYED_COLOR`) is the correct component to check, and
  whether it already exists in Pumpkin's data-component set (used for banners) —
  not confirmed in this pass, only `DyedColorImpl` (RGB) was seen.
- Confirm the exact translation key format (`"item.minecraft.shield." + colorName`,
  e.g. `light_blue` vs `lightBlue`) against `assets/minecraft/lang/en_us.json` if precise
  key matching matters for any Pumpkin-side localization file it maintains.

---

## 7. CrossbowItem firework-rocket ammo

### Vanilla source
`net/minecraft/world/item/CrossbowItem.java` (306 lines) — firework-specific pieces only
(the rest of the class, arrow-loading/charging/releasing, is largely already ported):
- `getSupportedHeldProjectiles()` (54-57) → `ARROW_OR_FIREWORK` predicate (what can be
  *loaded* into the crossbow — arrows or firework rockets) vs.
  `getAllSupportedProjectiles()` (59-62) → `ARROW_ONLY` (what the crossbow's "which
  ammo types satisfy `hasAmmo`-checks for e.g. creative/dispenser auto-load" considers —
  fireworks are explicitly excluded from *that* broader predicate, i.e. a dispenser or a
  creative-mode "give yourself ammo" search won't auto-select a firework the way it would
  an arrow, only manual loading does).
- `getShootingPower` (81-83): `1.6F` if the charged projectiles contain
  `Items.FIREWORK_ROCKET`, else `3.15F` — this is a **stack-level** check (charged
  projectiles are a list; vanilla checks the *set* contains a firework, implying a
  crossbow is only ever loaded with one item type at a time in practice, consistent with
  `tryLoadProjectiles`/`draw` only drawing one ammo type per load).
- `createProjectile` (150-164): if `projectile.is(Items.FIREWORK_ROCKET)`, spawns a
  `FireworkRocketEntity` at `(shooter.x, shooter.eyeY - 0.15, shooter.z)` with `true`
  passed for whatever the boolean ctor arg means (likely "shot from crossbow" flag
  affecting damage/behavior — not resolved in this pass, check `FireworkRocketEntity`'s
  ctor). Otherwise, defers to `super.createProjectile` (arrow path) and additionally
  sets the arrow's hit sound to `CROSSBOW_HIT` (a crossbow-specific arrow-impact sound
  distinct from a bow-shot arrow's default impact sound — this detail applies to the
  *arrow* path, not fireworks, but is adjacent code worth preserving if arrow-from-
  crossbow doesn't already set this).
- `getDurabilityUse` (166-169): `3` if the projectile `is(Items.FIREWORK_ROCKET)`, else
  `1` — this is per-shot durability cost on the crossbow itself, charged once per
  `performShooting` call regardless of `multishot` firing 3 projectiles from one durability
  hit (durability is consumed once per *use*, not per projectile fired).
- Loading-sound sequencing, `onUseTick` (201-238): `getChargingSounds(itemStack)` picks
  either the default `(CROSSBOW_LOADING_START, CROSSBOW_LOADING_MIDDLE,
  CROSSBOW_LOADING_END)` triple or an enchantment-overridden one (via
  `EnchantmentEffectComponents.CROSSBOW_CHARGING_SOUNDS`, e.g. Quick Charge could in
  theory swap sounds, though vanilla's actual enchantments don't currently override
  this — still, the lookup mechanism exists generically). `tickPercent = (useDuration -
  ticksRemaining) / getChargeDuration(...)` (charge duration itself is enchantment-
  modified via `EnchantmentHelper.modifyCrossbowChargingTime`, base 1.25s = 25 ticks).
  Below 20%: both start/mid flags reset to false (allows the sequence to replay if the
  player somehow re-enters a low-percent state, e.g. a charge-time enchantment change
  mid-use — an edge case). At ≥20%: play start sound once (`startSoundPlayed` guard).
  At ≥50%: play mid sound once (`midLoadSoundPlayed` guard). At 100% and not yet charged
  and `tryLoadProjectiles` succeeds: play end sound with the pitch-jitter formula shown
  in the source, and set `CHARGED_PROJECTILES`. This 3-stage sound sequence is **entirely
  ammo-agnostic** — fireworks and arrows share the same loading sound cues; the
  ammo-specific differences are only in power/durability/projectile-type, not sounds.

### Current Rust state
`pumpkin/src/item/items/crossbow.rs` (258 lines) is already a substantial, largely-
correct port covering: charge-time-with-enchantment, arrow consumption/creative
fallback, multishot (±10° yaw spread via `Enchantment::MULTISHOT`), per-shot pitch
jitter (tested), and firing existing `ChargedProjectiles` on `normal_use`. But it is
**arrow-only end to end**:
- `fire_projectiles` (169-244) unconditionally does
  `ArrowEntity::entity_type_for_item(projectile.item)` and
  `ArrowEntity::new_shot(...)`/`arrow.set_velocity_from_rotation(..., 3.15, ...)` — the
  `3.15` shooting power is hardcoded, never branches to `1.6` for fireworks, and there is
  no `FireworkRocketEntity` spawn path here at all (contrast vanilla's
  `createProjectile`'s `is(Items.FIREWORK_ROCKET)` branch).
- `on_stopped_using` always builds an **arrow** NBT stack
  (`ItemStack::new(1, &Item::ARROW)` for the creative-infinite-ammo fallback, and reads
  whatever's in the found ammo slot otherwise) — so a firework rocket *could* in
  principle be found by `player.find_arrow()` if that helper's search predicate happens
  to also match fireworks (not verified — likely it doesn't, since it's named
  `find_arrow`), meaning fireworks probably can't even be loaded as ammo right now, not
  just "fire with wrong power."
  `player.damage_held_item(1)` is called unconditionally after firing — this is the
  arrow's constant `1` durability cost; there's no branch for the firework's `3`.
- Sound sequencing: only `Sound::ItemCrossbowLoadingEnd` is played, and only from
  `on_stopped_using` (i.e., only once, at release/completion) — there is genuinely **no
  onUseTick-driven progressive start/mid sound**, confirming the census's finding. This
  is blocked on the shared `on_use_tick` prerequisite at the top of this doc, same as
  BrushItem.
- No `getSupportedHeldProjectiles`/`getShootingPower`-equivalent branch point exists at
  all in the current file — `fire_projectiles` and the loading path have no concept of
  "what kind of ammo is this," they're written assuming arrows unconditionally.

### Target design
- Add a `fn projectile_power(item: &Item) -> f32` (or inline match) helper:
  `1.6` if `item == &Item::FIREWORK_ROCKET`, else `3.15` — mirrors `getShootingPower`,
  called both when actually shooting (`fire_projectiles`, replacing the hardcoded
  `3.15`) and, if `releaseUsing`'s power-threshold check has a Rust equivalent anywhere
  (vanilla's `releaseUsing` uses `getPowerForTime(...) >= 1.0F` independent of ammo type
  — that check is charge-time-based, not ammo-power-based, so it may not need touching;
  verify `getShootingPower`'s `1.6`/`3.15` only feeds into the actual velocity/`shoot()`
  call, not any charge-completion gate, before assuming no other call site needs it).
- Add a `fn durability_use(item: &Item) -> i32`: `3` for `Item::FIREWORK_ROCKET`, `1`
  otherwise — replace the unconditional `player.damage_held_item(1)` in
  `fire_projectiles` with `player.damage_held_item(durability_use(&projectile.item))`,
  reading the ammo's item type from the same `charged.projectiles` NBT already being
  iterated there (need the type before the loop consumes it, or capture it once
  up-front since a crossbow only ever holds one ammo type per load, per vanilla's own
  single-type-at-a-time loading model).
- In `fire_projectiles`'s per-projectile loop: branch on `projectile.item ==
  &Item::FIREWORK_ROCKET` — if so, construct/spawn a `FireworkRocketEntity` instead of
  `ArrowEntity` (check whether Pumpkin already has a `FireworkRocketEntity` type — yes,
  `pumpkin/src/item/items/firework_rocket.rs` exists for the hand-thrown-firework item,
  so the *entity* likely already exists and is reusable; only the crossbow's dispatch
  to construct one at the shooter's eye-adjusted position, with the correct "fired from
  weapon"/crit-disabled flag if such a concept exists, needs adding) — set its velocity
  using the same shot-vector math already in `shootProjectile`
  (`get_projectile_shot_vector`... actually this exists only in vanilla; check whether
  Pumpkin's crossbow.rs has ported the multishot-angle math generically enough to reuse
  for a firework's single, non-jittered shot). Skip the multishot ±10° yaw spread for
  fireworks if vanilla's `shoot()` doesn't apply multishot logic to non-arrow ammo
  (verify — `CrossbowItem.shoot()`'s body wasn't read in this pass, only
  `performShooting`/`shootProjectile` were, so confirm multishot's arrow-vs-firework
  scope before porting).
- In loading (`on_stopped_using`/wherever ammo search happens): confirm/extend
  `player.find_arrow()`-equivalent to also find firework rockets in the inventory when
  the crossbow is being loaded, or add a parallel `find_firework()` search — check
  `getSupportedHeldProjectiles` (`ARROW_OR_FIREWORK`) is the loading-time predicate,
  distinct from `getAllSupportedProjectiles` (`ARROW_ONLY`, used elsewhere e.g. for
  "does this creative search count as ammo").
- Progressive loading sound cues: depends on the shared `on_use_tick` prerequisite.
  Once available, port `onUseTick`'s percent-threshold sound logic (20%/50%/100%)
  into `CrossbowItem::on_use_tick`, tracking `start_sound_played`/`mid_sound_played`
  state — note vanilla stores these as **instance fields on the `CrossbowItem` singleton
  itself** (`this.startSoundPlayed`/`this.midLoadSoundPlayed`), which is almost
  certainly a vanilla-only-safe simplification (single-player-adjacent assumption) that
  should **not** be copied as-is into a multi-player Rust server — these need to be
  per-player (or per-item-stack, per-use-session) state instead, e.g. stored alongside
  `item_use_time` on the `LivingEntity`/`Player`, not as fields on the stateless
  `CrossbowItem` unit struct (which is `Send + Sync` and shared across all players/all
  concurrent uses — mutable fields on it would be a correctness bug, not just a style
  choice). This is a case where directly transliterating vanilla's field layout would be
  actively wrong for Pumpkin's architecture; flag this explicitly for whoever implements
  it.
- Sounds themselves (`CROSSBOW_LOADING_START`/`MIDDLE`) are ammo-agnostic — no
  fireworks-vs-arrow branch needed there, only the reuse of the existing single
  `CROSSBOW_LOADING_END` call site generalized to fire at the right percentages instead
  of unconditionally at full charge completion (which is roughly what happens today
  already, just without the two earlier stages).

### Files to create/modify
- Modify `pumpkin/src/item/items/crossbow.rs`: add power/durability-by-ammo-type
  helpers, branch `fire_projectiles` to spawn `FireworkRocketEntity` for firework ammo,
  extend ammo-search to include fireworks, move any "sound already played this use"
  bookkeeping onto per-use/per-player state instead of the item struct.
- Depends on the shared `on_use_tick` addition to `pumpkin/src/item/mod.rs` and
  `pumpkin/src/entity/living.rs` (see top of doc) for the progressive sound cues; the
  ammo-type/power/durability/entity-type branching does **not** depend on that hook and
  can land independently/first.
- Possibly `pumpkin/src/item/items/firework_rocket.rs` if its `FireworkRocketEntity`
  construction helper needs a variant callable from the crossbow's shooting position
  rather than only from a hand-thrown trajectory.

### Needs wiki/vanilla verification
- `FireworkRocketEntity`'s crossbow-specific constructor argument (the trailing
  `true` in `new FireworkRocketEntity(level, projectile, shooter, x, y, z, true)`) —
  not resolved in this pass; likely a "shot from a weapon" flag affecting whether it
  can damage the shooter or some crit/attribution flag. Read `FireworkRocketEntity`'s
  Java source before implementing.
- Whether multishot's ±10° yaw spread applies to firework ammo in vanilla's actual
  `shoot()` method (not read in this pass — only `performShooting`/`shootProjectile`
  were).
- Whether `getPowerForTime`'s `releaseUsing` gate (`>= 1.0F`) is independent of ammo
  type (this doc assumes yes, based on it being charge-*time*-based, but wasn't
  independently re-derived against the full charge/release code path for fireworks
  specifically).
