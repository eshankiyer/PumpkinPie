#!/usr/bin/env python3
"""
Local-model implementation harness for Pumpkin.

Claude writes a precise spec; this drives a local model through the
edit / build / fix loop, which is where the token cost normally goes.

Spec file format:

    WORKTREE: /home/eshanki/pumpkin-wt/foo
    FILES: pumpkin/src/a.rs, pumpkin/src/b.rs
    CRATES: pumpkin
    ---
    <free-form instructions: the bug, the exact change wanted,
     the vanilla behaviour already verified by Claude>

The model replies only in SEARCH/REPLACE blocks, which are far more
reliable for small models than unified diffs, and which fail loudly
instead of silently corrupting a file when the anchor does not match.

Usage:
    python3 harness.py spec.md [--max-iters N] [--model NAME]
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time
import urllib.request

# llama-server's OpenAI-compatible endpoint. Ollama is retired: it cannot express
# per-tensor CPU offload, which is the whole reason a 30B MoE runs on an 8 GB card.
# See DECISIONS.md. Port is unchanged so nothing downstream had to move.
ENDPOINT = os.environ.get("LLAMA_ENDPOINT", "http://127.0.0.1:11434/v1/chat/completions")
DEFAULT_MODEL = "qwen3.6-35b-a3b"
CARGO_ENV = 'source ~/.cargo/env && export CARGO_INCREMENTAL=0 && export CARGO_PROFILE_DEV_DEBUG=0 && export CARGO_PROFILE_TEST_DEBUG=0 && '

SYSTEM = """You are a Rust engineer working on Pumpkin, a from-scratch Minecraft
server in Rust targeting Java Edition 26.2. Your changes go into pull requests
against a real upstream project with real reviewers.

# Output format

Reply with ONE unified diff and nothing else. No prose before or after. Wrap it
in a ```diff fence. Use paths with an a/ and b/ prefix, exactly like git:

```diff
--- a/pumpkin/src/some/file.rs
+++ b/pumpkin/src/some/file.rs
@@
 unchanged context line
-line being removed
+line being added
 unchanged context line
```

- Give at least three unchanged context lines above and below each change, so
  the hunk can be located. Context lines start with a single space.
- Copy context lines byte for byte from the file content you were given,
  including indentation. Do not retype them from memory or paraphrase them.
- The line numbers in the @@ header are not trusted and are recounted, so a
  bare `@@` is fine. Getting the CONTEXT right is what matters.
- Put every change to every file in the SAME diff, with one ---/+++ pair per
  file.
- If you are asked to fix build errors, diff against the CURRENT file contents,
  which already include your earlier edits.
- Never emit the whole file as one enormous hunk. Small, anchored hunks only.

# The single most important rule

You do not know what vanilla Minecraft does. Neither does the person who wrote
your spec, unless they cite a source, and if they cite one they have checked it.

If the spec tells you the vanilla behaviour, implement exactly that. If it does
not, or it says UNVERIFIED, do not guess and do not implement. Emit no blocks and
say what is missing. This project has twice had to publicly retract claims that
came from someone confidently remembering vanilla wrong. A wrong guess here is
worse than no change, because it looks correct and ships.

Specific traps that have already caused this:
- Game rules were renamed to snake_case in 25w44a. It is advance_weather, not
  doWeatherCycle, and random_tick_speed, not randomTickSpeed.
- Weather timers use inclusive UniformInt since 23w03a, not nextInt(bound)+base.
- Snow and ice melt on BLOCK light only, never sky light.
- Dimension types were reworked in 26.x. The End has sky light now.
So a 1.21-era source is not authoritative for this codebase.

# Correctness constraints specific to this repo

Concurrency. Block entities, hoppers and chunk data are mutated by tick tasks
that run concurrently as spawned futures. Never replace an atomic
read-modify-write such as fetch_sub or fetch_add with a load, arithmetic, and
store. That is a lost-update race even though it reads more clearly, and it has
already been caught in review once. If a fix seems to need it, keep the atomic
and adjust the comparison instead.

Locks. tick_environment holds the weather lock across await points, which
establishes a weather then level_time ordering. Do not take the weather lock
while holding another lock, and do not widen an existing critical section.

Clippy. The workspace denies all, pedantic, nursery and cargo lints. Common
rejections: needless_pass_by_value, redundant_closure, missing_const_for_fn,
option_if_let_else, unnested_or_patterns, cast_possible_truncation where not
already allowed. Write it idiomatically the first time rather than waiting for
the build to tell you.

doc_markdown is on, and it catches comments as well as code. Any identifier in a
doc comment needs backticks: write `RandomLookAroundGoal`, not the bare name.
This one fails the build on a comment-only change, so it is easy to trip.

Style. Match the file you are editing. Comments explain why, and where a vanilla
behaviour is being matched, name it. Do not reformat code you are not changing,
and do not fix unrelated lints you happen to notice.

Never write an emoji. Never write an em-dash. Both in code and in comments.
Maintainers flag emoji-bearing contributions for expedited closure.

# Scope

One logical fix per run. If you notice another bug while working, do not fix it.
It goes in a separate branch after a human has looked at it. A diff that quietly
fixes three things is harder to review and more likely to be rejected whole.

Prefer the smallest change that is actually correct. A one-character comparison
fix that a reviewer can verify in ten seconds is worth more than a refactor that
happens to also work.
"""


def log(msg):
    print(f"[harness] {msg}", flush=True)


def sh(cmd, cwd=None, timeout=1800):
    p = subprocess.run(
        ["bash", "-c", cmd], cwd=cwd, capture_output=True, text=True, timeout=timeout
    )
    return p.returncode, p.stdout, p.stderr


def parse_spec(path):
    raw = open(path).read()
    head, _, body = raw.partition("\n---\n")
    meta = {}
    for line in head.splitlines():
        if ":" in line:
            k, _, v = line.partition(":")
            meta[k.strip().upper()] = v.strip()
    wt = meta.get("WORKTREE")
    if not wt or not os.path.isdir(wt):
        sys.exit(f"spec needs a valid WORKTREE, got {wt!r}")
    files = [f.strip() for f in meta.get("FILES", "").split(",") if f.strip()]
    crates = [c.strip() for c in meta.get("CRATES", "pumpkin").split(",") if c.strip()]
    return wt, files, crates, body.strip()


def parse_asserts(path):
    """Post-build assertions, so a plausible no-op cannot pass as a fix.

    A diff can apply cleanly and build cleanly while doing nothing useful, for
    example by adding the comment a spec asked for and skipping the code change
    next to it. A build gate cannot see that. These can.

        ASSERT_ABSENT: some code that must no longer exist
        ASSERT_PRESENT: some code that must now exist

    Each may appear multiple times. Matching is a plain substring over the
    concatenated FILES, whitespace-normalised so indentation changes do not
    cause false failures.
    """
    absent, present = [], []
    head = open(path).read().partition("\n---\n")[0]
    for line in head.splitlines():
        k, _, v = line.partition(":")
        k, v = k.strip().upper(), v.strip()
        if k == "ASSERT_ABSENT" and v:
            absent.append(v)
        elif k == "ASSERT_PRESENT" and v:
            present.append(v)
    return absent, present


def _norm(s):
    return " ".join(s.split())


def check_asserts(worktree, files, absent, present):
    """Return a list of human-readable failures, empty if all assertions hold."""
    blob = ""
    for entry in files:
        rel = entry.partition(":")[0]
        try:
            blob += open(os.path.join(worktree, rel)).read() + "\n"
        except OSError:
            pass
    n = _norm(blob)
    problems = []
    for a in absent:
        if _norm(a) in n:
            problems.append(f"ASSERT_ABSENT still present: {a}")
    for p in present:
        if _norm(p) not in n:
            problems.append(f"ASSERT_PRESENT missing: {p}")
    return problems


def read_files(worktree, files):
    """Read the files a spec names.

    A spec entry may be `path` or `path:START-END` to feed only a line range.
    Whole large files are the main cause of slow generation here: throughput
    drops roughly fourfold once a few hundred lines are in context, because the
    MoE stops fitting comfortably. Prefer naming the function's range.
    """
    out = []
    for entry in files:
        rel, _, span = entry.partition(":")
        full = os.path.join(worktree, rel)
        try:
            text = open(full).read()
        except OSError as e:
            sys.exit(f"cannot read {full}: {e}")
        header = f"### FILE: {rel}"
        if span:
            try:
                start_s, _, end_s = span.partition("-")
                start, end = int(start_s), int(end_s)
            except ValueError:
                sys.exit(f"bad line span {span!r} in {entry!r}, want path:START-END")
            lines = text.splitlines()
            start = max(1, start)
            end = min(len(lines), end)
            text = "\n".join(lines[start - 1 : end])
            header += f"  (lines {start}-{end} only; anchors must come from here)"
        out.append(f"{header}\n```rust\n{text}\n```")
    return "\n\n".join(out)


def ask(model, messages, timeout=1800, max_tokens=None):
    """One chat completion against llama-server.

    The 900-token cap that suited the 7B is now a liability: Phase 2 allows
    function-level and small multi-file edits, and a legitimate three-file diff
    does not fit in 900 tokens. It gets truncated mid-hunk, the patch fails to
    apply, and the iteration is wasted. The cap exists to stop a rambling model
    burning minutes, not to constrain a correct answer, so it scales with the
    edit class instead.
    """
    payload = {
        "model": model,
        "messages": messages,
        "stream": False,
        "temperature": 0.1,
        "max_tokens": max_tokens or int(os.environ.get("LLAMA_MAX_TOKENS", "3000")),
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
    # OpenAI shape from llama-server; keep the Ollama shape working so an older
    # server or a replayed fixture does not blow up here.
    if "choices" in body:
        return body["choices"][0]["message"]["content"]
    return body["message"]["content"]


def parse_blocks(text):
    """Split on the SEARCH header first, then parse each chunk independently.

    A single regex over the whole reply mis-pairs when two blocks follow each
    other: the non-greedy replace group runs past its own >>>>>>> REPLACE and
    swallows the next block's header, which then gets written into the file as
    literal conflict markers. That corrupted a source file before this was
    rewritten, so keep the chunking.
    """
    out = []
    for chunk in text.split("<<<<<<< SEARCH")[1:]:
        end = chunk.find(">>>>>>> REPLACE")
        if end == -1:
            continue
        body = chunk[:end]
        parts = body.split("\n=======\n")
        if len(parts) < 3:
            continue
        path = parts[0].strip().lstrip("\n")
        search = parts[1]
        # anything after the second separator belongs to replace; rejoin in case
        # the replacement text legitimately contains a ======= line
        replace = "\n=======\n".join(parts[2:])
        if search.endswith("\n"):
            search = search[:-1]
        if replace.endswith("\n"):
            replace = replace[:-1]
        if path and search:
            out.append((path, search, replace))
    return out


DIFF_FENCE = re.compile(r"```(?:diff|patch)?\s*\n(.*?)```", re.S)


def _strip_fences(block):
    """Drop any ``` line and any prose before the first diff header.

    Truncation at the token cap regularly leaves a fence unterminated, and the
    bare-diff fallback then grabs the whole reply, fence included. git apply
    rejects the entire patch over that one line.
    """
    lines = [ln for ln in block.splitlines() if not ln.lstrip().startswith("```")]
    for i, ln in enumerate(lines):
        if ln.startswith("--- ") or ln.startswith("diff --git"):
            lines = lines[i:]
            break
    # a trailing truncated hunk is worse than no hunk; cut at the last usable line
    while lines and not (
        lines[-1].startswith((" ", "+", "-", "@"))
        or lines[-1].startswith("diff --git")
    ):
        lines.pop()
    return "\n".join(lines)


def extract_diffs(text):
    """Pull unified diffs out of a reply, fenced, unterminated, or bare."""
    cands = [m.group(1) for m in DIFF_FENCE.finditer(text)]
    if not any("@@" in c for c in cands):
        # unterminated fence: take everything after the opening marker
        m = re.search(r"```(?:diff|patch)?[ \t]*\n", text)
        if m:
            cands.append(text[m.end() :])
        elif "@@" in text and ("--- " in text or "+++ " in text):
            cands.append(text)
    out = []
    for c in cands:
        c = _strip_fences(c)
        if "@@" in c and ("--- " in c or "+++ " in c):
            out.append(c)
    return out


def apply_unified_diff(worktree, text):
    """Apply unified diffs, which is what the model actually emits by default.

    Models routinely miscount hunk headers, so --recount is essential; without
    it a correct edit is rejected over arithmetic. Fall back to patch(1) with
    fuzz, then to a 3-way merge, before giving up.
    """
    applied, failed = 0, []
    for i, d in enumerate(extract_diffs(text)):
        # A bare "@@" is rejected outright even with --recount, so give every
        # hunk a syntactically valid header and let --recount fix the numbers.
        d = re.sub(r"(?m)^@@[ \t]*$", "@@ -1,1 +1,1 @@", d)
        d = re.sub(r"(?m)^@@(?![ \t]*-)[ \t]*(.*)$", r"@@ -1,1 +1,1 @@ \1", d)
        if not d.endswith("\n"):
            d += "\n"
        p = os.path.join("/tmp", f"pumpkin_hunk_{os.getpid()}_{i}.patch")
        with open(p, "w") as f:
            f.write(d)
        for cmd in (
            f"git apply --recount -p1 {p}",
            f"git apply --recount -p0 {p}",
            f"patch -p1 -l -F3 --no-backup-if-mismatch -i {p}",
            f"git apply --recount -3 -p1 {p}",
        ):
            rc, _, err = sh(cmd, cwd=worktree, timeout=120)
            if rc == 0:
                applied += 1
                break
        else:
            failed.append(f"diff {i}: could not apply ({err.strip()[:160]})")
        try:
            os.remove(p)
        except OSError:
            pass
    return applied, failed


def hunks_to_pairs(text):
    """Turn diff hunks into (search, replace) pairs by CONTEXT, ignoring headers.

    Good code models emit clean hunks but often omit the ---/+++ headers, or
    miscount the @@ line numbers, and git apply rejects the whole patch over
    that. The content is fine; only the bookkeeping is wrong. So rebuild the
    before and after text from the body: context and removed lines are the
    search, context and added lines are the replacement. Header arithmetic
    stops mattering entirely.
    """
    pairs = []
    for block in extract_any_code_block(text):
        before, after = [], []
        for ln in block.splitlines():
            if ln.startswith(("--- ", "+++ ", "diff --git", "index ")):
                continue
            if ln.startswith("@@"):
                if before or after:
                    pairs.append(("\n".join(before), "\n".join(after)))
                    before, after = [], []
                continue
            if ln.startswith("-"):
                before.append(ln[1:])
            elif ln.startswith("+"):
                after.append(ln[1:])
            else:
                ctx = ln[1:] if ln.startswith(" ") else ln
                before.append(ctx)
                after.append(ctx)
        if before or after:
            pairs.append(("\n".join(before), "\n".join(after)))
    return [(b, a) for b, a in pairs if b.strip() and b != a]


def extract_any_code_block(text):
    """Every fenced block, plus the raw text if nothing is fenced."""
    blocks = re.findall(r"```[a-zA-Z]*\s*\n(.*?)(?:```|\Z)", text, re.S)
    return blocks if blocks else [text]


def apply_by_context(worktree, files, text):
    """Apply hunks by locating their context, matching on stripped lines.

    Exact matching cannot work: a model writes the marker in place of one
    indent space, so a removed line comes back one space short, and it often
    emits context lines with no leading space at all. Both are cosmetic, so
    match on stripped text and take indentation from the file itself. Added
    lines are re-indented to the indent of the line they follow.
    """
    applied, failed = 0, []
    paths = [os.path.join(worktree, e.partition(":")[0]) for e in files]

    for before, after in hunks_to_pairs(text):
        want = [ln.strip() for ln in before.splitlines() if ln.strip()]
        if not want:
            continue
        hit = False
        for full in paths:
            try:
                lines = open(full).read().splitlines(keepends=True)
            except OSError:
                continue
            stripped = [ln.strip() for ln in lines]
            starts = [
                i
                for i in range(len(lines) - len(want) + 1)
                if [s for s in stripped[i : i + len(want)]] == want
            ]
            if len(starts) != 1:
                continue
            i = starts[0]
            window = lines[i : i + len(want)]

            def indent_of(s):
                return s[: len(s) - len(s.lstrip())]

            # Walk the window and the replacement together. An unchanged line
            # keeps the file's exact original text; a new line inherits the
            # indent of the window line at the same position, which is the line
            # it is replacing. Positional, not keyed, because duplicate lines
            # such as a bare closing brace are common and would collide.
            new_lines = [ln.strip() for ln in after.splitlines() if ln.strip()]
            rebuilt, w = [], 0
            for idx, key in enumerate(new_lines):
                if w < len(window) and window[w].strip() == key:
                    rebuilt.append(window[w])
                    w += 1
                    continue
                ref = window[min(w, len(window) - 1)]
                rebuilt.append(f"{indent_of(ref)}{key}\n")
                # a replaced line consumes its counterpart in the window
                if w < len(window) and window[w].strip() not in new_lines[idx:]:
                    w += 1
            lines[i : i + len(want)] = rebuilt
            open(full, "w").write("".join(lines))
            applied += 1
            hit = True
            break
        if not hit:
            failed.append(f"context not found or not unique, starting: {want[0][:80]}")
    return applied, failed


LINE_EDIT_SYSTEM = """You are editing one line of Rust in the Pumpkin Minecraft server.

You will be given exactly one line of code and an instruction describing how to
change it.

Reply with the corrected line and NOTHING else. No diff, no fence, no prose, no
explanation, no line number. Just the single corrected line of code.

Preserve the leading indentation exactly as given.
Change only what the instruction asks for. Do not reformat, do not rename, do
not add or remove anything else on the line.
If the instruction cannot be satisfied by editing this one line, reply with
exactly: CANNOT
"""


def apply_line_edit(worktree, files, target, reply):
    """Replace one exact line with the model's single-line answer.

    For an atomic edit, diff syntax is pure overhead and pure risk: the model
    emitted a perfectly formed diff whose minus and plus lines were identical
    while the real target sat untouched in the context. Asking for just the
    replacement line removes every way to get the formatting wrong.

    Returns (applied, problems, written). `written` is the line now in the
    file, or None. The caller must adopt it as the new target: after a
    successful apply the original target line no longer exists, and a retry
    that still searches for it fails with "target line not found" forever.
    Specs 01 and 02 burned all their iterations against exactly that.
    """
    new = reply.strip()
    for fence in ("```rust", "```diff", "```"):
        new = new.replace(fence, "")
    new = new.strip("\n").strip()
    if not new or new == "CANNOT":
        return 0, ["model replied CANNOT or empty"], None
    if "\n" in new:
        lines = [ln for ln in new.splitlines() if ln.strip()]
        if len(lines) != 1:
            return 0, [f"expected one line, got {len(lines)}"], None
        new = lines[0]
    if new.strip() == target.strip():
        return 0, ["replacement is identical to the target line"], None

    for entry in files:
        full = os.path.join(worktree, entry.partition(":")[0])
        try:
            content = open(full).read()
        except OSError:
            continue
        lines = content.splitlines(keepends=True)
        hits = [i for i, ln in enumerate(lines) if ln.strip() == target.strip()]
        if len(hits) != 1:
            continue
        i = hits[0]
        indent = lines[i][: len(lines[i]) - len(lines[i].lstrip())]
        lines[i] = f"{indent}{new.strip()}\n"
        open(full, "w").write("".join(lines))
        return 1, [], new.strip()
    return 0, [f"target line not found or not unique: {target.strip()[:80]}"], None


def apply_blocks(worktree, text, files=None):
    """Unified diff, then context matching, then the SEARCH/REPLACE form."""
    applied, failed = apply_unified_diff(worktree, text)
    if applied:
        return applied, failed
    if files:
        applied, ctx_failed = apply_by_context(worktree, files, text)
        if applied:
            return applied, ctx_failed
        failed += ctx_failed
    for rel, search, replace in parse_blocks(text):
        if "<<<<<<<" in replace or ">>>>>>>" in replace:
            failed.append(f"{rel}: replacement contained conflict markers, refused")
            continue
        full = os.path.join(worktree, rel)
        if not os.path.isfile(full):
            failed.append(f"{rel}: no such file")
            continue
        content = open(full).read()
        if search not in content:
            first = search.splitlines()[0][:90] if search.splitlines() else "(empty)"
            failed.append(f"{rel}: SEARCH anchor not found. First line was: {first}")
            continue
        n = content.count(search)
        if n > 1:
            failed.append(f"{rel}: SEARCH anchor matched {n} times, needs to be unique")
            continue
        open(full, "w").write(content.replace(search, replace, 1))
        applied += 1
    return applied, failed


def build(worktree, crates):
    pkgs = " ".join(f"-p {c}" for c in crates)
    cmd = (
        CARGO_ENV
        + f'cd {worktree} && cargo fmt --all && RUSTFLAGS="-D warnings" cargo clippy {pkgs} --all-targets 2>&1'
    )
    rc, out, err = sh(cmd, timeout=2400)
    return rc, (out + err)


def distill_errors(output, limit=6):
    """Keep the model's context small: only real errors, deduped, capped."""
    chunks, cur = [], []
    for line in output.splitlines():
        if re.match(r"^(error|warning)(\[|:)", line):
            if cur:
                chunks.append("\n".join(cur))
            cur = [line]
        elif cur and (line.startswith(" ") or line.startswith("  -->")):
            cur.append(line)
        elif cur:
            chunks.append("\n".join(cur))
            cur = []
    if cur:
        chunks.append("\n".join(cur))
    real = [
        c for c in chunks if "proc-macro-error2" not in c and "future-incompat" not in c
    ]
    seen, uniq = set(), []
    for c in real:
        key = c.splitlines()[0]
        if key not in seen:
            seen.add(key)
            uniq.append(c[:1200])
    return "\n\n".join(uniq[:limit])


def unload(model):  # noqa: ARG001 - kept for call-site compatibility
    """No-op under llama-server.

    Ollama's keep_alive:0 trick evicted the model between cargo runs so builds
    and inference did not compete for VRAM. llama-server holds the model for its
    whole lifetime by design, and that is fine here: the MoE experts sit in
    system RAM and only ~6 GB of attention plus KV stays resident on the card,
    which cargo does not want anyway. Leave the server up.
    """
    time.sleep(1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("spec")
    ap.add_argument("--max-iters", type=int, default=6)
    ap.add_argument("--model", default=DEFAULT_MODEL)
    a = ap.parse_args()

    worktree, files, crates, instructions = parse_spec(a.spec)
    log(f"worktree={worktree} crates={crates} files={len(files)}")

    _, base_diff, _ = sh("git diff --stat", cwd=worktree)
    if base_diff.strip():
        log(f"WARNING: worktree already dirty:\n{base_diff}")

    msgs = [
        {"role": "system", "content": SYSTEM},
        {"role": "user", "content": f"{instructions}\n\n{read_files(worktree, files)}"},
    ]

    for it in range(1, a.max_iters + 1):
        log(f"--- iteration {it}: asking model ---")
        t0 = time.time()
        try:
            reply = ask(a.model, msgs)
        except Exception as e:
            log(f"model call failed: {e}")
            return 2
        log(f"model replied in {time.time() - t0:.0f}s, {len(reply)} chars")

        applied, failed = apply_blocks(worktree, reply, files)
        log(f"applied {applied} block(s), {len(failed)} failed")
        if failed and applied == 0:
            msgs += [
                {"role": "assistant", "content": reply},
                {
                    "role": "user",
                    "content": "None of your blocks applied:\n"
                    + "\n".join(failed)
                    + "\nCopy the SEARCH text verbatim from the file content above. Try again.",
                },
            ]
            continue

        log("building (model unloaded first)")
        unload(a.model)
        rc, out = build(worktree, crates)
        if rc == 0:
            log("BUILD CLEAN")
            _, diff, _ = sh("git diff", cwd=worktree)
            _, stat, _ = sh("git diff --stat", cwd=worktree)
            open("/tmp/harness_result.diff", "w").write(diff)
            print("\n===== RESULT: CLEAN =====")
            print(stat)
            print("full diff written to /tmp/harness_result.diff")
            return 0

        errs = distill_errors(out)
        log(f"build failed, feeding back {len(errs)} chars of errors")
        if not errs:
            errs = out[-3000:]
        msgs += [
            {"role": "assistant", "content": reply},
            {
                "role": "user",
                "content": "The build failed. Fix these, replying only in SEARCH/REPLACE blocks "
                "against the CURRENT file contents (your earlier edits are already applied):\n\n"
                + errs,
            },
        ]

    log("giving up after max iterations")
    _, stat, _ = sh("git diff --stat", cwd=worktree)
    print("\n===== RESULT: NOT CLEAN =====")
    print(stat)
    return 1


if __name__ == "__main__":
    sys.exit(main())
