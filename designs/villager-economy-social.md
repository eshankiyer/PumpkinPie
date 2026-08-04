# Villager economy & social systems — design document

Scope: four missing/stubbed subsystems in Pumpkin's villager-family entities, ported from
vanilla 26.2 decompiled source (`/tmp/pumpkin-vanilla-26.2/decompiled/`). This document
specifies data model, control flow, and file list for each. No code is written here.

Vanilla sources cited:
- `net/minecraft/world/entity/npc/villager/Villager.java`
- `net/minecraft/world/entity/npc/villager/AbstractVillager.java`
- `net/minecraft/world/item/trading/MerchantOffer.java`, `ItemCost.java`, `VillagerTrade.java`
- `net/minecraft/world/entity/npc/wanderingtrader/WanderingTrader.java`,
  `WanderingTraderSpawner.java`
- `net/minecraft/world/level/saveddata/WanderingTraderData.java`
- `net/minecraft/world/entity/animal/equine/TraderLlama.java`
- `net/minecraft/world/entity/ai/sensing/GolemSensor.java`
- `net/minecraft/world/entity/ai/goal/TradeWithPlayerGoal.java`

Current Rust code inspected:
- `pumpkin/src/entity/passive/villager/mod.rs`, `data.rs`, `gossip.rs`
- `pumpkin/src/entity/passive/wandering_trader.rs`
- `pumpkin/src/entity/passive/trader_llama.rs`, `pumpkin/src/entity/passive/llama.rs`
- `pumpkin/src/entity/ai/goal/{owner_hurt_by_target,revenge,track_target,goal_selector}.rs`
- `pumpkin/src/entity/ai/target_predicate.rs`
- `pumpkin/src/entity/mob/mod.rs`, `pumpkin/src/entity/mod.rs`
- `pumpkin-protocol/src/java/client/play/merchant_offers.rs`
- `pumpkin/src/block/blocks/carved_pumpkin.rs` (used as the reference pattern for spawning
  a golem-type entity into the world)

---

## 1. Villager golem-summoning

### Vanilla behavior

`Villager.java`:
- Constants (lines 105-107): `HOW_FAR_AWAY_TO_TALK_TO_OTHER_VILLAGERS_ABOUT_GOLEMS = 10`,
  `HOW_MANY_VILLAGERS_NEED_TO_AGREE_TO_SPAWN_A_GOLEM = 5`,
  `TIME_SINCE_SLEEPING_FOR_GOLEM_SPAWNING = 24000L` (ticks).
- `gossip(ServerLevel, Villager target, long timestamp)` (lines 814-822): when two villagers
  gossip-transfer (only happens on a 1200-tick per-pair cooldown), the initiator calls
  `spawnGolemIfNeeded(level, timestamp, 5)` afterward.
- `spawnGolemIfNeeded(ServerLevel, long timestamp, int villagersNeededToAgree)` (lines
  834-848): if `wantsToSpawnGolem(timestamp)`, scans a 10-block (each direction, so 20x20x20)
  AABB centered on self for `Villager` entities, filters to those that also
  `wantsToSpawnGolem`, caps the list at 5, and if `>= villagersNeededToAgree` (5) agree,
  calls `SpawnUtil.trySpawnMob(IRON_GOLEM, MOB_SUMMONED, level, blockPosition(), 10, 8, 6,
  LEGACY_IRON_GOLEM, false)`. On success, every villager in the (unfiltered) nearby list gets
  `GolemSensor.golemDetected(villager)` called on it (sets a 599-tick memory that suppresses
  further attempts).
- `wantsToSpawnGolem(long timestamp)` (lines 850-852): `golemSpawnConditionsMet(gameTime())`
  AND brain does NOT have `GOLEM_DETECTED_RECENTLY` memory set.
- `golemSpawnConditionsMet(long gameTime)` (lines 896-899): `LAST_SLEPT` memory is present
  and `gameTime - lastSlept < 24000`. **Key point**: despite the constant name/comment
  ("panicking near enough neighbors"), the actual gate is "slept within the last in-game
  day", not "currently panicking". Vanilla's panic-activity brain package (`VillagerPanicTrigger`,
  not read in full here — only referenced from `VillagerGoalPackages`) is what normally
  calls `gossip()` between panicking villagers who bump into each other, since that's the
  main place two villagers interact with each other's `gossips` at short range during a
  crisis. But the actual spawn gate checked in code is the sleep-recency check above, not a
  live "is panicking" flag. Do not assert the "panic" framing beyond what's in the code path
  actually read; if a stricter contract is needed, re-derive from `VillagerPanicTrigger.java`
  before implementing (not read for this doc — flag as **needs follow-up read**).
- `GolemSensor` (`GolemSensor.java`): every 200 ticks, if `IRON_GOLEM` is present in the
  `NEAREST_LIVING_ENTITIES` memory, sets `GOLEM_DETECTED_RECENTLY` (599-tick expiry). This is
  a *separate* suppression path from the one set by a successful `spawnGolemIfNeeded` call —
  both write the same memory key.
- `stopSleeping()` override (lines 890-894) sets `LAST_WOKEN` memory — not directly used by
  the golem path (that path uses `LAST_SLEPT`, set elsewhere by the sleep-behavior brain task,
  not read here).

### Rust gap

`VillagerEntity` (`pumpkin/src/entity/passive/villager/mod.rs`) has no golem-summoning logic
at all. It does have:
- `gossips: Mutex<GossipContainer>` — reputation storage exists (`gossip.rs`), including a
  *dormant* `transfer_from` (vanilla `GossipContainer::transferFrom`) that nothing currently
  calls, per its own doc comment ("Pumpkin has no brain/sensor system driving villagers to
  meet and gossip periodically").
- Sleep tracking: `mob_tick` (lines 906-1014) tracks `home_pos` and toggles pose
  Standing/Sleeping based on time-of-day and bed occupancy, but there is no `last_slept_time`
  field recording *when* a villager last actually entered the sleeping pose — vanilla's brain
  memory (`LAST_SLEPT`) equivalent does not exist.
- No memory-module analogue at all (no `GOLEM_DETECTED_RECENTLY`-style expiring flag
  mechanism on any entity).
- No AABB-scan-for-nearby-villagers-and-agree-then-spawn logic (the `on_mob_death` method in
  the same file already demonstrates the `world.get_all_at_box(&aabb)` + `cast_any().downcast_ref::<Self>()`
  pattern for finding nearby `VillagerEntity`s — reuse this).
- Entity-spawning precedent exists: `pumpkin/src/block/blocks/carved_pumpkin.rs` (lines
  61-67, 97-103) shows the pattern —
  `let entity = Entity::new(world.clone(), pos.to_centered_f64(), &EntityType::IRON_GOLEM);
  let golem = IronGolemEntity::new(entity); world.spawn_entity(golem).await;` — but that code
  always spawns at a fixed, pre-validated position (the carved-pumpkin snowman/golem
  structure). There is no generic "find a valid nearby spawn position" helper analogous to
  vanilla's `SpawnUtil.trySpawnMob` (which does placement/light/collision checks over a
  horizontal/vertical radius). `WanderingTraderSpawner.findSpawnPositionNear` (vanilla) shows
  the shape of such a search but Pumpkin has no Rust equivalent utility function today.

### Target Rust data model

Add to `VillagerEntity` (`pumpkin/src/entity/passive/villager/mod.rs`):

```rust
/// Vanilla `LAST_SLEPT` brain memory (Villager.golemSpawnConditionsMet).
/// World-age tick at which this villager last entered the sleeping pose. `None` if never slept.
pub last_slept_time: AtomicI64, // sentinel 0 == "never", matching existing last_gossip_decay_time convention
/// Vanilla `GOLEM_DETECTED_RECENTLY` brain memory (599-tick expiry), written both by a
/// successful golem-spawn call and by `GolemSensor`-equivalent golem-proximity detection.
pub golem_detected_until: AtomicI64, // world-age tick after which the memory is considered expired; 0 == not set
```

Rationale: Pumpkin has no generic expiring-memory-module system (unlike vanilla's `Brain`).
Rather than build one now, model the one memory value actually needed (`GOLEM_DETECTED_RECENTLY`)
as a plain "expires at tick N" field, following the existing `last_gossip_decay_time` /
`last_restock_time` idiom already used in this struct (`AtomicI64` timestamps compared against
`world.get_world_age()`).

No new struct/goal type is required for the golem sensor itself — vanilla's `GolemSensor` runs
every 200 ticks scanning `NEAREST_LIVING_ENTITIES`; Pumpkin's `mob_tick` already runs every 20
ticks (`age % 20 != 0` early-return, line 887) and already does AABB scans elsewhere in the
same file, so the "is there an iron golem nearby" check can piggyback on a periodic tick
rather than needing a distinct sensor abstraction.

### Control flow

1. **Recording sleep** — in the existing sleep-state-toggle block in `mob_tick`
   (`pumpkin/src/entity/passive/villager/mod.rs` lines ~975-991, where the villager transitions
   into `EntityPose::Sleeping`), add:
   `self.last_slept_time.store(world.get_world_age().await, Ordering::Relaxed);`
   This is the direct analogue of whatever vanilla's sleep-behavior brain task does to set
   `LAST_SLEPT` (not itself read in this pass — the memory's existence and 24000-tick-window use
   in `golemSpawnConditionsMet` is confirmed directly in `Villager.java:896-899`).

2. **`wants_to_spawn_golem(&self, world_age: i64) -> bool`** (new method on `VillagerEntity`):
   ```rust
   let last_slept = self.last_slept_time.load(Ordering::Relaxed);
   if last_slept == 0 || world_age - last_slept >= 24000 {
       return false;
   }
   let detected_until = self.golem_detected_until.load(Ordering::Relaxed);
   detected_until == 0 || world_age >= detected_until
   ```
   (mirrors `golemSpawnConditionsMet` AND NOT `GOLEM_DETECTED_RECENTLY`.)

3. **Golem-proximity suppression** (`GolemSensor` equivalent) — in `mob_tick`, on the existing
   20-tick cadence, scan a small box (vanilla uses `NEAREST_LIVING_ENTITIES`, which is itself
   populated from a follow-range-sized box; approximate with e.g. a 16-block box consistent with
   other approximations already made elsewhere in this file for the same brain/sensor gap) for
   `EntityType::IRON_GOLEM`; if found, set
   `self.golem_detected_until.store(world_age + 599, Ordering::Relaxed);`

4. **`spawn_golem_if_needed(&self, world: &Arc<World>, world_age: i64, villagers_needed: usize)`**
   (new async method on `VillagerEntity`, called from wherever gossip-transfer between two
   villagers happens — see prerequisite gap below):
   - If `!self.wants_to_spawn_golem(world_age)`, return.
   - Build a 20x20x20 AABB centered on `self.get_entity().pos.load()` (±10 each axis, matching
     `villagerSearchBox = getBoundingBox().inflate(10.0, 10.0, 10.0)`).
   - `world.get_all_at_box(&aabb)`, filter to `EntityType::VILLAGER`, `cast_any().downcast_ref::<VillagerEntity>()`
     (same pattern as `on_mob_death`), collect all "nearby" villagers.
   - Filter that list to those whose `wants_to_spawn_golem(world_age)` is also true, `.take(5)`
     (vanilla's `.limit(5L)` — note this is a *hard cap regardless of `villagers_needed`*,
     matching `Villager.java:838`).
   - If `filtered.len() >= villagers_needed`: attempt to find a valid spawn position near
     `self.get_entity().block_pos.load()` (radius ~10 horizontal, per vanilla's `trySpawnMob`
     args `10, 8, 6`: 10 horizontal radius, 8 vertical up, 6 vertical down — cite as
     `SpawnUtil.trySpawnMob` call-site args, `Villager.java:840-842`; **the exact
     placement-validity rules inside `SpawnUtil.trySpawnMob`/`LEGACY_IRON_GOLEM` strategy were
     not read for this doc — needs a follow-up read of `SpawnUtil.java` before implementing the
     position search precisely; a reasonable interim approximation is the
     collision/ground-clearance check pattern already used in `WanderingTraderSpawner.hasEnoughSpace`
     for a similarly-shaped problem**). On success (`Entity::new` + `IronGolemEntity::new` +
     `world.spawn_entity`, per the `carved_pumpkin.rs` pattern), call
     `golem_detected_until = world_age + 599` on **every** villager in the *unfiltered* nearby
     list (not just the 5 that agreed — matches `nearbyVillagers.forEach(GolemSensor::golemDetected)`,
     `Villager.java:844`).

### Prerequisite gap — no gossip-transfer trigger exists

`spawn_golem_if_needed` is vanilla-invoked only from `Villager::gossip(...)`, which is itself
only called from the panic-activity brain package when two panicking villagers meet. Pumpkin
has:
- No brain/activity system (villager AI here is `Goal`/`GoalSelector`-based, flat, no
  `Activity.PANIC` schedule).
- No call site at all for `GossipContainer::transfer_from` (confirmed dormant per its own doc
  comment in `gossip.rs`).

This is a **hard prerequisite**: without some periodic "two nearby villagers exchange gossip"
trigger, golem-summoning can never fire, no matter how faithfully steps 1-4 above are built.
Two implementation options, in order of fidelity vs. effort:
- **Faithful**: add a `PanicGoal`-equivalent for villagers (none currently exists — villagers
  currently only have `AvoidEntityGoal` per-threat-type goals, not a general panic state) and,
  while panicking, run pairwise gossip transfer + `spawn_golem_if_needed` against other
  panicking villagers within the 10-block gossip range noted in vanilla constants. This
  requires first building a villager panic goal, which is out of scope for this document but
  should be filed as its own tracked gap.
- **Pragmatic approximation**: call `gossips.transfer_from(...)` + `spawn_golem_if_needed(...)`
  periodically (e.g. every N ticks in `mob_tick`) between any two nearby villagers regardless
  of panic state. This changes vanilla's trigger condition (any-time vs. panic-only) and should
  be called out explicitly as a known deviation if chosen, since the standing rule in this repo
  is not to assert vanilla parity without verification.

Either way, note it explicitly in the PR/issue that golem-summoning's *trigger* condition is a
deliberately approximated gap, distinct from the summoning *mechanics* in steps 1-4 (which can
be implemented faithfully from the source cited above).

### Files to create/modify

- `pumpkin/src/entity/passive/villager/mod.rs` — add `last_slept_time`, `golem_detected_until`
  fields; add `wants_to_spawn_golem`, `spawn_golem_if_needed` methods; hook sleep-transition
  and golem-proximity-scan into `mob_tick`; add NBT round-trip for the two new fields (optional —
  vanilla does not persist brain memories to NBT for the `Villager` in this file, so persistence
  is not required for parity; can be left transient like `last_gossip_decay_time`'s general
  category of ephemeral timers, though note `last_gossip_decay_time` for comparison IS persisted
  as `LastGossipDecay` — check whether `LAST_SLEPT`/`GOLEM_DETECTED_RECENTLY` are ever persisted
  in vanilla before deciding; not confirmed in this pass, default to **not persisting** since
  they're described in `Brain`/memory terms in vanilla rather than `addAdditionalSaveData`).
- Wherever the panic-goal-or-approximation trigger is added (new file if a real `VillagerPanicGoal`
  is built, e.g. `pumpkin/src/entity/ai/goal/villager_panic.rs`; otherwise inline in `mob_tick`).
- No changes needed to `gossip.rs` itself — `transfer_from` already exists and is unit-tested,
  it just needs a call site.

---

## 2. MerchantOffer demand-based pricing

### Vanilla behavior

`MerchantOffer.java`:
- Fields: `demand` (int, default 0), `priceMultiplier` (float), `specialPriceDiff` (int),
  `uses`/`maxUses` (int).
- `getModifiedCostCount(ItemCost cost)` (lines 123-127):
  ```java
  int basePrice = cost.count();
  int demandDiff = Math.max(0, Mth.floor(basePrice * this.demand * this.priceMultiplier));
  return Mth.clamp(basePrice + demandDiff + this.specialPriceDiff, 1, cost.itemStack().getMaxStackSize());
  ```
  Called by `getCostA()` (line 120) to compute the actual displayed/charged buy-A cost — i.e.
  demand only ever raises price (clamped at 0 minimum contribution via `Math.max(0, ...)`), and
  `specialPriceDiff` (reputation/hero-of-the-village discounts) is applied on top and CAN be
  negative.
- `updateDemand()` (lines 145-147): `this.demand = this.demand + this.uses - (this.maxUses - this.uses)`.
  Equivalently `demand += 2*uses - maxUses`. Called once per `Villager.restock()` (`Villager.java:365-375`,
  called when `shouldRestock` fires — roughly twice a day) and, to "catch up" missed days, from
  `catchUpDemand()` (`Villager.java:421-434`) which calls `updateDemand()` `missedUpdates` times
  where `missedUpdates = 2 - numberOfRestocksToday` (i.e. if a villager wasn't ticked/restocked
  for a while, demand is recomputed as if the missed restocks had happened) — this needs
  `numberOfRestocksToday`/`lastRestockGameTime` tracking, which the Rust code already partially
  has (`restocks_today`, `last_restock_time` fields already exist on `VillagerEntity`, though
  restock-triggering logic itself does not appear to exist yet — see below).
- `resetUses()` (line 157-159): sets `uses = 0`, called every restock alongside `updateDemand()`.
- Enchanted-book double-price: **not** in `MerchantOffer` itself. It's in `VillagerTrade.java`
  (lines 150-200, specifically 183-187):
  ```java
  if (this.doubleTradePriceEnchantments.isPresent()) {
     HolderSet<Enchantment> enchantments = this.doubleTradePriceEnchantments.get();
     ItemEnchantments itemEnchantments = result.get(DataComponents.STORED_ENCHANTMENTS);
     if (itemEnchantments != null && itemEnchantments.keySet().stream().anyMatch(enchantments::contains)) {
        additionalCost *= 2;
     }
  }
  ```
  This runs once, at **offer-generation time** (when a `VillagerTrade` entry is turned into a
  concrete `MerchantOffer` via `getOffer(LootContext)`, lines 161-200), not at trade time. It
  doubles `additionalCost` (an extra emerald cost layered on top of the base `wants` cost via
  `DataComponents.ADDITIONAL_TRADE_COST` on the result item, e.g. bookshelves/enchant level
  driving extra emerald cost for enchanted-book trades from librarians) before folding it into
  the final `ItemCost` via `this.wants.toItemCost(lootContext, additionalCost)`. This is
  data-driven per-trade-definition (`double_trade_price_enchantments` field, `VillagerTrade.java:44`),
  keyed to specific trade-set entries (librarian enchanted book trades primarily) — it is not a
  universal "if item has any enchantment, double price" rule.

### Rust gap

`pumpkin-protocol/src/java/client/play/merchant_offers.rs` — the wire-format `MerchantOffer`
struct already has a `demand: i32` field (line 21) and `special_price: i32` (line 19) and
`price_multiplier: f32` (line 20), so the *wire protocol* already supports sending demand/special
price to the client. But nothing in `pumpkin/src/entity/passive/villager/mod.rs` ever computes a
modified cost from `demand`/`price_multiplier`/`special_price`, nor calls anything resembling
`update_demand`. Concretely:
- `add_trades` (lines 250-284) always sets `demand: 0` and `special_price: 0` when creating a
  fresh offer, and never reads back existing demand on offer regeneration.
- The trade `on_trade` handler (lines 384-474, inside `create_screen_handler`) increments
  `offer.uses` but never recomputes `offer.demand` or adjusts what the player is actually
  charged based on it — the buy-cost the player pays is computed/enforced somewhere in
  `MerchantScreenHandler`/`pumpkin-inventory` (not read for this doc — **needs a read of
  `pumpkin-inventory/src/merchant/merchant_screen_handler.rs` before implementing** to find
  where `base_cost_a` is currently treated as the final charged amount vs. where a
  `get_modified_cost_count`-equivalent needs to be inserted).
- `restock()` (vanilla `Villager.restock()`, lines 365-375) has no Rust equivalent at all —
  confirmed no method by that name or matching shape exists in `villager/mod.rs`. This means
  demand cannot currently ever change post-generation regardless of what `updateDemand()` logic
  is added, because nothing calls it periodically.
- No `ItemCost`-equivalent abstraction exists on the Rust side — offers directly embed
  `ItemStackSerializer` cost fields (`base_cost_a`, `cost_b`, `output` — see
  `merchant_offers.rs` lines 12-14), with counts baked in at creation time
  (`ItemStack::new(trade.wants.count as u8, trade.wants.item)`, `mod.rs` line 265). There is no
  separation between "base cost" (fixed per trade definition) and "modified/effective cost"
  (base + demand + special price, clamped) the way vanilla splits `getBaseCostA()` vs `getCostA()`.
- No enchanted-book double-price logic anywhere; no `additional_trade_cost` component handling
  either (would require checking whether Pumpkin's item/component system has an equivalent of
  `DataComponents.ADDITIONAL_TRADE_COST` — **not checked in this pass, needs follow-up**).

### Target Rust data model

The cleanest fit for this codebase's existing idiom (flat structs mirroring the wire-protocol
type, not a `Villager.java`-style OOP hierarchy) is to keep `demand`/`special_price` fields on
the wire-format `MerchantOffer` (they already exist) but stop treating `base_cost_a`'s embedded
count as authoritative for what's charged. Introduce a small helper — either a free function or
a method on the wire `MerchantOffer` itself:

```rust
// pumpkin-protocol/src/java/client/play/merchant_offers.rs, or a villager-side wrapper if
// pumpkin-protocol should stay wire-format-only (see note below)
impl MerchantOffer {
    /// Vanilla `MerchantOffer::getModifiedCostCount` (MerchantOffer.java:123-127).
    /// `base_count` is the trade definition's un-modified want count; `max_stack_size` clamps
    /// the upper bound (vanilla uses `cost.itemStack().getMaxStackSize()`).
    pub fn modified_cost_count(base_count: i32, demand: i32, price_multiplier: f32, special_price: i32, max_stack_size: i32) -> i32 {
        let demand_diff = ((base_count as f32) * (demand as f32) * price_multiplier).floor().max(0.0) as i32;
        (base_count + demand_diff + special_price).clamp(1, max_stack_size)
    }

    /// Vanilla `MerchantOffer::updateDemand` (MerchantOffer.java:145-147).
    pub fn update_demand(&mut self) {
        self.demand += self.uses - (self.max_uses - self.uses);
    }
}
```

Because `pumpkin-protocol`'s `MerchantOffer` is a wire-serialization type (used directly by
`CMerchantOffers`), and because the Rust villager code currently stores the "live" offer as
exactly this same struct (`offers: Mutex<Vec<pumpkin_protocol::...::MerchantOffer>>` in
`villager/mod.rs` line 60 — there is no separate "server-side offer model" distinct from the
wire type, unlike vanilla's split between `MerchantOffer` (server model) and
`ClientboundMerchantOffersPacket` (wire)), the pragmatic fix that matches existing structure is:
add a `base_cost_a_count: i32` (or reuse the existing count embedded in `base_cost_a`'s
`ItemStack` as the "base" and always recompute a display copy) — **recommend**: keep
`base_cost_a`'s `ItemStackSerializer` count as the immutable base count (set once at trade
generation, never mutated thereafter), and change every place that currently reads
`offer.base_cost_a.0.item_count` as "the cost to charge" to instead call
`modified_cost_count(offer.base_cost_a.0.item_count, offer.demand, offer.price_multiplier, offer.special_price, max_stack_size)`
at the point of charging/display. This mirrors vanilla's `getBaseCostA()` (line 115-117, raw)
vs `getCostA()` (line 119-121, modified) split without needing a second struct.

### Control flow

1. **Villager-side periodic restock** — new method on `VillagerEntity`:
   ```rust
   pub async fn restock(&self) {
       // Villager::restock, Villager.java:365-375
       let mut offers = self.offers.lock().await;
       for offer in offers.iter_mut() { offer.update_demand(); offer.uses = 0; }
       drop(offers);
       self.resend_offers_to_trading_player().await; // needs implementing; see gap below
       self.last_restock_time.store(world_age, Ordering::Relaxed);
       self.restocks_today.fetch_add(1, Ordering::Relaxed);
   }
   ```
   Call this from `mob_tick` when `should_restock` conditions are met (vanilla
   `Villager.shouldRestock` / `needsToRestock` / `allowedToRestock`, `Villager.java:387-419`):
   a villager restocks up to twice a "day" (`numberOfRestocksToday < 2`, 2400-tick cooldown
   between them) whenever any offer `needsRestock()` (i.e. `uses > 0`). This requires a
   day-boundary check (`shouldRestock`'s `isNewDay` logic, lines 401-419) — Pumpkin's
   `restocks_today`/`last_restock_time` fields already exist for this but nothing currently
   reads or resets them; wire up a day-rollover check analogous to the existing
   `last_gossip_decay_time` 24000-tick pattern already in `mob_tick`.
2. **Trade-time charge** — wherever the player's payment is actually deducted (in
   `pumpkin-inventory`'s merchant screen handler, or in the `on_trade` closure in
   `villager/mod.rs` lines 384-474 if that's where item removal happens — **needs a read of
   `MerchantScreenHandler` before implementing** to find the exact charge site), replace the
   raw `base_cost_a` count with `modified_cost_count(...)`.
3. **Offer-generation-time enchanted-book double price** — in `add_trades`
   (`villager/mod.rs` lines 250-284), after building the base `ItemCost` for a librarian-type
   trade whose output carries stored enchantments matching a to-be-defined "double price"
   enchantment set, double the *additional* cost component before finalizing. This needs the
   trade-set data model (`trade.wants`, `trade.gives`, etc., referenced but not fully read in
   this pass — **needs a read of the Rust trade-set data structures, likely under
   `pumpkin-data` or wherever `profession.trade_set(level)` is defined, to see whether
   enchanted-book trades and `additional_trade_cost` already exist as data** before this can be
   scoped precisely). Flag as **partially specified — needs follow-up read of the Rust
   trade-data source before implementation** since the enchanted-book special case is
   data-driven in vanilla and the Rust trade-data equivalent wasn't located in this pass.

### Prerequisite gaps

- No `resend_offers_to_trading_player`-equivalent exists (vanilla `Villager.resendOffersToTradingPlayer`,
  `Villager.java:377-385`) — needed so a currently-trading player sees updated prices/demand
  live after a restock; without it, restock-driven price changes silently apply only to the
  next time the trade window is reopened.
- `catchUpDemand`'s "missed restocks" backfill (`Villager.java:421-434`) requires
  `numberOfRestocksToday` to be meaningfully reset/tracked across day boundaries — the fields
  exist (`restocks_today`, `last_restock_time`) but the day-rollover logic that resets them
  (vanilla `shouldRestock`'s `isNewDay` check against a `Timelines.OVERWORLD_DAY` clock,
  lines 401-412) has no Rust equivalent; confirm whether Pumpkin has any "in-game day count"
  primitive before implementing (not checked in this pass).
- No `ItemCost`/`additional_trade_cost` data-component equivalent confirmed on the Rust side —
  needed for the enchanted-book case specifically, not for base demand pricing.

### Files to create/modify

- `pumpkin-protocol/src/java/client/play/merchant_offers.rs` — add `modified_cost_count` and
  `update_demand` associated functions/methods on `MerchantOffer`.
- `pumpkin/src/entity/passive/villager/mod.rs` — add `restock`, wire into `mob_tick`;
  add `resend_offers_to_trading_player` (needs a handle to the currently-trading player —
  check whether `VillagerEntity` currently tracks one; `AbstractVillager` in vanilla has
  `tradingPlayer` (`AbstractVillager.java:58`) — confirm Rust equivalent exists or add it);
  update `add_trades` for enchanted-book double pricing once trade-data gap is resolved.
- Trade-charging site inside `pumpkin-inventory` (exact file TBD — needs the follow-up read
  noted above) to use `modified_cost_count` instead of the raw base count.

---

## 3. WanderingTrader trade-offer generation

### Vanilla behavior

`WanderingTrader.java`:
- `updateTrades(ServerLevel level)` (lines 129-135): pulls three trade sets in a fixed order —
  `WANDERING_TRADER_BUYING`, then `WANDERING_TRADER_UNCOMMON`, then `WANDERING_TRADER_COMMON`
  — via the shared `AbstractVillager.addOffersFromTradeSet` helper
  (`AbstractVillager.java:232-253`), which itself resolves the registry `TradeSet`, builds a
  `LootContext` (origin = trader position, `THIS_ENTITY` = trader), computes
  `tradeSet.calculateNumberOfTrades(lootContext)`, and adds that many trades (with or without
  duplicates per `tradeSet.allowDuplicates()`).
- `mobInteract` (lines 106-127): if alive, not already trading, not a baby, and item in hand
  isn't a spawn egg: awards `TALKED_TO_VILLAGER` stat (main hand only), and if
  `getOffers().isEmpty()` returns `CONSUME` (i.e. no trade menu opens — trader has nothing to
  sell), else `setTradingPlayer(player)` + `openTradingScreen(player, displayName, 1)` (note:
  villager level fixed at `1` — wandering traders don't have a villager-level progression the
  way profession villagers do).
- `despawnDelay` (fields at lines 52-53, accessors 195-201): defaults to 0 (meaning "never
  despawns on its own" until set). `aiStep` → `maybeDespawn` (lines 203-215): if
  `despawnDelay > 0 && !isTrading()`, decrements it each tick; on hitting 0, `discard()`s the
  entity. `WanderingTraderSpawner.spawn` (its own file, lines ~85-105) sets
  `trader.setDespawnDelay(48000)` on successful spawn (48000 ticks = 40 minutes).
- Persistence: `addAdditionalSaveData`/`readAdditionalSaveData` (lines 138-150) store
  `"DespawnDelay"` (int) and `"wander_target"` (nullable `BlockPos`).
- The actual world-spawner (`WanderingTraderSpawner.java`) is a `CustomSpawner` ticked
  per-level: every 1200 ticks it decrements a `spawnDelay` saved-data counter
  (`WanderingTraderData`, persisted per-world via `SavedDataStorage`/`SavedDataType`); when that
  hits 0, resets it to 24000 and rolls a `spawnChance` (starts at 25, +25 per failed window,
  capped 75) against `random.nextInt(100)`; on a hit, does one more `random.nextInt(10) != 0`
  gate, then searches for a valid position near a random player (biome-tag-gated via
  `BiomeTags.WITHOUT_WANDERING_TRADER_SPAWNS`), spawns the trader, and also tries to spawn 2
  leashed `TraderLlama`s nearby.

### Rust gap

`pumpkin/src/entity/passive/wandering_trader.rs` is a near-empty stub: `WanderingTraderEntity`
has only `mob_entity: MobEntity`, a `new()` that wires up four generic goals (swim, wander,
look-at-player, random-look), an empty `NBTStorage` impl (no save/load at all), and no
`mob_interact`, no `updateTrades` equivalent, no despawn-delay field, no wander-target field.
Concretely missing relative to vanilla:
- No `offers` field at all (villager's struct has `offers: Mutex<Vec<...MerchantOffer>>` —
  wandering trader has nothing analogous).
- No `mob_interact` override — falls through to whatever `Mob`'s default `mob_interact` does
  (`pumpkin/src/entity/mob/mod.rs` line 495, generic leash-only handling per earlier grep — not
  trade-menu-aware).
- No `despawn_delay` field/tick logic.
- No `wander_target` field or the `WanderToPositionGoal` equivalent (vanilla's private inner
  goal class, lines 225-269, that steers the trader back toward its `wanderTarget` — e.g. the
  village meeting point it spawned near — using a two-step "walk toward a point 10 blocks along
  the vector if farther than that, else walk straight to the target" logic).
- No world-level `WanderingTraderSpawner`/`WanderingTraderData` equivalent — nothing currently
  spawns a `WanderingTraderEntity` at all via the natural periodic-spawn path (only a manual
  spawn-egg or command presumably works, via generic entity-type spawn machinery — not
  confirmed either way in this pass).

### Target Rust data model

```rust
// pumpkin/src/entity/passive/wandering_trader.rs
pub struct WanderingTraderEntity {
    pub mob_entity: MobEntity,
    pub offers: Mutex<Vec<pumpkin_protocol::java::client::play::MerchantOffer>>, // same type villager uses
    pub despawn_delay: AtomicI32,   // vanilla `despawnDelay`, default 0
    pub wander_target: std::sync::Mutex<Option<BlockPos>>, // vanilla `wanderTarget`
    pub trading_player: std::sync::Mutex<Option<Uuid>>, // if not already tracked generically — see prerequisite below
}
```

Follow the exact pattern `VillagerEntity` already uses for `offers`/NBT round-trip (same
`ItemStackSerializer`-based read/write of `buy`/`buyB`/`sell`/`uses`/`maxUses`/`rewardExp`/
`xp`/`priceMultiplier`/`specialPrice`/`demand` under an `"Offers"` → `"Recipes"` compound-list,
`villager/mod.rs` lines 519-555 write side / 649-691 read side) rather than inventing a new
serialization shape — the wire format and NBT format are shared across all `AbstractVillager`
subtypes in vanilla and should be shared here too (candidate refactor: extract a free function
`write_merchant_offers(&mut NbtCompound, &[MerchantOffer])` / `read_merchant_offers(&NbtCompound) -> Vec<MerchantOffer>`
usable by both `VillagerEntity` and `WanderingTraderEntity`, to avoid duplicating ~70 lines).

### Control flow

1. **`generate_trades` equivalent** — new async method, e.g. `update_trades(&self, world: &Arc<World>)`:
   push offers from `WANDERING_TRADER_BUYING`, then `_UNCOMMON`, then `_COMMON` trade sets, in
   that order (`WanderingTrader.java:132-134`). This depends on whatever the Rust trade-set data
   representation is for `TradeSets::WANDERING_TRADER_*` — the villager code already references
   `profession.trade_set(level)` (a per-profession/level trade set), so the analogous per-trader
   trade sets need to exist as data; **needs a follow-up read of the Rust trade-set/profession
   data module** (likely near `pumpkin-data` or `villager/data.rs`) to confirm whether
   wandering-trader-specific trade tables already exist as data (unlikely, since the entity
   stub has zero trading logic) or need to be authored from `VillagerTrades.java`'s wandering
   trader entries.
2. **`mob_interact`** override on `WanderingTraderEntity`, following `VillagerEntity::mob_interact`
   as the template (`villager/mod.rs` lines 1130-1177) but simpler — no baby/profession checks,
   no `set_unhappy` (wandering traders don't have the "unhappy" particle-and-sound reaction in
   vanilla; confirmed absent from `WanderingTrader.java`'s `mobInteract`): if offers empty,
   lazily call `update_trades`, and if still empty return early (vanilla returns `CONSUME`,
   i.e. "acknowledge but no menu" — check how `VillagerEntity`'s analogous branch signals this
   through the `mob_interact` trait's `bool` return to replicate correctly); otherwise open the
   trading screen (reuse `open_trading_screen`-style logic already on `VillagerEntity` — likely
   worth extracting a shared helper rather than duplicating the `ScreenHandlerFactory` glue,
   though `ScreenHandlerFactory` is implemented per-concrete-type so some duplication of the
   `impl ScreenHandlerFactory for WanderingTraderEntity` block is unavoidable given Rust's trait
   model — model it closely on `villager/mod.rs` lines 360-484, sans the XP-level-up branch
   since wandering traders never level up (`Villager.java` level fixed at `1`)).
3. **Despawn tick** — in `WanderingTraderEntity`'s `mob_tick` (needs adding — the `Mob` trait
   presumably has a default no-op `mob_tick` today since none is implemented in the current
   stub; confirm the trait's default before overriding), replicate `maybeDespawn`
   (`WanderingTrader.java:211-215`): if `despawn_delay > 0 && !is_trading()`, decrement; at 0,
   remove the entity from the world (find the Rust equivalent of `discard()` — likely
   `world.remove_entity(...)` or similar; confirm exact name before implementing).
4. **Wander-to-target goal** — new `Goal` impl, e.g.
   `pumpkin/src/entity/ai/goal/wander_to_position.rs`, modeled directly on vanilla's inner
   class (lines 225-269): `can_start` true if `wander_target.is_some()` and farther than 2
   blocks from it; on tick, if farther than 10 blocks, walk toward a point 10 blocks along the
   direction vector (prevents suddenly teleporting attention across the whole map — actually
   it's a normal pathfind step, just capped at a 10-block waypoint so the trader paths
   incrementally); else walk straight to the target. Add to the goal selector in `new()` at
   priority 2, between `TradeWithPlayerGoal`-equivalent (priority 1, not yet ported — see next
   point) and `MoveTowardsRestrictionGoal` (priority 4, already present per grep in `mod.rs`
   line 34).
5. **`TradeWithPlayerGoal` equivalent** — vanilla's `TradeWithPlayerGoal.java`: while
   `getTradingPlayer() != null` and within 4 blocks, stop navigation and hold position; on
   `stop()`, clear `tradingPlayer`. Both `VillagerEntity` and `WanderingTraderEntity` need this
   (it's on `AbstractVillager`, not `Villager`-specific) — check whether `VillagerEntity`
   already has an equivalent goal (not found by name in the goal directory listing — **this
   appears to be a second, villager-family-wide gap**: neither entity currently stops to face
   the trading player during a trade, since `goal_selector` listings in both `mod.rs` files show
   no trade-related goal). Worth building once, generically, if the two entities can share a
   `is_trading()`/`trading_player` accessor shape.

### Prerequisite gaps

- No `trading_player` tracking confirmed anywhere in the villager-family Rust code (searched
  `villager/mod.rs`; the trade `on_trade` closure operates off a `player_uuid` captured at
  screen-handler-creation time, not a persistent "currently trading" entity field) — needed for
  `maybeDespawn`'s `!isTrading()` guard, for `TradeWithPlayerGoal`, and to correctly block
  wandering-trader despawn mid-trade. This should probably be added once, generically, on
  `AbstractVillager`-equivalent shared state if Pumpkin has (or should grow) such a shared base
  — currently `VillagerEntity` and `WanderingTraderEntity` share no common struct/trait beyond
  `Mob`, so either accept duplication or introduce a small shared component
  (`pub trading_player: std::sync::Mutex<Option<Uuid>>` + `is_trading()`) — flagging as a design
  choice for whoever implements this, not resolving it here.
- No `WanderingTraderSpawner`/`WanderingTraderData`-equivalent world-tick spawner exists —
  without it, wandering traders can only ever appear via manual spawn (egg/command), never
  naturally. This is a separate, larger piece of work (a per-world periodic spawner with
  persisted saved-data state) — out of scope to fully design here but flagged as required for
  full parity; if only "trading works once a trader exists" is in scope, this can be deferred.
- Trade-set data for `WANDERING_TRADER_BUYING`/`_UNCOMMON`/`_COMMON` likely needs authoring from
  `VillagerTrades.java` if not already present as Rust data (needs follow-up read).

### Files to create/modify

- `pumpkin/src/entity/passive/wandering_trader.rs` — add fields, `update_trades`,
  `mob_interact`, despawn tick, NBT save/load, `ScreenHandlerFactory` impl.
- `pumpkin/src/entity/ai/goal/wander_to_position.rs` (new) — the `WanderToPositionGoal` port.
- `pumpkin/src/entity/ai/goal/trade_with_player.rs` (new, shared) — the `TradeWithPlayerGoal`
  port, usable by both villager types if a shared `is_trading()` shape is introduced.
- Possibly `pumpkin/src/entity/passive/villager/mod.rs` — if a shared trading-player field is
  factored out, or if the NBT offer read/write helper is extracted for reuse.
- New spawner module (name TBD, e.g. `pumpkin/src/world/wandering_trader_spawner.rs` +
  world-saved-data type) — out of scope to fully spec here, flagged as a following piece of work.

---

## 4. TraderLlama defending a WanderingTrader

### Vanilla behavior

`TraderLlama.java`:
- `registerGoals` (lines 62-69): adds `PanicGoal(this, 2.0)` at goal-selector priority 1 (on
  top of whatever `Llama`'s base `registerGoals` sets up via `super.registerGoals()` — not read
  in this pass), and on the **target selector**: priority 1
  `TraderLlamaDefendWanderingTraderGoal`, priority 2 `NearestAttackableTargetGoal<Zombie>`
  (excluding zombified piglins), priority 2 `NearestAttackableTargetGoal<AbstractIllager>`.
- `TraderLlamaDefendWanderingTraderGoal` (inner static class, lines 132-165): extends
  `TargetGoal` (not read in full — a `Goal.Flag.TARGET`-flagged goal base). State:
  `ownerLastHurtBy: LivingEntity`, `timestamp: int`.
  - `canUse()` (lines 143-154): false if not leashed; false if leash holder isn't a
    `WanderingTrader`; else reads `owner.getLastHurtByMob()` and
    `owner.getLastHurtByMobTimestamp()`, and returns true iff the timestamp differs from the
    goal's stored one AND `canAttack(ownerLastHurtBy, TargetingConditions.DEFAULT)` (a
    `TargetGoal` helper, not read — presumably checks alive/attackable/distance).
  - `start()` (lines 156-164): `mob.setTarget(ownerLastHurtBy)`, refreshes `timestamp` from the
    trader, calls `super.start()`.
  - Net effect: whenever the trader the llama is leashed to gets hurt by something, the llama
    targets that attacker, once per distinct hurt-event (gated by timestamp equality, same
    pattern as vanilla's `RevengeGoal`/Pumpkin's `RevengeGoal` port already does for
    self-defense).
- `doPlayerRide(Player player)` override (lines 75-81): if the llama's leash holder is a
  `WanderingTrader`, **does nothing** (blocks the normal ride-mounting behavior); otherwise
  calls `super.doPlayerRide(player)` (normal llama mounting). I.e. players cannot ride a trader
  llama while it's leashed to its wandering trader.
- `despawnDelay` (lines 28-29, 71-73): default `47999`, persisted (lines 51-60). `maybeDespawn`
  (lines 91-99): only runs if `canDespawn()` (lines 101-107: not tamed, not leashed-to-something-
  other-than-the-trader, no exactly-one-player-passenger, not age-locked, not
  persistence-required). If despawnable: if leashed to a `WanderingTrader`, sync the llama's
  `despawnDelay` to `trader.getDespawnDelay() - 1` **every tick** (i.e. the llama's counter
  slaves itself to the trader's, always one tick behind whatever the trader currently reports —
  this means if the trader's despawn delay is reset/extended, the llama picks that up
  automatically rather than needing its own independent countdown); otherwise decrements its
  own `despawnDelay`. At `<= 0`: `removeLeash()` then `discard()`.
- `finalizeSpawn` (lines 117-130): if spawn reason is `EVENT` (i.e. spawned by
  `WanderingTraderSpawner`), forces age to adult (`setAge(0)`).

### Rust gap

`pumpkin/src/entity/passive/trader_llama.rs` is a near-empty stub, structurally identical in
poverty to the wandering trader stub: `TraderLlamaEntity { mob_entity: MobEntity }`, `new()`
wires up swim/two escape-danger-goals/wander/look-at-player/random-look, empty `NBTStorage`,
no target selector at all. Missing relative to vanilla:
- No `target_selector` goals at all (confirmed absent from the `new()` body — compare to
  `iron_golem.rs`/`enderman.rs`/`creaking.rs` which do set up `target_selector`, per earlier
  grep, so the infrastructure (`MobEntity.target_selector: Mutex<GoalSelector>`) exists and is
  used elsewhere — trader llama just doesn't use it yet).
- No `despawn_delay` field, no persistence, no despawn-tick logic at all (`NBTStorage` is
  literally empty — `impl NBTStorage for TraderLlamaEntity {}`, no override of either method).
- No `doPlayerRide`/mount-blocking override — but see prerequisite gap below: **Pumpkin appears
  to have no generic ridable-mob/mount system at all**, so this specific override has nothing
  to attach to yet.
- No `finalizeSpawn`/spawn-reason-based age-forcing override.

Rust infrastructure that already exists and is directly reusable, confirmed by reading:
- `pumpkin/src/entity/mod.rs`: `leashed_to: Mutex<Option<Arc<dyn EntityBase>>>` (line 808),
  with `leash_to`/`unleash`/`tick_leash` methods (lines 3105, 3133, 3159) — this is exactly
  vanilla's `getLeashHolder()`/leash mechanic, already fully built and already used by
  `WanderingTraderSpawner`-style leashing (per `mob/mod.rs` leash-toggle-on-interact logic,
  lines ~495-523). Casting the leash holder down to a `WanderingTraderEntity` (once that struct
  exists per section 3) via `cast_any().downcast_ref::<WanderingTraderEntity>()` (the same
  pattern used in `villager/mod.rs`'s `on_mob_death`, line 869) is sufficient to check "is this
  llama leashed to a wandering trader" — no new leash primitive is needed.
- `pumpkin/src/entity/ai/goal/owner_hurt_by_target.rs`: this is **structurally the exact
  template** for `TraderLlamaDefendWanderingTraderGoal` — it already implements "read an
  owner's `last_attacked_time`/`last_attacker_id`, gate on timestamp-changed, target the
  attacker, clear on stop" for the tamed-pet-defends-owner case (dogs defending their owner).
  The only differences needed for the llama variant: (a) "owner" resolution is
  `leashed_to`-then-downcast-to-`WanderingTraderEntity` instead of `get_owner_uuid()` +
  player lookup; (b) no `is_sitting()` check (llamas don't sit); (c) the attacker/target
  liveness and distance checks in `should_continue` can likely be copied verbatim.
- `pumpkin/src/entity/ai/target_predicate.rs`: `TargetPredicate::create_attackable()` /
  `.ignore_visibility()` — usable directly if a `TargetPredicate`-based check is preferred over
  the simpler `is_alive()` check `owner_hurt_by_target.rs` currently uses inline (vanilla uses
  `TargetingConditions.DEFAULT` here, whose exact shape wasn't read — **needs follow-up read of
  `TargetingConditions.java`** to confirm whether `DEFAULT` differs meaningfully from
  `create_attackable()`'s defaults before deciding whether to reuse `TargetPredicate` as-is or
  add a distinct preset).
- `living_entity.last_attacker_id` / `last_attacked_time` (`AtomicI32`s, confirmed present and
  already used both by `RevengeGoal` and `owner_hurt_by_target.rs`, and already populated for
  `VillagerEntity` per the `on_damage` override in `villager/mod.rs` lines 818-841) — this means
  `WanderingTraderEntity` (once built per section 3) will automatically get correctly-populated
  `last_attacker_id`/`last_attacked_time` fields for free, since they live on the shared
  `LivingEntity`/`MobEntity` base, not on a per-species struct. No new field is needed on
  `WanderingTraderEntity` to support this goal — it's pure reuse of existing `LivingEntity` state.

### Target Rust data model

```rust
// pumpkin/src/entity/passive/trader_llama.rs
pub struct TraderLlamaEntity {
    pub mob_entity: MobEntity,
    pub despawn_delay: AtomicI32, // vanilla `despawnDelay`, default 47999
}
```

No other new fields needed — leash state, last-attacker tracking, and target selection all ride
on infrastructure that already exists generically on `Entity`/`LivingEntity`/`MobEntity`.

### Control flow

1. **`TraderLlamaDefendWanderingTraderGoal`** — new file
   `pumpkin/src/entity/ai/goal/trader_llama_defend_wandering_trader.rs`, built by copying
   `owner_hurt_by_target.rs`'s shape:
   ```rust
   pub struct TraderLlamaDefendWanderingTraderGoal {
       target: Option<Arc<dyn EntityBase>>,
       last_attacked_time: i32,
   }
   ```
   - `can_start`: get `mob.get_entity().leashed_to.lock().await.clone()`; if `None`, false.
     `cast_any().downcast_ref::<WanderingTraderEntity>()` on the holder; if it doesn't downcast,
     false (this is the `!(leashHolder instanceof WanderingTrader)` check,
     `TraderLlama.java:147`). Otherwise read the trader's
     `living_entity.last_attacked_time`/`last_attacker_id` (same fields `RevengeGoal` reads off
     any `LivingEntity`), gate on `attacked_time != self.last_attacked_time`, resolve the
     attacker entity via `world.get_entity_by_id`, and apply an attackable/alive check (mirror
     `owner_hurt_by_target.rs` lines 55-61, substituting the trader for the "owner").
   - `start`: `mob.set_mob_target(self.target.clone())`, refresh `self.last_attacked_time` from
     the trader's current `last_attacked_time`.
   - `should_continue`/`stop`: copy `owner_hurt_by_target.rs` verbatim (target-alive +
     distance-within-follow-range check, clear target on stop).
   - Register at target-selector priority 1 in `TraderLlamaEntity::new()`, ahead of
     zombie/illager `NearestAttackableTargetGoal`s at priority 2 (mirrors vanilla's priority
     ordering, `TraderLlama.java:66-68`; Pumpkin already has a `NearestAttackableTargetGoal`
     equivalent — confirm exact name, `nearest_hostile_target.rs` looks like the closest match
     from the goal-directory listing, used by `iron_golem.rs`/others — **needs a quick read to
     confirm its generic-over-entity-type shape matches vanilla's `NearestAttackableTargetGoal<Zombie>`
     / `<AbstractIllager>` usage** before wiring up priority-2 goals; not required for the
     defend-goal itself).
2. **Despawn tick** — add a `mob_tick` (or equivalent per-tick hook — confirm the `Mob` trait's
   tick method name/default from `mob/mod.rs`) override on `TraderLlamaEntity`:
   ```rust
   // canDespawn: not tamed, not leashed-to-non-trader, no single player passenger,
   // not age-locked, not persistence-required (TraderLlama.java:101-107)
   if can_despawn {
       if let Some(holder) = leashed_to() && let Some(trader) = downcast::<WanderingTraderEntity>(holder) {
           despawn_delay.store(trader.despawn_delay.load(Relaxed) - 1, Relaxed);
       } else {
           despawn_delay.fetch_sub(1, Relaxed);
       }
       if despawn_delay.load(Relaxed) <= 0 {
           entity.unleash().await; // or a `remove_leash` equivalent — confirm exact method name
           world.remove_entity(...).await; // confirm exact discard/removal API
       }
   }
   ```
   `is_tamed()`/`is_age_locked()`/`is_persistence_required()`/single-passenger-check equivalents
   need confirming against whatever generic entity/mob flags Pumpkin already tracks (llamas are
   tameable in vanilla via `AbstractHorse`; whether Pumpkin's llama already has a "tamed" concept
   wasn't checked in this pass — **needs a read of `pumpkin/src/entity/passive/llama.rs`'s full
   54 lines plus whatever `Llama`/horse-family base it extends**, since `llama.rs` as read here
   is only a thin stub itself and may not yet carry taming state at all).
3. **`doPlayerRide` mount-block** — **blocked entirely** on the prerequisite gap below (no
   generic ride/mount system exists in Pumpkin at all, confirmed by the absence of any
   `start_riding`/`Mountable`/saddle-interaction code path in `mob/mod.rs` or `entity/mod.rs`;
   the only "passenger" logic found is minecart-pickup-specific, `entity/mod.rs` lines
   ~470-545). This cannot be meaningfully implemented until basic animal-mounting exists.
   Document the intended override site for whoever builds mounting: wherever a generic
   "player right-clicks a rideable mob → mount it" interaction handler eventually lives (likely
   in `mob/mod.rs`'s `mob_interact` default, or a new `Rideable`/`Mountable` trait method a
   `TraderLlamaEntity` impl could override), the check to add is exactly vanilla's: if
   `leashed_to` downcasts to `WanderingTraderEntity`, skip mounting; else proceed normally.
4. **Spawn-reason age-forcing** — vanilla's `finalizeSpawn` override
   (`TraderLlama.java:117-130`) forces adult age when spawned by the `WanderingTraderSpawner`
   (`EntitySpawnReason.EVENT`). This depends on Pumpkin having an `EntitySpawnReason`-equivalent
   concept threaded through entity spawning — **not checked in this pass**; if such a concept
   doesn't exist, this becomes a smaller, standalone gap to file rather than block on.

### Prerequisite gaps

- **No generic animal-mounting/riding system.** This is the single largest blocker in this
  subsystem: `doPlayerRide` cannot be overridden if there's no base `doPlayerRide` (or
  equivalent) to override. Confirmed absent by grep across `entity/mod.rs`, `mob/mod.rs`, and
  `llama.rs` for `start_riding`/`Mountable`/`Rideable`/saddle-interaction keywords — only
  minecart-passenger and leash-follow logic exist. Building this is out of scope for this
  document but must be filed as an explicit prerequisite: the `TraderLlamaDefendWanderingTraderGoal`
  and despawn-sync logic (items 1-2 above) can be implemented and are independently useful today;
  the ride-blocking behavior (item 3) cannot land until mounting exists.
- **`WanderingTraderEntity` doesn't exist yet in a form with `despawn_delay`** — this subsystem
  is directly dependent on section 3's `WanderingTraderEntity.despawn_delay` field existing
  before the "slave llama despawn timer to trader's" logic (item 2 above) can be written.
  Implementation order should be: section 3 first (or at least its `despawn_delay` field), then
  this section.
- Llama taming/age-lock/persistence-required flags need confirming — `llama.rs` (54 lines) is
  too thin to have shown any of this in the initial read; a full read of that file plus
  whatever horse-family base it uses is needed before `can_despawn` can be written precisely.

### Files to create/modify

- `pumpkin/src/entity/passive/trader_llama.rs` — add `despawn_delay` field, target-selector
  wiring, despawn-tick override, NBT persistence of `despawn_delay` (mirror
  `WanderingTrader`'s `"DespawnDelay"` key/shape once section 3 establishes the pattern).
  `doPlayerRide`-equivalent left as a documented TODO pointing at the mounting-system
  prerequisite.
- `pumpkin/src/entity/ai/goal/trader_llama_defend_wandering_trader.rs` (new) —
  `TraderLlamaDefendWanderingTraderGoal` port, modeled on `owner_hurt_by_target.rs`.
- Depends on `pumpkin/src/entity/passive/wandering_trader.rs` (section 3) for
  `WanderingTraderEntity.despawn_delay` to exist.

---

## Cross-cutting notes

- **Shared `AbstractVillager` concept.** Vanilla's `Villager` and `WanderingTrader` both
  extend `AbstractVillager`, which owns `tradingPlayer`, `offers`, trade-XP reward hooks, and
  the `addOffersFromTradeSet` helper. Pumpkin's `VillagerEntity` and `WanderingTraderEntity`
  currently share nothing but the `Mob` trait. Sections 2 and 3 both surface the same
  underlying gap (no shared trading-player/offers base) from different angles. Recommend
  deciding once, before implementing any of these four sections, whether to introduce a small
  shared struct/trait (e.g. `MerchantState { offers: Mutex<Vec<MerchantOffer>>, trading_player:
  Mutex<Option<Uuid>> }`, composed into both entity structs the way `mob_entity: MobEntity` is
  already composed today) versus accepting duplication across the two files. This document
  does not resolve that choice — it affects the exact field layout in sections 2 and 3 above
  but not the control-flow logic, which is described in terms that work either way.
- **No brain/memory/sensor system.** Section 1 (golem-summoning) and, to a lesser extent, the
  gossip-decay logic already in `villager/mod.rs` are the only places in this codebase that
  need vanilla's `Brain`/`MemoryModuleType`/`Sensor` machinery, and both are handled today via
  ad-hoc `AtomicI64` timestamp fields rather than a general expiring-memory abstraction. This
  document follows that existing precedent rather than proposing a new generic memory system,
  since introducing one would be a much larger, separately-scoped change.
- **Items flagged "needs follow-up read" or "needs wiki/manual verification" above are not
  guesses** — they are vanilla or Rust source files this pass did not open, listed explicitly so
  the implementing engineer knows exactly what to check before writing code, per this repo's
  standing rule against asserting vanilla behavior from memory.
