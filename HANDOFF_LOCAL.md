# Local parity campaign handoff

Keep this file local. Do not stage or commit it.

## Pinned oracle and tooling

- User-supplied bundle: `/home/eshanki/Downloads/server.jar`
- Minecraft version: 26.2
- Protocol: 776
- World/data version: 4903
- Bundle SHA-256: `cdacdfb25898de5e4b4b0e5ddcc2722f77067e46605709c2d886c000ebb63ec5`
- Nested server payload SHA-256: `183c0499c5f855570ee487dd38e141a53f0121f83a0b07a3bac2d8b6698823e8`
- Extracted payload: `/tmp/pumpkin-vanilla-26.2/server-26.2.jar`
- Decompiled mapped sources: `/tmp/pumpkin-vanilla-26.2/decompiled`
- Temporary Java 25: `/tmp/pumpkin-vanilla-26.2/jdk25`
- Vineflower 1.12.0: `/tmp/pumpkin-vanilla-26.2/vineflower-1.12.0.jar`

The `/tmp` artifacts are not durable across reboot. Recreate them from the bundle if
needed. The repository roadmap and JAR quirk backlog retain hashes and audit results.

## Standing implementation rules

Read `AGENTS.md`, `CLAUDE.md`, `ROADMAP_26_2_PARITY.md`, `JAR_QUIRKS_26_2.md`, and
`PARITY.md` before assigning work. `PARITY.md` is partially stale; trust executable
tests and current source first.

Routine implementation workers must use the lower-cost Terra tier at low reasoning.
Use Sol/high only for architecture, difficult source audits, or review of concurrency,
protocol, and data-corruption work. Every worker task must name the closest Pumpkin
analogue to read first and the exact decompiled vanilla class/method.

## Commits added in this session

Starting after `cc5d51e`:

- `dd9648d` preserve stronger active status effects
- `a830d32` honor configured simulation distance
- `b898895` stone pressure-plate entity filtering
- `bd5f744` flat/reapplying lingering potion clouds
- `7e68706` strict-lint cleanup for parity helpers
- `d6c60f2` milk-bucket consume effects and remainder
- `0be39ae` Minecraft 26.2 parity roadmap
- `fbd7489` distinguish empty from charged crossbows
- `5041a01` JAR-derived quirk backlog
- `6962def` correct advance-weather gating
- `b5a4bd2` keep PvE attacks enabled when PvP is disabled
- `fd09679` apply random-tick-speed gamerule
- `97e2fc8` ignore stale/duplicate teleport confirms
- `1402ec7` per-recipe fractional furnace XP rounding
- `d0136f2` transmute matching and crafting remainder handler changes
- `53d8b8b` enable generated recipe-remainder data for inventory crate
- `931c96d` spawn aquatic entities from mob buckets
- `c4fbcbb` standing agent implementation guidelines

Do not rewrite shared history merely to split `d0136f2`/`53d8b8b`; the tree is
functional and the changes overlap the same handler. Preserve them as an ordered pair.

## Current uncommitted work

- `pumpkin-world/src/chunk/format/anvil.rs`: M3 world-I/O fix in review. Chunk and
  entity-region writers advertised stale data version 4790 (26.1.2), while the
  supported 26.2 level format is 4903. `WORLD_DATA_VERSION` now derives from
  `MAXIMUM_SUPPORTED_WORLD_DATA_VERSION`; a regression test asserts the invariant.
  Focused test and strict `pumpkin-world` Clippy passed. Do not commit until the
  coordinator reviews the exact single-file diff.
- `pumpkin/src/net/java/play.rs`: combat authority fix in review. Both Java attack
  packet routes now reject targets beyond vanilla's default 3.0/5.0 weapon reach,
  plus the listener's 3.0 buffer and 0.3 hitbox margin, measured from the eye to the
  nearest target bounding-box point. Direct attacks also ignore spectators as vanilla
  does. Local source evidence: `ServerGamePacketListenerImpl.handleAttack` and
  `Player.isWithinAttackRange`; `AttackRange.isInRange`. Two focused tests and strict
  Pumpkin Clippy passed. Do not commit until coordinator review.
- `pumpkin-inventory/src/crafting/crafting_screen_handler.rs`: M5/M6 transmute
  component-preservation fix in review. `RecipeResult` now carries an optional cloned
  input component patch for `CraftingTransmute`, and `ResultSlot::refill_output`
  applies it to the result. Vanilla evidence: `TransmuteRecipe.computeResult` calls
  `createWithOriginalComponents`, which applies the input component patch. Four focused
  transmute tests and strict inventory Clippy passed. Do not commit until coordinator
  review.
- `pumpkin/src/world/mod.rs`: M7 deterministic tick-order fix in review. The three
  vanilla chunk tick phases now execute serially in collected order: scheduled blocks,
  scheduled fluids, then random ticks. This follows `ServerLevel.tick`,
  `ServerLevel.tickBlock`, `ServerLevel.tickFluid`, and `ServerLevel.tickChunk` in the
  pinned 26.2 decompilation. The focused Tokio regression and strict `pumpkin`
  `--all-targets` Clippy pass; package formatting and `git diff --check` pass.
- `pumpkin-world/src/tick/{mod,scheduler}.rs`, `level.rs`, and `chunk/format/mod.rs`:
  M3 scheduled-tick persistence fix in review. The fixed 256-slot timing wheel is now
  an absolute signed-time ordered queue, preserving imported `t=300` rather than
  wrapping it and reconstructing equal-priority loaded list order. It follows vanilla
  `SavedTick.unpack`, `ScheduledTick`, and `LevelChunkTicks.pack`. Three focused
  scheduler regressions, the full `pumpkin-world` library suite (99 tests), strict
  `pumpkin-world --all-targets` Clippy, package formatting, and `git diff --check`
  pass. Do not combine it with the concurrent inventory or attack changes.
- `pumpkin-protocol/src/java/client/play/{chunk_data,light_update}.rs`: M4 canonical
  light-packet fix in review. Initial chunk data and incremental light updates now share
  the same padded section masks (physical section zero is bit one; both padding sections
  are empty) and transmit the native low-nibble-first `DataLayer` bytes unchanged.
  Vanilla evidence: `ClientboundLightUpdatePacketData.prepareSectionData` iterates the
  engine's padded light-section indices and copies `DataLayer.getData()` directly. The
  focused mask test and strict `pumpkin-protocol --all-targets` Clippy pass.
- `pumpkin/src/net/java/play.rs`: belongs to the concurrent attack-validation worker;
  do not include it in the deterministic tick-order change.

## Verification state

The workspace suite started from `c4fbcbb` and passed all unit and doc tests, including:

- 172 `pumpkin` tests
- 95 `pumpkin-world` tests
- 64 `pumpkin-util` tests
- 41 `pumpkin-protocol` tests
- 32 `pumpkin-nbt` tests
- 26 `pumpkin-data` tests
- 9 `pumpkin-codecs` tests
- 5 new `pumpkin-inventory` tests

Strict Clippy passed after the final source commit for both `pumpkin --all-targets` and
`pumpkin-inventory --all-targets` with `RUSTFLAGS="-D warnings"`. The first combined
Clippy run found four mechanical issues: test modules before later items, two helpers
eligible for `const`, and missing backticks around `PvP`; these were corrected in
`1c6c7d0`.

The final release build completed successfully. Executable:

`/home/eshanki/Pumpkin/target/release/pumpkin`

- Size: 78,306,352 bytes
- SHA-256: `3355739fbf17580e1200ca07ef827bc72e7cc1b0c5c624601026c2b47329cbd1`

A clean startup smoke under `/tmp/pumpkin-release-smoke` loaded every world and logged
the server running on Java port 25565 and Bedrock port 19132. A scripted `stop` was
then sent; the PTY session subsequently closed.

Final commands completed successfully:

```bash
cargo test --workspace --no-fail-fast
RUSTFLAGS="-D warnings" cargo clippy -p pumpkin --all-targets
RUSTFLAGS="-D warnings" cargo clippy -p pumpkin-inventory --all-targets
cargo build --release -p pumpkin
```

Then record `sha256sum target/release/pumpkin`, run a clean temporary startup smoke,
and generate ordered patches from `cc5d51e..HEAD`.

An ordered feature-only patch set has been generated under `parity-patches/`. It omits
roadmap/audit/agent/handoff material. Follow `parity-patches/series`; do not apply the
files by alphabetical glob because each individually generated patch starts with 0001.

## Next implementation queue

User explicitly reprioritized toward the highest-value changes that maximize normal
vanilla behavior. Do not report a numeric parity percentage until M0 has measured
coverage; the current state is clearly below the roadmap's 95% claim threshold because
multiple critical strata have no complete oracle coverage and known failures remain.

Prioritize correctness/data safety before feature breadth:

1. Build the differential conformance runner (roadmap M0).
2. Fix data version 4790/4903 drift and lossless unknown-NBT round trips.
3. Serialize scheduled/block/fluid/random ticks deterministically and serially.
4. Unify chunk and incremental light serialization/nibble order.
5. Add authoritative attack reach/border/target validation.
6. Add server-side movement collision/rate correction.
7. Preserve transmute/decorated-pot components.
8. Implement generic consumable effects and tipped-arrow payloads.
9. Complete comparator notifications, fluid containers, precipitation, and lightning.
10. Complete inventory transaction property tests before more container breadth.

Use `JAR_QUIRKS_26_2.md` for minimized scenarios and exact vanilla evidence. Add every
new discovery to the conformance matrix before implementing it.

## GitHub publication

Before publication, use `gh auth status`, inspect current upstream PRs for duplicates,
and inspect branch protection. Follow `CLAUDE.md` branch/PR hygiene unless the user has
explicitly authorized another safe publication method. Never force-push upstream main.
Use the configured commit identity:

`Eshan I. <79593316+eshankiyer@users.noreply.github.com>`

GitHub CLI is authenticated as `eshankiyer`, but `gh repo view` reports only `READ`
permission on `Pumpkin-MC/Pumpkin`; direct upstream commits are impossible. Publish
minimal branches to `eshankiyer/Pumpkin` and open PRs against upstream. Existing open
PRs already cover simulation distance (#2616), stone plates (#2617), area clouds
(#2619), effect merging (#2620), and below-world skylight (#2629). Do not duplicate
them. Random-tick-speed PR #2587 is closed, so a fresh tested PR may be appropriate.

Keep feature PRs free of `ROADMAP_26_2_PARITY.md`, `JAR_QUIRKS_26_2.md`, `AGENTS.md`,
this handoff, downloaded JDK/decompiler artifacts, and decompiled Mojang sources.

Current-upstream PR worktrees prepared from `origin/master` at `75994f3`:

- `/tmp/pumpkin-pr-milk`, branch `fix/milk-consume-effects`, commit `1cc24e8`
- `/tmp/pumpkin-pr-crossbow`, branch `fix/crossbow-empty-charge`, commit `d5b67e5`
- `/tmp/pumpkin-pr-random-tick`, branch `fix/random-tick-speed-26-2`, commit `f3fbaba`
- `/tmp/pumpkin-pr-teleport`, branch `fix/stale-teleport-confirms`, commit `20772a2`

Milk and teleport cherry-picks conflicted with newer upstream code. Resolutions preserved
upstream behavior and removed unrelated effect-merge/PvP changes. The branches were
pushed to `eshankiyer/Pumpkin` and these minimal upstream PRs were opened:

- #2663 milk bucket consumption
- #2664 crossbow empty-charge handling
- #2665 random-tick-speed gamerule
- #2666 stale teleport confirmations
- #2667 consistent Minecraft 26.2 chunk/entity-region data version
- #2668 serial scheduled/random chunk-tick phases
- #2669 authoritative attack-packet reach validation
- #2670 lossless scheduled-tick timing and ordering
- #2671 non-finite player-position rejection
- #2672 strict transmute matching and input-component preservation
- #2673 canonical initial/incremental light serialization
- #2674 one-packet authoritative movement-rate correction
- #2675 piercing arrows track unique targets
- #2676 comparator output notifications
- #2677 data-driven consumable status effects
- #2678 PvP-off still permits PvE attacks
- #2679 melee AI requires line of sight
- #2680 placed firework rockets consume the stack
- #2681 melee damage includes shield-facing source positions
- #2682 water spreads into dry waterloggable blocks

Milk and crossbow passed focused tests, formatting, diff checks, and strict Clippy in
isolated target directories. Random tick passed its 3 focused tests, formatting, and
diff check; its branch-specific strict Clippy was delayed by Cargo target locking, while
the same source had already passed strict Clippy on the integration branch. Teleport
passed formatting/diff review and was confirmed free of unrelated PvP/debug changes;
its final focused Cargo validation was likewise delayed by concurrent target locks.

PR #2667 is the first high-value M3 persistence fix: it replaces the stale Anvil
`DataVersion` 4790 with the shared supported 26.2 value 4903 for both chunk and entity
region serializers. Its focused test and strict `pumpkin-world --all-targets` Clippy
passed before publication.

PR #2668 removes concurrent execution of dependent scheduled block, scheduled fluid,
and random tick phases, matching vanilla phase/collection ordering; its upstream-base
focused test passed. PR #2669 adds eye-to-nearest-AABB attack reach checks to both Java
attack routes plus Spectator rejection; its two upstream-base boundary tests passed.
Both had previously passed strict all-target Clippy on the integration branch.

PR #2670 replaces the wrapping 256-slot scheduled-tick wheel with absolute ordered
trigger times, preserves signed NBT delays and loaded sub-tick order, and passed all 99
`pumpkin-world` library tests plus strict all-target Clippy. PR #2671 applies vanilla's
finite-coordinate validation to both movement packet variants; its focused test passed.

PR #2672 is based directly on current upstream and contains no crafting-remainder work;
it fixes input/material classification and carries the input component patch into the
transmuted result. Its 3 focused tests and strict inventory Clippy passed. PR #2673
shares padded light masks between initial/incremental packets and removes erroneous
nibble-byte rotation; its focused test and strict protocol Clippy passed.

PR #2674 adds the available vanilla movement-speed gates to both position handlers and
documents that first-good position/per-tick packet scaling remain future work. Its
upstream-base focused test passed. Integration commit: `ffaf2c5`; minimal PR commit:
`6f7766d`.

PR #2675 tracks pierced entity IDs and allows exactly piercing-level plus one unique
targets; focused integration/upstream tests pass. PR #2676 notifies direct and
through-conductor comparators for block and dirty block-entity output changes; its
focused upstream-base test passes. PR #2677 replaces hardcoded golden-apple effects
with generated consumable-component effects while preserving potion/clear paths; its
focused upstream-base test passes.

PR #2678 applies the PvP toggle only to player targets. PR #2679 gates melee AI attacks
on the existing eye-to-eye world raycast. PR #2680 consumes successfully placed rockets
outside Creative and has a focused survival/Creative test. Firework elytra-use
consumption and payload/lifetime/damage remain separate unimplemented paths.

PR #2681 wires existing shield-direction logic for player, sweep, and mob melee damage.
PR #2682 preserves dry waterloggable hosts during water spread and schedules contained
water; its focused water/lava test passes.

Decorated-pot component preservation is implemented and committed locally as `bb52fe4`.
The official code generator was run from `pumpkin-codegen`; broad nondeterministic
Bedrock/WIT/lockfile churn was discarded, retaining the exact PotDecorations generated
hunk. The focused crafting test and strict data/inventory Clippy pass. Publication is
deferred until PR #2672 lands because both recipes require the same new `RecipeResult`
component-patch plumbing; do not open a duplicate stacked diff against master.

Integration commits: `7006d01` (serial ticks) and `5f8d851` (attack reach). Their
minimal current-upstream PR commits are `75da74e` and `513fe86`, respectively.

## Active local-only movement validation

The integration worktree contains an uncommitted, isolated movement validation fix in
`pumpkin/src/net/java/play.rs`: `handle_position` previously rejected only NaN, while
vanilla 26.2 `ServerGamePacketListenerImpl.handleMovePlayer` calls
`containsInvalidValues` before coordinate clamping. The new `has_finite_position`
helper makes both position packet variants reject positive/negative infinity as well;
its unit test covers finite, NaN, and both infinity signs. The diff is limited to that
file and must be kept separate from all local audit artifacts. Formatting and
`git diff --check` pass. Focused test is compiling in isolated
`target/validate-movement`; run the focused test and strict Pumpkin Clippy before
committing or publishing.

## Active local-only movement rate correction

An additional uncommitted `pumpkin/src/net/java/play.rs` change implements the available
single-packet part of vanilla 26.2 movement-rate correction. It compares squared client
displacement minus authoritative velocity squared against vanilla's 100 normal / 300
fall-flying thresholds, corrects with `force_tp` before state/event changes, and applies
the available vanilla gates: normal tick-rate execution, `player_movement_check`,
`elytra_movement_check`, and non-sleeping player state. Exact vanilla references are
`ServerGamePacketListenerImpl.handleMovePlayer` lines 1093–1119 and
`shouldCheckPlayerMovement` lines 1206–1219 in the local decompilation. Pumpkin lacks
vanilla `firstGood*`, per-tick move-packet counters, and dimension-transition state, so
the implementation is deliberately documented as a one-packet baseline rather than
claiming full movement parity. It has a focused decision-table test, and formatting plus
`git diff --check` pass. The test/Clippy run is currently blocked before compiling this
file by unrelated `pumpkin-data` errors: duplicate `Cow` import, invalid unit-like use
of `PotDecorationsImpl`, and `NbtTag::String` expecting `Box<str>`.

## Active local-only generic consumable effects

An uncommitted `pumpkin/src/entity/living.rs` change replaces hardcoded golden-apple
effect application with a shared data-driven `apply_consumable_effects` hook at the
common completion point. It handles `ConsumableImpl::ApplyEffects` using vanilla's
strict `random < probability` semantics, resolves configured status effects, preserves
instance fields, and delegates to `LivingEntity::add_effect`. Clear-all and potion
paths remain in place. Vanilla evidence is local only:
`Consumable.onConsume` lines 77–89 and `ApplyStatusEffectsConsumeEffect.apply` lines
49–60. The focused probability-boundary test passed after correcting the Cow slice
iteration. Formatting and diff checks pass. Strict Pumpkin Clippy was started in
`target/validate-consumables`; record its final result before commit.

## Active local-only melee line-of-sight gate

An uncommitted isolated change to `pumpkin/src/entity/ai/goal/melee_attack.rs` makes
melee attacks raycast from the mob eye to the target eye and refuse attack when a solid
block outline intersects the path. It reuses `World::raycast` and the established
Enderman visibility predicate. Vanilla evidence is local:
`MeleeAttackGoal.canPerformAttack` line 144 requires `getSensing().hasLineOfSight`.
Formatting and diff checks pass. A focused build/test is running in
`target/validate-melee-los`; an integration fixture with a constructed server/world is
still needed for a direct wall-blocking regression beyond compilation.

## Active local-only shield source-position fix

An uncommitted change supplies the attacker position in the existing damage context for
primary player melee, player sweep damage, and mob melee. `LivingEntity` already applies
shield direction checks only when this optional position is present; these sources had
all passed `None`, making ordinary melee unblockable. Vanilla reference is
`LivingEntity.applyItemBlocking`, which obtains `DamageSource.getSourcePosition()` for
the view-angle calculation. Formatting/diff checks pass; a focused compile is running
under `target/validate-shield-source`.

## Active local-only hurt-cooldown domain correction

An uncommitted one-line `pumpkin/src/entity/living.rs` correction makes
`last_damage_taken` retain the full post-reduction `effective_amount` after every
accepted hit. Previously a stronger hit within the cooldown stored only its incremental
damage, then incorrectly allowed a later weaker hit. Vanilla 26.2
`LivingEntity.hurtServer` compares against and stores the same full post-blocking
`damage` value (`lastHurt`). Formatting and diff checks pass; add a direct two-hit
cooldown regression before publication if a lightweight LivingEntity fixture becomes
available.

## 2026-07-31 14:00 EDT checkpoint (supersedes the stale "active" sections above)

The movement non-finite/rate, generic consumable, melee LOS, shield source-position,
waterlogging, and hurt-cooldown changes described above are no longer uncommitted. The
integration worktree is clean except for this local handoff and `parity-patches/`.

Published since the prior checkpoint:

- PR #2683 / integration commit `b7a1f6a`: preserve the full post-reduction damage as
  the hurt-cooldown baseline. Full workspace tests and strict Pumpkin Clippy pass on
  the integration branch. The isolated current-master worktree encountered unrelated
  missing generated WIT bindings; upstream CI is queued.
- PR #2684 / integration commit `9ffaa59`: consume the main- or offhand firework used
  for an elytra boost outside Creative. The focused firework test, formatting, and
  diff checks pass; upstream CI is queued. Payload copying, component-derived lifetime,
  explosion damage, and attached-rocket lifecycle parity remain open.

PR #2674's amended movement-rate commit now passes formatting, spelling, dependency,
and both strict lint jobs; cross-platform build/test jobs are still running.

Current local validation baseline after `9ffaa59`:

- `cargo test --workspace --no-fail-fast`: pass before the final two focused changes
  (Pumpkin 182 tests, world 99, protocol 42, util 64, all other workspace/doc tests).
- `RUSTFLAGS='-D warnings' cargo clippy -p pumpkin --all-targets`: pass with the
  hurt-cooldown change.
- `cargo test -p pumpkin item::items::firework_rocket::tests`: 1 passed.

High-value foundations identified by worker audit:

- Lossless chunk NBT: `ChunkData::internal_from_bytes` consumes the root compound and
  `internal_to_bytes` constructs a fresh one, losing all unmodeled root data. Add an
  unknown/raw compound owner, remove modeled keys on decode, start encode from the
  retained compound and overwrite modeled keys, then extend the same approach to
  nested section tags. Cover with a synthetic unknown-tag decode/encode roundtrip.
- Precipitation ticking: vanilla samples a precipitation operation once per 48 random
  tick attempts, then applies biome/heightmap-aware freezing, snow accumulation, and
  precipitation hooks. Pumpkin has no runtime precipitation phase. Implement this as
  a coordinated chunk-tick phase, not a partial random block mutation.
- Skeleton ranged AI: Pumpkin lacks both a reusable ranged attack goal and a shared
  mob arrow-launch primitive. Build those foundations before attaching skeleton goals.

Patch series now includes the hurt-cooldown and elytra-firework commits. Keep all JAR,
Vineflower, and decompilation artifacts under `/tmp/pumpkin-vanilla-26.2` or otherwise
outside Git. Do not include this file or `parity-patches/` in feature PRs.

PR #2685 / integration commit `17daecc` now implements the first lossless chunk-NBT
foundation slice: unknown root tags survive decode/save while Pumpkin's modeled keys
are overwritten normally. `cargo test -p pumpkin-world` passes all 100 tests and strict
Pumpkin World Clippy passes. The current-upstream PR commit is `e46d8d4` because master
already has inhabited-time fields; conflict resolution retained both fields and did
not import the integration branch's unrelated skylight tests. Unknown nested fields
inside `sections`, palettes, and other modeled compounds remain open follow-up work.

Integration commit `9301949` completes the next nested chunk-NBT slice: section-level
unknown tags and unknown fields inside the modeled `block_states` and `biomes`
compounds survive a save, while Pumpkin regenerates their modeled data/palette keys.
Strict world Clippy and all four chunk codec tests pass. Hold publication until #2685
lands, because this commit intentionally builds on its retained-root ownership field;
opening it against master would duplicate #2685.

PR #2686 / integration commit `1b8c405` adds functional skeleton ranged combat:
default bow/stone-sword equipment, a ranged goal with navigation and LOS, real arrow
spawning, vanilla ballistic lead, 40-tick cadence, 15-block range, and shoot sound.
Focused tests and strict Pumpkin Clippy pass. Follow-ups: generic initial equipment
packet broadcast (the bow is server-side but may not render immediately), dynamic
weapon reassessment, difficulty-specific cadence/inaccuracy, and bogged/stray arrow
effects. Current-upstream PR commit: `d4461ec`.

PR #2687 / integration commit `e7f6491` copies the full firework item stack into the
entity, syncs item metadata, applies the exact component-derived lifetime formula, and
uses vanilla's pre-increment expiry boundary. Focused tests and strict Pumpkin Clippy
pass. The current-upstream PR commit `45b5221` was manually isolated to omit placed and
elytra consumption already covered by #2680/#2684. Explosion damage remains open.

Final checkpoint validation is complete: `cargo test --workspace --no-fail-fast`
passes (Pumpkin 186 tests, world 101, and all other crate/doc suites), strict Pumpkin
and Pumpkin World Clippy pass, and `cargo build --release -p pumpkin` completed. The
release executable SHA-256 above is current through integration commit `1b8c405`.
A clean temporary-directory smoke test loaded all three dimensions and bound the Java,
Bedrock, and query listeners before the 12-second timeout ended it. The fresh empty
directory logged an existing `level.dat` initialization save warning but reached
"Server is now running" successfully.

CI follow-up: #2684 initially omitted the shared consumption predicate when isolated
from the earlier placed-rocket commit; amended current-upstream commit `43e6cf2` adds
the predicate and its survival/Creative regression. #2685's test initially passed a
`Bytes` cursor unsupported by current master's newer NBT reader; amended commit
`e46d8d4` converts it to the supported `Vec<u8>` cursor. Both branches were updated
with `--force-with-lease`; CI reruns are expected.
