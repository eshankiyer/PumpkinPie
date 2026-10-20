#!/usr/bin/env bash
# One parity cycle: observe, then fix. Meant to run unattended, nightly.
#
#   1. ensure the local model server is up
#   2. oracle discovery: diff vanilla vs Pumpkin, file evidenced gaps to
#      DISCOVERED.md (proposals, never auto-fixed)
#   3. fix spree: mini-swe-agent works the CONFIRMED queue, clippy + oracle
#      gated, banks candidates
#
# "Slowly but surely": each night this closes a few known gaps and grows the
# discovered-gap list from observation. It does not chase a finish line and it
# never fixes an unconfirmed behaviour. Candidates are reviewed before merge.
set -u
L="$HOME/pumpkin-local"
# One shared build cache for the whole loop. Without this every worktree grows
# its own ~3.4 GB target/; 19 of them was 65 GB. mini and verify inherit this
# env, so all their cargo builds land in one place, bounded at ~4 GB.
export CARGO_TARGET_DIR="$L/.loop-target"
ts() { date '+%Y-%m-%d %H:%M'; }

# Keep cycle.log from growing without bound: trim to the last 3000 lines.
if [ -f "$L/logs/cycle.log" ] && [ "$(wc -l < "$L/logs/cycle.log")" -gt 3000 ]; then
  tail -n 3000 "$L/logs/cycle.log" > "$L/logs/cycle.log.tmp" && mv "$L/logs/cycle.log.tmp" "$L/logs/cycle.log"
fi

echo "===== parity cycle $(ts) ====="

# 1. model server
if ! curl -sf http://127.0.0.1:11434/health >/dev/null 2>&1; then
  echo "[cycle] starting llama-server"
  bash "$L/serve.sh" || { echo "[cycle] serve.sh failed, aborting"; exit 1; }
fi

# 2+3 in PARALLEL. Discovery only does RCON to the NUC (no local model), so it
# does not compete with the spree for the GPU. Launch it in the background and
# start the spree immediately instead of making the spree wait out discovery.
echo "[cycle] oracle discovery (background)"
python3 "$L/oracle_discover.py" > "$L/logs/discover.log" 2>&1 &
disc_pid=$!

echo "[cycle] fix spree (foreground)"
python3 "$L/mini_spree.py" 2>&1 | tail -8

# Discovery is quick and almost always done by now; wait so its output is
# complete before the summary.
wait "$disc_pid" 2>/dev/null
echo "[cycle] discovery result:"; tail -3 "$L/logs/discover.log"

# 4. behavioural differential oracle. Needs a client holding Pumpkin's spawn
# chunk loaded (Pumpkin loads no chunks over RCON, forceload is broken), so
# start the raw-protocol bot, give it time to spawn, probe, then stop it - it
# reconnect-spams every 30s and must not linger. Guarded: a gap is filed only
# when vanilla reproduces the expected behaviour AND Pumpkin differs, so an
# immature probe files nothing rather than noise.
# Behavioural oracle. DISABLED in the cycle by default: the holder bot
# reconnects every ~30s (protocol 776 vs the library's 1.21 ceiling) and
# stalls probes, and on 2026-07-30 it hung the whole nightly for 6 hours. It
# is hard-timeout-wrapped AND opt-in (COVER_BEHAVIOUR=1) so it can never block
# the cycle again. Run it by hand while watching until the chunk-holder is
# stable. The kill is unconditional so no bot is ever left reconnect-spamming.
if [ "${COVER_BEHAVIOUR:-0}" = "1" ]; then
  echo "[cycle] behavioural oracle (bot + probes, 6min cap)"
  ( cd "$HOME/pumpkin-bot" && node holder.js Verifier > "$L/logs/holder.log" 2>&1 ) &
  bot_pid=$!
  sleep 22
  timeout 360 python3 "$L/nuc_behavior.py" 2>&1 | tail -8 || echo "[cycle] behavioural oracle timed out, skipped"
  kill "$bot_pid" 2>/dev/null; pkill -f "holder.js" 2>/dev/null
else
  echo "[cycle] behavioural oracle skipped (set COVER_BEHAVIOUR=1 to enable)"
fi

# 5. automated review: classify discoveries, drop noise, park features, and
# promote only small localizable fixes to the queue. Conservative by design -
# it will not queue whole-feature work. Promotions land for the NEXT cycle.
echo "[cycle] triage discoveries"
python3 "$L/triage_discoveries.py" --promote 2>&1 | tail -8

banked=$(ls "$L/candidates" 2>/dev/null | wc -l)
disc=$(grep -c '^- \[ \]' "$L/DISCOVERED.md" 2>/dev/null || echo 0)
echo "===== cycle done $(ts): ${banked} candidate diffs on disk, ${disc} open discoveries ====="
