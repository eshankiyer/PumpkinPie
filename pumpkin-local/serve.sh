#!/usr/bin/env bash
# Launch the local coding model. Run this instead of touching llama-server directly.
#
# Every invocation re-runs the stray/VRAM pre-flight. On an 8 GB card a single orphaned
# llama-server pins several GB and llama.cpp silently degrades to 0 layers on GPU, which
# reads as "the model got slow" rather than "a process leaked". See DECISIONS.md.
set -u

BIN="${LLAMA_BIN:-$HOME/.local/ollama/lib/ollama/llama-server}"
# 2026-07-29: Qwen3.6-27B dense is the only model on disk now; the A3B MoE
# ggufs were deleted at Ragini's request. Dense on the 8 GB card REQUIRES
# auto-fit placement, so NGL/NCMOE below default to hands-off.
MODEL="${LLAMA_MODEL:-$HOME/pumpkin-local/gguf/Qwen3.6-27B-UD-Q4_K_XL.gguf}"
PORT="${LLAMA_PORT:-11434}"
# 99 = everything on GPU, correct for the MoE models whose experts ncmoe
# already routes to CPU. Dense models that exceed VRAM need partial offload:
# set LLAMA_NGL to the layer count that fits (tune with capprobe.sh).
NGL="${LLAMA_NGL:--1}"
# 16384, not 32768. Halving the KV cache freed 3.5 GB of VRAM, which bought eight
# more expert layers on the card and more than doubled throughput. No spec has
# ever needed more context: the largest function ever fed to the model was 111
# lines, and FUNC mode sends one function rather than whole files.
CTX="${LLAMA_CTX:-16384}"
# Layers whose MoE experts live in system RAM. Measured on Qwen3.6-35B at ctx
# 16384: 42 gives 13.4 tok/s, 34 gives 22.4 tok/s at 5275 MiB resident, 26 fails
# to create a context outright. Raise this, not the context, if a future model
# OOMs; dropping context costs less than dropping layers, but only down to the
# point where the KV cache still fits.
NCMOE="${LLAMA_NCMOE:-0}"
MIN_FREE_MIB="${LLAMA_MIN_FREE:-7000}"
LOG="$HOME/pumpkin-local/logs/llama.log"

mkdir -p "$(dirname "$LOG")"

log() { echo "[serve] $*"; }

# --- 1. kill strays by resolved PID -------------------------------------------------
# Never `pkill -f llama` here: the pattern matches this script's own command line.
mapfile -t STRAYS < <(ps -eo pid=,cmd= \
  | awk '$0 ~ /llama-server|ollama serve/ && $0 !~ /awk/ {print $1}')
if [ "${#STRAYS[@]}" -gt 0 ]; then
  log "killing strays: ${STRAYS[*]}"
  kill -9 "${STRAYS[@]}" 2>/dev/null
  sleep 4
fi

# --- 2. assert the card is actually clear -------------------------------------------
read -r USED TOTAL < <(nvidia-smi --query-gpu=memory.used,memory.total \
  --format=csv,noheader,nounits | tr ',' ' ')
FREE=$(( TOTAL - USED ))
log "VRAM ${USED}/${TOTAL} MiB used, ${FREE} MiB free"
if [ "$FREE" -lt "$MIN_FREE_MIB" ]; then
  log "ABORT: only ${FREE} MiB free, need ${MIN_FREE_MIB}. Holders:"
  nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader
  exit 1
fi

[ -f "$MODEL" ] || { log "ABORT: model not found at $MODEL"; exit 1; }
[ -x "$BIN" ]   || { log "ABORT: llama-server not executable at $BIN"; exit 1; }

# --- 3. launch ----------------------------------------------------------------------
# -ngl 99      : all layers nominally on GPU
# -ncmoe N     : ...except the MoE expert tensors of the first N layers, which go to RAM.
#                Attention + KV must fit in ~6 GB. If this OOMs, cut CTX before cutting
#                -ngl; dropping layers costs far more throughput than context does.
# -fa on       : flash attention, materially shrinks the KV cache
# The CUDA backend ships as a dynamically-loaded ggml plugin in its own
# directory. Without these two the binary starts, prints "no usable GPU found",
# ignores -ngl entirely, and runs 30B on the CPU at a speed that looks like a
# bad quant rather than a missing backend.
CUDA_DIR="$(dirname "$BIN")/cuda_v13"
[ -d "$CUDA_DIR" ] || CUDA_DIR="$(dirname "$BIN")/cuda_v12"
# GGML_BACKEND_PATH wants the shared object itself. Pointed at the directory it
# logs "cannot read file data: Is a directory" at error level, then continues
# and reports no usable GPU, so the only visible symptom is a slow model.
export GGML_BACKEND_PATH="$CUDA_DIR/libggml-cuda.so"
export LD_LIBRARY_PATH="$CUDA_DIR:$(dirname "$BIN")${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

if ! "$BIN" --list-devices 2>/dev/null | grep -q CUDA; then
  log "ABORT: CUDA backend did not load, refusing to run 30B on the CPU"
  exit 1
fi

# Reasoning, but on a leash.
#
# Qwen3.6 thinks by default and will happily spend thousands of tokens on it. At
# roughly 9 tok/s an unbounded preamble is minutes per attempt, and with a
# 6-iteration cap that is the whole budget spent on deliberation rather than
# code. GLM-4.7 failed here for exactly that reason.
#
# But switching thinking off entirely throws away the thing this model is good
# at. --reasoning-budget caps it instead: think, briefly, then answer. When the
# budget runs out llama.cpp injects the end-of-thinking tag and the model
# proceeds to the answer, so a truncated thought degrades into a quick answer
# rather than a dropped response.
#
# --reasoning-format deepseek moves the thought into message.reasoning_content
# so it never reaches the code parser. That matters: a <think> block left in
# message.content would be spliced into a Rust file verbatim.
REASONING="${LLAMA_REASONING:-on}"
# -1 = uncapped. The 512 budget saved tokens but starved the hard specs: type
# puzzles like i32/usize casts and match-pattern rules are exactly what more
# thinking fixes, and a forced-closed thought plus a long function was pushing
# total decode into the max_tokens cap, truncating code mid-word (spec 02).
BUDGET="${LLAMA_REASONING_BUDGET:--1}"

log "starting: ctx=$CTX ncmoe=$NCMOE port=$PORT reasoning=$REASONING/$BUDGET"
setsid "$BIN" \
  --model "$MODEL" \
  --host 127.0.0.1 --port "$PORT" \
  --ctx-size "$CTX" \
  $( [ "$NGL" != "-1" ] && echo -ngl "$NGL" ) \
  $( [ "$NCMOE" != "0" ] && echo -ncmoe "$NCMOE" ) \
  -fa on \
  --parallel 1 \
  --no-webui \
  --jinja \
  --reasoning "$REASONING" \
  --reasoning-budget "$BUDGET" \
  --reasoning-format deepseek \
  </dev/null >"$LOG" 2>&1 &

# --- 4. wait for readiness ----------------------------------------------------------
for i in $(seq 1 180); do
  if curl -sf "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
    log "ready after ${i}s"
    grep -iE "offloaded|CUDA[0-9]* model buffer|KV self size" "$LOG" | tail -5
    nvidia-smi --query-gpu=memory.used --format=csv,noheader
    exit 0
  fi
  sleep 1
done

log "ABORT: server did not become healthy in 180s, tail of log:"
tail -30 "$LOG"
exit 1
