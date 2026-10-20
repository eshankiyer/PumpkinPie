#!/bin/bash
source ~/.cargo/env
WT=$1
cd "$WT" || exit 1
cargo fmt --all
echo "=== CLIPPY $WT"
RUSTFLAGS="-D warnings" cargo clippy --all-targets --all-features --message-format short 2>&1 | grep -E "error" | head -30
echo "=== TESTS"
cargo test --workspace 2>&1 | grep -E "^test result: FAILED|^error|panicked" | head -20
cargo test --workspace 2>&1 | grep -c "^test result: ok"
echo "GATE DONE $WT"
