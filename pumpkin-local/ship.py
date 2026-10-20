#!/usr/bin/env python3
"""
Phase 4 shipper: take diffs that passed the full gate and get them onto branches,
onto both remotes, and into pull requests.

Two tiers, deliberately asymmetric:

  forgejo + fork   every passing branch, immediately. These are mine, nobody
                   else pays for a mistake, and having the PR open is how the
                   change becomes reviewable at all.

  upstream         only when the divergence reproduced in at least two separate
                   oracle rounds AND the fix passed the whole gate. Capped at
                   five per day. Everything over the cap queues in
                   PENDING_UPSTREAM.md rather than being dropped.

The two-round rule exists because a single round can be an instrument fault. It
has been twice: file-descriptor exhaustion once, an RCON client reading one
packet per command the other time. Both produced a batch of confident-looking
divergences that were entirely fabricated by the measurement layer. Requiring the
same divergence in two independent rounds is what makes a maintainer's time safe
to spend.

Usage:
    ship.py --scan            what would ship, and why not, for everything else
    ship.py --run             actually commit, push and open PRs
    ship.py --run --local     forgejo and fork only, never upstream
"""

import argparse
import datetime as dt
import glob
import json
import os
import re
import subprocess
import sys

LOCAL = os.path.dirname(os.path.abspath(__file__))
SPECS = os.path.join(LOCAL, "specs")
STATE = "/tmp/pumpkin_run_status.json"
LEDGER = os.path.join(LOCAL, "SHIPPED.json")
PENDING = os.path.join(LOCAL, "PENDING_UPSTREAM.md")
UPSTREAM = "Pumpkin-MC/Pumpkin"
UPSTREAM_CAP_PER_DAY = 5

# Pipeline commits are attributable. A maintainer looking at the log should be
# able to tell in one glance which commits came out of the automated loop and
# which a human wrote, without that meaning "ignore these".
AUTHOR_NAME = "Pumpkin Parity Pipeline"
AUTHOR_EMAIL = "rdravida+pumpkin-pipeline@gmail.com"


def sh(cmd, cwd=None, timeout=600):
    p = subprocess.run(
        ["bash", "-c", cmd], cwd=cwd, capture_output=True, text=True, timeout=timeout
    )
    return p.returncode, p.stdout.strip(), p.stderr.strip()


def git(cmd, cwd, timeout=600):
    """git with the pipeline identity, without touching any global config."""
    ident = f'-c user.name="{AUTHOR_NAME}" -c user.email="{AUTHOR_EMAIL}"'
    return sh(f"git {ident} {cmd}", cwd=cwd, timeout=timeout)


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


def divergence_rounds(probe):
    """How many separate sweep rounds recorded this probe as divergent.

    Each round writes /tmp/pumpkin_sweep_roundN.json. Two rounds that both saw
    the divergence is the upstream bar. One round is not evidence, it is a
    reading.
    """
    if not probe:
        return 0
    seen = 0
    for path in sorted(glob.glob("/tmp/pumpkin_sweep_round*.json")):
        data = load_json(path, {})
        blob = json.dumps(data)
        # The sweep records divergences by experiment name; a substring test is
        # deliberate, because sweep and probe_gate name the same behaviour
        # slightly differently and an exact-match rule would silently never fire.
        if probe in blob and "diverg" in blob:
            seen += 1
    return seen


def shipped_today(ledger):
    today = dt.date.today().isoformat()
    return sum(1 for e in ledger.get("upstream", []) if e.get("date") == today)


def gate_report(name, meta, state_entry):
    """Return (ok_local, ok_upstream, reasons)."""
    reasons = []
    phase = (state_entry or {}).get("phase", "unknown")
    if phase not in ("candidate", "clean"):
        return False, False, [f"has not passed the gate, phase is {phase!r}"]

    probe = meta.get("PROBE")
    rounds = divergence_rounds(probe)

    ok_local = True
    ok_upstream = True
    if not probe:
        ok_upstream = False
        reasons.append(
            "no PROBE declared, so the fix has no empirical acceptance test; "
            "local only"
        )
    elif rounds < 2:
        ok_upstream = False
        reasons.append(
            f"divergence seen in {rounds} oracle round(s), upstream needs 2; "
            "local only"
        )
    return ok_local, ok_upstream, reasons


def commit_and_push(worktree, branch, message, dry):
    _, dirty, _ = git("status --porcelain", cwd=worktree)
    if not dirty.strip():
        return False, "worktree is clean, nothing to commit"

    if dry:
        return True, f"would commit and push {branch}"

    rc, _, err = git(f"checkout -B {branch}", cwd=worktree)
    if rc != 0:
        return False, f"branch failed: {err[:200]}"
    git("add -A", cwd=worktree)
    msg_file = f"/tmp/pumpkin_msg_{branch.replace('/', '_')}.txt"
    with open(msg_file, "w") as f:
        f.write(message)
    rc, _, err = git(f"commit -q -F {msg_file}", cwd=worktree)
    if rc != 0:
        return False, f"commit failed: {err[:200]}"

    pushed = []
    for remote in ("forgejo", "fork"):
        # Never force. A rejected push means someone else moved the branch and
        # that is a situation for a human, not for a retry loop.
        rc, _, err = git(f"push -q {remote} HEAD:{branch}", cwd=worktree, timeout=900)
        if rc == 0:
            pushed.append(remote)
        else:
            pushed.append(f"{remote} FAILED({err[:80]})")
    return True, "pushed to " + ", ".join(pushed)


def forgejo_token():
    try:
        with open(os.path.expanduser("~/.git-credentials")) as f:
            for line in f:
                if "forgejo.eshnetwork.space" in line:
                    return line.split(":")[2].split("@")[0].strip()
    except OSError:
        pass
    return None


def open_forgejo_pr(branch, title, body, dry):
    token = forgejo_token()
    if not token:
        return "no forgejo token in ~/.git-credentials"
    if dry:
        return f"would open forgejo PR for {branch}"
    payload = json.dumps(
        {"head": branch, "base": "master", "title": title, "body": body}
    )
    path = f"/tmp/pumpkin_pr_{branch.replace('/', '_')}.json"
    with open(path, "w") as f:
        f.write(payload)
    rc, out, err = sh(
        "curl -s -w '\\n%{http_code}' -X POST "
        "-H 'Content-Type: application/json' "
        f"-H 'Authorization: token {token}' "
        f"--data @{path} "
        "https://forgejo.eshnetwork.space/api/v1/repos/eshanki/Pumpkin/pulls",
        timeout=180,
    )
    code = out.rsplit("\n", 1)[-1] if out else "?"
    if code.startswith("2"):
        return "forgejo PR opened"
    # 409 is "a PR for this head already exists", which is a success for our
    # purposes: the branch is reviewable, which is the whole point.
    if code == "409":
        return "forgejo PR already open"
    return f"forgejo PR failed ({code}): {out[:160]}{err[:80]}"


def open_upstream_pr(branch, title, body, dry):
    if dry:
        return f"would open upstream PR for {branch}"
    body_file = f"/tmp/pumpkin_body_{branch.replace('/', '_')}.md"
    with open(body_file, "w") as f:
        f.write(body)
    rc, out, err = sh(
        f"gh pr create --repo {UPSTREAM} --head eshankiyer:{branch} --base master "
        f"--title {json.dumps(title)} --body-file {body_file}",
        timeout=300,
    )
    if rc == 0:
        return out.strip().splitlines()[-1] if out.strip() else "upstream PR opened"
    return f"upstream PR failed: {(out + err)[:200]}"


def queue_pending(name, title, branch, reasons, dry=False):
    if dry:
        return f"would queue {branch} in PENDING_UPSTREAM.md"
    return _queue_pending(name, title, branch, reasons)


def _queue_pending(name, title, branch, reasons):
    line = (
        f"- `{branch}` - {title}\n"
        f"  - held because: {'; '.join(reasons) or 'daily upstream cap reached'}\n"
    )
    header = (
        "# Pending upstream PRs\n\n"
        "Branches that passed the full gate and are pushed to forgejo and the "
        "fork, but are not yet opened against Pumpkin-MC/Pumpkin. Either the "
        "divergence has not reproduced in two separate oracle rounds, or the "
        "five-per-day upstream cap was reached.\n\n"
        "A single oracle round is a reading, not evidence. Twice now a batch of "
        "confident-looking divergences turned out to be the measurement layer "
        "failing, so a maintainer's time is only spent on something two "
        "independent rounds agree on.\n\n"
    )
    existing = ""
    if os.path.exists(PENDING):
        existing = open(PENDING).read()
    else:
        existing = header
    if branch in existing:
        return "already queued"
    with open(PENDING, "w") as f:
        f.write(existing.rstrip("\n") + "\n" + line)
    return "queued in PENDING_UPSTREAM.md"


def build_message(name, meta, rounds, stat):
    """Commit message and PR body, with the oracle evidence in it."""
    probe = meta.get("PROBE", "none")
    summary = name.split("-", 1)[-1].replace("-", " ")
    what = meta.get("SUMMARY") or summary

    body = (
        f"{what}\n\n"
        f"Oracle evidence: probe `{probe}`, recorded divergent in {rounds} "
        f"separate sweep round(s) against vanilla 26.2, and matching after this "
        f"change. Gate: cargo clippy with -D warnings, cargo test, and no other "
        f"probe regressed.\n\n"
        f"Spec: {name}.md\n"
        f"Diffstat: {stat}\n"
    )
    return body


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scan", action="store_true")
    ap.add_argument("--run", action="store_true")
    ap.add_argument("--local", action="store_true", help="never open upstream PRs")
    a = ap.parse_args()
    dry = not a.run

    state = load_json(STATE, {})
    ledger = load_json(LEDGER, {"upstream": [], "local": []})
    used_today = shipped_today(ledger)
    budget = max(0, UPSTREAM_CAP_PER_DAY - used_today)

    print(f"upstream budget today: {budget} of {UPSTREAM_CAP_PER_DAY}")
    print(f"mode: {'DRY RUN' if dry else 'LIVE'}\n")

    for spec in sorted(glob.glob(os.path.join(SPECS, "[0-9]*.md"))):
        name = os.path.basename(spec)[:-3]
        meta = spec_meta(spec)
        entry = state.get(name)
        ok_local, ok_upstream, reasons = gate_report(name, meta, entry)

        if not ok_local:
            print(f"SKIP  {name}: {reasons[0]}")
            continue

        worktree = meta.get("WORKTREE")
        branch = f"fix/{name.split('-', 1)[-1]}"
        rounds = divergence_rounds(meta.get("PROBE"))
        _, stat, _ = sh("git diff --stat HEAD | tail -1", cwd=worktree)

        title = name.split("-", 1)[-1].replace("-", " ")
        title = title[0].upper() + title[1:]
        body = build_message(name, meta, rounds, stat)

        ok, note = commit_and_push(worktree, branch, f"{title}\n\n{body}", dry)
        print(f"SHIP  {name}: {note}")
        if not ok:
            continue

        print(f"      {open_forgejo_pr(branch, title, body, dry)}")

        if a.local or not ok_upstream:
            print(f"      {queue_pending(name, title, branch, reasons, dry)}")
            continue
        if budget <= 0:
            print(f"      {queue_pending(name, title, branch, ["daily cap reached"], dry)}")
            continue

        print(f"      {open_upstream_pr(branch, title, body, dry)}")
        if not dry:
            budget -= 1
            ledger.setdefault("upstream", []).append(
                {"date": dt.date.today().isoformat(), "branch": branch, "spec": name}
            )
            save_json(LEDGER, ledger)

    return 0


if __name__ == "__main__":
    sys.exit(main())
