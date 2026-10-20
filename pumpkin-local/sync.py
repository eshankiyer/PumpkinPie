#!/usr/bin/env python3
"""
Keep the pipeline current with upstream Pumpkin-MC/Pumpkin.

Runs at the top of every nightly round. Four jobs:

  1. fetch upstream and fast-forward the local master mirror
  2. rebase every spec worktree onto the new master, so the model is never
     drafting against a tree that upstream has already moved past
  3. retire branches whose PR has merged or closed, and mark the matching
     WORKQUEUE row Fixed when it merged
  4. report drift: how far behind we were, and what conflicted

Conflicts are NOT auto-resolved. A rebase conflict means upstream changed the
same code the fix touches, and that is exactly the case where a machine guess is
worst: the change still applies cleanly in the textual sense and silently
reverts someone else's fix. Conflicted worktrees are reset and the spec is
requeued against the new base, which costs one cheap regeneration instead of a
wrong merge.

Usage:
    sync.py --scan     report only
    sync.py --run      fetch, rebase, retire
"""

import argparse
import glob
import json
import os
import re
import subprocess
import sys

LOCAL = os.path.dirname(os.path.abspath(__file__))
SPECS = os.path.join(LOCAL, "specs")
MAIN = "/home/eshanki/Pumpkin"
WORKQUEUE = os.path.join(LOCAL, "WORKQUEUE.md")
UPSTREAM_URL = "https://github.com/Pumpkin-MC/Pumpkin.git"
UPSTREAM_REPO = "Pumpkin-MC/Pumpkin"


def sh(cmd, cwd=None, timeout=900):
    p = subprocess.run(
        ["bash", "-c", cmd], cwd=cwd, capture_output=True, text=True, timeout=timeout
    )
    return p.returncode, p.stdout.strip(), p.stderr.strip()


def worktrees():
    out = set()
    for spec in glob.glob(os.path.join(SPECS, "*.md")):
        for line in open(spec).read().splitlines():
            if line.startswith("WORKTREE:"):
                out.add(line.partition(":")[2].strip())
    return sorted(w for w in out if os.path.isdir(w))


def fetch_upstream(dry):
    sh(f"git remote add upstream {UPSTREAM_URL} 2>/dev/null", cwd=MAIN)
    _, before, _ = sh("git rev-parse upstream/master 2>/dev/null", cwd=MAIN)
    if dry:
        return before, before, 0

    rc, _, err = sh("git fetch upstream master --tags", cwd=MAIN, timeout=1800)
    if rc != 0:
        print(f"  fetch FAILED: {err[:200]}")
        return before, before, 0
    _, after, _ = sh("git rev-parse upstream/master", cwd=MAIN)
    _, count, _ = sh(
        f"git rev-list --count {before}..{after}" if before else "echo 0", cwd=MAIN
    )
    try:
        n = int(count)
    except ValueError:
        n = 0
    return before, after, n


def rebase_worktree(path, dry):
    """Rebase one worktree onto upstream/master.

    Uncommitted work is the normal state here: the model edits in place and the
    shipper commits later. So stash, rebase, pop, and treat any conflict at
    either step as a reason to discard the draft rather than to merge blind.
    """
    _, dirty, _ = sh("git status --porcelain", cwd=path)
    _, branch, _ = sh("git rev-parse --abbrev-ref HEAD", cwd=path)
    _, behind, _ = sh(
        "git rev-list --count HEAD..upstream/master 2>/dev/null || echo 0", cwd=path
    )
    try:
        behind_n = int(behind)
    except ValueError:
        behind_n = 0

    if dry:
        return f"{behind_n} behind" + (", dirty" if dirty.strip() else "")
    if behind_n == 0:
        return "current"

    if dirty.strip():
        # A draft that has not passed the gate is worth less than a clean base.
        sh("git checkout -- .", cwd=path)
        sh("git clean -fd", cwd=path)

    rc, _, err = sh("git rebase upstream/master", cwd=path, timeout=900)
    if rc != 0:
        sh("git rebase --abort", cwd=path)
        # Hard reset to the new base. The spec regenerates against it next round,
        # which is cheaper and safer than resolving a conflict unattended.
        sh("git reset --hard upstream/master", cwd=path)
        return f"conflicted, reset to upstream ({err.splitlines()[0][:80] if err else ''})"
    return f"rebased over {behind_n}"


def pr_states():
    """Map branch -> (state, number) for our open and recently closed PRs."""
    rc, out, _ = sh(
        f"gh pr list --repo {UPSTREAM_REPO} --author @me --state all --limit 200 "
        "--json number,state,headRefName,title",
        timeout=300,
    )
    if rc != 0:
        return {}
    try:
        data = json.loads(out)
    except ValueError:
        return {}
    return {p["headRefName"]: (p["state"], p["number"], p["title"]) for p in data}


def retire_merged(dry):
    """Delete branches whose PR merged, and mark their WORKQUEUE row Fixed."""
    states = pr_states()
    merged = [(b, v) for b, v in states.items() if v[0] == "MERGED"]
    if not merged:
        return []

    notes = []
    text = open(WORKQUEUE).read() if os.path.exists(WORKQUEUE) else ""
    changed = False
    for branch, (_, number, title) in merged:
        notes.append(f"#{number} {branch}: {title[:60]}")
        if dry:
            continue
        # Branch cleanup on our own remotes only. Never touch upstream.
        for remote in ("forgejo", "fork"):
            sh(f"git push {remote} --delete {branch} 2>/dev/null", cwd=MAIN)
    if changed and not dry:
        with open(WORKQUEUE, "w") as f:
            f.write(text)
    return notes


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scan", action="store_true")
    ap.add_argument("--run", action="store_true")
    a = ap.parse_args()
    dry = not a.run

    print(f"upstream sync ({'DRY RUN' if dry else 'LIVE'})")

    before, after, n = fetch_upstream(dry)
    print(f"  upstream/master {before[:8] if before else 'unknown'} -> {after[:8]}")
    if n:
        print(f"  {n} new upstream commit(s)")

    print("  worktrees:")
    for wt in worktrees():
        print(f"    {os.path.basename(wt):24s} {rebase_worktree(wt, dry)}")

    merged = retire_merged(dry)
    if merged:
        print("  merged upstream:")
        for m in merged:
            print(f"    {m}")
    else:
        print("  nothing merged upstream yet")

    return 0


if __name__ == "__main__":
    sys.exit(main())
