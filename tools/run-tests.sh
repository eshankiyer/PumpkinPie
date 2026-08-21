#!/usr/bin/env bash
# PumpkinPie test-suite entry point.
#
# Stages, in order:
#   1. conformance   method-level vanilla coverage      (static, needs the 26.2 decompile)
#   2. tracker       registry coverage                  (static, repo-only)
#   3. fuzzer-build  cargo build -p pumpkin-fuzzer      (static)
#   4. bot-build     cargo build in tools/parity-bot    (static, own nightly toolchain)
#   5. server-boot   boot a scratch server on a free port          (skipped without a binary)
#   6. parity-bot    join that server for N seconds and record     (skipped if 5 skipped)
#   7. fuzzer-run    short packet fuzz against that server         (skipped if 5 skipped)
#
# Scratch data goes under $SCRATCH; nothing is written inside the repo.
# `tools/differential/run.py` is NOT run here: it needs two disposable RCON endpoints,
# one of them a real vanilla 26.2 server, and its probes are destructive.

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRATCH="${PUMPKIN_TEST_SCRATCH:-/tmp/claude-1000/-home-eshanki-Pumpkin/807382eb-1624-4763-98eb-00109277ea8d/scratchpad}/run-tests"
PORT="${PUMPKIN_TEST_PORT:-25599}"
BOT_SECONDS="${PUMPKIN_BOT_SECONDS:-20}"
FUZZ_SECONDS="${PUMPKIN_FUZZ_SECONDS:-5}"
BOOT_TIMEOUT="${PUMPKIN_BOOT_TIMEOUT:-240}"
DECOMPILE="${VANILLA_DECOMPILE:-$HOME/pumpkin-vanilla-26.2/decompiled}"
CARGO_ENV="$HOME/.cargo/env"

SRV_DIR="$SCRATCH/srv"
LOGS="$SCRATCH/logs"
rm -rf "$SRV_DIR"
mkdir -p "$SRV_DIR" "$LOGS"

STAGES=()
STATUS=()
NOTES=()
SERVER_PID=""

record() { STAGES+=("$1"); STATUS+=("$2"); NOTES+=("$3"); }

cleanup() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null
    for _ in $(seq 1 20); do kill -0 "$SERVER_PID" 2>/dev/null || break; sleep 0.5; done
    kill -9 "$SERVER_PID" 2>/dev/null
  fi
}
trap cleanup EXIT INT TERM

port_open() { (exec 3<>/dev/tcp/127.0.0.1/"$PORT") 2>/dev/null; }

echo "== repo:    $REPO"
echo "== scratch: $SCRATCH"
echo "== port:    $PORT"
echo

# ---------------------------------------------------------------- 1. conformance
echo "--- conformance"
if [[ -d "$DECOMPILE" ]]; then
  if python3 "$REPO/tools/conformance/conformance.py" --out "$SCRATCH/conformance.json" \
      >"$LOGS/conformance.log" 2>&1; then
    record conformance PASS "$(grep -m1 'with a Rust counterpart' "$LOGS/conformance.log" | tr -s ' ')"
    tail -25 "$LOGS/conformance.log"
  else
    record conformance FAIL "see $LOGS/conformance.log"
    tail -20 "$LOGS/conformance.log"
  fi
else
  record conformance SKIP "no decompile at $DECOMPILE (run tools/decompile-vanilla.sh)"
fi
echo

# ---------------------------------------------------------------- 2. tracker
echo "--- tracker/build_surface"
if python3 "$REPO/tools/tracker/build_surface.py" --out "$SCRATCH/surface.json" \
    >"$LOGS/tracker.log" 2>&1; then
  record tracker PASS "$(grep -m1 '^total' "$LOGS/tracker.log" | tr -s ' ')"
  tail -6 "$LOGS/tracker.log"
else
  record tracker FAIL "see $LOGS/tracker.log"
  tail -20 "$LOGS/tracker.log"
fi
echo

# ---------------------------------------------------------------- 3. fuzzer build
echo "--- build pumpkin-fuzzer"
if bash -c "source '$CARGO_ENV' && cd '$REPO' && cargo build -p pumpkin-fuzzer" \
    >"$LOGS/fuzzer-build.log" 2>&1; then
  record fuzzer-build PASS ""
else
  record fuzzer-build FAIL "see $LOGS/fuzzer-build.log"
  tail -20 "$LOGS/fuzzer-build.log"
fi
echo

# ---------------------------------------------------------------- 4. parity-bot build
echo "--- build parity-bot"
if bash -c "source '$CARGO_ENV' && cd '$REPO/tools/parity-bot' && cargo build" \
    >"$LOGS/bot-build.log" 2>&1; then
  record bot-build PASS ""
else
  record bot-build FAIL "see $LOGS/bot-build.log"
  tail -20 "$LOGS/bot-build.log"
fi
echo

# ---------------------------------------------------------------- 5. server boot
SERVER_BIN=""
for cand in "$REPO/target/release/pumpkin" "$REPO/target/debug/pumpkin"; do
  [[ -x "$cand" ]] && { SERVER_BIN="$cand"; break; }
done

LIVE=0
echo "--- server boot"
if [[ -z "$SERVER_BIN" ]]; then
  record server-boot SKIP "no target/{release,debug}/pumpkin (tree may not compile; not built here)"
elif port_open; then
  record server-boot FAIL "port $PORT already in use; set PUMPKIN_TEST_PORT"
else
  cat > "$SRV_DIR/pumpkin.toml" <<EOF
[networking.java]
address = "127.0.0.1:$PORT"
online_mode = false
encryption = false
view_distance = 6
simulation_distance = 6

[networking.bedrock]
enabled = false

[networking.query]
enabled = false

[networking.rcon]
enabled = false

[networking.lan_broadcast]
enabled = false
EOF
  ( cd "$SRV_DIR" && exec "$SERVER_BIN" ) >"$LOGS/server.log" 2>&1 &
  SERVER_PID=$!
  deadline=$((SECONDS + BOOT_TIMEOUT))
  ok=0
  while (( SECONDS < deadline )); do
    kill -0 "$SERVER_PID" 2>/dev/null || break
    if port_open; then ok=1; break; fi
    sleep 1
  done
  if (( ok )); then
    record server-boot PASS "pid $SERVER_PID on 127.0.0.1:$PORT ($(basename "$(dirname "$SERVER_BIN")") build)"
    LIVE=1
  else
    record server-boot FAIL "did not accept connections within ${BOOT_TIMEOUT}s; see $LOGS/server.log"
    tail -20 "$LOGS/server.log"
    SERVER_PID=""
  fi
fi
echo

# ---------------------------------------------------------------- 6. parity-bot run
echo "--- parity-bot run"
BOT_BIN="$REPO/tools/parity-bot/target/debug/parity-bot"
if (( ! LIVE )); then
  record parity-bot SKIP "no live server"
elif [[ ! -x "$BOT_BIN" ]]; then
  record parity-bot SKIP "parity-bot binary not built"
else
  timeout $((BOT_SECONDS + 90)) "$BOT_BIN" \
    --server "127.0.0.1:$PORT" --username ParityBot \
    --packet SetHealth --packet Login \
    --seconds "$BOT_SECONDS" >"$LOGS/bot.jsonl" 2>"$LOGS/bot.err"
  bot_rc=$?
  events=$(grep -c '^{' "$LOGS/bot.jsonl" 2>/dev/null || true)
  if kill -0 "$SERVER_PID" 2>/dev/null; then
    if (( bot_rc == 0 )) && (( events > 0 )); then
      record parity-bot PASS "$events JSON events"
    else
      record parity-bot FAIL "exit $bot_rc, $events JSON events; see $LOGS/bot.err"
    fi
  else
    cp -r "$SRV_DIR" "$SCRATCH/srv-died-$(date +%s)" 2>/dev/null
    record parity-bot FAIL "SERVER DIED during bot session: $(tail -3 "$LOGS/server.log" | tr '\n' ' ')"
    LIVE=0; SERVER_PID=""
  fi
  head -5 "$LOGS/bot.jsonl" 2>/dev/null
fi
echo

# ---------------------------------------------------------------- 7. fuzzer run
echo "--- fuzzer run"
FUZZ_BIN="$REPO/target/debug/pumpkin-fuzzer"
if (( ! LIVE )); then
  record fuzzer-run SKIP "no live server"
elif [[ ! -x "$FUZZ_BIN" ]]; then
  record fuzzer-run SKIP "pumpkin-fuzzer binary not built"
else
  if timeout $((FUZZ_SECONDS + 60)) "$FUZZ_BIN" --host 127.0.0.1 --port "$PORT" \
      --duration "$FUZZ_SECONDS" --concurrency 4 >"$LOGS/fuzzer.log" 2>&1; then
    if kill -0 "$SERVER_PID" 2>/dev/null; then
      record fuzzer-run PASS "server survived ${FUZZ_SECONDS}s"
    else
      cp -r "$SRV_DIR" "$SCRATCH/srv-died-$(date +%s)" 2>/dev/null
      record fuzzer-run FAIL "SERVER DIED under fuzz: $(tail -3 "$LOGS/server.log" | tr '\n' ' ')"
      SERVER_PID=""
    fi
  else
    record fuzzer-run FAIL "see $LOGS/fuzzer.log"
  fi
  tail -8 "$LOGS/fuzzer.log" 2>/dev/null
fi
echo

# ---------------------------------------------------------------- shutdown
cleanup
SERVER_PID=""
# Fallback only: a server this script started that ignored SIGTERM. Never pkill -f.
pgrep -x pumpkin >/dev/null && echo "note: a 'pumpkin' process is still running (possibly another agent's); not killed"

# ---------------------------------------------------------------- summary
rc=0
echo "================ SUMMARY ================"
printf "%-14s %-5s %s\n" STAGE STATUS NOTE
for i in "${!STAGES[@]}"; do
  printf "%-14s %-5s %s\n" "${STAGES[$i]}" "${STATUS[$i]}" "${NOTES[$i]}"
  [[ "${STATUS[$i]}" == FAIL ]] && rc=1
done
echo "========================================="
echo "logs: $LOGS"
exit $rc
