#!/bin/bash
# Continuous autoport loop. night_run.py's lockfile prevents overlap;
# budget.py caps daily sonnet spend no matter how many cycles run.
cd "$HOME/pumpkin-local"
while true; do
    python3 night_run.py
    echo "[loop] cycle done $(date -Is), sleeping 10m"
    sleep 600
done
