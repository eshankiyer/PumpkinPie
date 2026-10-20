#!/usr/bin/env python3
"""
Dead-wiring detector: finds code that is DECLARED but never CONNECTED, a bug class
`method_gaps.py` structurally cannot see because it only matches vanilla method names
against Rust `fn` names -- it has no concept of "exists but nothing calls it".

Real examples that motivated this (all invisible to name-matching, because the names
and files existed and were even correctly named):

  * SSetBeacon fully defined, no dispatch arm -- fell through to `_ => warn!`.
  * LevelData had complete serde derives for spawn/difficulty/border -- nothing wrote
    or read them; every restart silently reset them.
  * Sign edit-lock (`try_claim_sign`) fully implemented, packet handler never called it.
  * EnderDragonEntity::hurt existed with zero callers; the real damage path bypassed it.
  * NodeEvaluator::set_can_open_doors existed and was never called.
  * AreaEffectCloud's `impl NBTStorage for X {}` was an empty no-op, so clouds never
    persisted.

Three detectors, each mechanical, no AI calls:

  1. dead_packets       -- serverbound `S*` packet structs in pumpkin-protocol with no
                            `::to_id(` or `::read(` call anywhere under crates/pumpkin/src/net/.
  2. dead_pub_fns        -- `pub fn` in crates/ whose name has zero call-shaped occurrences
                            (`\bname(`) anywhere outside its own declaration line.
  3. empty_trait_impls   -- `impl Trait for Type {}` with a fully empty body, where the
                            trait declares at least one method with a no-op default body
                            (so the empty impl silently opts out of real behaviour instead
                            of failing to compile).

WHAT THIS IS NOT
----------------
A finding is a LEAD. Known false-positive sources, from the calibration sample:

  * dead_packets: a packet handled via a differently-named alias (`use S as SFoo`) is
    resolved (aliases are tracked), but a NEW packet added with an exotic dispatch
    pattern this script does not recognise would still be a false "dead".
  * dead_pub_fns: pub API surface meant for plugins (crates/pumpkin/src/plugin/**),
    generated code, derive-driven trait methods (Display::fmt, Serialize::serialize,
    Drop::drop, operator overloads) called by the compiler rather than by name text,
    and binary/example entry points (`main`).
  * empty_trait_impls: many empty impls are a CORRECT "nothing extra to do" -- e.g. a
    boat with no fields beyond base Entity state legitimately has nothing to persist.
    The bug only exists when the type has state that vanilla persists and this impl
    drops it on the floor. This detector cannot tell those apart; every hit needs a
    read of the type's fields against vanilla.

MEASURED PRECISION (2026-08-06)
--------------------------------
20 findings were hand-verified by reading the declaring file (and, for NBTStorage
impls, the vanilla-equivalent state the struct carries) across two random samples
(seed 4903, before and after fixing three detector bugs the first sample exposed):
turbofish calls (`erase::<K>()`) and generic-parameter definitions
(`fn skip_content<'a, R>(`) were invisible to the original substring-count method;
`#[proc_macro_attribute]` fns in pumpkin-macros are invoked as `#[attr]`, never as a
call; path references passed as bare function values (`.map(Type::method)`, no
trailing paren at the reference site) were also missed. All four are fixed in the
code below; every dead_packets/dead_pub_fns/empty_trait_impls count in this docstring
is from the POST-fix tool.

Of the 20 verified findings (all correctly "nothing calls this" -- the mechanical
claim itself was right in every case after the fixes):

    REAL_GAP              7   confirmed behavioural divergence from vanilla
    NARROW_REAL_GAP        2   technically real, rarely observable in practice
    DEAD_CODE_NOT_A_BUG    8   genuinely unreferenced, but functionally inconsequential
                                (superseded by another code path, dev/test-only feature,
                                a cache that is always freshly scoped)
    FALSE_POSITIVE         3   empty impl is actually correct -- the type has no extra
                                state beyond its base, so there is nothing to lose

So strict gap-worthy precision is 7/20 = 35%, loose (counting the narrow cases)
9/20 = 45%. dead_packets found 0 candidates against the current tree (the SSetBeacon
class of bug is not currently present in the Java packet dispatch table) -- clean, not
unmeasured; re-run after any new packet type lands to catch a regression.

The three FALSE_POSITIVEs were all `impl NBTStorage for X {}` on entities whose
struct has no field beyond the base entity handle (e.g. `LlamaSpitEntity { thrown:
ThrownItemEntity }`) -- there being nothing there IS the correct persisted state.
The detector cannot see "does this type have state vanilla persists"; every
empty_trait_impl hit still needs a field-by-field read against vanilla.

Worked REAL_GAP examples, as a model of what a confirmed finding looks like:
  * `IllusionerEntity` (entity/mob/illusioner.rs) carries a `spellcaster:
    SpellcasterState` field (vanilla `SpellcasterIllager.spellCastingTickCount` /
    `currentSpell`) but `impl NBTStorage for IllusionerEntity {}` is empty -- the
    exact AreaEffectCloud bug shape, on a different type.
  * `Bee::set_stay_out_of_hive_countdown` (entity/passive/bee.rs:219) is never
    called anywhere, though the `stay_out_of_hive_countdown` field it writes IS read
    and decremented elsewhere -- the AI branch that should arm the "avoid the hive
    for a while" cooldown never fires.
  * `BlendingData::iterate_heights` / `iterate_biomes`
    (pumpkin-world/src/generation/blender/blending_data.rs) are dead while the
    sibling `iterate_densities` is wired -- terrain blending at old/new chunk
    borders applies to density but not height or biome.
  * `Player::fire_packet_sent` (entity/player.rs:2331) is never called -- the
    `PacketSentEvent` plugin hook can never fire for any packet.
  * `CopperGolemStatueBlockEntity::remove_statue` (block/entities/
    copper_golem_statue.rs:72) is dead and says so in its own doc comment: no
    `BlockBehaviour` is registered for `minecraft:copper_golem_statue` yet, so the
    axe-on-statue-to-revive-golem interaction has no trigger.
"""
import json
import random
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
HERE = Path(__file__).resolve().parent
CRATES_DIR = REPO_ROOT / "crates"
NET_DIR = "crates/pumpkin/src/net"
PROTOCOL_SERVER_DIR = "crates/pumpkin-protocol/src/java/server"

# ---------------------------------------------------------------------------
# shared helpers
# ---------------------------------------------------------------------------


def rg(pattern: str, paths: list[str], extra: list[str] | None = None) -> str:
    cmd = ["rg", "--no-heading", "-n", "-g", "*.rs"] + (extra or []) + [pattern] + paths
    out = subprocess.run(cmd, cwd=REPO_ROOT, capture_output=True, text=True, check=False)
    return out.stdout


def rg_count(pattern: str, paths: list[str]) -> int:
    out = subprocess.run(
        ["rg", "-c", "-g", "*.rs", pattern] + paths,
        cwd=REPO_ROOT, capture_output=True, text=True, check=False,
    )
    total = 0
    for line in out.stdout.splitlines():
        _, _, n = line.rpartition(":")
        if n.isdigit():
            total += int(n)
    return total


# ---------------------------------------------------------------------------
# 1. dead_packets
# ---------------------------------------------------------------------------


def _rg_match_text(rg_output: str) -> list[str]:
    """Strip the `path:line:` prefix rg -n adds, keeping the matched text intact
    even when that text itself contains colons (e.g. `Foo::to_id(`)."""
    out = []
    for line in rg_output.splitlines():
        parts = line.split(":", 2)
        if len(parts) == 3:
            out.append(parts[2])
    return out


def detect_dead_packets() -> list[dict]:
    defined = rg(r"pub struct (S\w+)", [PROTOCOL_SERVER_DIR], ["-o", "-r", "$1"])
    names = set()
    name_to_file: dict[str, str] = {}
    for line in defined.splitlines():
        parts = line.split(":", 2)
        if len(parts) != 3:
            continue
        path, _, name = parts
        names.add(name)
        name_to_file.setdefault(name, path)

    # A packet is "wired" if its name (or an import alias of it) is ever used in a
    # `::to_id(` dispatch check or a direct `::read(` call under net/. Both patterns
    # are used in this codebase: big `match packet.id` states use `::to_id(`, small
    # fixed-sequence states (handshake, status) call `::read(` directly.
    to_id_hits = {
        m[: -len("::to_id(")]
        for m in _rg_match_text(rg(r"S\w+::to_id\(", [NET_DIR], ["-o"]))
    }
    read_hits = {
        m[: -len("::read(")]
        for m in _rg_match_text(rg(r"S\w+::read\(", [NET_DIR], ["-o"]))
    }
    wired = to_id_hits | read_hits

    # `use path::SFoo as SBar;` aliases -- if SBar is wired, SFoo (the real struct) is too.
    for rest in _rg_match_text(rg(r"(S\w+) as (S\w+)", [NET_DIR], ["-o", "-r", "$1 as $2"])):
        if " as " not in rest:
            continue
        orig, _, alias = rest.partition(" as ")
        if alias in wired:
            wired.add(orig)

    dead = []
    for name in sorted(names):
        if name in wired:
            continue
        dead.append({
            "kind": "dead_packet",
            "packet": name,
            "defined_in": name_to_file[name],
            "note": "no ::to_id( or ::read( call under crates/pumpkin/src/net/",
        })
    return dead


# ---------------------------------------------------------------------------
# 2. dead_pub_fns
# ---------------------------------------------------------------------------

# fn names the compiler calls structurally (operators, derives, trait objects invoked
# through dynamic dispatch machinery, entry points) rather than by name text appearing
# anywhere. Flagging these is pure noise, confirmed during script development.
STRUCTURAL_FN_NAMES = {
    "main", "new", "default", "fmt", "from", "try_from", "into", "try_into",
    "eq", "ne", "partial_cmp", "cmp", "hash", "clone", "clone_from", "drop",
    "deref", "deref_mut", "index", "index_mut", "next", "poll", "poll_next",
    "serialize", "deserialize", "visit_map", "visit_seq", "visit_str", "borrow",
    "as_ref", "as_mut", "to_string", "to_owned", "deserialize_with", "serialize_with",
    "add", "sub", "mul", "div", "neg", "not", "bitand", "bitor", "bitxor",
    "call", "call_mut", "call_once", "iterator", "into_iter", "len", "is_empty",
}

# Paths whose pub fns are deliberately public API surface (plugin ABI, generated
# bindings, re-exported helpers) rather than internal wiring -- a "zero internal
# callers" reading there is expected, not a bug.
PUBLIC_SURFACE_PREFIXES = (
    "crates/pumpkin/src/plugin/",
    "crates/pumpkin-api-macros/",
    "crates/pumpkin-plugin-api/",
    "crates/pumpkin-data/src/generated/",
    # proc-macro crates: their pub fns are invoked via `#[attr]` / `#[derive(Name)]`
    # syntax, never call-shaped text, so "zero call sites" is the expected reading.
    "crates/pumpkin-macros/",
)


def in_test_context(lines: list[str], idx: int) -> bool:
    # Cheap heuristic: walk back up to 15 lines looking for a #[cfg(test)] or
    # #[test] attribute closer than any blank-then-`fn`/`impl`/`mod` boundary.
    for back in range(1, 16):
        j = idx - back
        if j < 0:
            break
        line = lines[j].strip()
        if line.startswith("#[cfg(test)]") or line.startswith("#[test]"):
            return True
        if line.startswith(("fn ", "pub fn ", "impl ", "mod ", "pub mod ")) and back > 1:
            break
    return False


def detect_dead_pub_fns(sample_cap: int = 4000) -> list[dict]:
    rust_files = sorted(CRATES_DIR.rglob("*.rs"))
    decls: dict[str, list[tuple[str, int]]] = defaultdict(list)
    fn_re = re.compile(r"^\s*pub(?:\([^)]*\))?\s+(?:async\s+)?fn\s+(\w+)\s*[<(]")

    for f in rust_files:
        rel = str(f.relative_to(REPO_ROOT))
        if "/tests/" in rel or rel.endswith("_test.rs") or "/examples/" in rel:
            continue
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        lines = text.splitlines()
        for i, line in enumerate(lines):
            m = fn_re.match(line)
            if not m:
                continue
            name = m.group(1)
            if name in STRUCTURAL_FN_NAMES or len(name) <= 3:
                continue
            if name.startswith("test_") or name.startswith("_"):
                continue
            if in_test_context(lines, i):
                continue
            decls[name].append((rel, i + 1))

    # Build a (name -> set of (file, line)) index of every call-SHAPED occurrence,
    # covering both plain calls (`name(`) and turbofish calls (`name::<T>(`). A naive
    # substring/text-frequency comparison against declaration count breaks silently
    # on generics: `fn skip_content<'a, R>(` does not itself match `name(`, and
    # `erase::<WalkTargetMemory>()` does not match `name(` either -- both were
    # confirmed as false "dead" during calibration before this fix. Matching exact
    # (file, line) sites and excluding only the known declaration sites is immune to
    # both: any occurrence anywhere else, in any bracket style, counts as a call.
    call_re = re.compile(r"\b([A-Za-z_]\w*)\s*(?:::<[^;{}]*?>)?\s*\(")
    # A function can also be referenced without call parens at all -- passed as a
    # value to `.map(Type::method)`, `.and_then(...)`, stored in a table, etc. Miss
    # this and a wired callback reads as dead (confirmed on CreakingHeartBlockEntity
    # ::get_analog_output_signal, referenced via `.map(...::get_analog_output_signal)`
    # with no trailing paren at the reference site).
    path_ref_re = re.compile(r"::\s*([A-Za-z_]\w*)\b")
    occurrences: dict[str, set[tuple[str, int]]] = defaultdict(set)
    candidate_names = set(decls)
    for f in rust_files:
        rel = str(f.relative_to(REPO_ROOT))
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for m in call_re.finditer(text):
            name = m.group(1)
            if name not in candidate_names:
                continue
            line_no = text.count("\n", 0, m.start()) + 1
            occurrences[name].add((rel, line_no))
        for m in path_ref_re.finditer(text):
            name = m.group(1)
            if name not in candidate_names:
                continue
            line_no = text.count("\n", 0, m.start()) + 1
            occurrences[name].add((rel, line_no))

    dead = []
    for name, sites in decls.items():
        decl_sites = set(sites)
        live_sites = occurrences.get(name, set()) - decl_sites
        if not live_sites:
            for rel, line in sites:
                if rel.startswith(PUBLIC_SURFACE_PREFIXES):
                    continue
                dead.append({
                    "kind": "dead_pub_fn",
                    "fn": name,
                    "file": rel,
                    "line": line,
                    "note": "no call-shaped occurrence of this name anywhere outside "
                             "its own declaration line(s)",
                })
    dead.sort(key=lambda d: (d["file"], d["line"]))
    return dead[:sample_cap]


# ---------------------------------------------------------------------------
# 3. empty_trait_impls
# ---------------------------------------------------------------------------

EMPTY_IMPL_RE = re.compile(
    r"impl(?:<[^>]*>)?\s+([A-Za-z_]\w*)(?:<[^>]*>)?\s+for\s+([A-Za-z_]\w*)(?:<[^>]*>)?\s*\{\s*\}",
    re.MULTILINE,
)

# Auto/marker traits: an empty impl is the ENTIRE point, never a bug.
MARKER_TRAITS = {
    "Eq", "Send", "Sync", "Copy", "Unpin", "Sized", "StructuralEq",
    "StructuralPartialEq", "RefUnwindSafe", "UnwindSafe",
}


def trait_has_noop_default_method(trait_name: str, trait_defs: dict[str, str]) -> bool:
    """True if the trait is declared in this workspace and has >=1 method with a
    default body that is empty / trivially `{}` / `Box::pin(async {})` -- i.e. an
    empty impl silently no-ops instead of failing to compile."""
    body = trait_defs.get(trait_name)
    if not body:
        return False
    # crude default-method body scan: `fn name(...) { ... }` inside the trait block
    for m in re.finditer(r"fn\s+\w+[^;{}]*\{", body):
        start = m.end()
        depth = 1
        j = start
        while j < len(body) and depth:
            if body[j] == "{":
                depth += 1
            elif body[j] == "}":
                depth -= 1
            j += 1
        inner = body[start:j - 1].strip()
        if inner in ("", "()", "Ok(())") or "Box::pin(async {})" in inner or inner == "async {}":
            return True
        if re.fullmatch(r"Box::pin\(async\s*(move)?\s*\{\s*\}\)", inner):
            return True
    return False


def build_trait_defs() -> dict[str, str]:
    out = {}
    for f in CRATES_DIR.rglob("*.rs"):
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for m in re.finditer(r"pub trait (\w+)[^{]*\{", text):
            name = m.group(1)
            start = m.end()
            depth = 1
            j = start
            while j < len(text) and depth:
                if text[j] == "{":
                    depth += 1
                elif text[j] == "}":
                    depth -= 1
                j += 1
            out[name] = text[start:j - 1]
    return out


def detect_empty_trait_impls() -> list[dict]:
    trait_defs = build_trait_defs()
    findings = []
    for f in CRATES_DIR.rglob("*.rs"):
        rel = str(f.relative_to(REPO_ROOT))
        if "/tests/" in rel:
            continue
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        # Skip anything inside a `#[cfg(test)] mod ... { ... }` block -- test doubles
        # (e.g. a `RemoveMeGoal` fixture with `impl Goal for RemoveMeGoal {}`) are not
        # gameplay code and were confirmed as a false-positive source during calibration.
        test_mod_spans: list[tuple[int, int]] = []
        for tm in re.finditer(r"#\[cfg\(test\)\]\s*mod\s+\w+\s*\{", text):
            start = tm.end()
            depth = 1
            j = start
            while j < len(text) and depth:
                if text[j] == "{":
                    depth += 1
                elif text[j] == "}":
                    depth -= 1
                j += 1
            test_mod_spans.append((tm.start(), j))

        for m in EMPTY_IMPL_RE.finditer(text):
            trait_name, type_name = m.group(1), m.group(2)
            if trait_name in MARKER_TRAITS:
                continue
            if any(s <= m.start() < e for s, e in test_mod_spans):
                continue
            if not trait_has_noop_default_method(trait_name, trait_defs):
                continue
            line = text.count("\n", 0, m.start()) + 1
            findings.append({
                "kind": "empty_trait_impl",
                "trait": trait_name,
                "type": type_name,
                "file": rel,
                "line": line,
                "note": f"impl {trait_name} for {type_name} {{}} -- trait has a "
                         "no-op default method, so this silently opts out",
            })
    findings.sort(key=lambda d: (d["trait"], d["file"]))
    return findings


# ---------------------------------------------------------------------------
# calibration (filled in by hand-verification -- see conformance/dead_wiring_triage.md
# process notes in the PR/report, this constant is the durable record)
# ---------------------------------------------------------------------------

CALIBRATION = {
    "measured": "2026-08-06",
    "sample_size": 20,
    "seed": 4903,
    "real_gap": 7,
    "narrow_real_gap": 2,
    "dead_code_not_a_bug": 8,
    "false_positive": 3,
    "strict_precision_pct": 35.0,
    "loose_precision_pct": 45.0,
    "note": "see module docstring MEASURED PRECISION section for the worked examples "
            "and the detector bugs this sample found and fixed (turbofish calls, "
            "generic fn defs, proc-macro attrs, bare fn-pointer references).",
}


def main() -> int:
    dead_packets = detect_dead_packets()
    dead_fns = detect_dead_pub_fns()
    empty_impls = detect_empty_trait_impls()

    report = {
        "generated_by": "conformance/dead_wiring.py",
        "calibration": CALIBRATION,
        "counts": {
            "dead_packets": len(dead_packets),
            "dead_pub_fns": len(dead_fns),
            "empty_trait_impls": len(empty_impls),
        },
        "dead_packets": dead_packets,
        "dead_pub_fns": dead_fns,
        "empty_trait_impls": empty_impls,
    }
    out_path = HERE / "dead_wiring.json"
    out_path.write_text(json.dumps(report, indent=1))

    print(f"dead_packets: {len(dead_packets)}")
    for d in dead_packets:
        print(f"  {d['packet']:<28} {d['defined_in']}")
    print(f"\ndead_pub_fns: {len(dead_fns)} (showing first 40)")
    for d in dead_fns[:40]:
        print(f"  {d['fn']:<32} {d['file']}:{d['line']}")
    print(f"\nempty_trait_impls: {len(empty_impls)} (showing first 40)")
    for d in empty_impls[:40]:
        print(f"  {d['trait']:<20} for {d['type']:<28} {d['file']}:{d['line']}")

    if "--calibrate" in sys.argv:
        all_findings = dead_packets + dead_fns + empty_impls
        random.seed(4903)
        sample = random.sample(all_findings, min(15, len(all_findings)))
        print(f"\n=== calibration sample (seed 4903, n={len(sample)}) ===")
        for d in sample:
            print(json.dumps(d))

    print(f"\n-> {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
