#!/usr/bin/env python3
"""
Escalation tier: hand the specs the local 30B could not finish to Claude Code.

Sits between the local model and a live session. The local model gets six
iterations; anything it fails goes here for exactly one attempt, and anything
this fails goes to a human. Nothing retries against a quota.

Constraints, all enforced here rather than by convention:

  model            sonnet, never opus
  budget           3 escalated specs per night, hardest-first by oracle value
  attempts         one per spec per night, full gate applies
  on failure       PENDING_SESSION.md, no retry
  on rate limit    stop escalating for the night and log it

The rate-limit rule is the important one. A retry loop against a limit burns the
quota that the next morning's session needs, and it converts a temporary
condition into an outage. So the probe runs BEFORE each escalation, costs one
token, and a limit response ends the night's escalation immediately.

Usage:
    escalate.py --probe       just check whether escalation is available
    escalate.py --scan        what would be escalated, in order
    escalate.py --run         actually escalate
"""

import argparse
import datetime as dt
import glob
import json
import os
import subprocess
import sys

LOCAL = os.path.dirname(os.path.abspath(__file__))
SPECS = os.path.join(LOCAL, "specs")
STATE = "/tmp/pumpkin_run_status.json"
PENDING_SESSION = os.path.join(LOCAL, "PENDING_SESSION.md")
LEDGER = os.path.join(LOCAL, "ESCALATED.json")

CLI = os.environ.get(
    "CLAUDE_CLI", "/home/eshanki/.config/Claude/claude-code/2.1.215/claude"
)
MODEL = "sonnet"
NIGHTLY_BUDGET = 3

# Phases that mean the local model finished the job. Anything else is a
# candidate for escalation.
DONE_PHASES = ("candidate", "clean")


def sh(cmd, timeout=1800, cwd=None):
    p = subprocess.run(
        ["bash", "-c", cmd], capture_output=True, text=True, timeout=timeout, cwd=cwd
    )
    return p.returncode, p.stdout.strip(), p.stderr.strip()


def load_json(path, default):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return default


def save_json(path, data):
    with open(path, "w") as f:
        json.dump(data, f, indent=2)


def spec_meta(path):
    head = open(path).read().partition("\n---\n")[0]
    meta = {}
    for line in head.splitlines():
        k, _, v = line.partition(":")
        k = k.strip().upper()
        if k and v.strip():
            meta.setdefault(k, v.strip())
    return meta


def probe_quota():
    """One-token call to find out whether escalation is possible at all.

    Returns (available: bool, reason: str). Anything that is not a clean success
    is treated as unavailable; this is deliberately not clever about
    distinguishing failure modes, because the only safe response to an ambiguous
    quota signal is to stop.
    """
    if not os.path.exists(CLI):
        return False, f"claude CLI not found at {CLI}"
    rc, out, err = sh(
        f'{CLI} -p --model {MODEL} "Reply with the single character: 1" </dev/null',
        timeout=180,
    )
    blob = (out + err).lower()
    if "not logged in" in blob:
        return False, "claude CLI is not logged in, escalation unavailable"
    for marker in ("rate limit", "usage limit", "429", "quota", "too many requests"):
        if marker in blob:
            return False, f"rate limited ({marker}), no escalation tonight"
    if rc != 0:
        return False, f"probe failed rc={rc}: {blob[:200]}"
    return True, "available"


def oracle_value(meta):
    """Rank for hardest-first ordering.

    A spec with a PROBE is worth more than one without, because its fix can be
    proven against vanilla rather than argued about. Beyond that, more files and
    a bigger blast radius means more value in spending the stronger model on it.
    """
    score = 0
    if meta.get("PROBE"):
        score += 100
    if meta.get("TEST"):
        score += 40
    score += 5 * len([f for f in meta.get("FILES", "").split(",") if f.strip()])
    if meta.get("FUNC"):
        score += 10
    return score


def candidates():
    state = load_json(STATE, {})
    out = []
    for spec in sorted(glob.glob(os.path.join(SPECS, "[0-9]*.md"))):
        name = os.path.basename(spec)[:-3]
        entry = state.get(name, {})
        phase = entry.get("phase", "never ran")
        if phase in DONE_PHASES:
            continue
        # A spec still mid-flight is not failed; only escalate what the local
        # model gave up on or never reached.
        if phase.startswith(("generating", "building", "testing", "queue")):
            continue
        meta = spec_meta(spec)
        out.append((oracle_value(meta), name, spec, meta, phase))
    out.sort(reverse=True)
    return out


def already_escalated_tonight(ledger):
    today = dt.date.today().isoformat()
    return [e["spec"] for e in ledger.get("runs", []) if e["date"] == today]


def queue_session(name, phase, reason):
    header = (
        "# Pending live session\n\n"
        "Specs that the local 30B could not finish and that the escalation tier "
        "also failed or could not attempt. These need a person, or a session with "
        "more context than a single prompt can carry.\n\n"
        "Nothing here is retried automatically. A spec that has failed both tiers "
        "is failing for a reason that another attempt at the same prompt will not "
        "fix, and retrying it burns quota that the next session needs.\n\n"
    )
    existing = open(PENDING_SESSION).read() if os.path.exists(PENDING_SESSION) else header
    if f"`{name}`" in existing:
        return "already queued"
    line = f"- `{name}` - local phase was {phase!r}\n  - escalation: {reason}\n"
    with open(PENDING_SESSION, "w") as f:
        f.write(existing.rstrip("\n") + "\n" + line)
    return "queued in PENDING_SESSION.md"


def escalate_one(name, spec_path, meta):
    """One attempt, one spec. The full gate still applies afterwards."""
    worktree = meta.get("WORKTREE")
    body = open(spec_path).read()
    prompt = (
        "You are working in an existing Rust codebase (Pumpkin, a Minecraft "
        "server targeting Java Edition 26.2). Below is a specification that a "
        "smaller model failed to implement.\n\n"
        "Implement it. Edit the files directly in this working directory. Make "
        "the smallest change that is actually correct, and do not fix anything "
        "the spec did not ask for.\n\n"
        "Hard constraints: the workspace denies clippy all, pedantic, nursery "
        "and cargo. Never replace an atomic fetch_sub or fetch_add with a load, "
        "arithmetic and store, because tick tasks run concurrently and that is a "
        "lost-update race. u8 subtraction must saturate. Identifiers in doc "
        "comments need backticks. No emoji and no em-dashes.\n\n"
        "If the spec states vanilla behaviour, implement exactly that. If it "
        "does not, stop and say what is missing rather than guessing.\n\n"
        f"--- SPEC {name} ---\n{body}\n"
    )
    prompt_file = f"/tmp/pumpkin_escalate_{name}.txt"
    with open(prompt_file, "w") as f:
        f.write(prompt)

    rc, out, err = sh(
        f'cd {worktree} && {CLI} -p --model {MODEL} '
        f'--permission-mode acceptEdits --max-turns 60 "$(cat {prompt_file})" </dev/null',
        timeout=3600,
    )
    return rc, (out + err)[-2000:]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--probe", action="store_true")
    ap.add_argument("--scan", action="store_true")
    ap.add_argument("--run", action="store_true")
    a = ap.parse_args()

    if a.probe:
        ok, why = probe_quota()
        print(f"escalation {'AVAILABLE' if ok else 'UNAVAILABLE'}: {why}")
        return 0 if ok else 1

    ledger = load_json(LEDGER, {"runs": []})
    done_tonight = already_escalated_tonight(ledger)
    budget = NIGHTLY_BUDGET - len(done_tonight)
    print(f"escalation budget tonight: {budget} of {NIGHTLY_BUDGET}")

    ranked = [c for c in candidates() if c[1] not in done_tonight]
    if not ranked:
        print("nothing to escalate")
        return 0

    print("\nhardest-first order:")
    for value, name, _, _, phase in ranked[:10]:
        print(f"  {value:4d}  {name}  (local phase: {phase})")

    if not a.run:
        return 0

    for value, name, spec_path, meta, phase in ranked:
        if budget <= 0:
            print("\nbudget exhausted for tonight")
            break

        ok, why = probe_quota()
        if not ok:
            print(f"\nSTOP: {why}")
            queue_session(name, phase, why)
            break

        sys.path.insert(0, LOCAL)
        from budget import take  # noqa: PLC0415

        allowed, note = take("escalate")
        print(f"  budget: {note}")
        if not allowed:
            print("  daily Claude budget exhausted, stopping escalation")
            break

        print(f"\nescalating {name} (value {value})")
        rc, tail = escalate_one(name, spec_path, meta)
        ledger.setdefault("runs", []).append(
            {
                "date": dt.date.today().isoformat(),
                "spec": name,
                "rc": rc,
                "value": value,
            }
        )
        save_json(LEDGER, ledger)
        budget -= 1

        if rc != 0:
            print(f"  failed rc={rc}: {tail[:200]}")
            print("  " + queue_session(name, phase, f"escalation failed rc={rc}"))
        else:
            print("  edited; the normal gate decides whether it ships")

    return 0


if __name__ == "__main__":
    sys.exit(main())
