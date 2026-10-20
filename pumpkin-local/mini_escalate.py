#!/usr/bin/env python3
"""
Escalation tier: hand the confirmed gaps the local model FAILED to Claude Code.

Split model strategy (Ragini's choice): the local 27B grinds every confirmed
item for free; whatever it cannot land goes to Sonnet via headless Claude Code,
which reimplements the fix properly. Cost falls only on the hard subset.

Discipline carried from escalate.py, all enforced here:
  model      sonnet, never opus
  budget     BUDGET_N escalations per night, hardest-first by oracle value
  attempts   one per item per night, full clippy gate applies after
  probe      one cheap call first; a rate limit ends the night's escalation
  legality   reimplements from the SPEC (observed behaviour), never from
             Minecraft/Paper source. Clean-room is not optional.

    python3 mini_escalate.py --scan   # what would escalate, in order
    python3 mini_escalate.py --run    # actually escalate (spends quota)
"""

import argparse
import datetime as dt
import glob
import json
import os
import subprocess

import mini_spree as ms  # reuse parse_queue, eligible, task_text, sh, WORKTREE

HERE = os.path.dirname(os.path.abspath(__file__))
CLI = os.environ.get("CLAUDE_CLI", os.path.expanduser("~/.local/bin/claude"))
MODEL = "sonnet"
BUDGET_N = int(os.environ.get("ESCALATE_BUDGET", "3"))
BUDGET_FILE = os.path.join(HERE, "BUDGET.json")
CAND = os.path.join(HERE, "candidates")
PENDING = os.path.join(HERE, "PENDING_SESSION.md")


def failed_items():
    """Eligible confirmed items with NO candidate diff yet = local model failed."""
    done = {os.path.basename(c).split("-")[0] for c in glob.glob(f"{CAND}/*.diff")}
    out = []
    for it in ms.parse_queue():
        if ms.eligible(it)[0] and it["id"] not in done:
            out.append(it)
    return out


def spent_today():
    try:
        b = json.load(open(BUDGET_FILE))
    except (OSError, ValueError):
        b = {}
    today = dt.date.today().isoformat()
    return b.get("spent", {}).get("escalate", 0) if b.get("date") == today else 0


def record_spend(n):
    today = dt.date.today().isoformat()
    try:
        b = json.load(open(BUDGET_FILE))
    except (OSError, ValueError):
        b = {}
    if b.get("date") != today:
        b = {"date": today, "spent": {}}
    b.setdefault("spent", {})["escalate"] = b["spent"].get("escalate", 0) + n
    json.dump(b, open(BUDGET_FILE, "w"), indent=2)


def probe_quota():
    """One cheap call. False on any auth/limit problem - never retry a limit."""
    if not os.path.exists(CLI):
        return False, f"claude CLI not found at {CLI}"
    try:
        p = subprocess.run(
            f'printf "Reply with: 1" | {CLI} -p --model {MODEL}',
            shell=True, capture_output=True, text=True, timeout=90,
        )
    except subprocess.TimeoutExpired:
        return False, "probe timed out"
    if p.returncode != 0:
        return False, f"probe rc={p.returncode}: {(p.stdout + p.stderr)[:160]}"
    return True, "available"


ESCALATE_PROMPT = """You are working in an existing Rust codebase (Pumpkin, a \
Minecraft server targeting Java Edition 26.2). A smaller local model failed to \
implement the fix below. Implement it correctly.

Edit files directly in this working directory. Make the SMALLEST change that is \
actually correct; do not fix anything the spec did not ask for. Then run \
`cargo clippy -p pumpkin --all-targets -- -D warnings` and make sure it is clean.

Hard constraints: clippy all/pedantic/nursery/cargo are denied. Never replace an \
atomic fetch_sub/fetch_add with load-arith-store (tick tasks are concurrent; \
that is a lost-update race). u8 subtraction must saturate. Identifiers in doc \
comments need backticks. No emoji, no em-dashes.

LEGALITY: reimplement the described BEHAVIOUR from scratch. Do NOT copy or \
translate Minecraft, Paper, Spigot, or any decompiled source. Clean-room only.

If the spec states the vanilla behaviour, implement exactly that. If it does \
not, stop and say what is missing rather than guessing.

--- TASK ---
{task}
"""


def escalate_one(item):
    ms.sh("git checkout -- . && git clean -fdq", cwd=ms.WORKTREE)
    prompt = ESCALATE_PROMPT.format(task=ms.task_text(item))
    pf = f"/tmp/pumpkin_mesc_{item['id']}.txt"
    open(pf, "w").write(prompt)
    cmd = (
        f'cd {ms.WORKTREE} && {CLI} -p --model {MODEL} '
        f'--permission-mode acceptEdits --max-turns 80 "$(cat {pf})" </dev/null'
    )
    try:
        subprocess.run(["bash", "-c", cmd], timeout=3600, capture_output=True)
    except subprocess.TimeoutExpired:
        return "TIMEOUT"

    _, diff = ms.sh("git diff", cwd=ms.WORKTREE)
    if not diff.strip():
        return "NOEDIT"
    rc, out = ms.sh(
        "source ~/.cargo/env && cargo clippy -p pumpkin --all-targets "
        "-- -D warnings 2>&1 | tail -40",
        cwd=ms.WORKTREE, timeout=2400,
    )
    if rc != 0:
        return f"BUILDFAIL:{ms.real_errors(out)}"
    os.makedirs(CAND, exist_ok=True)
    name = f"{item['id']}-esc.diff"
    open(os.path.join(CAND, name), "w").write(diff)
    return "CANDIDATE"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scan", action="store_true")
    ap.add_argument("--run", action="store_true")
    a = ap.parse_args()

    items = failed_items()
    # hardest-first by oracle value if present, else by id
    items.sort(key=lambda i: i.get("value", 0), reverse=True)
    left = max(0, BUDGET_N - spent_today())
    print(f"{len(items)} local-failed item(s); budget left tonight: {left}")
    picks = items[:left]
    for it in picks:
        print(f"  would escalate {it['id']}  {it['name'][:50]}")
    if a.scan or not a.run:
        return
    if not picks:
        print("nothing to escalate (budget spent or no failures)")
        return

    ok, why = probe_quota()
    if not ok:
        print(f"escalation unavailable: {why}")
        return

    n = 0
    for it in picks:
        print(f"escalating {it['id']} ...", flush=True)
        res = escalate_one(it)
        print(f"  {it['id']}: {res}", flush=True)
        n += 1
        record_spend(1)
        if res.startswith(("TIMEOUT", "BUILDFAIL")):
            with open(PENDING, "a") as f:
                f.write(f"- {dt.date.today()} {it['id']} escalation {res}, needs a human\n")
    print(f"\nescalated {n}; candidates in {CAND}/ (marked -esc)")


if __name__ == "__main__":
    main()
