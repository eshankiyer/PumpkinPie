#!/usr/bin/env python3
"""
Parallel scheduler for the local Pumpkin agent.

Runs many specs concurrently without letting them collide. Three things are
genuinely exclusive on this machine and each gets its own gate:

  GPU     one slot. There is one 8 GB card and the model is 19 GB with CPU
          offload; a second concurrent load thrashes and both runs crawl.
  BUILD   two slots. rustc on this workspace is memory hungry and the box has
          30 GB total, most of which the model wants while loaded.
  WORKTREE one lock per directory. Two specs must never share a worktree, or
          they overwrite each other's edits and the diff becomes nonsense.

So a spec alternates between the GPU gate and the build gate, which means while
spec A is compiling, spec B is generating. That is the real parallelism
available here, and it roughly halves wall-clock time versus running serially.

Usage:
    python3 run_all.py specs/*.md
    python3 run_all.py specs/*.md --builds 2 --max-iters 6
    python3 run_all.py --status
"""

import argparse
import glob
import json
import os
import queue
import re
import subprocess
import sys
import threading
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ENDPOINT = os.environ.get("LLAMA_ENDPOINT", "http://127.0.0.1:11434/v1/chat/completions")
MODEL = os.environ.get("PUMPKIN_LOCAL_MODEL", "qwen3.6-27b")
CARGO_ENV = 'source ~/.cargo/env && export CARGO_INCREMENTAL=0 && export CARGO_PROFILE_DEV_DEBUG=0 && export CARGO_PROFILE_TEST_DEBUG=0 && '
STATUS = "/tmp/pumpkin_run_status.json"
DONE_MARKER = "/tmp/pumpkin_run_DONE.txt"

sys.path.insert(0, HERE)
from apisurface import (  # noqa: E402
    api_pack,
    error_types,
    repo_map,
    type_surface,
    types_in_text,
)
from func_edit import (  # noqa: E402
    FUNC_SYSTEM,
    INSERT_SYSTEM,
    MULTI_SYSTEM,
    apply_func_edit,
    apply_insert,
    apply_multi_func,
    read_block,
    read_blocks,
    read_function,
    read_functions,
)
from harness import (  # noqa: E402
    LINE_EDIT_SYSTEM,
    apply_line_edit,
    check_asserts,
    parse_asserts,
    SYSTEM,
    apply_blocks,
    distill_errors,
    parse_spec,
    read_files,
    sh,
)

GPU = threading.Semaphore(1)
BUILD = None  # set in main
WORKTREE_LOCKS = {}
WORKTREE_LOCKS_GUARD = threading.Lock()
STATE = {}
STATE_GUARD = threading.Lock()
PRINT_GUARD = threading.Lock()


def say(name, msg):
    with PRINT_GUARD:
        print(f"[{name}] {msg}", flush=True)


def set_state(name, **kw):
    with STATE_GUARD:
        STATE.setdefault(name, {}).update(kw)
        STATE[name]["t"] = time.strftime("%H:%M:%S")
        try:
            with open(STATUS, "w") as f:
                json.dump(STATE, f, indent=2)
        except OSError:
            pass


def worktree_lock(path):
    with WORKTREE_LOCKS_GUARD:
        if path not in WORKTREE_LOCKS:
            WORKTREE_LOCKS[path] = threading.Lock()
        return WORKTREE_LOCKS[path]


def ask(messages, timeout=2400, max_tokens=None):
    """One completion against llama-server's OpenAI-compatible endpoint.

    The old 900-token ceiling was tuned for single-line edits on a 7B. Phase 2
    allows function-level and up-to-three-file diffs, which do not fit in 900
    tokens: the reply gets cut mid-hunk, the patch will not apply, and an
    iteration is burned on a formatting artifact rather than on the fix.

    max_tokens caps TOTAL decode, reasoning included. With thinking now
    uncapped server-side, 3000 was not enough headroom: a long thought plus a
    long function truncated the code mid-word, which is how spec 02 burned six
    iterations without ever reaching a build.
    """
    payload = {
        "model": MODEL,
        "messages": messages,
        "stream": False,
        "temperature": 0.1,
        "max_tokens": max_tokens or int(os.environ.get("LLAMA_MAX_TOKENS", "6000")),
        # Thinking is bounded server-side by --reasoning-budget and routed into
        # message.reasoning_content by --reasoning-format deepseek, so it costs a
        # few hundred tokens and never reaches the code parser. Do not disable it
        # here: a short deliberation is worth more than none on this model.
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


# Phase 2 size limits. The model is now allowed function-level and small
# multi-file edits, but "small" has to be enforced mechanically, because the
# failure mode of a stronger model is a confident 400-line refactor that builds
# and cannot be reviewed.
MAX_FILES = int(os.environ.get("PUMPKIN_MAX_FILES", "3"))
MAX_LINES = int(os.environ.get("PUMPKIN_MAX_LINES", "60"))


def diff_size(diff):
    """(files touched, lines added or removed) for a unified diff."""
    files = sum(1 for ln in diff.splitlines() if ln.startswith("+++ "))
    lines = sum(
        1
        for ln in diff.splitlines()
        if (ln.startswith("+") and not ln.startswith("+++"))
        or (ln.startswith("-") and not ln.startswith("---"))
    )
    return files, lines


def build(worktree, crates):
    pkgs = " ".join(f"-p {c}" for c in crates)
    cmd = (
        CARGO_ENV
        + f'cd {worktree} && cargo fmt --all && RUSTFLAGS="-D warnings" cargo clippy {pkgs} --all-targets 2>&1'
    )
    rc, out, err = sh(cmd, timeout=3600)
    return rc, (out + err)


def test(worktree, crates, filter_=None):
    """cargo test, run only after clippy is clean so a broken tree fails fast.

    `filter_` narrows to one test path. Subsystem-scaffold specs use it: the
    stub's own unit test is the gate, because an oracle probe cannot see any of
    a half-wired subsystem, and running the whole suite for a one-function stub
    costs minutes per iteration against a 6-iteration cap.
    """
    pkgs = " ".join(f"-p {c}" for c in crates)
    narrow = f" {filter_}" if filter_ else ""
    cmd = CARGO_ENV + f"cd {worktree} && cargo test {pkgs}{narrow} 2>&1"
    rc, out, err = sh(cmd, timeout=5400)
    return rc, (out + err)


def chain_blocked(spec_path, results_so_far):
    """A spec whose prerequisite has not passed yet must not run.

    Scaffold specs land in dependency order. Letting a dependent run first wastes
    its whole iteration budget failing against a `todo!()` it cannot see, and the
    failure looks like the model being wrong rather than the queue being early.
    """
    chain = spec_meta(spec_path, "CHAIN")
    if not chain or chain.strip().lower() == "none":
        return None
    needed = [c.strip() for c in chain.split(",") if c.strip()]
    missing = [c for c in needed if results_so_far.get(c) not in ("CANDIDATE", "CLEAN")]
    return ", ".join(missing) if missing else None


def probe_gate(name):
    """Require the spec's oracle probe to flip divergent -> matching.

    Returns (ok, message). A spec with no PROBE declared cannot pass this gate,
    by design: the whole point of Phase 2 is that acceptance is a measurement
    against vanilla and not a sentence someone wrote in a spec file.

    Exit 4 from the gate means a server was unhealthy. That is NOT treated as a
    pass and NOT treated as a failure of the edit; it is an instrument fault,
    and the run is held rather than credited.
    """
    if not name:
        return False, "spec declares no PROBE, so it cannot be accepted"
    rc, out, err = sh(
        f"python3 {HERE}/oracle/probe_gate.py --gate {name}", timeout=900
    )
    text = (out + err).strip()
    if rc == 4:
        return False, f"oracle unhealthy, verdict withheld: {text[:300]}"
    return rc == 0, text[:600]


TRIAGE_SYSTEM = """You are the triage judge for an automated Rust bug-fixing
pipeline. A small local model attempts one spec at a time; some failures are
worth more retries and some are structurally unwinnable. You read the failure
history and decide which. You never write code.

Verdicts, with the real cases they are calibrated on:

BLINDSPOT - the fix requires editing something the mode cannot return. FUNC
mode may only return one function; TARGET mode one line. Errors that require
changing an attribute, another function, or widening the shown signature are
BLINDSPOT: no retry can ever fix them. (Note: unused-import errors are NOT
blindspot; the harness deletes those automatically now.)

BADSPEC - the errors show the spec's premise contradicts the source: it names
an API that does not exist and no listed alternative fits, or it points at the
wrong function. (Real case: a spec promised the damage type was reachable from
the arguments; the signature carries no such thing.)

ESCALATE - the model is not learning: the same error class repeats across
iterations with correct compiler feedback available, or the code shows a
skill gap like inventing syntax. (Real case: 'expected a pattern, found an
expression' four times, 18-36 errors each time.)

RETRY - anything else. Different errors each iteration, shrinking error
counts, or plausible near-misses are all RETRY. If you are unsure, RETRY.

Reply with exactly two lines and nothing else:
VERDICT: RETRY|BLINDSPOT|BADSPEC|ESCALATE
WHY: <one line of evidence from the history>
"""


def _parse_verdict(reply):
    """Strict tokens, lenient everything else. Unparseable means RETRY."""
    m = re.search(r"VERDICT:\s*(RETRY|BLINDSPOT|BADSPEC|ESCALATE)", reply or "", re.I)
    if not m:
        return "RETRY", "triager reply unparseable"
    why = re.search(r"WHY:\s*(.+)", reply)
    return m.group(1).upper(), (why.group(1).strip() if why else "no reason given")


def triage(name, mode_desc, instructions, history):
    """One classification call over the failure history. Never raises.

    The triager is a decision point, not a coder: its only power is to stop
    burning iterations. Any failure of its own is a RETRY, because a broken
    judge must never be able to kill a healthy spec.
    """
    evidence = "\n".join(history)[-3500:]
    msgs = [
        {"role": "system", "content": TRIAGE_SYSTEM},
        {
            "role": "user",
            "content": f"Spec: {name}\nMode: {mode_desc}\n\n"
            f"Task (abridged):\n{instructions[:900]}\n\n"
            f"Failure history:\n{evidence}",
        },
    ]
    try:
        with GPU:
            # max_tokens caps TOTAL decode including thinking. 400 starved the
            # first live call: the thought ate the whole cap and the content
            # came back empty. Two answer lines need room to think first.
            reply = ask(msgs, timeout=900, max_tokens=2000)
    except Exception as e:
        return "RETRY", f"triager unavailable: {e}"
    try:
        with open(f"/tmp/pumpkin_{name}_triage.txt", "w") as fh:
            fh.write(reply)
    except OSError:
        pass
    return _parse_verdict(reply)


def record_triage(name, verdict, why):
    """Verdicts are decisions about the queue; they go in a reviewable ledger."""
    import datetime

    try:
        with open(os.path.join(HERE, "TRIAGE.md"), "a") as fh:
            stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M")
            fh.write(f"- {stamp}  {name}  **{verdict}**  {why}\n")
    except OSError:
        pass


def snap_files(worktree, files):
    """Contents of the spec's files, for keep-best snapshots."""
    snap = {}
    for entry in files:
        rel = entry.partition(":")[0]
        try:
            with open(os.path.join(worktree, rel)) as fh:
                snap[rel] = fh.read()
        except OSError:
            pass
    return snap


def restore_files(worktree, snap):
    for rel, content in snap.items():
        try:
            with open(os.path.join(worktree, rel), "w") as fh:
                fh.write(content)
        except OSError:
            pass


def count_errors(out):
    """Real rustc error headers, excluding the could-not-compile summaries."""
    return len(
        [
            ln
            for ln in (out or "").splitlines()
            if re.match(r"^error(\[|:)", ln) and "could not compile" not in ln
        ]
    )


def fix_unused_imports(worktree, files, build_out):
    """Delete whole-line unused imports rustc names. Returns (deleted, only).

    FUNC mode has a blind spot the model cannot escape: when its rewrite makes
    a pre-existing `use` line unused, that line sits above the function,
    outside the span it is allowed to return. Spec 09 burned three iterations
    against 'unused import: crate::entity::EntityBase' it had no way to
    delete. Like the clippy::todo attribute, this is bookkeeping the applier
    must do deterministically: rustc names the exact file and line.

    Only `use ...;` lines are touched, only within the spec's FILES. Whole
    lines are deleted; for group imports (`use a::{B, C}` with only B unused)
    just the named member is removed, since deleting the line would break the
    used half. `only` is True when unused imports were the sole real errors,
    meaning one rebuild should pass.
    """
    blocks = re.findall(
        r"^error: unused import: `([^`]+)`[^\n]*\n\s*--> ([^\n:]+):(\d+)",
        build_out,
        re.M,
    )
    headers = [
        ln
        for ln in build_out.splitlines()
        if ln.startswith("error") and "could not compile" not in ln
    ]
    only = bool(blocks) and all(h.startswith("error: unused import") for h in headers)
    allowed = {e.partition(":")[0] for e in files}
    per = {}
    for imp, path, line in blocks:
        rel = path.strip().lstrip("./")
        if rel in allowed:
            per.setdefault(rel, {})[int(line)] = imp
    deleted = 0
    for rel, nums in per.items():
        full = os.path.join(worktree, rel)
        try:
            with open(full) as fh:
                lines = fh.readlines()
        except OSError:
            continue
        for n in sorted(nums, reverse=True):
            if n > len(lines):
                continue
            s = lines[n - 1].strip()
            if not s.startswith("use ") or not s.endswith(";"):
                continue
            if "{" not in s:
                del lines[n - 1]
                deleted += 1
                continue
            # Group import, one member unused: `use a::{B, C};` with B named.
            # Remove just that member; deleting the line would break C.
            member = nums[n].split("::")[-1].strip()
            new = re.sub(rf"\b{re.escape(member)}\b\s*,\s*", "", s, count=1)
            if new == s:
                new = re.sub(rf",\s*\b{re.escape(member)}\b", "", s, count=1)
            if new == s:
                new = re.sub(rf"\b{re.escape(member)}\b", "", s, count=1)
            if re.search(r"\{\s*\}", new):
                del lines[n - 1]
                deleted += 1
            elif new != s:
                indent = lines[n - 1][: len(lines[n - 1]) - len(lines[n - 1].lstrip())]
                lines[n - 1] = indent + new + "\n"
                deleted += 1
        with open(full, "w") as fh:
            fh.writelines(lines)
    return deleted, only and deleted > 0


def log_errs(name, it, stage, errs):
    """Persist the error text that was sent back to the model.

    The nightly log used to record only the length ("288 chars back to
    model"). Diagnosing a dead run then meant rebuilding the worktree to see
    the error again, and the locate specs failed three whole rounds on one
    288-character lint that nobody ever read. The length of an error is not
    evidence of its content.
    """
    try:
        with open(f"/tmp/pumpkin_{name}_errors.log", "a") as fh:
            fh.write(f"\n--- iter {it} {stage} ---\n{errs}\n")
    except OSError:
        pass
    first = errs.strip().splitlines()
    if first:
        say(name, f"    err: {first[0][:160]}")


def run_one(spec_path, max_iters, results):
    """Wrapper so a thread can never die silently and drop its result."""
    name = os.path.basename(spec_path).replace(".md", "")
    try:
        _run_one(spec_path, name, max_iters, results)
    except BaseException as e:  # SystemExit included: harness helpers call sys.exit
        import traceback

        say(name, f"ABORTED: {type(e).__name__}: {e}")
        say(name, traceback.format_exc(limit=3))
        set_state(name, phase="aborted", error=str(e)[:300])
        results.put((name, "ABORTED", str(e)[:200]))


def spec_meta(path, key):
    head = open(path).read().partition("\n---\n")[0]
    for line in head.splitlines():
        k, _, v = line.partition(":")
        if k.strip().upper() == key:
            return v.strip()
    return None


def spec_meta_all(path, key):
    """All values for a repeatable header key, e.g. API: lines."""
    head = open(path).read().partition("\n---\n")[0]
    out = []
    for line in head.splitlines():
        k, _, v = line.partition(":")
        if k.strip().upper() == key and v.strip():
            out.append(v.strip())
    return out


def _run_one(spec_path, name, max_iters, results):
    worktree, files, crates, instructions = parse_spec(spec_path)
    absent, present = parse_asserts(spec_path)
    target = spec_meta(spec_path, "TARGET")

    with STATE_GUARD:
        done = {k: v.get("result") for k, v in STATE.items() if v.get("result")}
    blocked_by = chain_blocked(spec_path, done)
    if blocked_by:
        say(name, f"blocked, waiting on {blocked_by}")
        set_state(name, phase=f"blocked on {blocked_by}")
        results.put((name, "BLOCKED", f"waiting on {blocked_by}"))
        return

    lock = worktree_lock(worktree)
    if not lock.acquire(blocking=False):
        say(name, f"waiting for worktree {worktree}")
        set_state(name, phase="waiting-worktree", worktree=worktree)
        lock.acquire()

    try:
        set_state(name, phase="start", worktree=worktree)
        _, dirty, _ = sh("git diff --stat", cwd=worktree)
        if dirty.strip():
            # Several specs legitimately share one worktree: the three /locate
            # stubs all live in the same file. The lock serialises them, but the
            # previous holder leaves its edit in the tree, so the next one used
            # to refuse and report DIRTY forever. That is how 21 and 22 never
            # ran at all.
            #
            # Any diff worth keeping was already written to /tmp/pumpkin_*.diff
            # before the worktree was released, so resetting here loses nothing.
            # Refuse only if nothing has run yet, which means the dirt is a
            # human's uncommitted work and is not ours to discard.
            # Whether the dirt is ours has to be judged from the ON-DISK status
            # file, not from in-process STATE. STATE starts empty in every new
            # invocation, so the escalation follow-up run considered every
            # worktree foreign and refused all of them with DIRTY. That is why
            # seven specs reported DIRTY in a run where nothing was wrong.
            ours = False
            with STATE_GUARD:
                if any(v.get("result") for v in STATE.values()):
                    ours = True
            if not ours:
                try:
                    with open(STATUS) as fh:
                        prior = json.load(fh)
                    ours = any(
                        v.get("worktree") == worktree for v in prior.values()
                    )
                except (OSError, ValueError):
                    ours = False
            if not ours:
                say(name, "worktree dirty and no spec has ever run here, refusing")
                results.put((name, "DIRTY", dirty.strip()[:200]))
                return
            say(name, "resetting worktree left dirty by an earlier spec")
            sh("git checkout -- . && git clean -fdq", cwd=worktree)

        # One FUNC line is single-function mode; several is MULTI, where the
        # fix legitimately spans more than one function (the triager's most
        # common BLINDSPOT). func stays the first for single-mode call sites;
        # funcs drives MULTI.
        funcs = spec_meta_all(spec_path, "FUNC")
        func = funcs[0] if funcs else None
        multi = len(funcs) > 1
        # INSERT adds NEW items (a helper, a trait method) that have no span to
        # replace. Distinct from FUNC/MULTI, which edit existing functions.
        inserts = spec_meta_all(spec_path, "INSERT")

        def format_reminder(show_current=True):
            """The reply format a retry must ask for, per mode.

            The old retry messages demanded SEARCH/REPLACE blocks or unified
            diffs no matter the mode. In FUNC mode the applier rejects both,
            so the first failed build made the rest of the run unwinnable:
            the model obeyed the retry format, the applier refused it, and the
            complaint pushed it further from the only format that applies.
            Specs 07, 09-17 and 21 all burned out exactly this way. The retry
            must ask for the same shape of reply as the first prompt did.

            `show_current` re-sends the code as it now stands. Only worth the
            context when an edit actually landed; if nothing applied, the
            code is unchanged from what the conversation already shows.
            """
            if inserts:
                shown_now = ""
                if show_current:
                    cur, _ = read_blocks(worktree, files, inserts)
                    if cur:
                        shown_now = f"\n\nThe target blocks as they stand now:\n\n{cur}"
                return shown_now + (
                    "\n\nReply with the complete new item for each block, one "
                    "```rust fence each, tagged `// INTO: <anchor>` on the "
                    "first line. No prose, no diff."
                )
            if multi:
                shown_now = ""
                if show_current:
                    cur, _ = read_functions(worktree, files, funcs)
                    if cur:
                        shown_now = f"\n\nThe functions as they stand now:\n\n{cur}"
                return shown_now + (
                    "\n\nReply with the complete corrected version of each "
                    "function you change, one ```rust fence per function, each "
                    "starting at its signature line. No prose, no diff."
                )
            if func:
                shown_now = ""
                if show_current:
                    current = read_function(worktree, files, func)
                    if current:
                        shown_now = f"\n\nThe function as it stands now:\n\n{current}"
                return shown_now + (
                    "\n\nReply with the complete corrected function: start at "
                    "the first line shown, end at its closing brace, all of it "
                    "in one ```rust fence. No prose, no diff, no "
                    "SEARCH/REPLACE blocks."
                )
            if target:
                return (
                    f"\n\nThe line as it stands now:\n{target}\n\n"
                    "Reply with the single corrected replacement line and "
                    "nothing else."
                )
            return (
                "\n\nReply only in SEARCH/REPLACE blocks against the CURRENT "
                "file contents, copying the SEARCH text verbatim."
            )

        if target:
            # Atomic edit: hand it one line and ask for one line back. No diff
            # syntax means no diff syntax to get wrong.
            msgs = [
                {"role": "system", "content": LINE_EDIT_SYSTEM},
                {
                    "role": "user",
                    "content": f"{instructions}\n\nThe line to change:\n{target}\n",
                },
            ]
        elif inserts:
            # INSERT: add new items into named blocks. No span to replace, so
            # the model names an anchor block and the applier inserts before
            # that block's closing brace (or replaces the item if it already
            # exists, making retries idempotent).
            shown, missing = read_blocks(worktree, files, inserts)
            if missing:
                say(name, f"INSERT anchors not found: {missing}")
                results.put((name, "BADSPEC", f"anchor not unique: {missing}"))
                return
            apis = spec_meta_all(spec_path, "API")
            extra = [t for a in apis for t in types_in_text(a)]
            pack = api_pack(worktree, f"{instructions}\n{shown}", extra_types=extra)
            rmap = repo_map(worktree, files)
            api_note = (
                "\n\nAPIs the spec author verified for this change:\n"
                + "\n".join(apis)
                if apis
                else ""
            )
            msgs = [
                {"role": "system", "content": INSERT_SYSTEM},
                {
                    "role": "user",
                    "content": f"{instructions}\n\n{shown}"
                    + api_note
                    + (f"\n\n{pack}" if pack else "")
                    + (f"\n\n{rmap}" if rmap else ""),
                },
            ]
        elif multi:
            # Multi-function edit: the fix spans several functions, each still
            # returned whole in its own fence and applied by the same
            # deterministic span replace. This is the bounded way out of the
            # one-function blindspot: it does NOT reopen free-form diffing,
            # which failed 9/10 on bookkeeping.
            shown, missing = read_functions(worktree, files, funcs)
            if missing:
                say(name, f"MULTI targets not found: {missing}")
                results.put((name, "BADSPEC", f"FUNC not unique: {missing}"))
                return
            apis = spec_meta_all(spec_path, "API")
            extra = [t for a in apis for t in types_in_text(a)]
            pack = api_pack(worktree, f"{instructions}\n{shown}", extra_types=extra)
            rmap = repo_map(worktree, files)
            api_note = (
                "\n\nAPIs the spec author verified for this change:\n"
                + "\n".join(apis)
                if apis
                else ""
            )
            msgs = [
                {"role": "system", "content": MULTI_SYSTEM},
                {
                    "role": "user",
                    "content": f"{instructions}\n\n{shown}"
                    + api_note
                    + (f"\n\n{pack}" if pack else "")
                    + (f"\n\n{rmap}" if rmap else ""),
                },
            ]
        elif func:
            # Whole-function replacement. Same reasoning as line-edit mode, one
            # level up: give it exactly one function and take exactly one
            # function back, so there is no addressing scheme to get wrong.
            shown = read_function(worktree, files, func)
            if shown is None:
                say(name, f"FUNC not found or not unique: {func}")
                results.put((name, "BADSPEC", f"FUNC not found or not unique: {func}"))
                return
            # Be the autocomplete: the surfaces of every project type the
            # instructions or the shown function mention, read off this exact
            # tree. The model invents methods precisely when it has to code
            # against types it cannot see.
            apis = spec_meta_all(spec_path, "API")
            extra = [t for a in apis for t in types_in_text(a)]
            pack = api_pack(worktree, f"{instructions}\n{shown}", extra_types=extra)
            api_note = (
                "\n\nAPIs the spec author verified for this change:\n"
                + "\n".join(apis)
                if apis
                else ""
            )
            msgs = [
                {"role": "system", "content": FUNC_SYSTEM},
                {
                    "role": "user",
                    "content": f"{instructions}\n\n{shown}"
                    + api_note
                    + (f"\n\n{pack}" if pack else ""),
                },
            ]
        else:
            msgs = [
                {"role": "system", "content": SYSTEM},
                {
                    "role": "user",
                    "content": f"{instructions}\n\n{read_files(worktree, files)}",
                },
            ]

        mode_desc = (
            f"INSERT ({len(inserts)} new item(s) into named blocks)"
            if inserts
            else f"MULTI ({len(funcs)} functions, each returned whole in its own fence)"
            if multi
            else "FUNC (the model may only return one complete function)"
            if func
            else "TARGET (the model may only return one line)"
            if target
            else "DIFF (search/replace blocks against the files)"
        )
        history = []

        # The Karpathy-loop core: every iteration is scored, the best version
        # of the files is kept, and a regression is rolled back instead of
        # becoming the new starting point. Spec 10 went from 18 errors to 36
        # and the loop kept iterating on the 36; without a score, retrying is
        # a random walk, not an optimization.
        best = {"score": None, "iter": 0, "snap": None}

        def scored(stage_rank, out_text, it):
            """Score (higher is better), keep best, roll back regressions.

            Stages: 1 build fails < 2 compiles but asserts fail < 3 tests
            fail. Within a stage, fewer errors wins. Returns a note for the
            retry message when a rollback happened.
            """
            nonlocal target
            score = (stage_rank, -count_errors(out_text))
            if best["score"] is None or score > best["score"]:
                best.update(
                    score=score,
                    iter=it,
                    snap=snap_files(worktree, files),
                    target=target,
                )
                return ""
            if score < best["score"] and best["snap"]:
                restore_files(worktree, best["snap"])
                # TARGET mode aims at a tracked line; after a rollback the
                # file holds the best version's line, so the aim must follow.
                target = best.get("target")
                say(
                    name,
                    f"iter {it} scored {score}, worse than iter {best['iter']} "
                    f"{best['score']}: rolled back to best",
                )
                history.append(
                    f"iter {it}: regression (score {score} < {best['score']}), "
                    "rolled back"
                )
                return (
                    "\n\nNOTE: this attempt was WORSE than your attempt at "
                    f"iteration {best['iter']} (more or deeper errors), so the "
                    "files have been rolled back to that better version. The "
                    "code shown below is the best version so far; improve on "
                    "it instead of your last reply."
                )
            return ""

        for it in range(1, max_iters + 1):
            # Triage checkpoints: after 2 and 4 failed iterations, one cheap
            # call decides whether the remaining iterations are worth burning.
            # Its only power is stopping early; a wrongly stopped spec is not
            # lost, rerun.sh re-queues everything that is not a candidate.
            if it in (3, 5) and len(history) >= 2:
                verdict, why = triage(name, mode_desc, instructions, history)
                say(name, f"triage at iter {it}: {verdict}: {why[:140]}")
                if verdict != "RETRY":
                    record_triage(name, verdict, why)
                    set_state(name, phase=f"triaged-{verdict.lower()}", result=verdict)
                    results.put((name, verdict, why[:200]))
                    return

            set_state(name, phase=f"queue-gpu (iter {it})")
            with GPU:
                set_state(name, phase=f"generating (iter {it})")
                say(name, f"iter {it}: generating")
                t0 = time.time()
                try:
                    reply = ask(msgs)
                except Exception as e:
                    say(name, f"model call failed: {e}")
                    results.put((name, "MODELFAIL", str(e)[:200]))
                    return
                say(name, f"iter {it}: replied in {time.time() - t0:.0f}s")

            # Every iteration's reply, kept. Spec 02 died six times and left
            # one overwritten file: the last reply, mid-word, and no way to
            # tell whether the first five failed the same way.
            try:
                with open(f"/tmp/pumpkin_{name}_r{it}.txt", "w") as fh:
                    fh.write(reply)
            except OSError:
                pass

            if target:
                applied, failed, written = apply_line_edit(
                    worktree, files, target, reply
                )
                if applied and written:
                    # The file now holds the written line. Retries must aim at
                    # what is actually there, or they search for a line that no
                    # longer exists.
                    target = written
            elif inserts:
                applied, failed = apply_insert(worktree, files, inserts, reply)
            elif multi:
                applied, failed = apply_multi_func(worktree, files, funcs, reply)
            elif func:
                applied, failed = apply_func_edit(worktree, files, func, reply)
            else:
                applied, failed = apply_blocks(worktree, reply, files)
            say(name, f"iter {it}: applied {applied}, failed {len(failed)}")
            for reason in failed[:3]:
                say(name, f"    why: {reason}")
            if applied == 0 and failed:
                history.append(f"iter {it} apply failed: " + "; ".join(failed)[:300])
            if applied == 0:
                # keep the raw reply so the format can be diagnosed later
                try:
                    with open(f"/tmp/pumpkin_{name}_reply.txt", "w") as fh:
                        fh.write(reply)
                except OSError:
                    pass
            if applied == 0:
                # Nothing changed. Building here would compile the untouched tree
                # and report a false pass, which is exactly what happened before
                # this guard existed.
                if failed:
                    complaint = "Your reply could not be applied:\n" + "\n".join(failed)
                else:
                    complaint = "Your reply could not be parsed."
                msgs += [
                    {"role": "assistant", "content": reply},
                    {
                        "role": "user",
                        # Nothing landed, so the code is unchanged: no point
                        # re-sending it.
                        "content": complaint + format_reminder(show_current=False),
                    },
                ]
                continue

            set_state(name, phase=f"queue-build (iter {it})")
            with BUILD:
                set_state(name, phase=f"building (iter {it})")
                say(name, f"iter {it}: building")
                unload()
                rc, out = build(worktree, crates)
                if rc != 0:
                    n_del, only_imports = fix_unused_imports(worktree, files, out)
                    if n_del:
                        say(
                            name,
                            f"deleted {n_del} unused import(s) outside the "
                            "model's span",
                        )
                    if only_imports:
                        say(name, "unused imports were the only errors, rebuilding")
                        rc, out = build(worktree, crates)

            if rc == 0:
                _, stat, _ = sh("git diff --stat", cwd=worktree)
                _, diff, _ = sh("git diff", cwd=worktree)
                if not diff.strip():
                    # A clean build over an unchanged tree is not a success.
                    say(name, "build clean but diff is EMPTY, treating as failure")
                    results.put((name, "NOCHANGE", "clean build but no edit was made"))
                    return
                problems = check_asserts(worktree, files, absent, present)
                # Universal guard: the dominant failure mode is a diff that adds
                # only the comment the spec asked for and skips the code change
                # beside it. That compiles, so nothing else catches it.
                code_added = [
                    ln[1:].strip()
                    for ln in diff.splitlines()
                    if ln.startswith("+") and not ln.startswith("+++")
                ]
                code_added = [
                    ln
                    for ln in code_added
                    if ln and not ln.startswith("//") and not ln.startswith("/*")
                ]
                if not code_added:
                    problems.append(
                        "the diff only adds comments, no code was changed"
                    )
                nfiles, nlines = diff_size(diff)
                if nfiles > MAX_FILES or nlines > MAX_LINES:
                    problems.append(
                        f"the diff is too large: {nfiles} files and {nlines} changed "
                        f"lines, the limit is {MAX_FILES} files and {MAX_LINES} lines. "
                        "Make the smallest change that fixes the described behaviour."
                    )
                if problems:
                    # Builds fine but did not do the job, e.g. added the comment
                    # the spec asked for and skipped the code change beside it.
                    say(name, "assertions failed: " + "; ".join(problems))
                    history.append(
                        f"iter {it} compiled but asserts failed: "
                        + "; ".join(problems)[:300]
                    )
                    note = scored(2, "", it)
                    msgs += [
                        {"role": "assistant", "content": reply},
                        {
                            "role": "user",
                            "content": "It compiles, but the required change was not made:\n"
                            + "\n".join(problems)
                            + "\nMake the actual code change, not just a comment."
                            + note
                            + format_reminder(),
                        },
                    ]
                    continue
                # Clippy is clean. Now the rest of the Phase 2 gate: tests, then
                # the empirical one.
                test_filter = spec_meta(spec_path, "TEST")
                set_state(name, phase=f"testing (iter {it})")
                say(name, f"iter {it}: cargo test {test_filter or '(full suite)'}")
                with BUILD:
                    trc, tout = test(worktree, crates, test_filter)
                if trc == 0 and test_filter:
                    # A filter that matches nothing exits 0, which would let a
                    # stub through on a test that never ran. But the check has
                    # to SUM the pass counts across targets: cargo test runs the
                    # lib, the bin and doctests, and the targets the filter does
                    # not touch each print "0 passed". Matching the substring
                    # "0 passed" therefore failed runs whose lib target had
                    # passed everything, the model resubmitted the same correct
                    # code, and the applier reported it as "identical". Spec 21
                    # burned its whole budget on a correct answer this way.
                    total_passed = sum(
                        int(n) for n in re.findall(r"(\d+) passed", tout)
                    )
                    if total_passed == 0:
                        trc = 1
                        tout += (
                            f"\n\nThe test filter {test_filter!r} matched no "
                            "tests. The function must keep the name the spec "
                            "gave it."
                        )
                if trc != 0:
                    errs = distill_errors(tout) or tout[-3000:]
                    say(name, f"iter {it}: TESTS FAILED")
                    log_errs(name, it, "test", errs)
                    history.append(f"iter {it} tests failed:\n{errs[:800]}")
                    note = scored(3, tout, it)
                    msgs += [
                        {"role": "assistant", "content": reply},
                        {
                            "role": "user",
                            "content": "It compiles but the test suite fails. "
                            "Fix these:\n\n" + errs + note + format_reminder(),
                        },
                    ]
                    continue

                # Candidates are work product, not diagnostics: they live in
                # the repo dir, not /tmp. A reboot wiped /tmp on 2026-07-29
                # and took three banked candidate diffs with it.
                os.makedirs(os.path.join(HERE, "candidates"), exist_ok=True)
                outp = os.path.join(HERE, "candidates", f"{name}.diff")
                with open(outp, "w") as f:
                    f.write(diff)

                # The probe gate needs the change actually running on the live
                # server, which this scheduler does not deploy. So record the
                # diff as a candidate and let the deploy step adjudicate it.
                # Nothing reaches a PR on a green build alone.
                probe = spec_meta(spec_path, "PROBE")
                say(name, f"BUILD AND TESTS CLEAN, probe={probe or 'NONE'}")
                set_state(
                    name, phase="candidate", diff=outp, probe=probe, result="CANDIDATE"
                )
                results.put(
                    (
                        name,
                        "CANDIDATE" if probe else "NOPROBE",
                        stat.strip().splitlines()[-1] if stat.strip() else "",
                    )
                )
                return

            errs = distill_errors(out) or out[-3000:]
            say(name, f"iter {it}: build failed, {len(errs)} chars back to model")
            log_errs(name, it, "build", errs)
            history.append(f"iter {it} build failed:\n{errs[:800]}")
            # Score before building the retry: a rollback must happen before
            # format_reminder reads the files, so the model sees the version
            # it is being told to improve on.
            note = scored(1, out, it)
            # E0599 backstop: the compiler just named the exact type the model
            # guessed wrong about. Answer with that type's real surface, so the
            # retry corrects against facts instead of guessing twice.
            surfaces = []
            for t in error_types(errs)[:2]:
                s = type_surface(worktree, t, max_sigs=60)
                if s:
                    surfaces.append(s)
                    say(name, f"    injecting real API of {t}")
            inject = ("\n\n" + "\n\n".join(surfaces)) if surfaces else ""
            msgs += [
                {"role": "assistant", "content": reply},
                {
                    "role": "user",
                    "content": "The build failed. Fix these:\n\n"
                    + errs
                    + inject
                    + note
                    + format_reminder(),
                },
            ]

        say(name, "gave up after max iterations")
        set_state(name, phase="failed")
        results.put((name, "FAILED", "max iterations"))
    finally:
        lock.release()


def notify(title, body):
    """Best-effort desktop notification, plus a marker file and a terminal bell."""
    try:
        with open(DONE_MARKER, "w") as f:
            f.write(f"{title}\n\n{body}\n")
    except OSError:
        pass
    for cmd in (
        f'notify-send -u critical {json.dumps(title)} {json.dumps(body)}',
        f'zenity --info --title={json.dumps(title)} --text={json.dumps(body)}',
    ):
        rc, _, _ = sh(f"command -v {cmd.split()[0]} >/dev/null && {cmd}", timeout=30)
        if rc == 0:
            break
    sys.stdout.write("\a")
    sys.stdout.flush()


def main():
    global BUILD
    ap = argparse.ArgumentParser()
    ap.add_argument("specs", nargs="*")
    ap.add_argument("--builds", type=int, default=2, help="concurrent cargo builds")
    ap.add_argument("--max-iters", type=int, default=6)
    ap.add_argument("--status", action="store_true")
    a = ap.parse_args()

    if a.status:
        try:
            print(open(STATUS).read())
        except OSError:
            print("no run in progress")
        return 0

    BUILD = threading.Semaphore(a.builds)

    paths = []
    for pat in a.specs:
        paths += sorted(glob.glob(pat))
    if not paths:
        print("no specs matched")
        return 2

    if os.path.exists(DONE_MARKER):
        os.remove(DONE_MARKER)

    print(f"running {len(paths)} specs, gpu=1 builds={a.builds}\n")
    t_start = time.time()
    results = queue.Queue()
    threads = []
    for p in paths:
        t = threading.Thread(target=run_one, args=(p, a.max_iters, results), daemon=True)
        t.start()
        threads.append(t)
        time.sleep(1)  # stagger so they do not all hit the GPU gate at once

    for t in threads:
        t.join()

    rows = []
    while not results.empty():
        rows.append(results.get())
    rows.sort()

    mins = (time.time() - t_start) / 60
    clean = [r for r in rows if r[1] == "CLEAN"]
    lines = [f"{status:9} {name}  {info}" for name, status, info in rows]
    summary = (
        f"{len(clean)}/{len(rows)} specs built clean in {mins:.0f} min\n\n"
        + "\n".join(lines)
        + f"\n\nDiffs: {HERE}/candidates/<name>.diff"
        + "\nNothing was pushed. Review, then: python3 driver.py --rebase-only"
    )

    print("\n===== ALL DONE =====")
    print(summary)
    notify("Pumpkin local agent finished", summary[:900])
    print(f"\nMarker written to {DONE_MARKER}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
