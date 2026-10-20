#!/usr/bin/env bash
# Overnight loop: sweep the oracle, run the agent, sweep again, repeat.
#
# Nothing here pushes. Every artefact is left for review in the morning.
# Deliberately conservative: the agent is only reliable on line-edit specs, so
# a night of this produces a small number of real fixes plus a lot of evidence,
# not a pile of speculative diffs.
set -uo pipefail

LOCAL=/home/eshanki/pumpkin-local
ORACLE=$LOCAL/oracle
LOG=/tmp/pumpkin_nightly.log
ROUNDS="${ROUNDS:-6}"

exec >>"$LOG" 2>&1
echo "=============================================================="
echo "nightly start $(date '+%F %T')"

for round in $(seq 1 "$ROUNDS"); do
  echo ""
  echo "---- round $round of $ROUNDS  $(date '+%T') ----"

  # 1. Evidence first. Costs nothing and cannot break anything.
  echo "[sweep]"
  timeout 600 python3 "$ORACLE/sweep.py" 2>&1 | tail -40
  cp /tmp/pumpkin_sweep.json "/tmp/pumpkin_sweep_round${round}.json" 2>/dev/null

  # 2. Reset every worktree the agent owns, so a failed round cannot poison
  #    the next one.
  for w in skylightbounds growthrates diagvalid crithit anvilhelm knockback wplate hoppertick; do
    (cd "/home/eshanki/pumpkin-wt/$w" 2>/dev/null && git checkout -- . 2>/dev/null)
  done
  rm -f /tmp/pumpkin_*.diff /tmp/pumpkin_*_reply.txt /tmp/pumpkin_run_DONE.txt

  # 3. Let the agent attempt the queue.
  echo "[agent]"
  timeout 5400 python3 "$LOCAL/run_all.py" "$LOCAL/specs/0*.md" \
    --builds 2 --max-iters 6 2>&1 | tail -30

  # 4. Preserve anything that came out clean, so a later round cannot clobber it.
  mkdir -p /tmp/pumpkin_keep
  for d in /tmp/pumpkin_*.diff; do
    [ -e "$d" ] || continue
    cp "$d" "/tmp/pumpkin_keep/round${round}_$(basename "$d")"
  done

  echo "[round $round done $(date '+%T')]"
done

echo ""
echo "nightly finished $(date '+%F %T')"
echo "clean diffs kept in /tmp/pumpkin_keep/"
echo "sweep reports in /tmp/pumpkin_sweep_round*.json"
