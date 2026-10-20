#!/usr/bin/env python3
"""gate.py -- the mechanical referee for spree candidate diffs.

    python3 gate.py <candidate_diff> <repo_path>

A candidate is banked only if the scripts, not the model, say so. It must:

  1. carry no escape artifacts   (u0026/u003c/u003e, stray \\u00 sequences),
  2. touch real logic            (not just a `#[cfg(test)] mod tests;` stub),
  3. apply cleanly               (git apply --check, then apply, in a throwaway
                                  worktree cut from verify/all),
  4. orphan no code              (every changed .rs file stays brace/paren/
                                  bracket balanced after the patch),
  5. build                       (cargo check -p pumpkin),
  6. lint                        (cargo clippy -p pumpkin -- -D warnings),
  7. pass tests                  (cargo test -p pumpkin).

Why this file exists: mini_spree banked HTML/JSON-escaped garbage as "clippy
clean" because its gate ran `cargo clippy ... | tail -80` with no pipefail, so
the recorded exit code was tail's (0), never clippy's. Here every cargo stage
is run without a pipe and its own return code is the verdict.

The three cargo stages hold an exclusive flock on ~/pumpkin-local/.gate.lock so
two gate runs never build into the shared .loop-target at once (they would
corrupt each other's incremental state and thrash RAM against the 27B). The
worktree is always removed, pass or fail. A one-line normalized record is
appended to gate_failures.log for every failure so the runner can grow
RULEBOOK.md from recurring patterns.

Verdict {pass, stage_failed, reason, log_tail} is written to
<candidate>.gate.json; exit status is 0 only on pass.
"""

import fcntl
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
LOCK = os.path.join(HERE, ".gate.lock")
FAILLOG = os.path.join(HERE, "gate_failures.log")
# One shared build cache for the whole loop (see parity_cycle.sh). Every
# worktree's cargo lands here; the flock keeps concurrent gate runs from
# writing it at the same time.
TARGET = os.path.join(HERE, ".loop-target")

# Timeouts mirror verify.py (build_timeout=2400). Tests get more headroom
# because a cold run compiles the test harness on top of the crate.
CHECK_TIMEOUT = 2400
CLIPPY_TIMEOUT = 2400
TEST_TIMEOUT = 3600


def item_id(diff_path):
    """A2-skydarken... -> 'A2'; instantdouble.diff -> 'instantdouble'."""
    stem = os.path.splitext(os.path.basename(diff_path))[0]
    m = re.match(r"([A-Za-z]\d+)", stem)
    return m.group(1) if m else stem


def record_failure(iid, stage, reason):
    """One normalized line per failure -> the runner mines this for patterns."""
    ts = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    line = f"{ts}\t{iid}\t{stage}\t{reason}\n"
    try:
        with open(FAILLOG, "a") as f:
            f.write(line)
    except OSError:
        pass


def emit(candidate, iid, passed, stage, reason, log_tail=""):
    """Write the verdict next to the candidate and return an exit code."""
    verdict = {
        "pass": passed,
        "item": iid,
        "stage_failed": None if passed else stage,
        "reason": "" if passed else reason,
        "log_tail": log_tail[-4000:] if log_tail else "",
    }
    try:
        with open(candidate + ".gate.json", "w") as f:
            json.dump(verdict, f, indent=2)
            f.write("\n")
    except OSError:
        pass
    if passed:
        print(f"PASS {iid}: build + clippy + test clean", flush=True)
    else:
        print(f"FAIL {iid} [{stage}]: {reason}", flush=True)
        record_failure(iid, stage, reason)
    return 0 if passed else 1


# --- static sanity: text-only, runs before any worktree or build ------------

# The exact artifacts observed banked: `&` written as u0026, `<`/`>` as
# u003c/u003e (the backslash of a JSON \uXXXX escape eaten by sed), plus the
# stray backslash form \u00.. that is never valid Rust (Rust uses \u{..}).
_ARTIFACT = re.compile(r"u00(?:26|3c|3e)", re.I)
_STRAY_U = re.compile(r"\\u00[0-9a-fA-F]{2}")
# HTML entity escapes that show up when model output is round-tripped as HTML.
_HTML_ENT = re.compile(r"&(?:amp|lt|gt|quot|#\d+|#x[0-9a-fA-F]+);")


def added_lines(diff):
    """Body lines the diff introduces (real '+' lines, not the +++ header)."""
    return [l[1:] for l in diff.splitlines()
            if l.startswith("+") and not l.startswith("+++")]


def removed_lines(diff):
    return [l[1:] for l in diff.splitlines()
            if l.startswith("-") and not l.startswith("---")]


def check_escape_artifacts(diff):
    """Reject JSON/HTML escape debris smuggled into added code."""
    for ln in added_lines(diff):
        for rx, label in ((_ARTIFACT, "u00xx"),
                          (_STRAY_U, "backslash-u"),
                          (_HTML_ENT, "html-entity")):
            m = rx.search(ln)
            if m:
                return f"escape_artifact:{label}:{m.group(0)!r}"
    return None


# Lines that carry no logic: blank, comment, or a bare test-module scaffold.
# `mod tests` counts as scaffold only when the line is JUST the declaration or
# its opening brace -- `mod tests { real_code(); }` on one line has real logic
# after the brace and must not be swept up as noise.
_STUB = re.compile(
    r"^\s*(#\[cfg\(test\)\]|#!\[.*\]|mod\s+tests\b\s*[{;]?|use\s+super::.*|\}|\{)\s*$"
)


def _is_noise(line):
    s = line.strip()
    if not s:
        return True
    if s.startswith("//") or s.startswith("/*") or s.startswith("*"):
        return True
    return bool(_STUB.match(line))


def check_noop(diff):
    """Reject diffs that change no real logic (empty, or only a test stub)."""
    added = added_lines(diff)
    removed = removed_lines(diff)
    if not added and not removed:
        return "noop:empty_diff"
    substantive_add = [l for l in added if not _is_noise(l)]
    substantive_del = [l for l in removed if not _is_noise(l)]
    if not substantive_add and not substantive_del:
        # Nothing but scaffolding/comments touched. The classic offender is a
        # lone `#[cfg(test)] mod tests;` appended to look productive.
        return "noop:no_logic_touched"
    return None


# --- structural sanity: no orphaned code ------------------------------------
# The observed offender deleted a function signature (and a `for` header) but
# left the body behind as context, so an `if { } else { }` block ended up with
# no enclosing scope. That desynchronizes braces. We compare the changed file's
# bracket balance before and after the patch: valid Rust is balanced, so a file
# that was balanced and is no longer has been orphaned by the edit.

_LINE_COMMENT = re.compile(r"//[^\n]*")
_BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.S)
_STRING = re.compile(r'"(?:\\.|[^"\\])*"')
_CHAR = re.compile(r"'(?:\\.|[^'\\])*'")
_RAWSTR = re.compile(r'r#*".*?"#*', re.S)
_LIFETIME = re.compile(r"'[a-zA-Z_][a-zA-Z0-9_]*")


def _strip_rust(src):
    """Best-effort removal of comments and literals before counting brackets."""
    src = _BLOCK_COMMENT.sub(" ", src)
    src = _RAWSTR.sub(" ", src)
    src = _STRING.sub(" ", src)
    src = _LIFETIME.sub(" ", src)   # 'a etc, so the char rule can't misfire
    src = _CHAR.sub(" ", src)
    src = _LINE_COMMENT.sub(" ", src)
    return src


def _balance(src):
    """(curly, paren, bracket) net counts after stripping literals/comments."""
    s = _strip_rust(src)
    return (s.count("{") - s.count("}"),
            s.count("(") - s.count(")"),
            s.count("[") - s.count("]"))


def changed_files(diff):
    """Post-image ('b/…') paths the diff modifies, excluding pure deletions."""
    files = []
    for l in diff.splitlines():
        if l.startswith("+++ b/"):
            p = l[6:].strip()
            if p and p != "/dev/null":
                files.append(p)
    return files


def check_orphan(diff, worktree):
    """After apply, no changed .rs file may become bracket-unbalanced.

    Only fires when the file was balanced before and is not after, so an
    imperfect literal-stripper can never turn a clean file into a false fail.
    """
    # Reconstruct the pre-image by reversing the applied patch in memory: read
    # the patched (post) file from the worktree, and the pre file from git.
    for rel in changed_files(diff):
        if not rel.endswith(".rs"):
            continue
        post_path = os.path.join(worktree, rel)
        try:
            with open(post_path, encoding="utf-8", errors="replace") as f:
                post = f.read()
        except OSError:
            continue
        rc, pre = _sh(f"git show HEAD:{_shq(rel)}", cwd=worktree)
        if rc != 0:
            pre = ""   # newly added file: nothing to compare, treat pre as {}
        pb = _balance(pre) if pre else (0, 0, 0)
        qb = _balance(post)
        for kind, bp, bq in zip(("curly", "paren", "bracket"), pb, qb):
            if bp == 0 and bq != 0:
                return (f"orphan_code:{rel}:{kind}_unbalanced_by_{bq}")
    return None


# --- shell + cargo helpers --------------------------------------------------

def _shq(s):
    return "'" + s.replace("'", "'\\''") + "'"


def _sh(cmd, cwd=None, timeout=120):
    p = subprocess.run(["bash", "-c", cmd], cwd=cwd, capture_output=True,
                       text=True, timeout=timeout)
    return p.returncode, p.stdout + p.stderr


def run_cargo(cmd, cwd, timeout):
    """Run one cargo stage; return (returncode, combined_output).

    No pipe: the return code is cargo's own, not some tail's -- this is the
    exact bug that let broken diffs bank as 'clippy clean'. Runs in its own
    session so a timeout kills the whole compiler tree (start_new_session +
    killpg), the way run_pg does in mini_spree. Builds land in the shared
    .loop-target so they stay incremental.
    """
    env = {**os.environ, "CARGO_TARGET_DIR": TARGET}
    full = f"source ~/.cargo/env && {cmd}"
    proc = subprocess.Popen(
        ["bash", "-c", full], cwd=cwd, env=env,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, start_new_session=True,
    )
    try:
        out, _ = proc.communicate(timeout=timeout)
        return proc.returncode, out
    except subprocess.TimeoutExpired:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            out, _ = proc.communicate(timeout=30)
        except Exception:
            out = ""
        return None, (out or "") + f"\n<gate: killed after {timeout}s>"


def tail(text, n=40):
    return "\n".join(text.splitlines()[-n:])


# --- build stages under the exclusive lock ----------------------------------

CARGO_STAGES = (
    ("check", "cargo check -p pumpkin", CHECK_TIMEOUT, "cargo_check_error"),
    ("clippy", "cargo clippy -p pumpkin -- -D warnings", CLIPPY_TIMEOUT,
     "clippy_warning"),
    ("test", "cargo test -p pumpkin", TEST_TIMEOUT, "test_failure"),
)


def build_gate(worktree):
    """check -> clippy -> test, serialized on .gate.lock. Returns
    (ok, stage, reason, log_tail)."""
    lock_fd = open(LOCK, "w")
    print("  waiting for .gate.lock ...", flush=True)
    fcntl.flock(lock_fd, fcntl.LOCK_EX)
    print("  lock held; building", flush=True)
    try:
        for stage, cmd, tmo, norm in CARGO_STAGES:
            print(f"  cargo {stage} ...", flush=True)
            rc, out = run_cargo(cmd, worktree, tmo)
            if rc is None:
                return False, stage, f"timeout:{stage}:{tmo}s", tail(out)
            if rc != 0:
                return False, stage, norm, tail(out)
        return True, None, "", ""
    finally:
        fcntl.flock(lock_fd, fcntl.LOCK_UN)
        lock_fd.close()


# --- worktree lifecycle + orchestration -------------------------------------

def make_worktree(repo):
    """Throwaway detached worktree at verify/all. Caller must remove it."""
    wt = tempfile.mkdtemp(prefix="gate-", dir=os.path.expanduser("~/pumpkin-wt"))
    # mkdtemp made the dir; git worktree add wants to create it itself.
    os.rmdir(wt)
    rc, out = _sh(
        f"git -C {_shq(repo)} worktree add --detach {_shq(wt)} verify/all",
        timeout=180,
    )
    if rc != 0:
        return None, out
    # `git worktree add` does NOT populate submodules, but the wasm plugin
    # loader's bindgen! macro reads the pumpkin-plugin-wit submodule at compile
    # time (path: "../pumpkin-plugin-wit/v0.1"). In a bare worktree that dir is
    # empty, so bindgen generates nothing and the crate fails to build with 35
    # `cannot find 'pumpkin' in 'v0_1'` errors -- which meant NO candidate could
    # ever pass the check stage. Populate submodules from the shared object
    # store (idempotent; no-op if already checked out).
    rc, out = _sh(
        f"git -C {_shq(wt)} submodule update --init --recursive",
        timeout=300,
    )
    if rc != 0:
        return None, out
    return wt, ""


def remove_worktree(repo, wt):
    if not wt:
        return
    _sh(f"git -C {_shq(repo)} worktree remove --force {_shq(wt)}", timeout=120)
    if os.path.exists(wt):
        import shutil
        shutil.rmtree(wt, ignore_errors=True)
    _sh(f"git -C {_shq(repo)} worktree prune", timeout=60)


def gate(candidate, repo):
    iid = item_id(candidate)
    with open(candidate, encoding="utf-8", errors="replace") as f:
        diff = f.read()

    # Static text checks first: cheap, and they never need a worktree.
    for check in (check_escape_artifacts, check_noop):
        reason = check(diff)
        if reason:
            return emit(candidate, iid, False, "static", reason)

    wt = None
    try:
        wt, err = make_worktree(repo)
        if not wt:
            return emit(candidate, iid, False, "worktree",
                        "worktree_create_failed", err)

        cand_abs = os.path.abspath(candidate)
        rc, out = _sh(f"git -C {_shq(wt)} apply --check {_shq(cand_abs)}",
                      timeout=120)
        if rc != 0:
            return emit(candidate, iid, False, "apply", "git_apply_check_failed",
                        out)
        rc, out = _sh(f"git -C {_shq(wt)} apply {_shq(cand_abs)}", timeout=120)
        if rc != 0:
            return emit(candidate, iid, False, "apply", "git_apply_failed", out)

        # Structural check needs the patched files on disk.
        reason = check_orphan(diff, wt)
        if reason:
            return emit(candidate, iid, False, "structure", reason)

        ok, stage, reason, log = build_gate(wt)
        if not ok:
            return emit(candidate, iid, False, stage, reason, log)
        return emit(candidate, iid, True, None, "")
    finally:
        remove_worktree(repo, wt)


def main():
    if len(sys.argv) != 3:
        print("usage: python3 gate.py <candidate_diff> <repo_path>",
              file=sys.stderr)
        return 2
    candidate, repo = sys.argv[1], sys.argv[2]
    if not os.path.isfile(candidate):
        print(f"no such candidate: {candidate}", file=sys.stderr)
        return 2
    if not os.path.isdir(os.path.join(repo, ".git")) and not os.path.isfile(
        os.path.join(repo, ".git")
    ):
        print(f"not a git repo: {repo}", file=sys.stderr)
        return 2
    return gate(candidate, repo)


if __name__ == "__main__":
    sys.exit(main())
