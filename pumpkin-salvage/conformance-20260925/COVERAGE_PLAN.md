# PumpkinPie vanilla-coverage closing plan (local only, not pushed)

Goal: close the gap between Pumpkin's current vanilla-class coverage and 100%,
for real (verified, cited, build-tested implementations), not by generating
unverified stub code to inflate the metric. See earlier session finding: the
raw "not covered" count is inflated by mechanical mapper gaps and by
architectural mismatches (Rust traits/composition vs Java abstract classes,
goal-based AI vs vanilla's Brain/sensor system) that will never 1:1 name-match
even when fully behaviorally covered.

Current state after mapper fixes (trait/const/static indexing, package-info
filtering, Menu->ScreenHandler alias): **500/1488 = 33.6%** mechanical
coverage. Per-subsystem not-covered counts: entity 478, item 232, block 201,
inventory 53, gameevent 12, material 8, food 2, border 2.

## Phase 0 — cheap mechanical wins (in progress)

Keep refining `map_coverage.py`'s alias table and matching rules whenever a
real semantic-rename family is found (like Menu->ScreenHandler). This is
denominator hygiene, not implementation work — do it first so later phases
aren't wasted "implementing" things that already exist under a different
name. Low effort, do opportunistically, don't over-invest.

## Phase 1 — triage the not-covered lists into real buckets

For each subsystem's not-covered list (starting with the two biggest: entity
478, item 232), run a classification pass — NOT implementation — that sorts
each class into:

1. **Noise** — no longer applicable (already filtered `package-info`, watch
   for more).
2. **Architectural, already behavior-covered** — abstract class / interface
   whose logic lives in Pumpkin's concrete implementations already. Mark
   N/A. Example: `AbstractContainerMenu` once `ScreenHandler` alias covers
   its concrete subclasses.
3. **Architectural, different paradigm, needs behavior-level check** —
   vanilla's Brain/sensor/AI internals (`Activity`, `AdultSensor`,
   `AcquirePoi`, `AirAndWaterRandomPos`, etc.) that Pumpkin implements via
   goal-based AI instead. These need a human/agent judgment call per
   *behavior*, not per class name: does the goal-based equivalent exist? If
   yes, N/A. If a specific behavior is missing, that's a real gap, filed
   under whichever goal file it belongs in, not under the Brain-class name.
4. **Real gap** — no analogue exists at all. This is the actual backlog.

This triage pass is exploratory/classification work — good fit for a
moderate-capability pass (Explore-tier), not full implementation. Output:
an updated `catalog.json` annotation (`triage: noise|na_architecture|
na_paradigm|real_gap`) per not-covered unit, plus a running `real_gap`
count per subsystem. This corrected number, not the raw 500/1488, is the
one to report and drive Phase 2 off of.

## Phase 2 — execute against the real-gap backlog

Split real gaps by whether they're mechanical/pattern-completion or
correctness-sensitive behavioral logic:

- **Mechanical (Haiku-tier subagents)**: gaps that follow an established,
  already-proven pattern elsewhere in the codebase — e.g. a new copper-family
  waxing arm (pattern: honeycomb.rs, fixed this session), a new dye-color
  match arm, a data-table entry (armor material, food property) that mirrors
  an existing table 1:1 from decompiled vanilla source. Batch these: one
  Haiku agent per small batch of near-identical items, given the exact
  existing pattern file + the vanilla source lines to port + explicit
  "verify against decompiled source, don't guess" instruction. Still
  requires cargo fmt/clippy/test verification before commit — Haiku doesn't
  get a lower quality bar, just a narrower/more templated task.
- **Correctness-sensitive (full-capability agents)**: entity AI/combat
  formulas, worldgen coefficients, concurrency-touching fields, anything
  needing genuine judgment about vanilla semantics. Use the audit-then-fix
  pipeline already working this session: parallel Explore agents cite
  decompiled vanilla source + current Pumpkin source, narrow confident
  findings only, then implement+verify+commit each one individually. This
  is NOT a Haiku task per CLAUDE.md/memory guidance.
- **Large features** (redstone torch burnout, GameEvent/vibration system,
  WorldBorder interpolation, villager Brain replacement pieces): scope
  separately, don't fold into the batch loop. Track in PARITY.md.

## Phase 3 — continuous verify loop

After each batch: re-run `map_coverage.py` (mechanical number) and spot-check
fidelity via `sample_fidelity.py` on newly-covered classes. Report the
corrected real-gap count trending down, not the raw denominator. Never
present the composite estimate as more than an estimate (existing
`score.py` caveat stays in place).

## Non-goals

- Do not generate unverified stub implementations purely to move the
  mechanical coverage number — this was explicitly rejected earlier this
  session and remains rejected.
- Do not push `conformance/` or this file to GitHub (local only, per
  `.git/info/exclude`).
- Do not open new upstream PRs from this work; keep shipping to
  `autoport/nightly` / fork / forgejo per standing instructions.
