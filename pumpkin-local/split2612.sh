#!/usr/bin/env bash
# Split PR #2612 into three atomic PRs, per Snowiiii's review.
# Each piece: branch off upstream master, take exactly one file from the old
# branch, build-gate it, push, open a PR. Nothing ships without its own build.
set -u
source ~/.cargo/env
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0

WT=/home/eshanki/pumpkin-wt/skylightbounds
SRC=fork/fix/startup-robustness
cd "$WT" || exit 1

git fetch fork fix/startup-robustness || exit 1
git fetch origin master || exit 1

ship() { # name file crate title bodyfile
  local name="$1" file="$2" crate="$3" title="$4" bodyfile="$5"
  echo "==== $name ===="
  git reset -q --hard origin/master
  git checkout -q -B "$name" origin/master || return 1
  git checkout -q "$SRC" -- "$file" || return 1
  RUSTFLAGS="-D warnings" cargo clippy -p "$crate" --all-targets 2>&1 | tail -3
  local rc=${PIPESTATUS[0]}
  if [ "$rc" -ne 0 ]; then echo "BUILD FAILED for $name, not shipping"; git reset -q --hard origin/master; return 1; fi
  git add -A
  git -c user.name="Ragini Dravida" -c user.email="rdravida@gmail.com" \
    commit -q -m "$title" || return 1
  git push -q fork "HEAD:$name" || return 1
  gh pr create --repo Pumpkin-MC/Pumpkin --head "eshankiyer:$name" --base master \
    --title "$title" --body-file "$bodyfile" 2>&1 | tail -1
}

cat > /tmp/b1.md <<"EOF"
Split out of #2612 per review: one fix per PR.

A uniform non-zero light section (LightContainer::Empty(15), e.g. fully sky-lit air) was omitted from the chunk NBT on save because only Full arrays were serialized, and the reader assumes 0 for a missing tag, so the light level was silently lost across save and load. Uniform sections are now materialized into the nibble array unless their value is the 0 the reader already assumes.
EOF

cat > /tmp/b2.md <<"EOF"
Split out of #2612 per review: one fix per PR.

A file logger that failed to initialize (unwritable logs directory, for example) panicked before any subscriber was installed, so the operator got a bare backtrace instead of a message. The failure is now reported on stderr and the server continues with console logging only.
EOF

cat > /tmp/b3.md <<"EOF"
Split out of #2612 per review: one fix per PR.

A failed level.dat backup copy aborted startup via unwrap even though the real level.dat is untouched by that failure. It now logs a warning and continues, since an unwritable world directory should degrade the backup, not the server.
EOF

ship fix/file-logger-fallback \
  pumpkin/src/lib.rs pumpkin \
  "fix: fall back to console logging when the file logger cannot start" /tmp/b2.md

ship fix/level-dat-backup-nonfatal \
  pumpkin/src/server/mod.rs pumpkin \
  "fix: do not abort startup when the level.dat backup copy fails" /tmp/b3.md

echo ALL_DONE
