#!/bin/bash
source ~/.cargo/env
cd "$1" || exit 1
cargo clippy --fix --allow-dirty --workspace --all-targets --all-features >/dev/null 2>&1
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets --all-features --message-format short 2>&1 | grep -E "^crates.*error"
echo "FIX DONE $1"
