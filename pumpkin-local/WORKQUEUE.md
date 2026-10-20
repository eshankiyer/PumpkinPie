# Pumpkin parity work queue

Backlog for the local agent. Target is **MC 26.2** (the repo declares
`0.1.0-dev+26.2`), NOT 1.21. Game rules were renamed to snake_case in 25w44a and
dimension types were reworked in 26.x, so a 1.21 source is not authoritative.

## How to read an item

```
ID    short name
STATE confirmed | suspected | absent
WHERE file paths
WHAT  what is wrong
VANILLA what vanilla does, and whether it has been verified
DO    the change to make
CARE  the specific way this one can go wrong
```

**STATE meanings, and they are not optional:**

- `confirmed` — someone read the code and a vanilla source. Implement it.
- `suspected` — the code smells wrong but vanilla behaviour is NOT verified.
  **Do not implement. Report findings and stop.** Claude verifies, then
  promotes it to confirmed with a source link.
- `absent` — feature missing entirely. Usually needs design, not a patch.

If an item's VANILLA line says "UNVERIFIED", you may not write code for it.
Guessing what vanilla does is the single most expensive mistake on this project;
it has already forced two public retractions.

---

# A. Lighting

### A1 get_sky_light_level returns 15 below the world
STATE confirmed
WHERE `pumpkin/src/lighting/runtime.rs`, both the sync and async variants
WHAT `section_index` is computed with an unsigned cast, so a `y` below `min_y`
wraps to a huge index and hits the out-of-bounds branch, which returns 15.
VANILLA Above build height 15 is correct. Below the world it is 0.
DO Replace the unsigned index computation with a signed bounds check that
distinguishes "above the top" from "below the bottom". Return 15 only for above.
CARE Do not simply flip the constant to 0; that breaks the above-build-height
case which is currently right. Also `pumpkin-world/src/lighting/storage.rs`
returns 0 for the same condition, so the two disagree; make them consistent.

### A2 skyDarken is never tracked
STATE confirmed
WHERE `pumpkin/src/world/mod.rs`, `get_max_local_raw_brightness`
WHAT It always passes `ambient_darkness = 0`, with a TODO saying the sky darken
value is not tracked. Grass spreads at night as if it were noon, and night mob
spawning thresholds are wrong.
VANILLA Sky light is reduced by an ambient darkness derived from time of day.
DO Track the value on the world, update it with the day cycle, and pass it.
CARE The subtraction is on a `u8`. An underflow here wraps to ~250 and then a
later clamp yields 15, i.e. a silent false-bright. Saturating-subtract.

### A3 Vanilla-imported worlds go dark above terrain
STATE confirmed, but Wave 2 (needs design)
WHERE `pumpkin-world/src/chunk/format/mod.rs`, `chunk_system/worker_logic.rs`
WHAT A vanilla chunk with `isLightOn: true` omits SkyLight for above-terrain
sections. We read a missing tag as `Empty(0)`, and `needs_relighting` refuses to
relight because `isLightOn` is true. Imported worlds are dark above terrain.
VANILLA A `DataLayer` is written only when it has a backing array. Sections above
`topSection` are omitted and read as 15; below it they are derived by repeating
the nearest layer above.
DO Top-down derivation: find the highest section carrying a SkyLight tag, 15
above it, repeat the nearest layer below. Plus a decision for dimensions without
sky light.
CARE **Write chunk-codec tests first; there are none.** Do not "fix" this by
defaulting a missing tag to 15, which lights up enclosed sections vanilla
derives as 0.

### A4 CLightUpdate has no send sites
STATE absent
WHERE `pumpkin-protocol/src/java/client/play/light_update.rs`
WHAT The packet exists and (after #2615) is encoded correctly, but nothing ever
sends it. Runtime light changes never reach clients, so F3 shows generation-time
light and a placed torch does not visibly relight until the chunk is re-sent.
DO Send it from the light engine when a lighting update completes.
CARE Rate-limit. A naive send per block change will flood clients.

---

# B. Commands

### B1 Block states rejected in setblock and clone filtered
STATE confirmed, Wave 2
WHERE `pumpkin/src/command/argument_types/block.rs`, `pumpkin/src/command/args/block.rs`
WHAT `minecraft:wheat[age=0]` is rejected by both parsers. The newer one stops
consuming at `[`; the legacy one hands the whole token to `Block::from_name`.
DO Both have `Item = &'static Block` and consumers use `block.default_state.id`.
Real support means changing the argument's item type to a resolved block state
and threading it through `setblock`, `fill`, `clone` and `execute if block`.
CARE This is a type change across four commands, not a parser tweak.

### B2 forceload marks chunks but never loads them
STATE confirmed
WHERE the forceload command and the chunk system
WHAT `forceload add` reports success and `forceload query` confirms the mark, but
the chunk is never loaded, so `setblock` there fails with "not loaded".
DO Make the forced set actually pin chunks loaded and ticking.
CARE Also fixes our own test rig, which currently keeps a protocol client
connected purely as a chunk loader.

### B3 Audit the rest of the command tree
STATE suspected
WHAT #2609 found that `ContextChain::try_flatten` pushed the child context
instead of the current one, so **every** execute subcommand silently skipped its
modifier. That class of bug hides elsewhere.
DO Walk every command that uses redirects or modifiers and check the modifier is
actually invoked. Report; do not fix blind.

---

# C. Redstone and block entities

### C1 Hopper chain cooldown (already PR'd as #2611, verify after merge)
### C2 Weighted plates, blackstone button, hopper tick (already PR'd as #2589)

### C3 Pistons and block-move rules
STATE suspected
WHAT Not audited at all. Vanilla piston behaviour is intricate: the 12-block
push limit, immovable blocks, block-entity movement, slime and honey blocks,
and quasi-connectivity.
DO Audit against vanilla and report rows. Do not implement from memory.

### C4 Observer chains and rail powering
STATE suspected
DO Audit. Observers have a 2-tick schedule and specific update ordering; rails
have their own powering rules.

### C5 Comparator container reading for all container types
STATE suspected
WHAT The generic formula was verified correct, but not every container type was
checked (shulker boxes, barrels, hoppers, brewing stands, jukeboxes, lecterns,
cauldrons, end portal frames, chiseled bookshelves all have their own rules).
DO Enumerate the container types with special comparator rules and check each.

---

# D. Status effects and potions

### D1 Instant Health and Damage ignore undead
STATE confirmed
WHERE `pumpkin/src/entity/living.rs`, `pumpkin/src/item/potion.rs`
WHAT No undead check anywhere; the only undead logic is the Smite enchantment.
VANILLA Both invert for undead: Instant Health damages them, Instant Damage heals.
DO Reuse the undead classification Smite already uses. Do not invent a second one.

### D2 Instant potions apply twice
STATE confirmed
WHERE `pumpkin/src/item/potion.rs`
WHAT It calls `heal`/`damage` directly and then calls `add_effect` with the same
instant type, which heals or damages again inside `living.rs` on a different,
unscaled formula.
DO Pick one path. The `add_effect` path is the one that knows about amplifier.
CARE Check splash and lingering sources too, they may scale differently.

### D3 Resistance is inert
STATE confirmed
WHERE damage handling
WHAT Nothing checks `StatusEffect::RESISTANCE`. Registered, does nothing.
VANILLA Reduces incoming damage by 20 percent per level, capped, and interacts
with armour and the damage-type resistance rules.
DO Implement in the damage pipeline. Part of upstream #764.
CARE #764 is partly stale; most other effects it lists are now implemented.

### D4 Milk bucket never clears effects
STATE absent
WHAT No `MILK_BUCKET` reference anywhere in `pumpkin/src`.
CARE Closed-unmerged PR #2548 attempted this. Read it and its review first;
there may be a reviewer objection worth honouring.

### D5 Tipped arrows apply nothing
STATE absent
WHERE `ArrowEntity` has no effect field, no `TIPPED_ARROW` handling.
DO Carry the potion contents on the arrow and apply on hit, at the vanilla
reduced duration.

### D6 Effect hidden-stack
STATE confirmed, Wave 2
WHAT #2617 merges by amplifier and duration but discards the superseded weaker
effect. Vanilla keeps it hidden and restores it when the stronger one expires.
DO Needs a place to store the hidden stack per effect type.

### D7 Beacon, conduit, and ambient effect sources
STATE suspected
DO Audit whether beacon range, tiers, and secondary effects match, and whether
conduit power is implemented at all.

---

# E. Entities and AI

### E1 Zombie villager curing does nothing
STATE confirmed
WHERE `pumpkin/src/net/java/play.rs`, `entity/mob/zombie/zombie_villager.rs`
WHAT Right-clicking any zombie villager with a golden apple grants the Zombie
Doctor advancement immediately. No Weakness check, no timer, no conversion.
VANILLA Requires the Weakness effect, then converts over a randomised timer,
with a visible shaking state, preserving profession and trades.
DO Implement properly; grant the advancement on completion, not on click.
CARE Look for existing conversion machinery first (zombie to drowned, pig to
zombified piglin). The cure trade discount depends on gossip, which is in
someone else's open PR #2408 — do not build gossip.

### E2 Villagers never breed
STATE confirmed
WHERE `entity/passive/villager/mod.rs`
WHAT No `BreedGoal` in the goal set, though a generic one exists and is wired up
for cows, pigs, sheep and wolves. `eat_until_full` has zero callers, so
`food_level` accumulates and can never trigger anything.
DO Wire willingness and breeding.

### E3 Iron golems never spawn from villages
STATE absent
WHAT Only the manually built golem works. No population-driven spawn check, and
`is_panicking()` returns false for every mob.
VANILLA 1/700 per tick given enough villagers and beds, plus one per 10 further
villagers.

### E4 Villager schedules, meeting points, bell gathering
STATE absent
WHAT Only bed and job-site logic exists. No meeting point, no bell response.

### E5 can_start_at diverges from vanilla
STATE confirmed, Wave 2
WHERE `pumpkin/src/entity/ai/pathfinder/walk_node_evaluator.rs`
WHAT Three divergences: no `!= OPEN` check, uses the static default malus rather
than the mob's own, and adds a collision test vanilla does not have.
CARE **Not a drop-in.** Vanilla's `getStart` always returns a node and
`canStartAt` only picks which corner; ours returns `None` when all candidates
fail. Making it stricter without porting `getStart`'s control flow turns "pick
another start block" into "pathfinding fails outright".

### E6 is_diagonal_valid missing the large-mob early-out
STATE confirmed, small
WHERE same file
WHAT Vanilla returns false for mobs wider than 1.0 when either adjacent node has
a positive cost malus.

### E7 GoalSelector::remove_goal index fixup is wrong
STATE confirmed, currently dead code
WHAT It fixes up indices as if `Vec::remove` were used while calling
`swap_remove`, and is wrong for multiple removals. Zero call sites today.
DO Fix or delete. Do not leave a landmine for whoever first calls it.

### E8 Mob spawning
STATE suspected, HIGH PRIORITY
WHAT Not audited. Category caps, light-level rules, spawn cycles, despawn
distances (the 32/128 rules), persistence flags, structure-specific spawns,
spawn eggs, and the mob-cap accounting that `MAGIC_NUMBER = 17 * 17` feeds.
CARE Intersects #2616 (simulation distance). Audit after that merges.

### E9 Remaining mob AI
STATE suspected
WHAT 32 of 169 entity types have AI. For the ones that DO, look for the shapes
that have been productive: a predicate built and never applied, a `should_continue`
missing so a trait default silently ends the goal, goals registered at the wrong
priority, a field written every tick and never read.

---

# F. World and blocks

### F1 isRainingAt unimplemented for lightning
STATE confirmed
WHERE `pumpkin/src/world/mod.rs`, `tick_spawning_chunk`
WHAT A literal `if true` where the position check belongs.
VANILLA Four checks: raining, can see sky, heightmap not above the position, and
biome precipitation is RAIN.
DO Reuse the existing `is_raining_at` helper. Also gate on the ramped levels
(`rainLevel > 0.2`, `thunderLevel > 0.9`), not the raw booleans.
CARE **A previous attempt read the biome at `heightmap + delta.y`**, which is
0-15 blocks above the surface, and that suppresses lightning vanilla allows.
Probe the strike position itself.

### F2 Berry bush and nether wart growth rates
STATE confirmed, filed as #2582
WHERE `plant/crop/sweet_berry_bush.rs`, `plant/nether_wart.rs`
WHAT The bush applies `random_range(0..=25)` inside its `CropBlockBase` override
on top of the outer `0..5` gate, giving about 1/130 per random tick where vanilla
is 1/5. Nether wart uses `0..=10` where vanilla is `nextInt(10)`.
DO Drop the bush's inner gate; change wart to `0..10`.
CARE Leave the unexplained `is_full_cube` check above the bush alone; it is not
in vanilla but removing it changes behaviour the light check does not cover.

### F3 Area effect cloud NBT is an empty impl
STATE confirmed
WHERE `pumpkin/src/entity/area_effect_cloud.rs`
WHAT `impl NBTStorage for AreaEffectCloudEntity {}` saves and loads nothing, so a
cloud restored through generic entity dispatch reverts to constructor defaults.
Radius, duration, effects, reapplication delay and both on-use decays are lost.

### F4 Dragon breath cloud radius_on_use sign
STATE suspected
WHAT Dragon breath passes `+0.5` where the lingering potion cloud passes `-0.5`.
DO Verify against a dragon-breath-specific source before touching.

### F5 Splash potion distance falloff
STATE suspected
WHAT `splash_potion.rs` applies its own distance falloff. Splash potions do have
one in vanilla, so this is probably correct, but the exact curve is unverified.

### F6 Nylium has no block behaviour
STATE absent
WHAT No spread, no bone meal vegetation.

### F7 Bone meal infrastructure
STATE absent
WHAT There is no `BonemealableBlock` equivalent. `BONE_MEAL` appears only in
composter, bamboo, bamboo sapling, sweet berry bush and sea pickles. Huge fungi,
grass, saplings, and most crops cannot be bone-mealed.
CARE This is the single biggest absent block feature. It wants a trait, not
per-block special cases. Open PR #2518 covers the huge-fungus worldgen half.

### F8 Random tick sampling ignores the tick order guarantees
STATE suspected
WHAT After #2587 lands, re-check that the sampling matches vanilla's per-section
semantics and that scheduled ticks still run at speed 0.

### F9 World border
STATE suspected, unaudited
WHAT Damage per block beyond, warning distance and time, `/worldborder` set with
time interpolation, and whether the border affects chunk loading and spawning.

### F10 Dimensions and portals
STATE suspected, unaudited
WHAT Nether coordinate scaling, portal linking and the search radius, portal
creation and frame validation, end gateways, respawn anchors, and the End exit
portal.

---

# G. Player systems

### G1 Food and hunger
STATE suspected, unaudited
WHAT Saturation, exhaustion sources and their exact rates, natural regeneration
thresholds and rates, food poisoning, and the difference between difficulty
levels.
CARE #2578 already touched hunger costs for attacking and mining, so read it
first.

### G2 Enchanting
STATE suspected, unaudited
WHAT Table mechanics and bookshelf power, the enchantment cost formula, anvil
costs and the "too expensive" cap, enchantment compatibility rules, treasure-only
enchantments, and whether the level cost and XP cost are distinguished.

### G3 Advancements
STATE suspected
WHAT E1 shows advancement triggers are not carefully guarded: Zombie Doctor fires
on any golden-apple click. Audit every trigger for a similar unguarded fire.
DO This is a high-yield audit because the failure is silent and player-visible.

### G4 Scoreboards and teams
STATE suspected, unaudited

### G5 Experience
STATE suspected
WHAT Orb merging, the level-to-XP curve, what drops on death and the 100-point
cap, and whether mending consumes orbs correctly.

---

# H. Infrastructure and correctness

### H1 Chunk codec has no tests
STATE confirmed
WHAT There is no test coverage for chunk save/load. A3 cannot be done safely
without it, and #2612 touched this area on reasoning alone.
DO Build round-trip tests: write a chunk, read it back, assert equality across
block states, light, block entities and heightmaps.
CARE This is the highest-leverage item in the whole queue. It unblocks A3 and
makes every future chunk change verifiable.

### H2 Startup robustness sweep
STATE suspected
WHAT #2612 fixed two `unwrap`/`expect` calls that aborted startup on recoverable
IO. There are likely more.
DO Grep the startup path for `unwrap`, `expect` and direct panics on IO, config
parsing, and world loading. Report a list; each is a small independent fix.

### H3 Concurrency audit of tick-path state
STATE suspected
WHAT Two real findings already: the hopper `fetch_sub` off-by-one, and a proposed
downgrade of an atomic RMW to load/store that would have lost a neighbouring
hopper's write.
DO Enumerate every `AtomicX` and `Mutex` touched from `tick`, and check each for
lost-update races and lock ordering.
CARE `tick_environment` holds the weather lock across awaits, establishing a
weather then level_time order. Anything taking weather while holding level_time
deadlocks.

---

# I. Standing rules, repeated because you have no memory between runs

1. No emojis anywhere. Maintainers flag them for expedited closure.
2. No em-dashes in code or comments.
3. Never downgrade an atomic read-modify-write to a load/store pair.
4. One logical fix per branch. Out-of-scope bugs get reported, not folded in.
5. Match surrounding style. Do not reformat untouched code.
6. Anchor SEARCH blocks on a few unique lines, never a whole function.
7. If an item says `suspected` or its VANILLA line says UNVERIFIED, report and
   stop. Do not write code.
8. Before starting anything, check upstream for an existing issue or PR:
   `gh issue list --repo Pumpkin-MC/Pumpkin --search "<kw>" --state all`
   and the `gh pr list` equivalent. #2408 and #2518 are active work by others;
   stay off villager trading and huge-fungus worldgen.

---

# J. Coverage inventory, blocks and block entities

From a top-down audit of vanilla block behaviour against the ~158 of 1196
blocks that have behaviour code. Highest-value rows only; the full 140-row
inventory is in the session record.

### J1 Saplings never grow into trees at all
STATE confirmed
WHERE pumpkin/src/block/blocks/plant/sapling.rs:29
WHAT `generate()` advances stage 0 to 1 and then hits a bare `//TODO generate tree`.
No sapling ever becomes a tree, which silently breaks bone meal on saplings,
tree farms, and anything downstream of forestry.
CARE This is probably the single biggest silent gap in the block layer. It needs
the tree feature invoked from the block path, so check what
pumpkin-world/src/generation/feature/features/tree/ already exposes before
designing anything.

### J2 Piston loses the block it is carrying across a save
STATE confirmed
WHERE pumpkin/src/block/entities/piston.rs:281-320
WHAT `from_nbt` hardcodes `pushed_block_state = Block::AIR.default_state`, and
neither `write_nbt` nor `chunk_data_nbt` serialise the real pushed block. Three
separate `// TODO: pushed_block_state` comments mark it. A piston mid-extension
whose chunk is saved and reloaded loses the identity of the block it carries.
SEVERITY Data corruption. This is the highest-severity block row.

### J3 Explosions have no per-block hook
STATE confirmed
WHERE pumpkin/src/block/mod.rs:92-94, only tnt.rs overrides
WHAT `explode()` is a default no-op that only TNT overrides, so any block needing
bespoke explosion behaviour has no way to express it. Beds and respawn anchors
exploding outside their home dimension have nowhere to live.

### J4 Self-documented rail power bug
STATE confirmed
WHERE redstone/rails/activator_rail.rs:25-36 and powered_rail.rs:25
WHAT The code comments describe the bug itself: a redstone source mid-chain
extends power along powered and activator rails instead of stopping at the last
powered rail. The same TODO is copy-pasted into both files.
CARE Cheap and high confidence, since the author already wrote out the wrong
behaviour.

### J5 Daylight detector output is an admitted placeholder
STATE confirmed
WHERE pumpkin/src/block/entities/daylight_detector.rs:72-89
WHAT `update_power` is labelled "THIS IS A PLACE HOLDER... fake implementation
using the time of day... doesnt consider weather", because the light engine has
no ambient-darkness equivalent. Rain and thunder do not dim it.
CARE Blocked on the same skyDarken gap as A2.

### J6 Small mushrooms spread without the vanilla light gate
STATE confirmed
WHERE plant/mushroom_plant.rs:44
WHAT `// TODO: Check light level and isOpaqueFullCube`. Mushrooms spread in
conditions vanilla forbids.

### J7 Game events are never emitted from several blocks
STATE confirmed
WHERE redstone/bell.rs:44, redstone/tripwire_hook.rs:330-340,
block/entities/shulker_box.rs:114,125
WHAT Bells, tripwire hooks and shulker boxes all have TODO'd `emitGameEvent`
calls, so sculk sensors cannot detect any of them. One coherent change.

### J8 Tripwire ignores mobs that should avoid traps
STATE confirmed
WHERE redstone/tripwire.rs:106-142
WHAT `entity.canAvoidTraps()` is a bare TODO, so foxes and cats trigger tripwire
when vanilla exempts them. Shears also take no durability when cutting it.

### J9 Blocks with entities but no registered behaviour
STATE absent
WHAT Ten block entities exist with no `blocks/*.rs` registering behaviour:
calibrated sculk sensor, creaking heart, vault, trial spawner, copper golem
statue, decorated pot, structure block, test block, test instance block.
CARE One coarse row. Each needs its own design; several are 26.x features.

### J10 Entirely absent blocks with prior closed attempts
STATE absent
WHAT Respawn anchor, lodestone, scaffolding, bubble column, cocoa, turtle egg,
frogspawn, sculk and sculk vein all have zero registered behaviour. Several have
closed-unmerged PRs (#1947 scaffolding, #1260 cocoa, and five separate bubble
column attempts), so this is stalled work rather than oversight.
CARE Read the closed PRs and their reviews before reattempting any of these.

---

# K. Coverage inventory, world generation

### K1 Carvers are hardcoded per dimension, not per biome
STATE confirmed
WHERE pumpkin-world/src/generation/carver/mod.rs:26-27,189-190
WHAT `OVERWORLD_CARVERS` and `NETHER_CARVERS` are fixed lists applied to every
chunk in the dimension. Vanilla assigns carvers per biome through the biome's
own `carvers` list, and the code comment admits the divergence.
CARE Clearest worldgen parity bug found. The Biome struct has no carvers field
at all, so this needs a data change as well as a code change.

### K2 Chunk light is never persisted
STATE confirmed
WHERE pumpkin-world/src/chunk/format/anvil.rs, zero matches for SkyLight,
BlockLight or isLightOn
WHAT The chunk codec writes no light tags whatsoever. This is the root of the
vanilla-imported-worlds-go-dark problem in A3, and it is broader than that row
assumed.
SEVERITY Data corruption.

### K3 Only three of six heightmaps are persisted
STATE confirmed
WHERE pumpkin-world/src/chunk/mod.rs:155-158
WHAT WorldSurface, MotionBlocking and MotionBlockingNoLeaves are tracked.
OceanFloor, OceanFloorWg and WorldSurfaceWg are missing. Anything needing ocean
floor height silently falls back to a heightmap that differs in water columns.

### K4 No chunk-content round trip test
STATE confirmed
WHAT Tests cover region-file container mechanics and the alternate Linear
format's internals, but nothing serialises a full ChunkData with blocks,
biomes, heightmaps and block entities and reads it back.
CARE This is the highest-leverage infrastructure row in the whole queue. K2 and
K3 cannot be fixed safely without it.

### K5 Structure and jigsaw coverage is much better than expected
STATE confirmed, no action
WHAT Dedicated piece systems exist for stronghold, nether fortress, mansion,
monument, mineshaft, ocean ruin, ruined portal with seven variants, and
shipwreck, plus a generic jigsaw engine driving village, bastion, ancient city,
outpost and trial chambers, with a real vanilla-weight regression test for
ancient city.
CARE Recorded so nobody re-audits this ground. The weak spot is that only
ancient city has a pool-weight test; village, outpost, bastion and trial
chambers have none.

### K6 Untested numeric paths in worldgen
STATE suspected
WHAT Aquifer sampling, ore vein thresholds and `find_nearest_structure` all have
working code and zero regression tests against known-seed vanilla output. The
ore vein constants 0.7 and -0.3 look right but were never diffed against source.


# L. Capability audit, vanilla server surface vs Pumpkin

Method: enumerated the full vanilla server capability surface (networking,
world, blocks, entities, items, player systems, commands, datapacks, admin),
then probed the codebase per capability with content-level verification, not
just file counts. Only confirmed gaps are listed. Areas verified PRESENT and
therefore NOT queued: GS4 query protocol (net/query.rs), RCON, sleep and
night skip with playersSleepingPercentage, /item, /tick, /transfer, /dialog,
recipe book handlers, boats, minecarts, fishing bobber, trial spawner and
vault block entities, bossbar, titles, whitelist, bans, op levels.

### L1 No datapack function layer at all
STATE confirmed, zero matches for mcfunction anywhere
WHERE no pumpkin/src/command/commands/{function,schedule,datapack}.rs
WHAT No .mcfunction parsing, no /function, /schedule, /return, /datapack,
no minecraft:load or minecraft:tick function tags, and /reload only exists
for whitelist and plugin hot swap, not packs. Loot tables and predicates
ARE loaded, so the datapack directory plumbing partially exists.
CARE Groundwork item: a function file is just newline-separated commands
run through the existing dispatcher at permission level 2 with a shared
command source. Start with parse plus /function plus load/tick tags; the
scheduler and /return can come later. This unlocks a huge amount of vanilla
map and pack compatibility in one move.

### L2 /locate and /place missing, structure lookup is single-purpose
STATE confirmed
WHERE only ender_eye.rs and proto_chunk.rs touch structure lookup
WHAT No /locate (structure, biome, poi) and no /place. The eye of ender
already locates strongholds, so a nearest-structure query exists in embryo
but is not generalised or exposed.
CARE Groundwork: generalise the stronghold search into a
nearest_structure(kind, origin) API on the world, then /locate is a thin
command over it and explorer maps (L5) reuse it.

### L3 Book editing has no packet handler
STATE confirmed, no handler for the edit-book serverbound packet in play.rs
WHAT Writable books cannot be written, signed, or copied. Lecterns (which
have behaviour code) have nothing to display from player-authored books.
CARE Check pumpkin-protocol for the SEditBook definition first; the packet
may already be decoded and simply unrouted.

### L4 No /debug or /perf profiling commands
STATE confirmed
WHAT No tick-profiling output for diagnosing the very perf work this queue
generates. Low priority but cheap: /tps exists, so plumbing for tick timing
is already collected somewhere.

### L5 Filled-map rendering pipeline is partial
STATE partial, needs verification depth
WHERE pumpkin/src/item/items/map.rs, map_item_data.rs, server map_id counter
WHAT Packet and id allocation exist. Unverified: terrain-to-color rendering,
map_state persistence to data/map_N.dat, scaling and zoom at the cartography
table, banners and decorations, and explorer maps (blocked by L2).
CARE Verify before building: hold a map in-game via the RCON rig and see
whether anything renders. Then queue the missing stages as separate items.

# M. Capability audit, multiplayer difficulty and village systems

### M1 Raids do not exist
STATE confirmed, zero raid types outside a bell listener
WHAT No raid manager, no Bad Omen trigger on captain kill, no wave
composition, no raid bossbar, no Hero of the Village. All raider mobs
(pillager, ravager, evoker, vex, witch, vindicator) already have entity
implementations, so this is a state machine over existing pieces.
CARE Groundwork order: Bad Omen effect wiring first (D-section effect
work overlaps), then village detection via POI, then the wave spawner.
Do not start until villager POI work (E4) has landed something usable.

### M2 Pillager patrols never spawn
STATE confirmed
WHAT The periodic patrol spawn attempt (every 10 min, outside villages,
leader with banner) is absent. Pillager entity and ominous banner exist.

### M3 Zombie sieges never happen
STATE confirmed
WHAT Midnight village siege attempts are absent. Depends on the same
village detection as M1, so build after it, not in parallel.

### M4 Regional (local) difficulty does not exist
STATE confirmed, zero matches, only a Peaceful check on the global value
WHAT Vanilla scales mob armor and weapon pickup, spider effects, zombie
reinforcement chance, and more by chunk inhabited time. Pumpkin spawns
every mob at flat difficulty. inhabited_time IS tracked in chunk data,
so the input exists; the ClampedRegionalDifficulty computation does not.
CARE Groundwork: one function, clamped_regional_difficulty(chunk), then
individual mob spawn paths adopt it incrementally.

### M5 Weather never freezes water or lays snow
STATE confirmed for the runtime path
WHAT freeze_top_layer exists only as a worldgen feature. During snowfall
at runtime, water does not freeze and snow layers do not accumulate on
random tick. Cauldrons probably do not fill in rain either; verify while
in there. The precipitation check (is_raining_at area, F1) gates all of
this, so land F1 first.

# N. Capability audit, admin and compatibility

### N1 Spawn protection is not implemented
STATE confirmed, only a translation-key constant exists
WHAT No config field, no build-permission check near spawn for non-ops.
Small, self-contained, good local-model candidate once specced with exact
assertions.

### N2 Old vanilla worlds are refused, not upgraded
STATE confirmed by design
WHERE pumpkin-world/src/world_info/anvil.rs version-range check
WHAT Anything outside MIN..=MAX supported DataVersion is rejected. Vanilla
runs DataFixerUpper. A full DFU port is out of scope; the realistic item is
a clear error message telling users to load the world once in vanilla to
upgrade it, plus widening the supported range after each snapshot bump.
CARE Do not let anyone start a DFU rewrite from this row.

### N3 Player idle timeout is scaffolded but unverified
STATE partial
WHERE server/mod.rs player_idle_timeout field, KickedForIdling reason
WHAT Field and kick reason exist, /setidletimeout exists. Unverified that
activity actually resets the timer and the kick fires. Cheap RCON-rig test.

---

# Z. Routing: who does what

The local agent is reliable for ONE shape of task and unreliable for everything
else. Measured, not assumed:

| Task shape | Result |
|---|---|
| One line, exact target line supplied in the spec | correct, ~2s, first try |
| Multi-line diff over a narrow region | applies, semantically wrong |
| Multi-line diff over a whole function | fails to apply at all |

So route by shape, not by difficulty or by subsystem.

## Z1 Local agent, line-edit mode

Eligible only if ALL of these hold:
- the whole fix is one line
- that exact line can be written into the spec's `TARGET:` field verbatim
- the line is unique in its file
- the correct replacement is already known and verified, so the spec states it
  rather than asking the model to work it out
- a paired `ASSERT_ABSENT` and `ASSERT_PRESENT` can express success exactly

Spec format for this mode:

    WORKTREE: /home/eshanki/pumpkin-wt/<name>
    FILES: path/to/file.rs
    CRATES: pumpkin
    TARGET: <the exact line, indentation included>
    ASSERT_ABSENT: <the old fragment>
    ASSERT_PRESENT: <the new fragment>
    ---
    One paragraph: what to change and what to leave alone.

DONE so far in this mode: F2 nether wart half, `0..=10` to `0..10`. Verified,
built clean, correct.

## Z2 Claude, everything else

Anything needing new control flow, a new function, a new field, reasoning across
lines, or a judgement about which of several vanilla behaviours applies. That is
the large majority of this queue, including every row in J, K, L, M and N.

The failure that defines the boundary: asked to REMOVE a redundant gate, the
local agent retuned it instead, changing `0..=25` to `0..5` so the gate then
applied twice. It compiled, satisfied a narrow assertion, and was still wrong.
A build gate cannot catch wrong-thing, only did-nothing.

## Z3 Atomic candidates that need verification before they can be specced

Found by grepping for inclusive ranges. Each MIGHT be an off-by-one, and each
needs its vanilla counterpart confirmed before a spec is written. Do NOT hand
these to the local agent until the correct value is known and stated.

- `world/loot.rs:145` `random_range(0..=(enchantment_level + 1))`. The wiki says
  Looting raises the maximum common drop by one per level, which suggests
  `0..=level`, one lower than this. But line 140 nearby uses
  `enchantment_level * bonus_multiplier` and the two are almost certainly
  different vanilla loot functions, plausibly `uniform_bonus_count` versus an
  ore-drops or binomial form. Identify which function each line implements
  before touching either.
- `world/loot.rs:140` `random_range(0..=(enchantment_level * *bonus_multiplier))`.
  Same caveat.
- `plant/bamboo.rs:127` `random_range(0..=3)`. Vanilla bamboo growth chance
  unverified.
- `ai/goal/pick_up_block.rs:52` and `ai/goal/place_block.rs:56`, both
  `random_range(0..=2)` on a Y offset. Vanilla range unverified.

## Z4 Verified CORRECT, do not "fix"

Recorded so nobody wastes a cycle on them:
- `plant/crop/mod.rs:59` and `plant/crop/gourds/stem.rs:87`, both
  `random_range(0..=(25.0 / f).floor())`. This matches vanilla's
  `nextInt((int)(25.0F / f) + 1)` exactly. The inclusive range is correct here.
- `plant/crop/sweet_berry_bush.rs:202` `random_range(0..=25)` is wrong, but the
  fix is to DELETE the gate, not retune it, because the caller already applies
  vanilla's only gate. This is a line deletion and therefore NOT line-edit
  eligible; the local agent has already got it wrong twice.
