"""
Real API surfaces for the model's prompt.

The dominant model failure after the retry-loop fix was invented APIs:
`is_in_water` on Player, `is_raining_at_unchecked` on World,
`BlockState::from_nbt`, light methods on BlockAccessor (twice). A 3B-active
model writing against an unfamiliar codebase guesses method names the way a
human would without autocomplete, because that is exactly what it is doing.

The permanent fix is to be the autocomplete. This module reads the true
signatures of a type's inherent impls and trait definitions straight off the
worktree, so the listing can never go stale: it is generated from the tree the
model is about to edit, every run.

Used two ways:
  - proactively: api_pack() at first-prompt time, for the types the shown
    function and instructions mention;
  - reactively: error_types() + type_surface() when an E0599 names the exact
    type the model guessed wrong about.
"""

import os
import re
import subprocess

# CamelCase names that appear constantly and are never project types worth
# listing, or whose surfaces are enormous and known to the model already.
COMMON = {
    "Self", "Some", "None", "Ok", "Err", "String", "Vec", "Option", "Result",
    "Box", "Arc", "Rc", "Mutex", "RwLock", "HashMap", "HashSet", "BTreeMap",
    "Ordering", "Duration", "Instant", "PhantomData", "Default", "Debug",
    "Clone", "Copy", "Display", "From", "Into", "TryFrom", "TryInto", "Send",
    "Sync", "Sized", "Iterator", "IntoIterator", "AsRef", "Deref", "Drop",
    "Fn", "FnMut", "FnOnce", "ToString", "PartialEq", "Eq", "PartialOrd",
    "Ord", "Hash", "Cow", "Cell", "RefCell", "AtomicBool", "AtomicI32",
    "AtomicU8", "AtomicU32", "AtomicI64", "TypeId",
}

_FN_RE = re.compile(
    r"^\s*(?:pub(?:\([a-z: ]+\))?\s+)?(?:default\s+)?(?:const\s+)?"
    r"(?:async\s+)?(?:unsafe\s+)?fn\s+[A-Za-z_]"
)


def _rg(worktree, pattern):
    """file:line hits for a regex, honouring .gitignore (skips target/)."""
    try:
        p = subprocess.run(
            ["rg", "-n", "--glob", "*.rs", "-e", pattern, "."],
            cwd=worktree, capture_output=True, text=True, timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired):
        return []
    hits = []
    for line in p.stdout.splitlines():
        path, _, rest = line.partition(":")
        num, _, _ = rest.partition(":")
        if num.isdigit():
            hits.append((path.lstrip("./"), int(num) - 1))
    return hits


def _block_signatures(lines, start):
    """fn signatures inside the brace block whose header is at `start`.

    Signatures may span lines; each is joined and cut at its `{` or `;`.
    """
    sigs, depth, i, opened = [], 0, start, False
    while i < len(lines) and (not opened or depth > 0):
        depth += lines[i].count("{") - lines[i].count("}")
        if "{" in lines[i]:
            opened = True
        if opened and depth == 0 and i > start:
            break
        if i > start and _FN_RE.match(lines[i]):
            sig, j = [], i
            while j < len(lines):
                sig.append(lines[j].strip())
                if "{" in lines[j] or ";" in lines[j]:
                    break
                j += 1
            joined = " ".join(sig)
            cut = min(
                (joined.index(c) for c in "{;" if c in joined), default=len(joined)
            )
            sigs.append(" ".join(joined[:cut].split()).rstrip())
        i += 1
    return sigs


def type_surface(worktree, type_name, max_sigs=40):
    """The real methods of a project type, read off the worktree.

    Inherent impls and the type's own trait definition first; trait impls ON
    the type (`impl X for T`) only as a fallback, because Display/From/Debug
    impls are noise next to the surface the model actually needs.
    """
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", type_name or ""):
        return None
    own = rf"^\s*(pub\s+)?((impl(<[^>]*>)?\s+{type_name}\b[^;]*)|(trait\s+{type_name}\b))"
    hits = _rg(worktree, own)
    if not hits:
        hits = _rg(worktree, rf"^\s*impl(<[^>]*>)?\s+[^;]*\bfor\s+{type_name}\b")
    # The main impl usually lives in a file named after the type
    # (player.rs, world/mod.rs). Without this, Player's surface started at
    # bossbar.rs and the methods that matter fell off the cap.
    hits.sort(key=lambda h: (type_name.lower() not in h[0].lower(), h[0]))
    sigs, files, seen = [], [], set()
    for path, lineno in hits[:8]:
        try:
            with open(os.path.join(worktree, path)) as fh:
                lines = fh.read().splitlines()
        except OSError:
            continue
        for s in _block_signatures(lines, lineno):
            if s not in seen:
                seen.add(s)
                sigs.append(s)
        if path not in files:
            files.append(path)
        if len(sigs) >= max_sigs:
            break
    if not sigs:
        return None
    head = f"### Real API of `{type_name}` (from {files[0]})"
    return head + "\n" + "\n".join(sigs[:max_sigs])


def types_in_text(text):
    """Project-type candidates, in order of first appearance."""
    out, seen = [], set()
    for m in re.finditer(r"\b[A-Z][A-Za-z0-9]*\b", text or ""):
        t = m.group(0)
        if t not in seen and t not in COMMON:
            seen.add(t)
            out.append(t)
    return out


def api_pack(worktree, text, extra_types=(), max_types=6, budget=6000):
    """A compact API listing for the types a prompt is about to mention.

    Budgeted in characters because the context window is 16k tokens and the
    listing must never crowd out the function or the instructions.
    """
    parts, used = [], 0
    candidates = list(extra_types) + types_in_text(text)
    seen = set()
    for t in candidates:
        if t in seen:
            continue
        seen.add(t)
        if len(parts) >= max_types or used >= budget:
            break
        s = type_surface(worktree, t)
        if s and used + len(s) <= budget:
            parts.append(s)
            used += len(s)
    if not parts:
        return ""
    return (
        "# Verified API surfaces\n\n"
        "These signatures were extracted from this exact source tree. A method "
        "on these types that is not listed here and not visible in the shown "
        "code DOES NOT EXIST.\n\n" + "\n\n".join(parts)
    )


_E0599 = re.compile(
    r"found for (?:struct|enum|union|reference|mutable reference|type|"
    r"opaque type|trait object|raw pointer)?\s*`([^`]+)`"
)


def error_types(errs):
    """Type names an E0599-style error says the model guessed wrong about."""
    out, seen = [], set()
    for raw in _E0599.findall(errs or ""):
        t = raw.strip().lstrip("&").strip()
        t = re.sub(r"^(mut|dyn)\s+", "", t)
        # Unwrap generic wrappers: std::sync::Arc<world::World> -> world::World
        while True:
            m = re.fullmatch(r"[\w:]+<(.+)>", t)
            if not m:
                break
            t = m.group(1).strip()
        t = t.split("::")[-1].strip()
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", t) and t not in seen:
            seen.add(t)
            out.append(t)
    return out



def repo_map(worktree, files, budget=2600):
    """A compact 'where things live' map for the crates a spec touches.

    Not the whole repo: the module tree of each FILES entry's crate, plus the
    public item names in the spec's own files. Gives a multi-function edit
    enough orientation to reference a real path instead of inventing a module
    (spec 15 hallucinated `pumpkin_api`; this is the cheap antidote).
    """
    import os

    crates, seen = [], set()
    for entry in files:
        rel = entry.partition(":")[0]
        crate = rel.split("/", 1)[0]
        if crate and crate not in seen:
            seen.add(crate)
            crates.append(crate)

    lines, used = [], 0
    for crate in crates:
        srcdir = os.path.join(worktree, crate, "src")
        if not os.path.isdir(srcdir):
            continue
        mods = []
        for root, _, fnames in os.walk(srcdir):
            for fn in fnames:
                if fn.endswith(".rs"):
                    p = os.path.relpath(os.path.join(root, fn), worktree)
                    mods.append(p)
        # Orient around the edit: modules in the same directory as a FILES
        # entry come first (the siblings the model is most likely to need),
        # then the rest alphabetically. Pure alphabetical buried the edit's
        # own neighbourhood under unrelated leading directories.
        near_dirs = {os.path.dirname(e.partition(":")[0]) for e in files}
        mods.sort(key=lambda m: (os.path.dirname(m) not in near_dirs, m))
        block = f"### {crate} modules\n" + "\n".join(mods[:80])
        if used + len(block) > budget:
            block = f"### {crate} modules\n" + "\n".join(mods[:40]) + "\n..."
        lines.append(block)
        used += len(block)
        if used >= budget:
            break
    if not lines:
        return ""
    return (
        "# Repo map\n\nWhere modules live in this tree. Use real paths from "
        "here; do not invent module names.\n\n" + "\n\n".join(lines)
    )
