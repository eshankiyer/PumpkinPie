#!/usr/bin/env python3
"""
Reclaim disk. Cargo target directories are the single largest thing this pipeline
produces, and nothing was deleting them.

Measured: 48 target directories, the largest 29 GB, 382 GB total against 403 GB
free. Each spec run grows its own. Left alone overnight the disk fills, and a
full disk fails builds with errors that point anywhere except at the disk.

Policy, cheapest first:

  1. never touch a worktree named by a spec that has not finished
  2. delete target directories for worktrees no spec references at all
  3. if still under the floor, delete the oldest target directories among
     finished specs, largest first, until back above it

A deleted target directory costs one cold rebuild, about six minutes. A full
disk costs the whole night.

    reap.py --scan     what would go
    reap.py --run      delete it
    reap.py --run --floor-gb 250
"""

import argparse
import glob
import json
import os
import shutil
import subprocess
import sys

LOCAL = os.path.dirname(os.path.abspath(__file__))
SPECS = os.path.join(LOCAL, "specs")
WT_ROOT = "/home/eshanki/pumpkin-wt"
STATUS = "/tmp/pumpkin_run_status.json"
DEFAULT_FLOOR_GB = 200


def free_gb(path="/"):
    st = os.statvfs(path)
    return st.f_bavail * st.f_frsize / 1e9


def dir_gb(path):
    rc = subprocess.run(
        ["bash", "-c", f"du -sb {path!r} 2>/dev/null | cut -f1"],
        capture_output=True,
        text=True,
    )
    try:
        return int(rc.stdout.strip()) / 1e9
    except ValueError:
        return 0.0


def active_worktrees():
    """Worktrees named by a spec that has not reached a terminal state.

    A spec mid-flight owns its build directory. Deleting it underneath a running
    cargo is how you get a corrupted target dir and a failure that looks like a
    compiler bug.
    """
    try:
        state = json.load(open(STATUS))
    except (OSError, ValueError):
        state = {}

    active, referenced = set(), set()
    for spec in glob.glob(os.path.join(SPECS, "*.md")):
        name = os.path.basename(spec)[:-3]
        wt = None
        for line in open(spec).read().splitlines():
            if line.startswith("WORKTREE:"):
                wt = line.partition(":")[2].strip()
                break
        if not wt:
            continue
        referenced.add(wt)
        phase = state.get(name, {}).get("phase", "")
        if phase and phase not in ("candidate", "clean", "failed", "aborted"):
            active.add(wt)
    return active, referenced


def candidates():
    active, referenced = active_worktrees()
    out = []
    for target in glob.glob(os.path.join(WT_ROOT, "*", "target")):
        wt = os.path.dirname(target)
        if wt in active:
            continue
        out.append(
            {
                "target": target,
                "worktree": wt,
                "gb": dir_gb(target),
                "referenced": wt in referenced,
                "mtime": os.path.getmtime(target),
            }
        )
    # Unreferenced first, then oldest, then largest. Unreferenced worktrees are
    # leftovers from finished work and cost nothing to rebuild because nothing
    # is going to rebuild them.
    out.sort(key=lambda d: (d["referenced"], d["mtime"], -d["gb"]))
    return out


def retire_worktrees(dry):
    """Remove worktrees no spec references and with nothing uncommitted.

    57 worktrees accumulated, most of them finished work whose branch is already
    pushed. A worktree is cheap to recreate from its branch and expensive to
    keep, so anything with no spec pointing at it and no unpushed state goes.

    Uncommitted changes are never discarded here: that is somebody's work in
    progress, and the branch may not exist anywhere else yet.
    """
    _, referenced = active_worktrees()
    removed, kept = [], []
    for wt in sorted(glob.glob(os.path.join(WT_ROOT, "*"))):
        if not os.path.isdir(os.path.join(wt, ".git")) and not os.path.isfile(
            os.path.join(wt, ".git")
        ):
            continue
        if wt in referenced:
            continue
        rc = subprocess.run(
            ["bash", "-c", "git status --porcelain"],
            cwd=wt,
            capture_output=True,
            text=True,
        )
        if rc.stdout.strip():
            kept.append((os.path.basename(wt), "uncommitted changes"))
            continue
        removed.append(os.path.basename(wt))
        if not dry:
            subprocess.run(
                ["bash", "-c", f"git worktree remove --force {wt!r}"],
                cwd="/home/eshanki/Pumpkin",
                capture_output=True,
                text=True,
            )
    return removed, kept


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scan", action="store_true")
    ap.add_argument("--run", action="store_true")
    ap.add_argument("--floor-gb", type=float, default=DEFAULT_FLOOR_GB)
    ap.add_argument(
        "--retire-worktrees",
        action="store_true",
        help="also remove worktrees no spec references",
    )
    a = ap.parse_args()
    dry = not a.run

    have = free_gb()
    print(f"free {have:.0f} GB, floor {a.floor_gb:.0f} GB")

    cands = candidates()
    total = sum(c["gb"] for c in cands)
    print(f"{len(cands)} reapable target dir(s), {total:.0f} GB")

    if have >= a.floor_gb:
        # Still take the free ones. An unreferenced target directory is pure
        # waste whether or not the disk is tight yet.
        cands = [c for c in cands if not c["referenced"]]
        if cands:
            print(f"above floor; reaping {len(cands)} unreferenced dir(s) anyway")
        else:
            print("above floor, no unreferenced target dirs")

    reclaimed = 0.0
    for c in cands:
        if have + reclaimed >= a.floor_gb and c["referenced"]:
            break
        tag = "unreferenced" if not c["referenced"] else "finished"
        print(f"  {'would delete' if dry else 'deleting'} {c['gb']:6.1f} GB  "
              f"{os.path.basename(c['worktree'])}  ({tag})")
        if not dry:
            shutil.rmtree(c["target"], ignore_errors=True)
        reclaimed += c["gb"]

    print(f"{'would reclaim' if dry else 'reclaimed'} {reclaimed:.0f} GB")

    if a.retire_worktrees:
        removed, kept = retire_worktrees(dry)
        print(
            f"{'would retire' if dry else 'retired'} {len(removed)} worktree(s)"
            + (f": {', '.join(removed[:12])}" if removed else "")
        )
        for name, why in kept[:8]:
            print(f"  kept {name}: {why}")

    if not dry:
        print(f"free now {free_gb():.0f} GB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
