#!/usr/bin/env python3
"""
Check specs before they cost six iterations each.

Written after a run where most failures were mine, not the model's. Spec 14 told
the model to "use the light accessor this codebase already exposes on
`BlockAccessor`". No such accessor exists: BlockAccessor has exactly four
methods, none about light. The model did the only thing left and invented
`get_light_level()`, then spent six iterations failing to compile a call to a
function that was never there. The log recorded that as a model failure.

So: every method name a spec names in backticks gets grepped for. A spec that
points at something nonexistent is rejected before it reaches the queue, where it
would otherwise burn an hour of GPU and look like incompetence at the other end.

Checks, cheapest first:
  1. FUNC locates exactly one function in the named files
  2. ASSERT_ABSENT strings actually appear right now (or the spec is already done)
  3. ASSERT_PRESENT strings do NOT already appear (or the spec is a no-op)
  4. every `identifier()` in the prose exists somewhere in the crate

    lint_specs.py            check all
    lint_specs.py --spec 14  check one
"""

import argparse
import glob
import os
import re
import subprocess
import sys

LOCAL = os.path.dirname(os.path.abspath(__file__))
SPECS = os.path.join(LOCAL, "specs")
REPO = "/home/eshanki/Pumpkin"

# Names that appear in backticks but are prose, not code, or are so common that
# grepping proves nothing.
IGNORE = {
    "todo", "unwrap", "expect", "clone", "into", "from", "new", "len", "iter",
    "map", "filter", "collect", "min", "max", "abs", "round", "floor", "sqrt",
    "cargo", "clippy", "rustc", "true", "false", "self", "Self", "Some", "None",
    "Ok", "Err", "Vec", "Option", "Result", "String", "str", "i32", "u8", "f64",
    "f32", "usize", "bool", "hypot", "saturating_sub", "fetch_sub", "fetch_add",
}


def sh(cmd, cwd=None):
    p = subprocess.run(
        ["bash", "-c", cmd], cwd=cwd, capture_output=True, text=True, timeout=300
    )
    return p.returncode, p.stdout.strip()


def parse(path):
    head, _, body = open(path).read().partition("\n---\n")
    meta = {}
    for line in head.splitlines():
        k, _, v = line.partition(":")
        k, v = k.strip().upper(), v.strip()
        if not k or not v:
            continue
        meta.setdefault(k, []).append(v)
    return meta, body


def norm(s):
    return " ".join(s.split())


def check(path):
    name = os.path.basename(path)
    meta, body = parse(path)
    problems = []

    worktree = (meta.get("WORKTREE") or [REPO])[0]
    base = worktree if os.path.isdir(worktree) else REPO
    files = [
        f.strip().partition(":")[0]
        for f in (meta.get("FILES") or [""])[0].split(",")
        if f.strip()
    ]

    for rel in files:
        if not os.path.exists(os.path.join(base, rel)):
            problems.append(f"FILES names a missing path: {rel}")

    blob = ""
    for rel in files:
        try:
            blob += open(os.path.join(base, rel)).read() + "\n"
        except OSError:
            pass

    funcs = meta.get("FUNC") or []
    if funcs:
        sys.path.insert(0, LOCAL)
        from func_edit import read_function  # noqa: PLC0415

        # Every FUNC line must locate a unique function; MULTI specs have
        # several, and one bad target fails the whole spec at load.
        for f in funcs:
            if not read_function(base, files, f):
                problems.append(f"FUNC does not locate a unique function: {f}")

    n = norm(blob)
    for a in meta.get("ASSERT_ABSENT", []):
        if norm(a) not in n:
            problems.append(
                f"ASSERT_ABSENT is already absent, so the spec is a no-op or "
                f"stale: {a[:70]}"
            )
    for p in meta.get("ASSERT_PRESENT", []):
        if norm(p) in n:
            problems.append(
                f"ASSERT_PRESENT is already present, so it cannot prove the fix "
                f"happened: {p[:70]}"
            )

    # API: lines are a promise to the model that a signature exists. A broken
    # promise here is worse than no line at all, because the model will trust
    # it over its own caution. Forms: `API: Type::method` or `API: fn method`.
    for api in meta.get("API", []):
        m = re.match(
            r"(?:([A-Za-z_][A-Za-z0-9_]*)::)?(?:fn\s+)?([a-z_][A-Za-z0-9_]*)", api
        )
        if not m:
            problems.append(f"API line is not Type::method or fn method: {api[:70]}")
            continue
        typ, fn_name = m.group(1), m.group(2)
        rc, files_out = sh(
            f"grep -rlw --include='*.rs' 'fn {fn_name}' .", cwd=base
        )
        if rc != 0:
            problems.append(f"API names a function that does not exist: {api[:70]}")
            continue
        if typ:
            scoped = False
            for f in files_out.splitlines():
                try:
                    src = open(os.path.join(base, f)).read()
                except OSError:
                    continue
                if re.search(rf"(impl|trait)[^;{{]*\b{typ}\b", src):
                    scoped = True
                    break
            if not scoped:
                problems.append(
                    f"API function exists but not on `{typ}` in any impl/trait "
                    f"block: {api[:70]}"
                )

    # Only identifiers written as a call, `foo()`. A bare backticked word is
    # usually prose, a clippy lint name, a test name or a data value, and
    # flagging those produced more noise than signal on the first attempt.
    called = set(re.findall(r"`([A-Za-z_][A-Za-z0-9_]*)\(\)`", body))
    for ident in sorted(called - IGNORE):
        # Presence anywhere in the crate is enough. Requiring a `fn` definition
        # missed trait methods, macro-generated ones, and inherent impls, which
        # made the check reject correct specs.
        rc, _ = sh(
            f"grep -rqw {ident!r} --include='*.rs' .", cwd=base
        )
        if rc != 0:
            problems.append(
                f"prose calls `{ident}()` but that name appears nowhere in the "
                "crate; the model will invent it and fail to compile"
            )

    return name, problems


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--spec")
    a = ap.parse_args()

    paths = sorted(glob.glob(os.path.join(SPECS, "[0-9]*.md")))
    if a.spec:
        paths = [p for p in paths if os.path.basename(p).startswith(a.spec)]

    bad = 0
    for path in paths:
        name, problems = check(path)
        if not problems:
            print(f"OK    {name}")
            continue
        bad += 1
        print(f"BAD   {name}")
        for p in problems:
            print(f"        {p}")
    print(f"\n{len(paths) - bad} clean, {bad} need work")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
