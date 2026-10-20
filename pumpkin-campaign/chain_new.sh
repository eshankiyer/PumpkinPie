#!/bin/bash
# chain_new.sh <lane> <worktree> <chunkfile> <tool> [model]
# Dispatches GREENFIELD feature work: vanilla classes with no Rust analogue at all
# (map_coverage.py found nothing), as opposed to chain.sh's gap-filling of an already-
# covered class. One chunk file, one run (no chunk-loop numbering).
LANE=$1; WT=$2; CHUNK=$3; TOOL=${4:-codex}; MODEL=$5
C=/home/eshanki/pumpkin-campaign
LOG=$C/${LANE}_chain.log
source ~/.cargo/env 2>/dev/null

[ -f "$CHUNK" ] || { echo "=== $LANE chunk missing $CHUNK" >> "$LOG"; exit 1; }
cp "$CHUNK" "$WT/WORKLIST.json"

echo "=== $LANE starting $(date -Is)" >> "$LOG"
PROMPT=$(cat <<EOF
You are implementing missing vanilla-parity FEATURES in the Pumpkin Minecraft server
(Rust) against the Minecraft 26.2 Mojang decompile at ~/pumpkin-vanilla-26.2/decompiled.

Worklist: WORKLIST.json - a JSON array of vanilla classes that a mechanical name-matcher
found NO Rust analogue for anywhere in the workspace (file stem, struct name, or enum
name). These are candidates for real new implementation work, not gap-filling.

STOP: THE NAME-MATCHER IS OFTEN WRONG. Before writing ANY code for a class, you MUST
disprove that it already exists under a different name. This has burned real dispatch
runs before: LavaFluid (matcher said missing) is actually crates/pumpkin/src/block/fluid/lava.rs,
DiodeBlock is an abstract Java base whose behavior is fully covered by
crates/pumpkin/src/block/blocks/redstone/{repeater,comparator}.rs (Rust doesn't need the
abstract-base type to have the behavior), and FishingHook is Yarn-renamed to
crates/pumpkin/src/entity/projectile/fishing_bobber.rs::FishingBobberEntity. For EACH
class:
1. Read ~/pumpkin-vanilla-26.2/decompiled/<vanilla_path> to understand what it actually does
   and what subclasses/callers reference it.
2. Grep the Rust workspace broadly for the BEHAVIOR, not just the name: try synonyms, Yarn-
   style renames, the class with common suffixes stripped (Block/Item/Entity/Argument/Fluid),
   and check whether a subclass or sibling class already implements the same thing.
3. If you find it already exists (even partially, even under a very different name): write
   NOT_A_GAP in WAVE_REPORT.md with the file that already covers it, and do NOT write any
   code for that class. This is a successful, valuable outcome - it corrects the instrument.
4. Only if you are confident after real greping that nothing implements this: design and
   implement it as new Rust code, following the architectural conventions already used
   nearby (same module layout, same trait patterns, same style as neighboring files).
5. Wire it into a real call site in the same run - new code with zero callers is a FAILED
   run, exactly as bad as skipping the class. A command needs to be registered in
   crates/pumpkin/src/command/commands/mod.rs; a block needs a block_properties entry and
   registration in crates/pumpkin/src/block/blocks/mod.rs; an argument type needs a real
   command argument that uses it. Grep an existing similar registration and match it exactly.

Do not invent APIs. Every type, method, constant and enum variant you reference must be one
you grepped for in this workspace this run.

Hard rules:
- Every change carries a comment citing the vanilla file and line numbers you read THIS
  run, e.g. \`FishingHook.java:200-210\`. Never cite from memory.
- Match surrounding style exactly. No emojis. Comments minimal and factual.
- Never downgrade an atomic read-modify-write into a load/store pair.
- Add a unit test wherever the behaviour is testable without a live server.
- Do NOT run cargo build/clippy/test, they are too slow. Match existing signatures exactly;
  grep for a signature when unsure.
- Do NOT commit, do NOT touch git.

End by appending to WAVE_REPORT.md one row per class:
| Class | NOT_A_GAP (existing file) OR methods IMPLEMENTED | vanilla file:lines read | new call sites wired |
EOF
)
if [ "$TOOL" = "codex" ]; then
  timeout 2400 codex exec --model "${MODEL:-gpt-5.6-luna}" --sandbox danger-full-access --cd "$WT" "$PROMPT" >> "$LOG" 2>&1
else
  (cd "$WT" && timeout 2400 opencode run --auto --model "${MODEL}" "$PROMPT") >> "$LOG" 2>&1
fi
RC=$?
echo "=== $LANE finished $(date -Is) rc=$RC" >> "$LOG"
echo "=== $LANE ALL CHUNKS DONE $(date -Is)" >> "$LOG"
