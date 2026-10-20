#!/usr/bin/env python3
"""
Batch driver: runs specs through the local model, then rebases, resolves
conflicts, and pushes.

    python3 driver.py specs/*.md              # implement, build, push each
    python3 driver.py --rebase-only           # just refresh existing branches
    python3 driver.py --status                # what is where

Design notes that matter:

- Upstream master moves. Every branch is rebased onto the latest upstream
  master before pushing, and textual conflicts are handed to the local model
  with both sides plus surrounding context.
- Conflict resolution is the one place a local model can do real damage
  quietly, so anything it resolves is REBUILT before pushing, and any branch
  whose rebase it could not finish cleanly is left untouched and reported
  rather than force-pushed half-resolved.
- Pushes go to Forgejo first, then the GitHub fork, matching project rules.
  PRs are NOT opened here; that stays a human/Claude step so the prose and
  the honesty of the verification section are never model-written.
"""

import argparse
import glob
import json
import os
import re
import subprocess
import sys
import time
import urllib.request

ENDPOINT = os.environ.get("LLAMA_ENDPOINT", "http://127.0.0.1:11434/v1/chat/completions")
MODEL = os.environ.get("PUMPKIN_LOCAL_MODEL", "qwen3.6-35b-a3b")
HARNESS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "harness.py")
MAIN = os.path.expanduser("~/Pumpkin")
UPSTREAM = "https://github.com/Pumpkin-MC/Pumpkin.git"
CARGO_ENV = "source ~/.cargo/env && "

CONFLICT_SYSTEM = """You resolve git merge conflicts in Rust code for the Pumpkin Minecraft server.

You are given a file containing conflict markers. Reply with the ENTIRE resolved
file content and nothing else. No prose, no markdown fences.

Rules:
- Keep BOTH sides' intent when they touch different concerns. Most conflicts here
  are two independent fixes landing in the same function.
- Never drop a fix from either side just to make the conflict go away.
- Remove every conflict marker: <<<<<<<, =======, >>>>>>>
- Do not reformat or "improve" code outside the conflicted region.
- Never write emojis or em-dashes.
- If the two sides genuinely contradict each other and you cannot keep both,
  reply with exactly: CANNOT RESOLVE
"""


def sh(cmd, cwd=None, timeout=2400):
    p = subprocess.run(
        ["bash", "-c", cmd], cwd=cwd, capture_output=True, text=True, timeout=timeout
    )
    return p.returncode, p.stdout.strip(), p.stderr.strip()


def log(m):
    print(f"[driver] {m}", flush=True)


def ask(messages, timeout=1800):
    payload = {
        "model": MODEL,
        "messages": messages,
        "stream": False,
        "temperature": 0.1, "max_tokens": 3000,
    }
    req = urllib.request.Request(
        ENDPOINT,
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        body = json.load(r)
    if "choices" in body:
        return body["choices"][0]["message"]["content"]
    return body["message"]["content"]


def unload():
    """No-op: llama-server keeps the model resident by design. See DECISIONS.md."""
    time.sleep(1)


def fetch_upstream():
    sh(f"git remote add upstream {UPSTREAM} 2>/dev/null", cwd=MAIN)
    rc, _, err = sh("git fetch upstream master", cwd=MAIN, timeout=900)
    if rc != 0:
        log(f"upstream fetch failed: {err}")
        return False
    return True


def strip_fences(text):
    text = text.strip()
    text = re.sub(r"^```[a-zA-Z]*\n", "", text)
    text = re.sub(r"\n```$", "", text)
    return text


def resolve_conflicts(worktree):
    """Ask the model to resolve each conflicted file. Returns (ok, notes)."""
    rc, out, _ = sh("git diff --name-only --diff-filter=U", cwd=worktree)
    files = [f for f in out.splitlines() if f.strip()]
    if not files:
        return True, []
    notes = []
    for rel in files:
        full = os.path.join(worktree, rel)
        content = open(full).read()
        if "<<<<<<<" not in content:
            continue
        log(f"  resolving {rel}")
        try:
            reply = ask(
                [
                    {"role": "system", "content": CONFLICT_SYSTEM},
                    {"role": "user", "content": f"File: {rel}\n\n{content}"},
                ]
            )
        except Exception as e:
            return False, notes + [f"{rel}: model call failed: {e}"]
        reply = strip_fences(reply)
        if "CANNOT RESOLVE" in reply[:200]:
            return False, notes + [f"{rel}: model declined to resolve"]
        if "<<<<<<<" in reply or ">>>>>>>" in reply:
            return False, notes + [f"{rel}: markers left in output"]
        if len(reply) < len(content) * 0.4:
            return False, notes + [
                f"{rel}: output suspiciously short ({len(reply)} vs {len(content)}), refusing"
            ]
        open(full, "w").write(reply if reply.endswith("\n") else reply + "\n")
        sh(f"git add -- '{rel}'", cwd=worktree)
        notes.append(f"{rel}: resolved by model")
    return True, notes


def build(worktree, crates=("pumpkin",)):
    pkgs = " ".join(f"-p {c}" for c in crates)
    cmd = (
        CARGO_ENV
        + f'cd {worktree} && cargo fmt --all && RUSTFLAGS="-D warnings" cargo clippy {pkgs} --all-targets 2>&1'
    )
    rc, out, err = sh(cmd, timeout=2400)
    return rc == 0, (out + err)[-4000:]


def rebase_and_push(worktree, branch, crates=("pumpkin",), do_push=True):
    log(f"rebasing {branch}")
    sh("git rebase --abort 2>/dev/null", cwd=worktree)
    rc, _, _ = sh("git rebase upstream/master", cwd=worktree, timeout=900)
    if rc != 0:
        ok, notes = resolve_conflicts(worktree)
        for n in notes:
            log(f"    {n}")
        if not ok:
            sh("git rebase --abort", cwd=worktree)
            return False, "conflicts not resolvable, branch left as it was"
        rc2, _, err2 = sh("git -c core.editor=true rebase --continue", cwd=worktree)
        if rc2 != 0:
            sh("git rebase --abort", cwd=worktree)
            return False, f"rebase --continue failed: {err2}"

    # anything the model touched must be rebuilt before it can be pushed
    log("  rebuilding after rebase")
    unload()
    ok, out = build(worktree, crates)
    if not ok:
        return False, "build failed after rebase, not pushing:\n" + out[-1500:]

    if not do_push:
        return True, "rebased and building clean, push skipped"

    for remote in ("forgejo", "fork"):
        rc, _, err = sh(f"git push {remote} HEAD:{branch} --force-with-lease", cwd=worktree, timeout=600)
        if rc != 0:
            return False, f"push to {remote} failed: {err}"
    return True, "rebased, built clean, pushed to forgejo and fork"


def run_spec(spec):
    log(f"=== spec {os.path.basename(spec)} ===")
    rc, out, err = sh(
        f"python3 {HARNESS} {spec} --model {MODEL}", timeout=7200
    )
    tail = (out + err).splitlines()[-12:]
    for line in tail:
        print("   " + line)
    return rc == 0


def branch_of(worktree):
    _, b, _ = sh("git branch --show-current", cwd=worktree)
    return b


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("specs", nargs="*")
    ap.add_argument("--rebase-only", action="store_true")
    ap.add_argument("--status", action="store_true")
    ap.add_argument("--no-push", action="store_true")
    a = ap.parse_args()

    if a.status:
        _, out, _ = sh("git worktree list", cwd=MAIN)
        print(out)
        return 0

    if not fetch_upstream():
        return 2

    if a.rebase_only:
        _, out, _ = sh("git worktree list", cwd=MAIN)
        results = []
        for line in out.splitlines():
            wt = line.split()[0]
            if "/pumpkin-wt/" not in wt:
                continue
            br = branch_of(wt)
            if not br or br.startswith("wip/"):
                continue
            ok, msg = rebase_and_push(wt, br, do_push=not a.no_push)
            results.append((br, ok, msg))
        print("\n===== REBASE SUMMARY =====")
        for br, ok, msg in results:
            print(f"{'OK  ' if ok else 'FAIL'} {br}: {msg}")
        return 0

    done = []
    for spec in a.specs:
        for s in sorted(glob.glob(spec)):
            ok = run_spec(s)
            done.append((s, ok))
    print("\n===== SPEC SUMMARY =====")
    for s, ok in done:
        print(f"{'CLEAN' if ok else 'FAILED'} {os.path.basename(s)}")
    print("\nNothing was pushed. Review each diff, then run with --rebase-only to push.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
