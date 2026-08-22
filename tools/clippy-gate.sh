#!/usr/bin/env bash
# Clippy gate that cannot miss a diagnostic. See tools/clippy-gate.py for why this
# exists rather than grepping `--message-format short`.
#
# Usage:
#   tools/clippy-gate.sh                  # whole workspace, all features (what CI runs)
#   tools/clippy-gate.sh -p pumpkin       # one crate, for a faster inner loop
#
# Exits non-zero if any clippy error is found.
set -uo pipefail

# shellcheck disable=SC1090
source ~/.cargo/env

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ARGS=("$@")
if [ ${#ARGS[@]} -eq 0 ]; then
  ARGS=(--workspace --all-targets --all-features)
fi

: "${CARGO_BUILD_JOBS:=3}"
export CARGO_BUILD_JOBS

RUSTFLAGS="-Dwarnings" cargo clippy "${ARGS[@]}" --message-format json 2>/dev/null \
  | python3 "$HERE/clippy-gate.py"
STATUS=$?

if [ $STATUS -eq 0 ]; then
  echo "clippy-gate: clean"
else
  echo "clippy-gate: FAILED"
fi
exit $STATUS
