#!/bin/bash
# chain.sh <lane> <worktree> <prefix> <nchunks> <tool> [model]
LANE=$1; WT=$2; PREFIX=$3; N=$4; TOOL=${5:-codex}; MODEL=$6
C=/home/eshanki/pumpkin-campaign
LOG=$C/${LANE}_chain.log
source ~/.cargo/env 2>/dev/null

for i in $(seq 1 "$N"); do
  CHUNK=$C/chunks/${PREFIX}_chunk${i}.json
  [ -f "$CHUNK" ] || { echo "=== $LANE chunk $i missing $CHUNK" >> "$LOG"; continue; }
  cp "$CHUNK" "$WT/WORKLIST.json"
  CHUNK=WORKLIST.json
  echo "=== $LANE chunk $i starting $(date -Is)" >> "$LOG"
  PROMPT=$(cat <<EOF
You are closing vanilla-parity gaps in the Pumpkin Minecraft server (Rust) against the
Minecraft 26.2 Mojang decompile at ~/pumpkin-vanilla-26.2/decompiled.

Worklist: $CHUNK - a JSON array of vanilla classes with vanilla_path, rust_file, and
absent_workspace_wide (method names with no name-match anywhere in the Rust workspace).
These are LEADS, not proven gaps: roughly one in six is real.

YOUR JOB IS TO SHIP CODE. A run that classifies every lead and writes no Rust is a
FAILED run. Previous runs failed exactly this way. Budget your time accordingly: at most
a third of it on reading, the rest on implementing.

For EACH class:
1. Read ~/pumpkin-vanilla-26.2/decompiled/<vanilla_path> and the Rust file in rust_file.
2. Classify each absent method: GENUINE_GAP, RENAMED (name the Rust fn), INLINED (name
   the caller), CLIENT_ONLY, or DATA_MODELED (name the generated table).
3. Implement the GENUINE_GAPs.

NEVER ADD CODE NOTHING CALLS. A method whose only callers are other methods you added in
the same run is dead code, and dead code is a FAILED lead, not a closed one. Before you
write any new public function, decide which existing call site will invoke it, and wire it
there in the same edit. If no live call site exists, the lead is BLOCKED, not implemented.
The last run produced 949 lines across ChunkHolder, ProtoChunk, MerchantOffer and Structure
that compiled, passed tests, and were all deleted for having zero callers. Adding a method
so a name-match detector stops reporting it is the single worst outcome of this task.

Do not invent APIs. Every type, method, constant and enum variant you reference must be one
you grepped for in this workspace this run. A previous run wrote \`crate::server\`,
\`AtomicU32\` and \`EquipmentSlot::MAINHAND\`, none of which exist here.

BLOCKED is heavily restricted. You may only write BLOCKED for a method if you ran a grep
and pasted its empty output proving the supporting machinery does not exist. "X is not
present" without that grep is forbidden - the last run wrote "rider input/jump protocol is
not present" when crates/pumpkin/src/entity/passive/happy_ghast.rs:389 reads rider input
via rider.last_input and SPlayerInput flags. Assume the machinery EXISTS and find it
before claiming otherwise.

Known precedents, use them instead of declaring new architecture is needed:
- Ridden entity input/jump: HappyGhastEntity::custom_travel (entity/passive/happy_ghast.rs)
  folds getRiddenInput/getRiddenRotation/tickRidden into the custom_travel hook.
- Brain/Memory/Activity mobs: entity/mob/warden.rs carries WardenAi's activity ladder as
  plain fields ticked from mob_tick, with targeting through the Goal system.
- Block hooks: crates/pumpkin/src/block/mod.rs has attack, player_will_destroy, broken,
  get_clone_item_stack, on_projectile_hit and more. Grep the trait before saying a hook
  does not exist.

Hard rules:
- Every change carries a comment citing the vanilla file and line numbers you read THIS
  run, e.g. \`Entity.java:1541-1547\`. Never cite from memory.
- player_will_destroy fires BEFORE the block is removed; broken fires AFTER it is already
  air. Never move logic between hooks that fire at different times.
- Match surrounding style. No emojis. Comments minimal and factual.
- Never downgrade an atomic read-modify-write into a load/store pair.
- Add a unit test wherever the behaviour is testable without a live server.
- Do NOT run cargo build/clippy/test, they are too slow. Match existing signatures exactly;
  grep for a signature when unsure.
- Do NOT commit, do NOT touch git.

End by appending to WAVE_REPORT.md one row per class:
| Class | methods IMPLEMENTED (names) | vanilla file:lines read | BLOCKED with grep evidence |
If the implemented column is empty for a class, state in that row exactly which greps
proved every lead was not a gap.
EOF
)
  if [ "$TOOL" = "codex" ]; then
    timeout 1500 codex exec --model "${MODEL:-gpt-5.6-luna}" --sandbox danger-full-access --cd "$WT" "$PROMPT" >> "$LOG" 2>&1
  else
    (cd "$WT" && timeout 1500 opencode run --auto --model "${MODEL}" "$PROMPT") >> "$LOG" 2>&1
  fi
  RC=$?
  echo "=== $LANE chunk $i finished $(date -Is) rc=$RC" >> "$LOG"
done
echo "=== $LANE ALL CHUNKS DONE $(date -Is)" >> "$LOG"
