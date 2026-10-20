#!/usr/bin/env python3
"""
Automated subsystem design pass. This is the piece that was still human.

A WORKQUEUE subsystem row (sections J to N) is a paragraph of prose. The local
30B cannot act on it, because there is no target function, no test, and no oracle
probe: the oracle cannot see a subsystem until it is wired end to end, so there
is nothing to gate against until somebody decides what the thing should be.

This runs that decision as a job. For one row it asks Claude Code (sonnet) to
produce, in the row's own worktree:

    a module skeleton, with todo!() bodies, that COMPILES
    a unit test per stub that defines correct behaviour
    one FUNC-mode spec per stub, chained in landing order

and then verifies the result mechanically before accepting it:

    the crate builds
    clippy is clean with -D warnings
    the new tests exist and FAIL, on the todo!() and not on a compile error
    every emitted spec's FUNC line locates a unique function

A scaffold whose tests pass immediately is rejected. That means the design pass
implemented the thing instead of specifying it, which defeats the point and
leaves the local model nothing to do. A scaffold whose tests fail to compile is
also rejected, because then the failure is the skeleton's, not the stub's.

Accepted scaffolds are committed on the subsystem branch and their specs land in
the queue, where the nightly loop fills them like any other spec.

Usage:
    scaffold.py --list          rows that are candidates, in value order
    scaffold.py --run           scaffold the next one
    scaffold.py --run --row M1  scaffold a specific row
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
WORKQUEUE = os.path.join(LOCAL, "WORKQUEUE.md")
MAIN = "/home/eshanki/Pumpkin"
WT_ROOT = "/home/eshanki/pumpkin-wt"
LEDGER = os.path.join(LOCAL, "SCAFFOLDED.json")
CLI = os.environ.get(
    "CLAUDE_CLI", "/home/eshanki/.config/Claude/claude-code/2.1.215/claude"
)
MODEL = "sonnet"
CARGO = 'source ~/.cargo/env && export CARGO_INCREMENTAL=0 && export CARGO_PROFILE_DEV_DEBUG=0 && export CARGO_PROFILE_TEST_DEBUG=0 && '

# Rows worth scaffolding, cheapest-first within each tier. Ordering is by how
# much of PARITY_SCORE the row can move per unit of design effort: a missing
# command is directly measurable by the command_surface probe, so it converts
# into score immediately, while a gameplay subsystem needs new probes first.
PRIORITY = [
    "L2",  # /locate and /place, measurable by command_surface
    "L1",  # datapack functions, measurable by command_surface
    "L4",  # /debug and /perf, measurable by command_surface
    "M5",  # weather freezing water and laying snow
    "M4",  # regional difficulty, a dependency of M1 to M3
    "J1",  # saplings never grow
    "K2",  # chunk light persistence
    "M2",  # pillager patrols
    "M1",  # raids, depends on M4
    "N1",  # spawn protection
]


def sh(cmd, cwd=None, timeout=1800):
    p = subprocess.run(
        ["bash", "-c", cmd], cwd=cwd, capture_output=True, text=True, timeout=timeout
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


def read_row(row_id):
    """The full WORKQUEUE block for one row id, verbatim."""
    text = open(WORKQUEUE).read()
    m = re.search(
        rf"^### {re.escape(row_id)} .*?(?=^### |\Z)", text, re.M | re.S
    )
    return m.group(0).strip() if m else None


def candidates():
    done = {e["row"] for e in load_json(LEDGER, {"runs": []}).get("runs", [])}
    out = []
    for row_id in PRIORITY:
        if row_id in done:
            continue
        body = read_row(row_id)
        if not body:
            continue
        # Never scaffold a row whose vanilla behaviour is unverified. The design
        # pass would have to guess, and a guess baked into a test is worse than
        # one in code: every stub filled against it inherits the mistake.
        if "UNVERIFIED" in body:
            continue
        out.append((row_id, body))
    return out


def probe_quota():
    if not os.path.exists(CLI):
        return False, f"claude CLI not found at {CLI}"
    rc, out, err = sh(
        f'{CLI} -p --model {MODEL} "Reply with the single character: 1" </dev/null',
        timeout=180,
    )
    blob = (out + err).lower()
    if "not logged in" in blob:
        return False, "claude CLI is not logged in"
    for marker in ("rate limit", "usage limit", "429", "quota", "too many requests"):
        if marker in blob:
            return False, f"rate limited ({marker})"
    return (rc == 0), ("available" if rc == 0 else f"probe rc={rc}")


def ensure_worktree(name):
    path = os.path.join(WT_ROOT, name)
    if os.path.isdir(path):
        sh("git fetch upstream master 2>/dev/null && git reset --hard upstream/master", cwd=path)
        return path
    sh(
        f"git worktree add {path} -b feat/{name} upstream/master 2>/dev/null "
        f"|| git worktree add {path} -b feat/{name} origin/master",
        cwd=MAIN,
        timeout=900,
    )
    sh("git submodule update --init --recursive", cwd=path, timeout=1800)
    return path if os.path.isdir(path) else None


PROMPT = """You are doing a DESIGN pass on the Rust codebase in this working
directory: Pumpkin, a from-scratch Minecraft server targeting Java Edition 26.2.

You are NOT implementing the feature. You are producing a scaffold that a much
smaller model will fill in, one function at a time, under an automated
build-and-test gate. Your output is judged on whether that gate can work, not on
whether the feature works.

Produce exactly three things:

1. A module skeleton that COMPILES and passes clippy with -D warnings.
   Every function that needs real logic has a `todo!("STUB: see tests::<name>")`
   body. Types, signatures, module layout and registration are fully written.

2. A `#[cfg(test)] mod tests` with at least two unit tests per stub, defining
   correct behaviour precisely enough that a correct implementation is forced.
   Derive the expected values from the vanilla behaviour stated in the work item
   below. If the work item does not state a value, DO NOT invent one: leave that
   stub out of the scaffold entirely and say so at the end.

3. One spec file per stub, written into the directory .pipeline-specs/
   INSIDE this working directory (create it if needed), named
   NN-<short-name>.md where NN continues from {next_num}. Do not write
   anywhere outside this working directory; writes elsewhere are blocked
   and the scaffold will be rejected as having produced no specs. Format, header lines then a line
   containing only ---, then prose:

       WORKTREE: {worktree}
       FILES: relative/path.rs:START-END
       CRATES: pumpkin
       FUNC: <the exact signature line of the stub>
       TEST: <cargo test filter that runs only that stub's tests>
       CHAIN: <comma separated spec names that must pass first, or none>
       ASSERT_ABSENT: todo!("STUB: see tests::<name>")
       ---
       Prose: what the function must do, what vanilla does and where that came
       from, what to leave alone, and the specific way this one goes wrong.

Hard requirements, all mechanically checked after you finish:

- `cargo clippy -p pumpkin --all-targets` with RUSTFLAGS="-D warnings" must be
  clean. The workspace denies clippy::all, pedantic, nursery and cargo, and it
  ALSO denies clippy::todo, so every stub needs
  `#[expect(clippy::todo, unused_variables, reason = "scaffold stub, filled by spec NN")]`.
  If a stub function is not yet called from anywhere, add a separate
  `#[allow(dead_code, reason = "...")]`. Use allow and not expect for dead_code:
  in the test build the function IS used, so an expect goes unfulfilled, and
  unfulfilled_lint_expectations is itself denied.
- `cargo test -p pumpkin <filter>` must COMPILE and the new tests must FAIL on
  the todo!(). Tests that pass immediately mean you implemented instead of
  specified, and the scaffold will be rejected.
- Each spec's FUNC line must match exactly one line in the named file.
- Never write an emoji. Never write an em-dash.

Constraints that have already broken things here:
- Never replace an atomic fetch_sub or fetch_add with a load, arithmetic and
  store. Tick tasks run concurrently as spawned futures; that is a lost-update
  race.
- u8 subtraction must saturate. An underflow wraps to ~250 and a later clamp
  turns it into 15, a silent false-bright.
- Identifiers in doc comments need backticks; doc_markdown is denied.
- Do not take a second lock while holding one, and do not widen a critical
  section across an await.

Split the work so each stub is genuinely small: one function, under about 60
changed lines to fill. Chain the specs so a stub that depends on another only
unlocks once its prerequisite passes. If a piece needs a live World, a generator,
or the world seed, do NOT stub it out: a unit test cannot supply those, so the
gate cannot see it. Leave it written by you, or leave it returning an error and
say so.

Finish by printing a line starting with `SCAFFOLD SUMMARY:` listing the stub
names, the spec files you wrote, and anything you deliberately left out.

Reference implementations are available read-only at
/home/eshanki/pumpkin-local/refs/minestom (Java) and
/home/eshanki/pumpkin-local/refs/cuberite (C++). Use them to cross-check
MECHANICS when the work item is ambiguous: how a system is decomposed, what
state it keeps, what order things happen in. Never translate their code into
Rust: different languages, different licenses, and both are reimplementations
with their own bugs, so the oracle measurement always outranks them. When a
design choice came from one of them, say so in the spec prose.

--- WORK ITEM ---
{row}
"""


def next_spec_number():
    nums = []
    for p in glob.glob(os.path.join(SPECS, "[0-9]*.md")):
        m = re.match(r"(\d+)-", os.path.basename(p))
        if m:
            nums.append(int(m.group(1)))
    return max(nums) + 1 if nums else 1


def new_specs_since(known):
    return sorted(set(glob.glob(os.path.join(SPECS, "*.md"))) - known)


def verify(worktree, spec_files):
    """Mechanical acceptance. Returns (ok, list of reasons).

    Cheap checks first. A clippy run on this workspace is about six minutes, so
    finding out afterwards that the design pass emitted no specs wastes six
    minutes to learn something a directory listing already knew.
    """
    problems = []

    if not spec_files:
        return False, ["the design pass wrote no spec files, nothing to gate"]

    sys.path.insert(0, LOCAL)
    from func_edit import read_function  # noqa: PLC0415

    for spec in spec_files:
        head = open(spec).read().partition("\n---\n")[0]
        keys = {
            ln.partition(":")[0].strip().upper()
            for ln in head.splitlines()
            if ln.strip()
        }
        missing = {"WORKTREE", "FILES", "FUNC", "TEST"} - keys
        if missing:
            problems.append(
                f"{os.path.basename(spec)}: missing header field(s) "
                f"{', '.join(sorted(missing))}"
            )
    if problems:
        return False, problems

    rc, out, err = sh(
        CARGO + f'cd {worktree} && cargo fmt --all && RUSTFLAGS="-D warnings" '
        "cargo clippy -p pumpkin --all-targets 2>&1",
        timeout=5400,
    )
    if rc != 0:
        tail = (out + err)[-1200:]
        problems.append(f"clippy failed:\n{tail}")
        return False, problems

    for spec in spec_files:
        head = open(spec).read().partition("\n---\n")[0]
        meta = {}
        for line in head.splitlines():
            k, _, v = line.partition(":")
            if k.strip() and v.strip():
                meta.setdefault(k.strip().upper(), v.strip())
        func = meta.get("FUNC")
        files = [f.strip() for f in meta.get("FILES", "").split(",") if f.strip()]
        name = os.path.basename(spec)
        if not func:
            problems.append(f"{name}: no FUNC line")
            continue
        if not read_function(meta.get("WORKTREE", worktree), files, func):
            problems.append(f"{name}: FUNC does not locate a unique function: {func}")

        test_filter = meta.get("TEST")
        if not test_filter:
            problems.append(f"{name}: no TEST line, the stub would have no gate")
            continue
        rc, out, err = sh(
            CARGO + f"cd {worktree} && cargo test -p pumpkin {test_filter} 2>&1",
            timeout=5400,
        )
        blob = out + err
        if "error[E" in blob or "could not compile" in blob:
            problems.append(f"{name}: tests do not compile")
        elif rc == 0:
            # Passing on a todo!() body is impossible, so this means the design
            # pass implemented the function. That is a rejection: it leaves the
            # local model nothing to do and skips the gate entirely.
            problems.append(
                f"{name}: tests PASS against the stub, so it was implemented "
                "rather than specified"
            )
        elif "not yet implemented" not in blob and "STUB" not in blob:
            problems.append(
                f"{name}: tests fail for some reason other than the todo!(), "
                "so the skeleton itself is broken"
            )

    return (not problems), problems


def scaffold_row(row_id, body, dry):
    name = f"sub{row_id.lower()}"
    worktree = ensure_worktree(name)
    if not worktree:
        return False, f"could not create worktree for {row_id}"

    prompt = PROMPT.format(
        next_num=f"{next_spec_number():02d}",
        worktree=worktree,
        row=body,
    )
    pfile = f"/tmp/pumpkin_scaffold_{row_id}.txt"
    with open(pfile, "w") as f:
        f.write(prompt)

    if dry:
        return True, f"would scaffold {row_id} in {worktree}"

    drop = os.path.join(worktree, ".pipeline-specs")
    os.makedirs(drop, exist_ok=True)
    print(f"  designing {row_id} in {worktree} ...")
    rc, out, err = sh(
        f'cd {worktree} && {CLI} -p --model {MODEL} --permission-mode acceptEdits '
        f'--max-turns 80 "$(cat {pfile})" </dev/null',
        timeout=7200,
    )
    tail = (out + err)[-1500:]
    if rc != 0:
        return False, f"design pass failed rc={rc}: {tail[:400]}"

    # The CLI only writes inside its working directory, so the design pass
    # drops specs in the worktree and they are promoted here once they pass.
    # Promoting first would leave a rejected scaffold's specs in the queue.
    written = sorted(glob.glob(os.path.join(drop, "*.md")))
    print(f"  wrote {len(written)} spec(s), verifying ...")
    ok, problems = verify(worktree, written)
    if not ok:
        # Reject cleanly. The specs never left the worktree, so resetting it is
        # enough, and a bad design pass cannot poison the queue for later rounds.
        sh("git checkout -- . && git clean -fd", cwd=worktree)
        return False, "rejected:\n    " + "\n    ".join(p[:300] for p in problems)

    promoted = []
    for src in written:
        dst = os.path.join(SPECS, os.path.basename(src))
        with open(src) as fh:
            body = fh.read()
        with open(dst, 'w') as fh:
            fh.write(body)
        promoted.append(dst)
    sh("rm -rf .pipeline-specs", cwd=worktree)
    written = promoted

    ident = (
        '-c user.name="Pumpkin Parity Pipeline" '
        '-c user.email="rdravida+pumpkin-pipeline@gmail.com"'
    )
    msg = (
        f"Scaffold {row_id}\n\n"
        f"Module skeleton with todo!() stubs and a unit test per stub defining "
        f"correct behaviour, so the spec queue can fill them one at a time under "
        f"the normal build and test gate.\n\n"
        f"Specs emitted: {', '.join(os.path.basename(s) for s in written)}\n"
    )
    mfile = f"/tmp/pumpkin_scaffold_msg_{row_id}.txt"
    with open(mfile, "w") as f:
        f.write(msg)
    sh(f"git add -A && git {ident} commit -q -F {mfile}", cwd=worktree)
    for remote in ("forgejo", "fork"):
        sh(f"git push -q {remote} HEAD:feat/{name}", cwd=worktree, timeout=900)

    return True, f"accepted, {len(written)} spec(s) queued: " + ", ".join(
        os.path.basename(s) for s in written
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--run", action="store_true")
    ap.add_argument("--row")
    a = ap.parse_args()

    cands = candidates()
    if a.list or not a.run:
        print("scaffold candidates, in priority order:")
        for row_id, body in cands:
            first = body.splitlines()[0]
            print(f"  {first}")
        return 0

    ok, why = probe_quota()
    if not ok:
        print(f"design pass unavailable: {why}")
        return 1

    # Shared daily cap across every tier that costs quota. Enforced here rather
    # than in the loop script so a manual run cannot bypass it either.
    sys.path.insert(0, LOCAL)
    from budget import take  # noqa: PLC0415

    allowed, note = take("scaffold")
    print(f"budget: {note}")
    if not allowed:
        return 1

    if a.row:
        body = read_row(a.row)
        if not body:
            print(f"no such row: {a.row}")
            return 1
        picked = [(a.row, body)]
    else:
        picked = cands[:1]

    if not picked:
        print("nothing left to scaffold")
        return 0

    ledger = load_json(LEDGER, {"runs": []})
    for row_id, body in picked:
        ok, note = scaffold_row(row_id, body, dry=False)
        print(f"{row_id}: {note}")
        ledger.setdefault("runs", []).append(
            {
                "date": dt.date.today().isoformat(),
                "row": row_id,
                "accepted": ok,
                "note": note[:400],
            }
        )
        save_json(LEDGER, ledger)

    return 0


if __name__ == "__main__":
    sys.exit(main())
