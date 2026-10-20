#!/usr/bin/env python3
"""
Whole-function replacement mode.

Unified diff is the wrong interface for this model. At 30B it reasons about the
change correctly and then loses on diff bookkeeping: it omits the `-` line for a
line it is replacing, drops the leading space on context lines, or emits a hunk
whose context is real but not unique. Every one of those is a wasted iteration
spent on formatting rather than on the fix, and the observed failure rate was
9 out of 10 on the first batch.

So: the spec names a function, the harness shows the model that function, and
the model returns the complete new function and nothing else. There is no
addressing scheme to get wrong. The function is located by its signature line
and replaced up to its matching closing brace.

A spec opts in with:

    FUNC: pub async fn add_effect(&self, effect: Effect) {

The value must appear on exactly one line of exactly one FILES entry.
"""

import re

FUNC_SYSTEM = """You are a Rust engineer working on Pumpkin, a from-scratch
Minecraft server in Rust targeting Java Edition 26.2.

# Output format

You are given one complete Rust function. Reply with the complete REPLACEMENT for
that function and nothing else: no prose, no explanation, no diff, no markers.

You may be shown attributes and doc comments above the signature. They are part
of what you are replacing. Start your reply at the first line you were shown and
end at the closing brace. Include the entire body, including the parts you did
not change, and preserve the original indentation exactly.

If you are filling a stub, DELETE any `#[expect(clippy::todo, ...)]` attribute
above it. That attribute exists only to let the unfilled stub compile. Once the
`todo!()` is gone the expectation is unfulfilled, and
`unfulfilled_lint_expectations` is denied in this workspace, so leaving it there
fails the build no matter how correct your code is. Keep the doc comment.

Wrap the whole thing in a ```rust fence. Nothing outside the fence.

If the change cannot be made inside this one function, reply with exactly:
CANNOT: <one line saying what else has to change>

# The single most important rule

You do not know what vanilla Minecraft does. If the instructions state the
vanilla behaviour, implement exactly that. If they do not, or they say
UNVERIFIED, reply CANNOT. This project has twice had to publicly retract claims
that came from someone confidently misremembering vanilla. A wrong guess is worse
than no change, because it looks correct and ships.

Traps that have already caused exactly that:
- Game rules were renamed to snake_case in 25w44a: random_tick_speed, not
  randomTickSpeed.
- Weather timers use inclusive UniformInt since 23w03a.
- Snow and ice melt on BLOCK light only, never sky light.
- Dimension types were reworked in 26.x; the End has sky light now.

# Project APIs: the second rule

You do not know this codebase's APIs either. The Rust standard library is fair
game, but do not call a method, function, or associated item on a project type
unless you can see it: in the code you were shown, in an API listing the prompt
provides, or in a compiler error's suggestion. Runs here have been lost to
invented methods that sound plausible (is_in_water, get_block_light, from_nbt)
and exist only in other codebases. If the operation you need is not visible
anywhere, reply CANNOT and name the missing API.

# Correctness constraints for this repo

Concurrency. Block entities, hoppers and chunk data are mutated by tick tasks
running concurrently as spawned futures. NEVER replace an atomic fetch_sub or
fetch_add with a load, arithmetic and store. That is a lost-update race even
though it reads more clearly, and it has already been caught in review here.

Locks. Do not take a second lock while holding one, and do not widen an existing
critical section across an await.

Arithmetic. u8 subtraction must saturate. An underflow wraps to ~250 and a later
clamp turns that into 15, which reads as a silent false-bright rather than a
crash.

Clippy denies all, pedantic, nursery and cargo. Common rejections:
needless_pass_by_value, redundant_closure, missing_const_for_fn,
option_if_let_else, unnested_or_patterns. doc_markdown applies to comments too,
so any identifier in a doc comment needs backticks.

Style. Match the surrounding file. Comments explain why. Where you are matching a
vanilla behaviour, name it. Do not reformat code you are not changing and do not
fix unrelated lints you notice.

Never write an emoji. Never write an em-dash.

# Scope

Make the smallest change that is actually correct. Do not refactor. Do not fix a
second bug you happen to see.
"""


def strip_thinking(text):
    """Remove any reasoning block that leaked into the answer.

    The server is configured with --reasoning-format deepseek, which routes the
    thought into message.reasoning_content, so this should never fire. It exists
    because the failure mode if it ever does is severe and silent: a <think>
    block spliced verbatim into a Rust file, which then fails to compile with an
    error that points at the wrong thing entirely.
    """
    t = re.sub(r"<think>.*?</think>", "", text, flags=re.S)
    # An unclosed tag means the budget cut the thought off mid-sentence. Keep
    # whatever came after it, which is the answer.
    if "<think>" in t:
        t = t.split("</think>")[-1] if "</think>" in t else t.split("<think>")[0]
    return t.strip()


def strip_fences(text):
    text = strip_thinking(text)
    t = text.strip()
    t = re.sub(r"^```[a-zA-Z]*\n", "", t)
    t = re.sub(r"\n```$", "", t)
    # A model that fences twice, or narrates before the fence, still gets read.
    if "```" in t:
        blocks = re.findall(r"```(?:[a-zA-Z]*)\n(.*?)```", text, re.S)
        if blocks:
            t = max(blocks, key=len)
    return t.strip("\n")


def _attribute_start(lines, sig_line):
    """Walk back from a signature over any attributes attached to it.

    This exists because of a real failure. Scaffold stubs carry
    `#[expect(clippy::todo, ...)]` immediately above the signature. The span used
    to begin AT the signature, so the attribute was invisible to the model and
    untouched by the applier. When the model correctly filled the stub, the
    `todo!()` disappeared but the `#[expect(clippy::todo)]` remained, the
    expectation became unfulfilled, and `unfulfilled_lint_expectations` is denied
    in this workspace. Every correct answer therefore failed to build, six times
    in a row, and the log said "build failed" as though the model were at fault.

    Including the attributes in the span means the model sees them and can drop
    the scaffold one along with the todo!().
    """
    i = sig_line - 1
    while i >= 0:
        s = lines[i].strip()
        if not s:
            break
        # Attribute, possibly multi-line: keep walking while the lines above are
        # part of the same bracketed group or another attribute or doc comment.
        if s.startswith("#[") or s.startswith("#!["):
            i -= 1
            continue
        if s.startswith(")]") or s.startswith("]"):
            # Tail of a multi-line attribute; walk up to its opening.
            j = i
            while j >= 0 and not lines[j].strip().startswith("#["):
                j -= 1
            if j < 0:
                break
            i = j - 1
            continue
        if s.startswith("///") or s.startswith("//!"):
            i -= 1
            continue
        break
    return i + 1


def find_function_span(lines, signature):
    """Return (start, end) line indices inclusive for the function, or None.

    Brace counting starts at the signature line and stops when the depth returns
    to zero, which is also how the end brace is found. Braces inside string
    literals and comments would break this; in practice Rust function bodies in
    this codebase do not open unbalanced braces inside a string, and a mistake
    here is caught immediately because the replacement will not compile.
    """
    want = signature.strip()
    hits = [i for i, ln in enumerate(lines) if ln.strip() == want]
    if len(hits) != 1:
        # Fall back to a containment match, still requiring uniqueness.
        hits = [i for i, ln in enumerate(lines) if want in ln]
        if len(hits) != 1:
            return None
    start = hits[0]

    depth = 0
    seen_open = False
    for i in range(start, len(lines)):
        for ch in lines[i]:
            if ch == "{":
                depth += 1
                seen_open = True
            elif ch == "}":
                depth -= 1
                if seen_open and depth == 0:
                    # Extend upward over attributes and doc comments so the model
                    # can remove a scaffold #[expect] along with the todo!() it
                    # covers. See _attribute_start.
                    return (_attribute_start(lines, start), i)
    return None


def _drop_dead_scaffold_attr(text):
    """Remove a scaffold `#[expect(clippy::todo, ...)]` once the todo!() is gone.

    Asking the model to delete this itself was fragile: it reproduced the
    attribute verbatim along with the rest of the function, which is exactly what
    "return the complete function" instructs it to do. The result was an
    unfulfilled lint expectation, which this workspace denies, so a correct fix
    still failed to build.

    Bookkeeping the applier can do deterministically should not be delegated to
    the model. If the replacement no longer contains a todo!(), the attribute
    covering it is dead and goes.
    """
    if "todo!" in text:
        return text

    out, i = [], 0
    lines = text.splitlines(keepends=True)
    while i < len(lines):
        stripped = lines[i].strip()
        if stripped.startswith("#[expect("):
            # Collect the whole attribute, which may span lines, and keep it only
            # if it is not the scaffold one.
            block, depth = [], 0
            while i < len(lines):
                block.append(lines[i])
                depth += lines[i].count("(") - lines[i].count(")")
                i += 1
                if depth <= 0:
                    break
            joined = "".join(block)
            if "clippy::todo" not in joined:
                out.extend(block)
            continue
        out.append(lines[i])
        i += 1
    return "".join(out)


def apply_func_edit(worktree, files, signature, reply):
    """Replace the named function with the model's reply.

    Returns (applied_count, problems).
    """
    import os

    new = strip_fences(reply)
    if not new or new.startswith("CANNOT"):
        return 0, [new[:200] or "empty reply"]

    new_lines = new.splitlines()
    if not new_lines:
        return 0, ["reply had no lines"]

    sig = signature.strip()
    if not any(sig in ln for ln in new_lines):
        return 0, [
            "the reply does not contain the function signature; it must start at "
            f"the signature line: {sig}"
        ]
    if new.count("{") != new.count("}"):
        return 0, ["braces are unbalanced in the reply, the function is truncated"]

    for entry in files:
        rel = entry.partition(":")[0]
        full = os.path.join(worktree, rel)
        try:
            with open(full) as fh:
                content = fh.read()
        except OSError:
            continue
        lines = content.splitlines(keepends=True)
        span = find_function_span([ln.rstrip("\n") for ln in lines], sig)
        if span is None:
            continue
        start, end = span

        old = "".join(lines[start : end + 1])
        replacement = "\n".join(new_lines) + "\n"
        replacement = _drop_dead_scaffold_attr(replacement)
        if old.strip() == replacement.strip():
            return 0, ["the replacement is identical to the current function"]

        lines[start : end + 1] = [replacement]
        with open(full, "w") as fh:
            fh.write("".join(lines))
        return 1, []

    return 0, [f"could not locate a unique function matching: {sig}"]


def read_function(worktree, files, signature):
    """The exact current text of the function, to show the model."""
    import os

    sig = signature.strip()
    for entry in files:
        rel = entry.partition(":")[0]
        full = os.path.join(worktree, rel)
        try:
            with open(full) as fh:
                content = fh.read()
        except OSError:
            continue
        raw = content.splitlines()
        span = find_function_span(raw, sig)
        if span is None:
            continue
        start, end = span
        body = "\n".join(raw[start : end + 1])
        return f"### FILE: {rel}  (this function only)\n```rust\n{body}\n```"
    return None



MULTI_SYSTEM = FUNC_SYSTEM.replace(
    "# Output format",
    """# Output format (MULTI)

You are given SEVERAL complete Rust functions, each under a `### FUNCTION N`
header with its file. The change spans more than one of them. Reply with the
complete REPLACEMENT for EACH function you change, and only those, one fenced
block per function:

```rust
// FUNCTION 1
<the whole function, signature to closing brace>
```

```rust
// FUNCTION 2
<the whole function>
```

Rules that still hold from single-function mode:""",
    1,
)


def _all_rust_fences(text):
    """Every ```rust ... ``` block, largest-first ties broken by order."""
    t = strip_thinking(text)
    blocks = re.findall(r"```(?:rust)?\s*\n(.*?)```", t, re.S)
    return [b.strip("\n") for b in blocks if b.strip()]


def read_functions(worktree, files, signatures):
    """Show several functions at once, each under a numbered header.

    Returns (shown_text, missing). `missing` names any signature that did not
    locate a unique function, so the caller can fail as BADSPEC rather than
    silently dropping a target.
    """
    parts, missing = [], []
    for n, sig in enumerate(signatures, 1):
        one = read_function(worktree, files, sig)
        if one is None:
            missing.append(sig)
            continue
        body = one.split("```rust\n", 1)[-1].rsplit("\n```", 1)[0]
        rel = one.split("### FILE: ", 1)[-1].split("  ", 1)[0]
        parts.append(f"### FUNCTION {n}  (in {rel})\n```rust\n{body}\n```")
    return "\n\n".join(parts), missing


def _replace_one(worktree, files, signature, body):
    """Replace exactly one function's span with `body` (already unfenced).

    The single-function core, factored out so MULTI can reuse the same
    deterministic span replace instead of a second, divergent applier.
    """
    import os

    sig = signature.strip()
    body_lines = body.splitlines()
    if not any(sig in ln for ln in body_lines):
        return 0, f"block for {sig[:50]} does not contain its signature line"
    if body.count("{") != body.count("}"):
        return 0, f"braces unbalanced for {sig[:50]}, the function is truncated"
    for entry in files:
        rel = entry.partition(":")[0]
        full = os.path.join(worktree, rel)
        try:
            with open(full) as fh:
                content = fh.read()
        except OSError:
            continue
        lines = content.splitlines(keepends=True)
        span = find_function_span([ln.rstrip("\n") for ln in lines], sig)
        if span is None:
            continue
        start, end = span
        old = "".join(lines[start : end + 1])
        replacement = _drop_dead_scaffold_attr("\n".join(body_lines) + "\n")
        if old.strip() == replacement.strip():
            return 0, f"{sig[:50]} is unchanged from the current function"
        lines[start : end + 1] = [replacement]
        with open(full, "w") as fh:
            fh.write("".join(lines))
        return 1, None
    return 0, f"could not locate a unique function matching {sig[:50]}"


def apply_multi_func(worktree, files, signatures, reply):
    """Apply a multi-function reply: one fenced block per changed function.

    Same deterministic span replace as single mode, run per target. A block
    is routed to the signature it contains, so order does not matter and the
    model may legitimately return a subset (it changed only some functions).
    Returns (applied_count, problems).
    """
    text = strip_thinking(reply)
    if text.strip().startswith("CANNOT"):
        return 0, [text.strip()[:200]]
    fences = _all_rust_fences(reply)
    if not fences:
        return 0, ["no ```rust blocks in the reply"]

    applied, problems, used = 0, [], set()
    for sig in signatures:
        match = next(
            (
                f
                for i, f in enumerate(fences)
                if i not in used and any(sig.strip() in ln for ln in f.splitlines())
            ),
            None,
        )
        if match is None:
            continue  # model did not change this one; legitimate
        used.add(fences.index(match))
        n, problem = _replace_one(worktree, files, sig, match)
        applied += n
        if problem:
            problems.append(problem)
    if applied == 0 and not problems:
        problems.append(
            "no block matched any of the target signatures; each block must "
            "start at a function's signature line"
        )
    return applied, problems



INSERT_SYSTEM = FUNC_SYSTEM.replace(
    "# Output format",
    """# Output format (INSERT)

The fix needs a NEW item (a method, a helper function, a trait method) that
does not exist yet. You are shown the block it must go into, under a
`### BLOCK: <anchor>` header. Reply with the complete new item and nothing
else, in one ```rust fence, tagged on its first line with the anchor:

```rust
// INTO: <anchor exactly as shown>
pub fn the_new_thing(...) -> ... {
    ...
}
```

If several new items are needed in different blocks, one fence each. Write
the whole item, signature to closing brace. It will be inserted just before
the block's closing brace; you do not write the brace of the block itself.

Rules that still hold:""",
    1,
)


def find_block_span(lines, anchor):
    """(start, end) inclusive for the block whose header matches `anchor`.

    Header is matched by containment and must be unique. The block runs from
    its header's opening brace to the matching close. Used to insert a new
    item before that closing brace, deterministically, instead of letting the
    model guess a byte offset.
    """
    want = anchor.strip()
    hits = [i for i, ln in enumerate(lines) if want in ln and "{" in "".join(lines[i : i + 3])]
    # Prefer an exact-ish header line (starts with impl/trait/mod/pub).
    strong = [i for i in hits if re.match(r"^\s*(pub\s+)?(impl|trait|mod)\b", lines[i])]
    hits = strong or hits
    if len(hits) != 1:
        return None
    start = hits[0]
    depth, seen_open = 0, False
    for i in range(start, len(lines)):
        for ch in lines[i]:
            if ch == "{":
                depth += 1
                seen_open = True
            elif ch == "}":
                depth -= 1
                if seen_open and depth == 0:
                    return (start, i)
    return None


def _item_name(body):
    """The fn/const/type/method name a new item defines, or None."""
    m = re.search(r"\b(?:fn|const|type|static)\s+([A-Za-z_][A-Za-z0-9_]*)", body)
    return m.group(1) if m else None


def read_block(worktree, files, anchor, max_lines=40):
    """Show the anchor block: header, a slice of its body, and its close.

    Enough for the model to match style and see the closing brace it must
    insert before, without pasting a 400-line impl into the prompt.
    """
    import os

    a = anchor.strip()
    for entry in files:
        rel = entry.partition(":")[0]
        full = os.path.join(worktree, rel)
        try:
            raw = open(full).read().splitlines()
        except OSError:
            continue
        span = find_block_span(raw, a)
        if span is None:
            continue
        start, end = span
        body = raw[start : end + 1]
        if len(body) > max_lines:
            head = body[: max_lines - 6]
            body = head + ["    // ... existing items ..."] + body[-5:]
        shown = "\n".join(body)
        return f"### BLOCK: {a}  (in {rel})\n```rust\n{shown}\n```"
    return None


def read_blocks(worktree, files, anchors):
    parts, missing = [], []
    for a in anchors:
        one = read_block(worktree, files, a)
        if one is None:
            missing.append(a)
        else:
            parts.append(one)
    return "\n\n".join(parts), missing


def _insert_one(worktree, files, anchor, item):
    """Ensure `item` exists in the anchor block, with this body.

    Idempotent by design: if an item of the same name is already in the
    block it is REPLACED (so a retry re-emitting the item does not duplicate
    it); otherwise the item is inserted just before the block's closing
    brace. That makes INSERT safe to run every iteration.
    """
    import os

    a = anchor.strip()
    name = _item_name(item)
    if item.count("{") != item.count("}"):
        return 0, f"braces unbalanced in the new item for {a[:40]}"
    for entry in files:
        rel = entry.partition(":")[0]
        full = os.path.join(worktree, rel)
        try:
            content = open(full).read()
        except OSError:
            continue
        lines = content.splitlines(keepends=True)
        span = find_block_span([ln.rstrip("\n") for ln in lines], a)
        if span is None:
            continue
        start, end = span
        block_txt = "".join(lines[start : end + 1])

        # Already present? Replace that item in place instead of inserting.
        if name and re.search(rf"\bfn\s+{re.escape(name)}\b", block_txt):
            inner = [ln.rstrip("\n") for ln in lines]
            # locate the fn by name within the block
            fn_hits = [
                start + k
                for k, ln in enumerate(inner[start : end + 1])
                if re.search(rf"\bfn\s+{re.escape(name)}\b", ln)
            ]
            if len(fn_hits) == 1:
                fsig = inner[fn_hits[0]].strip().rstrip("{").strip() + " {"
                sub = find_function_span(inner, fsig)
                if sub:
                    s2, e2 = sub
                    new_item = item if item.endswith("\n") else item + "\n"
                    old = "".join(lines[s2 : e2 + 1])
                    if old.strip() == new_item.strip():
                        return 0, f"{name} unchanged"
                    lines[s2 : e2 + 1] = [_reindent(new_item, lines[s2])]
                    open(full, "w").write("".join(lines))
                    return 1, None

        # Insert before the block's closing brace.
        close_line = lines[end]
        indent = close_line[: len(close_line) - len(close_line.lstrip())] + "    "
        block_txt_now = "".join(lines[start : end + 1])
        if name and re.search(rf"\bfn\s+{re.escape(name)}\b", block_txt_now):
            return 0, f"{name} already present but could not be located to replace"
        new_item = _reindent(item.rstrip("\n") + "\n", indent)
        lines[end:end] = ["\n", new_item]
        open(full, "w").write("".join(lines))
        return 1, None
    return 0, f"could not find a unique block matching {a[:40]}"


def _reindent(text, indent_ref):
    """Indent `text`'s lines to the given reference indent (its first line)."""
    base = indent_ref[: len(indent_ref) - len(indent_ref.lstrip())]
    out = []
    for ln in text.splitlines():
        out.append((base + ln.lstrip()) if ln.strip() else ln)
    return "\n".join(out) + "\n"


def apply_insert(worktree, files, anchors, reply):
    """Apply an INSERT reply: one fenced new item per anchor, tagged INTO:.

    Returns (applied_count, problems). Missing anchors in the reply are fine
    (the model added only some); a fence whose INTO: matches no declared
    anchor is a problem.
    """
    text = strip_thinking(reply)
    if text.strip().startswith("CANNOT"):
        return 0, [text.strip()[:200]]
    fences = _all_rust_fences(reply)
    if not fences:
        return 0, ["no ```rust blocks in the reply"]

    applied, problems = 0, []
    for f in fences:
        m = re.search(r"//\s*INTO:\s*(.+)", f)
        if not m:
            # Single-anchor specs: an untagged fence goes to the sole anchor.
            if len(anchors) == 1:
                anchor, item = anchors[0], f
            else:
                problems.append("a fence has no // INTO: tag and there are several anchors")
                continue
        else:
            tag = m.group(1).strip()
            anchor = next((a for a in anchors if a.strip() in tag or tag in a.strip()), None)
            if anchor is None:
                problems.append(f"INTO: {tag[:40]} matches no declared anchor")
                continue
            item = re.sub(r"//\s*INTO:.*\n?", "", f, count=1)
        n, problem = _insert_one(worktree, files, anchor, item)
        applied += n
        if problem:
            problems.append(problem)
    if applied == 0 and not problems:
        problems.append("no item could be inserted")
    return applied, problems
