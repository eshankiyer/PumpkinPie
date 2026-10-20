#!/usr/bin/env python3
"""
Autonomous parity-fix spree, driven by mini-swe-agent.

The bespoke FUNC/MULTI/INSERT appliers are retired here in favour of
mini-swe-agent as the edit engine: it edits with bash (sed/cat) in a real
worktree, so there is no addressing scheme of ours to get wrong. What this
driver keeps is the orchestration that mini has no opinion about: pull the
next CONFIRMED gap from WORKQUEUE.md, run mini on it in an isolated worktree,
build-and-clippy gate the result, and bank a candidate diff or record why it
failed.

Hard rule, inherited from WORKQUEUE.md and paid for twice in public
retractions: items whose STATE is not `confirmed`, or whose VANILLA line says
UNVERIFIED, are NEVER implemented. Guessing vanilla behaviour is the most
expensive mistake on this project. This driver refuses to touch them.

    python3 mini_spree.py               # whole confirmed queue, serial
    python3 mini_spree.py --limit 3     # first 3, for a supervised trial
    python3 mini_spree.py --only A1,B2  # specific IDs
"""

import argparse
import json
import os
import re
import subprocess
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WORKQUEUE = os.path.join(HERE, "WORKQUEUE.md")
WORKTREE = os.path.expanduser("~/pumpkin-wt/spree")
REPO = os.path.expanduser("~/Pumpkin")
CAND = os.path.join(HERE, "candidates")
LOGS = os.path.join(HERE, "logs")
GATE = os.path.join(HERE, "gate.py")
MINI = os.path.expanduser("~/.local/bin/mini")
MODEL = "openai/qwen3.6-27b"

ENV = {
    **os.environ,
    "MSWEA_COST_TRACKING": "ignore_errors",
    "OPENAI_API_BASE": "http://127.0.0.1:11434/v1",
    "OPENAI_API_KEY": "dummy",
}


def sh(cmd, cwd=None, timeout=3600):
    p = subprocess.run(
        ["bash", "-c", cmd], cwd=cwd, env=ENV,
        capture_output=True, text=True, timeout=timeout,
    )
    return p.returncode, p.stdout + p.stderr


def run_pg(cmd, cwd=None, timeout=3600):
    """Run in its own process group and kill the WHOLE group on timeout.

    subprocess.run(timeout=) only kills the direct child (the bash shell),
    leaving mini and its model calls orphaned. Those orphans then pile up and
    thrash the box - observed live: A1 timed out but its mini kept running
    while A2 started on top of it. start_new_session isolates the group so a
    timeout kills every descendant.
    """
    import os
    import signal

    proc = subprocess.Popen(
        ["bash", "-c", cmd], cwd=cwd, env=ENV, start_new_session=True
    )
    try:
        proc.wait(timeout=timeout)
        return True
    except subprocess.TimeoutExpired:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
        except ProcessLookupError:
            pass
        return False


def git_diff_bytes(cwd):
    """`git diff` as RAW BYTES, so nothing translates the patch on the way to
    disk. subprocess with text=True applies universal-newline decoding; for a
    candidate diff that must round-trip byte-for-byte through `git apply`, that
    is not good enough. (The escape debris seen in banked diffs -- `u0026` for
    `&`, `u003e` for `>` -- did not come from here: the model emitted sed
    commands with literal `\\u0026`/`\\u003e`, sed dropped the backslash, and the
    corruption landed in the worktree file itself. git diff faithfully recorded
    it. Capturing bytes keeps OUR half honest; gate.py rejects the debris.)"""
    p = subprocess.run(["git", "diff"], cwd=cwd, env=ENV, capture_output=True)
    return p.stdout


def parse_queue():
    """Every ### item in WORKQUEUE.md as a dict of its labelled fields."""
    text = open(WORKQUEUE).read()
    items = []
    # Split on '### ID short name' headers.
    for chunk in re.split(r"\n###\s+", text)[1:]:
        lines = chunk.splitlines()
        header = lines[0].strip()
        m = re.match(r"([A-Z]\d+)\s+(.*)", header)
        if not m:
            continue
        item = {"id": m.group(1), "name": m.group(2).strip(), "raw": chunk}
        for field in ("STATE", "WHERE", "WHAT", "VANILLA", "DO", "CARE", "PROBE"):
            fm = re.search(
                rf"^{field}\s+(.*?)(?=\n[A-Z]+\s|\n###|\Z)", chunk, re.S | re.M
            )
            item[field.lower()] = " ".join(fm.group(1).split()) if fm else ""
        items.append(item)
    return items


def eligible(item):
    """Only confirmed, vanilla-verified items may be implemented."""
    # "confirmed", "confirmed, Wave 2" etc are all confirmed; suspected /
    # absent / blank are not. Prefix match, not exact, so wave-tagged
    # confirmed items are not wrongly excluded.
    if not item.get("state", "").lower().startswith("confirmed"):
        return False, f"STATE is {item.get('state','?')!r}, not confirmed"
    if "UNVERIFIED" in item.get("vanilla", "").upper():
        return False, "VANILLA is UNVERIFIED"
    if not item.get("where") or not item.get("do"):
        return False, "missing WHERE or DO"
    return True, ""


def task_text(item):
    """The problem statement handed to mini. Vanilla is quoted, never guessed."""
    return (
        f"You are fixing a Minecraft-parity bug in the Pumpkin server (Rust, "
        f"target MC 26.2). Work only inside this repo.\n\n"
        f"Bug {item['id']}: {item['name']}\n"
        f"Files: {item['where']}\n"
        f"What is wrong: {item['what']}\n"
        f"Verified vanilla behaviour: {item['vanilla']}\n"
        f"The change to make: {item['do']}\n"
        f"Be careful: {item['care']}\n\n"
        "Make the smallest change that fixes exactly this. Do not touch "
        "unrelated code. When done, run `cargo build -p pumpkin` and make sure "
        "it compiles, then stop. Do not commit."
    )


def real_errors(out):
    return len(
        [l for l in out.splitlines()
         if re.match(r"^error(\[|:)", l) and "could not compile" not in l]
    )


def run_item(item, mini_timeout):
    name = f"{item['id']}-{re.sub(r'[^a-z0-9]+', '-', item['name'].lower())[:30]}"
    print(f"\n=== {name} ===", flush=True)

    sh("git checkout -- . && git clean -fdq", cwd=WORKTREE)
    task = task_text(item)

    traj = os.path.join(LOGS, f"spree-{item['id']}-traj.json")
    log = os.path.join(LOGS, f"spree-{item['id']}.log")
    # mini edits with bash in this worktree; -l 0 disables the cost limit.
    cmd = (
        f"cd {WORKTREE} && {MINI} -y -l 0 -m {MODEL} "
        f"-o {traj} -t {shq(task)} > {log} 2>&1"
    )
    t0 = time.time()
    if not run_pg(cmd, timeout=mini_timeout):
        print(f"  mini timed out after {mini_timeout}s (process group killed)", flush=True)
    dt = time.time() - t0

    raw = git_diff_bytes(WORKTREE)
    if not raw.strip():
        print(f"  no edit produced ({dt:.0f}s)", flush=True)
        return (item["id"], "NOEDIT", dt)

    os.makedirs(CAND, exist_ok=True)
    cand_path = os.path.join(CAND, f"{name}.diff")
    with open(cand_path, "wb") as f:      # bytes: no newline/encoding rewrite
        f.write(raw)

    # The gate is the referee, not an in-worktree clippy. The old gate ran
    # `cargo clippy ... | tail -80` and read the PIPELINE's exit code, which is
    # tail's (0) with no pipefail -- so every clippy failure looked clean and
    # broken diffs banked. gate.py cuts a fresh worktree from verify/all,
    # re-applies THIS candidate, runs the static escape/orphan/no-op checks,
    # then cargo check+clippy+test under an exclusive build lock, and writes a
    # {pass,...} verdict. We trust only its exit code.
    grc, _ = sh(f"python3 {shq(GATE)} {shq(cand_path)} {shq(REPO)}", timeout=7200)
    verdict = {}
    try:
        with open(cand_path + ".gate.json") as f:
            verdict = json.load(f)
    except (OSError, ValueError):
        pass
    if grc != 0 or not verdict.get("pass"):
        stage = verdict.get("stage_failed") or "?"
        reason = verdict.get("reason") or "gate failed"
        print(f"  REJECTED by gate [{stage}]: {reason} ({dt:.0f}s)", flush=True)
        rej = os.path.join(CAND, "rejected")
        os.makedirs(rej, exist_ok=True)
        os.replace(cand_path, os.path.join(rej, f"{name}.diff"))
        return (item["id"], f"REJECT:{stage}", dt)

    # Behavioural gate: if the item carries RCON probes, boot the patched
    # server here and diff it against the live vanilla oracle. This is the
    # real close-the-loop step: clippy proves it compiles, this proves it now
    # behaves like vanilla. Items with no PROBE line degrade to compile-only.
    probes = [c.strip() for c in item.get("probe", "").split(";") if c.strip()]
    if not probes:
        print(f"  CANDIDATE banked, gate clean, no probe ({dt:.0f}s)", flush=True)
        return (item["id"], "CANDIDATE", dt)

    import verify
    rows, note = verify.verify_local(WORKTREE, probes)
    if not rows:
        print(f"  clippy clean but probe rig failed: {note}", flush=True)
        return (item["id"], f"PROBE-RIG:{note}", dt)
    mism = [(c, v, p) for c, v, p, ok in rows if not ok]
    with open(os.path.join(LOGS, f"spree-{item['id']}-probe.log"), "w") as f:
        for c, v, p, ok in rows:
            f.write(f"[{'MATCH' if ok else 'DIFF'}] /{c}\n  vanilla: {v}\n  pumpkin: {p}\n")
    if mism:
        print(f"  compiles but still differs from vanilla on {len(mism)} probe(s)", flush=True)
        return (item["id"], f"PROBE-FAIL:{len(mism)}", dt)
    print(f"  VERIFIED: clippy clean AND matches vanilla ({dt:.0f}s)", flush=True)
    return (item["id"], "VERIFIED", dt)


def shq(s):
    return "'" + s.replace("'", "'\\''") + "'"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--only", default="")
    ap.add_argument("--mini-timeout", type=int, default=3600)
    a = ap.parse_args()

    items = parse_queue()
    only = {s.strip() for s in a.only.split(",") if s.strip()}
    rows, skipped = [], []
    for item in items:
        if only and item["id"] not in only:
            continue
        ok, why = eligible(item)
        if not ok:
            skipped.append((item["id"], why))
            continue
        rows.append(item)
    if a.limit:
        rows = rows[: a.limit]

    print(f"eligible: {len(rows)}   skipped: {len(skipped)}", flush=True)
    for sid, why in skipped[:12]:
        print(f"  skip {sid}: {why}", flush=True)

    results = []
    for item in rows:
        results.append(run_item(item, a.mini_timeout))

    print("\n===== SPREE DONE =====", flush=True)
    for iid, status, dt in results:
        print(f"{status:14} {iid}  {dt:.0f}s", flush=True)
    banked = sum(1 for _, s, _ in results if s == "CANDIDATE")
    print(f"\n{banked}/{len(results)} banked. Diffs in {CAND}/", flush=True)


if __name__ == "__main__":
    main()
