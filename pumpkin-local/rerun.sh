#!/usr/bin/env bash
# Rerun whatever failed the last queue run, with everything that landed today:
# mode-aware retries, API-surface packs, E0599 injection, unused-import
# auto-delete, per-iteration reply capture, uncapped thinking, 6000 max_tokens.
#
#   ./rerun.sh            rerun failures on qwen3.6 (the default engine)
#   ./rerun.sh --coder    swap the server to Qwen3-Coder-30B-A3B first; use
#                         for the residue that survives a qwen3.6 rerun
#
# Refuses while a queue run is live: serve.sh kills stray llama-servers, so
# restarting it mid-run would shoot the running queue in the head.
set -eu
LOCAL="$HOME/pumpkin-local"

if pgrep -f "run_all.py" >/dev/null; then
  echo "a queue run is still live, refusing (pgrep run_all.py)"
  exit 1
fi

FAILED=$(python3 - << 'EOF'
import json
try:
    d = json.load(open("/tmp/pumpkin_run_status.json"))
except (OSError, ValueError):
    d = {}
ok = {"candidate"}
print(" ".join(k for k, v in sorted(d.items()) if v.get("phase") not in ok))
EOF
)
if [ -z "$FAILED" ]; then
  echo "nothing failed in the last run, nothing to do"
  exit 0
fi
echo "rerunning: $FAILED"

# 2026-07-29: the A3B ggufs are gone; Qwen3.6-27B dense is the only model and
# serve.sh's defaults (auto-fit placement) are already correct for it. The old
# --coder / --dense27 flags are accepted and ignored so muscle memory and any
# armed chains do not break.
if [ -n "${1:-}" ]; then
  echo "note: '$1' ignored; Qwen3.6-27B dense is the only model now"
fi

bash "$LOCAL/serve.sh"   # kills strays, loads, waits for /health, exits 0

PATHS=""
for s in $FAILED; do
  [ -f "$LOCAL/specs/$s.md" ] && PATHS="$PATHS $LOCAL/specs/$s.md"
done

# shellcheck disable=SC2086
python3 "$LOCAL/run_all.py" $PATHS --max-iters 6 \
  2>&1 | tee "$LOCAL/logs/rerun-$(date +%m%d-%H%M).log"
