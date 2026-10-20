# Pumpkin vanilla-parity plan

Target note: the brief said "1.21", but the repo declares `0.1.0-dev+26.2` and
tracks the latest Java release. Everything below is written against **26.2**.
That distinction has already bitten us twice: game rules were renamed to
snake_case in 25w44a, and dimension types were reworked in 26.x. Never verify
against a 1.21 source and assume it still holds.

---

## 1. How to run this

```bash
# one-time per shell
export OLLAMA_HOST=127.0.0.1:11434
export OLLAMA_MODELS=$HOME/.local/ollama/models
~/.local/ollama/bin/ollama serve &          # if not already running

cd ~/pumpkin-local
python3 harness.py specs/12-whatever.md      # implement one spec, build-gated
python3 driver.py specs/1*.md                # batch of specs
python3 driver.py --rebase-only              # rebase all branches, resolve, push
python3 driver.py --status                   # worktree/branch overview
```

Model: `glm-4.7-flash:q4_K_M` (19 GB, MoE, ~3B active). Chosen because it is the
strongest thing that fits 7.3 GB VRAM plus 22 GB RAM on this laptop, and because
SWE-bench Verified is the benchmark that actually matches "fix a bug in a real
repo". Roughly 30 s per reply. `qwen2.5-coder:7b` is installed as a fast fallback
for trivial mechanical edits.

The harness unloads the model before every cargo run, because a 19 GB model and
a rustc invocation will not coexist in 30 GB of RAM.

---

## 2. Division of labour, and why

The local model does the part that is expensive and mechanical:

- apply a precisely specified edit
- iterate against `RUSTFLAGS="-D warnings" cargo clippy` until clean
- resolve textual rebase conflicts, then **rebuild before anything is pushed**

The local model must never do the part where being wrong is silent and costly:

- decide what vanilla does. Every behavioural claim in a spec is verified by
  Claude *before* the spec is written, with a source link included in the spec.
  A 3B-active model asked "what does vanilla do here" will produce a confident,
  plausible, wrong answer, which is exactly the failure mode that has already
  forced two public retractions on this project.
- write PR titles, bodies, or verification sections. Honest verification prose
  is the whole reason these PRs get taken seriously.
- decide whether a diff is safe to push.

`driver.py` deliberately does not open PRs.

---

## 3. Hard rules the specs must carry

Every spec repeats these, because the model has no memory between runs:

1. No emojis anywhere. Maintainers flag emoji PRs for expedited closure.
2. No em-dashes in code or comments.
3. Never downgrade an atomic read-modify-write to a load/store pair. Hoppers,
   block entities and chunk data are written by concurrent tick tasks.
4. One logical fix per branch. Out-of-scope bugs get reported, not folded in.
5. Match surrounding style; do not reformat untouched code.
6. Anchor SEARCH blocks on a few unique lines, never a whole function.

---

## 4. Wave 0 — open, waiting on upstream

25 PRs are open and mergeable. Nothing to do but respond to review.

The one that gates everything else is **#2587 (randomTickSpeed was a no-op)**.
Until it lands, no random-tick behaviour can be observed in a reasonable window:
a given block ticks about once per 68 s, so a 60 s test has roughly a 7 percent
chance of seeing anything. Every "verified live" claim about crops, ice, grass,
leaves or fire is worthless until this merges. Two such claims have already been
retracted on #2579 and #2580, and #2567 is flagged for maintainers to hold.

**When #2587 merges, immediately:** rebuild the NUC image, re-run the RCON suite
at `random_tick_speed 500`, and post real results to #2567, #2579, #2580.

---

## 5. Wave 1 — confirmed, specced, ready to run

Each is a known-real bug with the vanilla behaviour already verified. These are
spec-ready today.

| # | Fix | Files | Notes for the spec |
|---|-----|-------|--------------------|
| 1.1 | `get_sky_light_level` returns 15 below the world | `pumpkin/src/lighting/runtime.rs` | `section_index` is an unsigned cast, so `y < min_y` wraps to a huge index and takes the out-of-bounds branch. Above build height 15 is correct, so the fix is a **signed** bounds check, not flipping the constant. Also disagrees with `storage.rs`, which returns 0. Two call sites: sync and async variants. |
| 1.2 | Instant Health / Damage ignore undead | `pumpkin/src/entity/living.rs`, `pumpkin/src/item/potion.rs` | Vanilla inverts both for undead. Reuse whatever undead classification the Smite enchantment already uses; do not invent a second one. |
| 1.3 | Instant potions applied twice | `pumpkin/src/item/potion.rs` | Found while fixing effect merge: `potion.rs` calls `heal`/`damage` directly **and then** `add_effect` with the same instant type, which heals/damages again on a different formula. Confirm, then pick one path. |
| 1.4 | Resistance is inert | damage handling | No code path checks `StatusEffect::RESISTANCE`. Part of upstream #764, which is partly stale: most other effects listed there are now implemented. |
| 1.5 | Berry bush and nether wart growth rates | `plant/crop/sweet_berry_bush.rs`, `plant/nether_wart.rs` | Filed as #2582. Bush applies `0..=25` inside its override on top of the outer `0..5`, giving ~1/130 where vanilla is 1/5. Wart uses `0..=10` where vanilla is `nextInt(10)`. Proposal in the issue: drop the inner gate, change wart to `0..10`. Leave the unexplained `is_full_cube` check alone. |
| 1.6 | `isRainingAt` unimplemented for lightning | `pumpkin/src/world/mod.rs` | Literal `if true` in `tick_spawning_chunk`. Vanilla checks raining, can-see-sky, heightmap, and biome precipitation. **A previous attempt read the biome at `heightmap + delta.y`, 0-15 blocks above the surface, which suppresses lightning vanilla allows.** Probe the strike position. There is an existing `is_raining_at` helper to reuse. Also gate on ramped `rainLevel > 0.2` / `thunderLevel > 0.9`, not raw booleans. |
| 1.7 | skyDarken never tracked | `pumpkin/src/world/mod.rs` | `get_max_local_raw_brightness` always passes `ambient_darkness = 0`, so grass spreads at night as if noon and night mob spawning is wrong. Watch for u8 underflow when subtracting. |
| 1.8 | Area effect cloud NBT is an empty impl | `pumpkin/src/entity/area_effect_cloud.rs` | `impl NBTStorage for AreaEffectCloudEntity {}` saves and loads nothing, so a cloud restored via generic entity dispatch reverts to constructor defaults. Data loss. |
| 1.9 | Zombie villager curing does nothing | `pumpkin/src/net/java/play.rs`, `entity/mob/zombie/zombie_villager.rs` | Grants the advancement on click with no Weakness check, no timer, no conversion. Check for existing conversion machinery first. The cure discount depends on gossip, which is in someone else's open PR #2408 — do not build it. |
| 1.10 | Villagers never breed | `entity/passive/villager/mod.rs` | No `BreedGoal` in the goal set though a generic one exists and is wired for cows/pigs/sheep. `eat_until_full` has zero callers, so `food_level` accumulates and triggers nothing. |

---

## 6. Wave 2 — real, but bigger than a spec

These need design decisions before they can be handed to a local model.

- **Vanilla-imported worlds go dark above terrain.** A vanilla chunk with
  `isLightOn: true` omits SkyLight for above-terrain sections; we read those as
  `Empty(0)` and `needs_relighting` refuses to relight because `isLightOn` is
  true. Correct fix needs top-down derivation (find the highest section carrying
  a SkyLight tag, 15 above it, repeat the nearest layer below) plus a decision
  for dimensions without sky light. **There is no test coverage for the chunk
  codec; build that first.**
- **`CLightUpdate` has no send sites.** #2615 makes the packet correct; nothing
  ever sends it. Runtime light changes never reach clients, so F3 shows
  generation-time light. Filed in #2592.
- **Block states rejected in `setblock` / `clone filtered`.** Two independent
  parsers, neither handles brackets. Both have `Item = &'static Block` and
  consumers use `block.default_state.id`, so real support means changing the
  argument's item type to a resolved block state and threading it through
  `setblock`, `fill`, `clone` and `execute if block`. Filed in #2592.
- **`forceload` marks chunks but never loads them.** Filed in #2592. This one
  is also worth doing for our own sake: it would remove the need to keep a
  protocol client connected purely as a chunk loader during RCON testing.
- **`can_start_at` diverges from vanilla three ways** (no `!= OPEN` check,
  static malus instead of the mob's, an extra collision test). Not a drop-in:
  vanilla's `getStart` always returns a node and `canStartAt` only picks which
  corner, whereas ours returns `None` when all candidates fail. Making it
  stricter without porting `getStart`'s control flow turns "pick another start
  block" into "pathfinding fails outright".
- **Iron golems never spawn from villages.** Absent feature, one coarse issue.
- **Effect hidden-stack.** #2617's merge rule is correct but does not keep the
  superseded weaker effect to resurface later. Needs somewhere to live.

---

## 7. Wave 3 — discovery queue

One discovery worker per cycle, auditing an untouched subsystem against 26.2 and
returning PARITY.md rows, not code. Audited so far: potions and status effects
(7 rows), villagers (8 rows, most already covered by someone else's #2408).

Priority order, roughly by how much silently-wrong behaviour is likely:

1. **Mob spawning** — caps per category, light rules, despawn distance, mob cap
   accounting, spawn eggs. Intersects the simulation-distance work in #2616.
2. **Enchanting** — table mechanics, bookshelf power, anvil cost and the "too
   expensive" cap, enchantment compatibility, treasure-only rules.
3. **World border** — damage, warning distance, `/worldborder` semantics.
4. **Dimensions and portals** — nether coordinate scaling, portal linking and
   search radius, end gateways, respawn anchors.
5. **Food and hunger** — saturation, exhaustion sources and rates, natural
   regeneration thresholds, food poisoning.
6. **Redstone remainder** — pistons and block-move rules, quasi-connectivity,
   observer chains, rail powering.
7. **Scoreboards, teams, advancements** — criteria triggers especially, since
   the Zombie Doctor bug shows advancement triggers are not carefully guarded.
8. **Fishing, raids, bees** — self-contained systems likely to be partial.

Each audit prompt must carry the anti-hallucination rules verbatim and must
check upstream issues and PRs first. #2408 is the cautionary example: most of
the villager trading gaps were already someone else's open work.

---

## 8. Rebase, conflict, push protocol

25 open PRs against a moving master means conflicts are the default, not the
exception. `driver.py --rebase-only` does:

1. `git fetch upstream master`
2. per branch: `git rebase upstream/master`
3. on conflict, hand each conflicted file to the model with both sides
4. reject the resolution if markers survive, if it says CANNOT RESOLVE, or if
   the output is under 40 percent of the input length (the classic failure where
   a model silently deletes a hunk)
5. **rebuild** with `-D warnings` before pushing anything it touched
6. push to `forgejo` then `fork`, `--force-with-lease`
7. any branch it could not finish is left exactly as it was and reported

Known conflict pairs to expect, since they touch the same files:

- #2589 and #2611 both touch `block/entities/hopper.rs`
- #2590 and #2610 both touch `item/items/axe.rs`
- #2616 and several others touch `world/mod.rs`, which is the hottest file
- #2612 and #2615 both touch light handling, though different layers

---

## 9. What Claude checks when previewing

Every returned diff is gated on:

- **Vanilla citation actually supports the claim.** Not "a link exists" but
  "the linked text says the thing the diff assumes".
- **Concurrency.** Any atomic touched? Any lock ordering introduced? Any
  critical section widened?
- **Diff minimality.** Reformatting, drive-by refactors, and unrelated lint
  fixes all get rejected back.
- **Honesty of the verification section.** If it cannot be observed reliably,
  the PR must say so. "Not verified in game" is a perfectly good line and is
  much better than an implied test that never happened.
- **Scope.** One logical fix. Out-of-scope finds should appear in the report,
  not the diff.

---

## 10. Realistic assessment of "80-90 percent parity"

Worth being straight about this. Vanilla is roughly fifteen years of behaviour,
and Pumpkin has its own 1.0 tracking issue (#449) with many sub-trackers. The
coverage proxies measured earlier were 158 of 1196 blocks with behaviour and 32
of 169 entity types with AI, though both numbers mislead in opposite directions:
most vanilla blocks need no behaviour code at all, while a registered-but-partial
behaviour counts as done.

A single contributor is not going to close that to 90 percent, and a plan that
claims otherwise is not a plan. What this pipeline is actually good for is a
steady, high-quality stream of *correctness* fixes: the registered-but-wrong
class, where something runs and silently produces the wrong answer. That class
is where every fix so far has landed, it is what maintainers merge fastest, and
by that measure the campaign is going well. Track progress as rows closed in
PARITY.md, not as a percentage.
