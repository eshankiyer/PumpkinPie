#!/usr/bin/env python3
"""Method-level conformance: how much of each vanilla class this server actually declares.

Registry coverage (`tools/tracker/build_surface.py`) answers "does an implementation
exist at all". It cannot see that Efficiency, Piercing and Bane of Arthropods were
registered but did nothing. This instrument is the middle ground: for every vanilla class
Pumpkin claims an analogue of, how many of that class's methods have a plausibly-named
Rust counterpart.

Three stages, deliberately separate:

    enumerate   vanilla classes + methods from the 26.2 decompile     (the denominator)
    map         each vanilla class -> a Rust file in crates/          (which classes count)
    classify    each method -> in the mapped file / elsewhere / absent (the numerator)

    python3 tools/conformance/conformance.py                 # run + probes + summary
    python3 tools/conformance/conformance.py --sample 15 --seed 815 # draw leads to verify

Two headline numbers, both printed:

  strict  the vanilla method has a counterpart in the ONE Rust file mapped to its class
  loose   it has one anywhere in crates/. Generous: this codebase puts behaviour in trait
          default bodies, so a block that overrides nothing still matches every default.

The truth is between them. Strict under-credits split modules and trait impls in other
files; loose over-credits defaults.

WHAT THE NUMBER IS, AND IS NOT
------------------------------
This is a NAME-level measurement. A method counted present has a Rust fn whose name maps
to it; nobody checked that the body agrees with vanilla. A method counted absent is a
LEAD, not a proven defect. Known false-positive sources, in order of measured weight:

  * Pumpkin renamed it (Yarn naming, not Mojang). Mitigated by METHOD_ALIASES.
  * The behaviour is inlined into its caller rather than being its own fn.
  * The method is client/render-only and unreachable on a dedicated server. Mitigated by
    CLIENT_ONLY_METHODS.
  * The behaviour is expressed as generated data in pumpkin-data, not as a fn.
  * The method belongs to a nested Builder/codec class; enumerate() attributes nested-class
    methods to the outer class (LootTable.setParamSet is really LootTable.Builder's).

MEASURED PRECISION (2026-08-21, AFTER the many-to-many mapper fix)
-----------------------------------------------------------------------------------------
15 leads drawn with `--sample 15 --seed 2108` and verified by reading BOTH sides. Full
verdicts with evidence in sample_verdicts_2108.json:

    REAL_GAP        5   StructureTemplate.placeInWorld, AbstractHorse.positionRider,
                        Drowned.canReplaceCurrentItem, ShelfBlockEntity.applyImplicitComponents,
                        Piglin.playStepSound
    RENAME          2   Zoglin.doHurtTarget, BowItem.use
    INLINED         4   the behaviour exists but as a field or at the call site
    DATA_MODELED    3   generated tables / codec-builder plumbing
    CLIENT_ONLY     1   Display.getTeamColor

So about ONE IN THREE flagged leads is a real gap (5/15 = 33%; with n=15 the interval is
roughly 12-62%). Scale the lead count by ~0.33 for an expected backlog, never read it as a
defect count. Earlier samples of earlier runs scored 2/18 = 11% and 4/15 = 27%; the three
come from different instrument revisions, so treat the trend as directional only.

Re-measure with --sample after changing the tables rather than quoting a stale figure.

BLIND SPOTS (state these whenever the number is quoted)
------------------------------------------------------
1. Bodiless declarations are invisible. METHOD_RE requires `{`, so `public abstract
   boolean canUse();` and every interface method without a default is missing from the
   denominator. A probe pins this so widening the regex cannot pass unnoticed.
2. Behaviour is never compared, only names. Efficiency was registered and did nothing;
   this instrument would have called it present.
3. The loose tier credits trait defaults. `normal_use` is declared once on the Block
   trait, so every block class matches `useWithoutItem` whether or not it overrides it.
4. FIXED 2026-08-21: a class may now map to SEVERAL Rust files and several classes may
   share one file. The new risk runs the other way - `stems` can attach a same-named but
   unrelated file (Slime picks up block/blocks/slime.rs beside entity/mob/slime.rs), which
   over-credits the strict tier slightly. Bounded by MAX_STEM_FILES.
5. 30 covered classes still resolve to no readable Rust file and are dropped entirely (126
   before the fix; the list is emitted as `unresolved_classes`). Some mappings are still
   simply wrong - Attribute lands on a bedrock update_attributes packet file. A wrong
   mapping moves methods between the two present tiers; it does not create absences.
   Steel drops 135 classes this way, so its denominator is shrunk far harder than
   PumpkinPie's and the two loose percentages are NOT comparable head to head.
6. Client-only and data-modeled exclusions are curated lists, not analysis. They are
   certainly incomplete, which makes the number pessimistic rather than optimistic.
7. Denominator scope is the 17 gameplay packages in SUBSYSTEMS. util/datafix is excluded
   on purpose; anything outside those packages is not measured at all.

SELF-VALIDATION
---------------
Probes run on every invocation and abort the run on failure (--no-probes to skip, which
you should not). They assert known-true and known-false cases at each stage, because
every instrument in this campaign that was trusted without probes has been wrong at least
once: one reported 220.9% coverage, another counted `acacia_boat` as a block item, a
third silently indexed 0 Rust structs after crates/ moved and scored everything 0%.
"""

import argparse
import fnmatch
import json
import os
import pathlib
import random
import re
import subprocess
import sys
from collections import defaultdict

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from aliases import (  # noqa: E402
    CLASS_FILE_BLOCKLIST,
    CLASS_FILE_HINTS,
    CLASS_METHOD_ALIASES,
    CLIENT_ONLY_METHODS,
    DATA_MODELED_METHODS,
    KNOWN_ALIASES,
    KNOWN_COMMAND_ALIASES,
    METHOD_ALIASES,
    NOISE_CLASSES,
    STRUCTURAL_EXCLUSIONS,
    SUFFIX_REWRITES,
)

HERE = pathlib.Path(__file__).resolve().parent
REPO_ROOT = pathlib.Path(os.environ.get("PUMPKIN_REPO", HERE.parents[1]))
DECOMP_ROOT = pathlib.Path(
    os.environ.get("PUMPKIN_DECOMP", pathlib.Path.home() / "pumpkin-vanilla-26.2/decompiled")
)
VANILLA_ROOT = DECOMP_ROOT / "net/minecraft"
MILESTONE = "26.2"

# Packages that describe server-side gameplay. `util/datafix` (396 classes of legacy
# save-format upgraders back to 1.0) is excluded on purpose: Pumpkin targets 26.2 saves
# only, so counting them would inflate the denominator with code nobody intends to port.
SUBSYSTEMS = {
    "world/level/block": "block",
    "world/entity": "entity",
    "world/item": "item",
    "world/inventory": "inventory",
    "world/level/material": "material",
    "world/food": "food",
    "world/level/border": "border",
    "world/level/gameevent": "gameevent",
    "world/level/levelgen": "worldgen",
    "world/level/lighting": "lighting",
    "world/level/chunk": "chunk",
    "network/protocol": "protocol",
    # net/minecraft/commands is Brigadier plumbing; the actual /give, /tp, /fill
    # implementations live in server/commands, which is what Pumpkin mirrors.
    "commands": "commands",
    "server/commands": "commands",
    "world/level/storage": "storage",
    "world/level/saveddata": "saveddata",
    "server/level": "server_level",
}

CLASS_RE = re.compile(
    r"^\s*(?:public|protected)\s+(?:abstract\s+|final\s+|static\s+)*"
    r"(?:class|interface|enum|record)\s+(\w+)",
    re.MULTILINE,
)
METHOD_RE = re.compile(
    r"^\s*(?:public|protected)\s+"
    r"(?:static\s+|final\s+|abstract\s+|synchronized\s+|default\s+)*"
    r"(?:<[^>]+>\s+)?"
    r"[\w\[\]<>,.? ]+?\s+"
    r"(\w+)\s*\([^;{]*\)\s*"
    r"(?:throws\s+[\w.,\s]+)?\s*\{",
    re.MULTILINE,
)
KEYWORD_FALSE_POSITIVES = {"if", "for", "while", "switch", "catch", "synchronized", "return"}

SUFFIX_VARIANTS = ["", "Block", "Item", "Entity", "Impl"]
GETTER_RE = re.compile(r"^get([A-Z]\w*)$")
PACKET_RE = re.compile(r"^(Clientbound|Serverbound)(\w+?)(Packet)?$")
COMMAND_RE = re.compile(r"^(\w+?)(Commands?)$")


class ProbeFailure(Exception):
    pass


def probe(label: str, condition: bool, detail: str = "") -> None:
    if not condition:
        raise ProbeFailure(f"{label}{': ' + detail if detail else ''}")


# ---------------------------------------------------------------------------
# stage 1: enumerate
# ---------------------------------------------------------------------------


def _sanitize(text: str) -> str:
    """Blank out comments and literals, preserving offsets, so brace counting is sound.

    Without this a `"}"` inside a string or a brace in a javadoc snippet closes a class
    body early and every method after it is attributed to the wrong class.
    """
    out = list(text)
    n = len(text)

    def blank(a: int, b: int) -> None:
        for k in range(a, min(b, n)):
            if out[k] != "\n":
                out[k] = " "

    i = 0
    while i < n:
        c = text[i]
        if c == "/" and text[i + 1 : i + 2] == "/":
            j = text.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif c == "/" and text[i + 1 : i + 2] == "*":
            j = text.find("*/", i + 2)
            j = n if j < 0 else j + 2
            blank(i, j)
            i = j
        elif c == '"':
            if text[i : i + 3] == '"""':
                j = text.find('"""', i + 3)
                j = n if j < 0 else j + 3
            else:
                j = i + 1
                while j < n:
                    if text[j] == "\\":
                        j += 2
                        continue
                    if text[j] == '"':
                        j += 1
                        break
                    if text[j] == "\n":
                        break
                    j += 1
            blank(i, j)
            i = j
        elif c == "'":
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == "'":
                    j += 1
                    break
                j += 1
            blank(i, j)
            i = j
        else:
            i += 1
    return "".join(out)


TYPE_DECL_RE = re.compile(
    r"(?:^|[\s;{}])"
    r"((?:(?:public|protected|private|static|final|abstract|sealed|non-sealed|strictfp)\s+)*)"
    r"(class|interface|enum|record)\s+(\w+)"
)


def types_in_file(text: str) -> tuple[str, list[dict]]:
    """Every named type declared in one .java file, with the span of its body.

    Returns the sanitized text alongside, because method offsets are taken from it.
    Anonymous classes are deliberately NOT pushed: a method inside `new Runnable() {...}`
    belongs, for our purposes, to the named class that encloses it.
    """
    s = _sanitize(text)
    decls = {}
    for m in TYPE_DECL_RE.finditer(s):
        mods = m.group(1)
        decls[m.start(2)] = (m.group(3), "public" in mods or "protected" in mods)

    found: list[dict] = []
    stack: list[list] = []
    pending = None
    depth = 0
    for i, c in enumerate(s):
        if pending is None and i in decls:
            pending = decls[i]
        if c == "{":
            depth += 1
            if pending is not None:
                name, is_pub = pending
                qual = f"{stack[-1][0]}.{name}" if stack else name
                stack.append([qual, name, is_pub, i + 1, depth])
                pending = None
        elif c == "}":
            if stack and stack[-1][4] == depth:
                q, name, is_pub, start, _ = stack.pop()
                found.append(
                    {"qualified": q, "simple": name, "public": is_pub, "start": start, "end": i}
                )
            depth -= 1
        elif c == ";" and pending is not None:
            pending = None
    for q, name, is_pub, start, _ in stack:
        found.append(
            {"qualified": q, "simple": name, "public": is_pub, "start": start, "end": len(s)}
        )
    return s, found


def enumerate_vanilla() -> list[dict]:
    """One unit per NAMED vanilla type, nested types included as `Outer.Inner`.

    NESTED-CLASS ATTRIBUTION (fixed 2026-08-21). This used to take the first class
    declaration in the file as the name and every METHOD_RE hit anywhere in the file as
    that class's methods. Nested builders therefore hung on their outer class: 41 of
    `Item`'s 84 methods were really `Item.Properties`'s (`axe`, `durability`, `food`,
    `fireResistant`), and every one of them was flagged as a missing `Item` method.

    Nested types are emitted as their own units rather than merely dropped, because they
    ARE separate vanilla classes with their own API; dropping them would shrink the
    denominator silently and hide real gaps (`TreeConfiguration`, `OreConfiguration`).
    Only public/protected types are emitted - a package-private helper class in the same
    file is not part of the outer class's surface - but their methods are still attributed
    to them, so they no longer inflate the outer class.
    """
    if not VANILLA_ROOT.exists():
        raise SystemExit(
            f"decompile missing at {VANILLA_ROOT}\nrun tools/decompile-vanilla.sh, or set PUMPKIN_DECOMP"
        )
    units = []
    for subdir, subsystem in SUBSYSTEMS.items():
        pkg = VANILLA_ROOT / subdir
        if not pkg.exists():
            continue
        for java in sorted(pkg.rglob("*.java")):
            raw = java.read_text(encoding="utf-8", errors="replace")
            text, types = types_in_file(raw)
            if not types:
                continue
            # innermost-first, so the smallest containing body wins
            by_span = sorted(types, key=lambda t: t["end"] - t["start"])
            owned: dict[str, set[str]] = {t["qualified"]: set() for t in types}
            for m in METHOD_RE.finditer(text):
                name = m.group(1)
                if name in KEYWORD_FALSE_POSITIVES:
                    continue
                for t in by_span:
                    if t["start"] <= m.start() < t["end"]:
                        owned[t["qualified"]].add(name)
                        break

            emit = [t for t in types if t["public"]]
            if not emit:
                # no public type in the file (package-private helper): keep the outermost
                # so the file is still represented, matching the old fallback.
                emit = [max(types, key=lambda t: t["end"] - t["start"])]
            for t in emit:
                simple = t["simple"]
                if simple in NOISE_CLASSES or t["qualified"] in NOISE_CLASSES:
                    continue
                methods = {m for m in owned[t["qualified"]] if m != simple}
                methods -= STRUCTURAL_EXCLUSIONS
                units.append(
                    {
                        "subsystem": subsystem,
                        "vanilla_class": t["qualified"],
                        "simple_name": simple,
                        "nested": "." in t["qualified"],
                        "vanilla_path": str(java.relative_to(DECOMP_ROOT)),
                        "methods": sorted(methods),
                    }
                )
    return units


# ---------------------------------------------------------------------------
# stage 2: map vanilla class -> Rust
# ---------------------------------------------------------------------------


def camel_to_snake(name: str) -> str:
    s = re.sub(r"(.)([A-Z][a-z]+)", r"\1_\2", name)
    s = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", s)
    return s.lower()


def strip_suffix(name: str, suffix: str) -> str:
    if suffix and name.endswith(suffix) and len(name) > len(suffix):
        return name[: -len(suffix)]
    return name


def packet_names(class_name: str):
    """Vanilla Clientbound<X>Packet is Pumpkin's C<X>; Serverbound is S<X>.

    Without this the protocol subsystem scores under 1%, which is naming, not coverage.
    """
    m = PACKET_RE.match(class_name)
    if not m:
        return
    yield ("C" if m.group(1) == "Clientbound" else "S") + m.group(2)
    yield m.group(2)


def command_flatten(class_name: str):
    """BanIpCommands -> 'banip'. Only ever matched against the command-file index.

    Feeding a flattened one-word body into the generic struct index makes ItemCommands
    match an unrelated generated `struct Item` - a real collision, confirmed 2026-08-06.
    """
    m = COMMAND_RE.match(class_name)
    return m.group(1).lower() if m else None


def source_roots() -> list[str]:
    """Top-level directories holding Rust crates, for whichever repo is being measured.

    Hardcoding `crates/` scored SteelMC at 0%: it lays its crates out as `steel-core/`,
    `steel-registry/` and so on. A workspace member is any directory with a Cargo.toml, plus
    the children of a `crates/`-style container.
    """
    override = os.environ.get("CONFORMANCE_SRC_ROOTS")
    if override:
        return [r for r in override.split(",") if (REPO_ROOT / r).is_dir()]
    roots: list[str] = []
    for entry in sorted(REPO_ROOT.iterdir()):
        if not entry.is_dir() or entry.name.startswith("."):
            continue
        if entry.name in {"target", "assets", "docs", "tools"}:
            continue
        children = [c for c in entry.iterdir() if c.is_dir() and (c / "Cargo.toml").is_file()]
        if children:
            roots += [str(c.relative_to(REPO_ROOT)) for c in sorted(children)]
        elif (entry / "Cargo.toml").is_file():
            roots.append(entry.name)
    return roots


SRC_ROOTS = source_roots()
# Some probes assert facts about THIS fork's layout; measuring another repo must not fail
# on them, but every repo-agnostic probe still runs, so no run ships unprobed.
IS_PUMPKIN = (REPO_ROOT / "crates" / "pumpkin").is_dir()
# The crate that holds most gameplay behaviour, used only to break ties between two files
# declaring the same name. Pumpkin puts it in crates/pumpkin, Steel in steel-core.
MAIN_CRATE = next(
    (r for r in SRC_ROOTS if r.endswith(("/pumpkin", "-core", "/core"))),
    SRC_ROOTS[0] if SRC_ROOTS else "",
)


def rg(pattern: str, replacement: str, with_filename: bool) -> list[str]:
    crates = SRC_ROOTS
    args = ["rg", "-g", "*.rs", "-o", pattern, "-r", replacement]
    args += ["--no-heading", "--with-filename"] if with_filename else ["--no-filename"]
    out = subprocess.run(args + crates, cwd=REPO_ROOT, capture_output=True, text=True, check=False)
    return out.stdout.splitlines()


def _decl_score(name: str, path: str) -> tuple:
    """Rank candidate files declaring `name`; higher is better.

    rg's match order is nondeterministic, so taking the first hit picked the wrong file
    whenever a name is declared twice: vanilla `Player` resolved to pumpkin-protocol's
    PlayerInfoUpdate struct instead of the entity, making all 201 methods look absent.
    """
    return (
        pathlib.Path(path).stem == camel_to_snake(name),
        # The decompile is the JAVA edition. A `pub struct Attribute` in a bedrock packet
        # module is a different protocol's payload, not this class - it was winning the
        # Attribute and LevelChunk mappings outright.
        "/bedrock/" not in path,
        bool(MAIN_CRATE) and path.startswith(f"{MAIN_CRATE}/"),
        "/generated/" not in path,
        -len(path),
        path,
    )


def build_indexes() -> dict:
    decls: dict[str, list[str]] = defaultdict(list)
    for line in rg(r"^\s*pub (?:struct|enum|trait) (\w+)", "$1", True):
        path, _, name = line.partition(":")
        decls[name].append(path)
    best = {n: max(p, key=lambda x: _decl_score(n, x)) for n, p in decls.items()}

    # const/static catch vanilla's data-table classes (Foods, ArmorMaterials, MapColor)
    # that Pumpkin models as generated data rather than as a type.
    structs = set(best) | set(rg(r"^\s*pub (?:const|static) (\w+)", "$1", False))

    # One stem can name several files (entity/passive/pig.rs and a plugin-api pig_zap.rs
    # sibling, pumpkin-data/item.rs and pumpkin/item/mod.rs). Keeping only the first, as
    # this did, silently preferred whichever crate sorted first - usually the generated
    # data table over the behaviour. Keep them all, main crate first.
    stems: dict[str, list[str]] = defaultdict(list)
    all_rs = [q for root in SRC_ROOTS for q in (REPO_ROOT / root).rglob("*.rs")]
    for path in sorted(all_rs):
        rel = str(path.relative_to(REPO_ROOT))
        stem = path.stem if path.stem != "mod" else path.parent.name
        stems[stem].append(rel)
    for stem, paths in stems.items():
        paths.sort(key=lambda x: ("/bedrock/" in x,
                                  not (MAIN_CRATE and x.startswith(f"{MAIN_CRATE}/")),
                                  "/generated/" in x, len(x), x))

    cmd_dirs = [d for d in (REPO_ROOT / r for r in SRC_ROOTS)
                if (d / "src/command/commands").is_dir()]
    commands = {
        p.stem: str(p.relative_to(REPO_ROOT))
        for d in cmd_dirs
        for p in sorted((d / "src/command/commands").glob("*.rs"))
        if p.stem != "mod"
    }

    fns = set(rg(r"\bfn\s+(\w+)", "$1", False))

    return {
        "decls": best,
        "structs": structs,
        "structs_ci": {s.lower(): s for s in structs},
        "stems": stems,
        "commands": commands,
        "fns": fns,
    }


# Nested class names that describe a STRUCTURAL role rather than a domain concept. A bare
# match on these is a coin flip (`Builder` occurs 52 times in the enumerated set alone), so
# a nested class named one of these is matched only as Outer+Inner.
GENERIC_NESTED_NAMES = {
    "Action", "Builder", "Cache", "Config", "Context", "Data", "Entry", "Factory", "Flowing",
    "Handler", "Holder", "Impl", "Info", "Instance", "Key", "Layer", "Listener", "Mode",
    "Mutable", "Node", "Operation", "Options", "Output", "Packed", "Pair", "Properties",
    "Provider", "Result", "Rule", "Serializer", "Settings", "Source", "State", "Status",
    "Template", "Type", "Types", "Value", "Variant", "Visitor", "Block", "Item", "Entity",
    "Pos", "Set", "List", "Map",
}


def nested_candidates(class_name: str):
    """`Item.Properties` -> ItemProperties, then Properties unless the inner name is generic.

    Concatenation first because a Rust port that models a nested Java class nearly always
    names it after both halves (TreeConfiguration, GeodeCrackSettings).
    """
    outer, _, inner = class_name.rpartition(".")
    outer = outer.replace(".", "")
    yield outer + inner
    # A ONE-WORD inner name is not distinctive enough to identify a class by itself.
    # Measured 2026-08-21 over the 65 nested classes that matched on the bare inner name:
    # multi-word names (ShipwreckPiece, GhastMoveControl, FoxPounceGoal) were right almost
    # every time, while one-word names were wrong about half the time - Column.Range hit a
    # spline Range, SetNameFunction.Target hit a pathfinder Target, VaultBlockEntity.Server
    # hit the game server. Requiring an internal camel boundary drops both groups' worth of
    # noise at the cost of a handful of correct one-word hits (ServerStatus.Players).
    if inner not in GENERIC_NESTED_NAMES and re.search(r"[a-z][A-Z]", inner):
        yield inner


def candidate_names(class_name: str):
    seen = set()
    if "." in class_name:
        for name in nested_candidates(class_name):
            if name not in seen:
                seen.add(name)
                yield name
        return
    for suffix in SUFFIX_VARIANTS:
        for name in (class_name, strip_suffix(class_name, suffix)):
            if name not in seen:
                seen.add(name)
                yield name
    for vanilla_suffix, rust_suffix in SUFFIX_REWRITES:
        if class_name.endswith(vanilla_suffix):
            name = class_name[: -len(vanilla_suffix)] + rust_suffix
            if name not in seen:
                seen.add(name)
                yield name
    for name in packet_names(class_name):
        if name not in seen:
            seen.add(name)
            yield name


# A stem shared by more than this many files is ambiguous rather than informative
# (`mod`-style names), so it is not used as evidence.
MAX_STEM_FILES = 3


def hint_files(class_name: str) -> list[str]:
    """Hand-verified extra files for a class. Missing paths drop out silently."""
    out: list[str] = []
    for pattern, paths in CLASS_FILE_HINTS.items():
        if pattern == class_name or ("*" in pattern and fnmatch.fnmatchcase(class_name, pattern)):
            for path in paths:
                if path not in out and (REPO_ROOT / path).is_file():
                    out.append(path)
    return out


# Path fragments that a Rust file implementing a class of this subsystem should contain.
# Used only to BREAK TIES: when several same-named files exist and at least one sits in the
# right area, the ones sitting in a demonstrably different area are dropped. Vanilla Slime
# (world/entity) matched both entity/mob/slime.rs and block/blocks/slime.rs, and the block
# file's fns were credited to the mob. Subsystems whose layout has no single obvious
# fragment are deliberately absent, so they keep every candidate.
SUBSYSTEM_PATH_HINTS = {
    "entity": ("/entity/",),
    "block": ("/block/", "/blocks/"),
    "item": ("/item/", "/items/"),
    "inventory": ("/inventory/", "/screen"),
    "protocol": ("/protocol/",),
    "worldgen": ("/generation/",),
    "chunk": ("/chunk/",),
    "commands": ("/command",),
}


def _prefer_subsystem(files: list[str], subsystem: str | None) -> list[str]:
    frags = SUBSYSTEM_PATH_HINTS.get(subsystem or "")
    if not frags or len(files) < 2:
        return files
    right = [f for f in files if any(g in f for g in frags)]
    if not right:
        return files
    other = {g for s, gs in SUBSYSTEM_PATH_HINTS.items() if s != subsystem for g in gs}
    return [f for f in files if f in right or not any(g in f for g in other)]


def _finish(files: list[str], class_name: str, subsystem: str | None) -> list[str]:
    blocked = CLASS_FILE_BLOCKLIST.get(class_name, ())
    files = [f for f in files if f not in blocked]
    # The decompile is the Java edition throughout, so a bedrock-protocol file is never the
    # best home for a vanilla class when any other candidate exists.
    if any("/bedrock/" not in f for f in files):
        files = [f for f in files if "/bedrock/" not in f]
    return _prefer_subsystem(files, subsystem)


def map_class(class_name: str, idx: dict, subsystem: str | None = None) -> tuple[str, str | None, list[str]]:
    """-> (match kind, matched Rust name, every Rust file believed to implement it).

    MANY-TO-MANY (2026-08-21). This used to return one file and stop at the first stage
    that produced a NAME, which caused two measured defects:

      * a stage that matched a name but resolved no file killed the chain. 112 of the 126
        "unresolved" classes were `struct_match_ci` landing on a generated `pub const PIG`
        while `pig` sat in the stem index pointing at entity/passive/pig.rs. Every stage
        now contributes, and only the KIND is taken from the first stage that fired.
      * one class could not span two files, so vanilla Item (a behaviour base class split
        here across the data table, the ItemBehaviour trait and ItemStack) scored all 72
        of its behavioural methods absent by construction.

    Nothing here forbids two classes sharing a file - that direction always worked, it
    just had no way to reach a file like copper_weathering.rs that declares no type.
    """
    files: list[str] = []
    kind: str | None = None
    matched: str | None = None

    def add(*paths) -> bool:
        hit = False
        for path in paths:
            if path and path not in files:
                files.append(path)
            hit = hit or bool(path)
        return hit

    def note(new_kind: str, name: str | None) -> None:
        nonlocal kind, matched
        if kind is None:
            kind, matched = new_kind, name

    hints = hint_files(class_name)
    if hints:
        note("file_hint", class_name)
        add(*hints)

    if class_name in KNOWN_ALIASES and KNOWN_ALIASES[class_name] in idx["structs"]:
        alias = KNOWN_ALIASES[class_name]
        note("known_alias", alias)
        add(idx["decls"].get(alias))
        return kind, matched, _finish(files, class_name, subsystem)

    flat = command_flatten(class_name)
    if flat is not None:
        alias = KNOWN_COMMAND_ALIASES.get(class_name)
        if alias and alias in idx["commands"]:
            note("known_alias", alias)
            add(idx["commands"][alias])
        elif flat in idx["commands"]:
            note("command_file_match", flat)
            add(idx["commands"][flat])
        return kind or "none", matched, _finish(files, class_name, subsystem)

    cands = list(candidate_names(class_name))
    for cand in cands:
        if cand in idx["structs"]:
            note("struct_match", cand)
            add(idx["decls"].get(cand))
            break
    for cand in cands:
        if cand.lower() in idx["structs_ci"]:
            hit = idx["structs_ci"][cand.lower()]
            note("struct_match_ci", hit)
            add(idx["decls"].get(hit))
            break
    for cand in cands:
        paths = idx["stems"].get(camel_to_snake(cand), [])
        if not paths:
            continue
        if len(paths) <= MAX_STEM_FILES:
            note("filename_match", cand)
            add(*paths)
        break

    return kind or "none", matched, _finish(files, class_name, subsystem)


# ---------------------------------------------------------------------------
# stage 3: classify methods
# ---------------------------------------------------------------------------


def method_spellings(java_name: str, vanilla_class: str = "") -> set[str]:
    out = {camel_to_snake(java_name)}
    m = GETTER_RE.match(java_name)
    if m:  # getShape -> shape as well as get_shape
        out.add(camel_to_snake(m.group(1)))
    out |= METHOD_ALIASES.get(java_name, set())
    out |= CLASS_METHOD_ALIASES.get(vanilla_class, {}).get(java_name, set())
    return out


# Methods a derive or attribute macro writes for the type it is applied to. Without this
# an entire crate can look empty: SteelMC declares its packets as derive-only structs
# (`#[derive(ReadFrom, WriteTo, ClientPacket)] pub struct SUseItemOn { .. }`) with not one
# literal `fn` in the file, so every packet file scanned as zero functions and its class was
# dropped as unresolved - 46 of Steel's 135 dropouts. Names verified 2026-08-21 by reading
# the macro crates: pumpkin-macros/src/lib.rs:203/228/524/585/635 and steel-macros'
# read_from.rs:187, write_to.rs:241, packet.rs:53.
MACRO_METHODS = {
    # SteelMC
    "ReadFrom": {"read"},
    "WriteTo": {"write"},
    "ClientPacket": {"get_id", "packet_id"},
    "ServerPacket": {"get_id", "packet_id"},
    "block_behavior": {"block_behavior"},
    "item_behavior": {"item_behavior"},
    "entity_behavior": {"entity_behavior"},
    # Pumpkin
    "PacketWrite": {"write"},
    "PacketRead": {"read"},
    "PacketReadSlice": {"read_slice"},
    "java_packet": {"to_id", "packet_id"},
    "bedrock_packet": {"packet_id"},
    # shared / std, only where vanilla has a same-shaped method
    "Default": {"default"},
    "Serialize": {"serialize"},
    "Deserialize": {"deserialize"},
}
DERIVE_RE = re.compile(r"#\[derive\(([^)]*)\)\]")
ATTR_MACRO_RE = re.compile(r"^\s*#\[(\w+)[\(\]]", re.MULTILINE)


def macro_fns(text: str) -> set[str]:
    names: set[str] = set()
    for m in DERIVE_RE.finditer(text):
        for part in m.group(1).split(","):
            names |= MACRO_METHODS.get(part.strip().rpartition("::")[2], set())
    for m in ATTR_MACRO_RE.finditer(text):
        names |= MACRO_METHODS.get(m.group(1), set())
    return names


def fns_in_file(rel: str) -> set[str]:
    path = REPO_ROOT / rel
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return set()
    return set(re.findall(r"\bfn\s+(\w+)", text)) | macro_fns(text)


def classify(units: list[dict], idx: dict) -> dict:
    rows: list[dict] = []
    unresolved: list[str] = []
    fnless: list[str] = []
    covered_classes = 0
    for unit in units:
        kind, matched, refs = map_class(unit["vanilla_class"], idx, unit["subsystem"])
        unit["coverage_match_kind"] = kind
        unit["rust_files"] = refs
        unit["matched_name"] = matched
        if kind == "none":
            continue
        covered_classes += 1
        if not refs:
            unresolved.append(unit["vanilla_class"])
            continue
        local: set[str] = set()
        for ref in refs:
            local |= fns_in_file(ref)
        if not local:
            # RESOLVED, but the file declares no callable of any kind: a plain data struct.
            # This used to be lumped in with "unresolved" and dropped from the report, which
            # is a mapping-failure verdict on what is actually a successful mapping. Keep it
            # analysed - every method then falls to the elsewhere/absent tiers, which is the
            # honest reading - and count it separately.
            fnless.append(unit["vanilla_class"])

        in_file, elsewhere, absent, client_only, data_modeled = [], [], [], [], []
        for method in unit["methods"]:
            if method in CLIENT_ONLY_METHODS:
                client_only.append(method)
                continue
            if method in DATA_MODELED_METHODS:
                data_modeled.append(method)
                continue
            spellings = method_spellings(method, unit["vanilla_class"])
            if spellings & local:
                in_file.append(method)
            elif spellings & idx["fns"]:
                elsewhere.append(method)
            else:
                absent.append(method)

        rows.append(
            {
                "subsystem": unit["subsystem"],
                "vanilla_class": unit["vanilla_class"],
                "vanilla_path": unit["vanilla_path"],
                "rust_file": refs[0],
                "rust_files": refs,
                "match_kind": kind,
                "total_methods": len(unit["methods"]),
                "in_declaring_file": len(in_file),
                "elsewhere_in_workspace": len(elsewhere),
                "client_only_skipped": len(client_only),
                "data_modeled_skipped": len(data_modeled),
                "present_in_declaring_file": in_file,
                "present_elsewhere": elsewhere,
                "absent_workspace_wide": absent,
            }
        )

    rows.sort(key=lambda r: (-len(r["absent_workspace_wide"]), r["vanilla_class"]))

    scored = sum(
        r["total_methods"] - r["client_only_skipped"] - r["data_modeled_skipped"] for r in rows
    )
    present = sum(r["in_declaring_file"] + r["elsewhere_in_workspace"] for r in rows)
    strict = sum(r["in_declaring_file"] for r in rows)
    absent = sum(len(r["absent_workspace_wide"]) for r in rows)

    by_sub: dict[str, list[int]] = defaultdict(lambda: [0, 0])
    for r in rows:
        by_sub[r["subsystem"]][0] += r["in_declaring_file"] + r["elsewhere_in_workspace"]
        by_sub[r["subsystem"]][1] += (
            r["total_methods"] - r["client_only_skipped"] - r["data_modeled_skipped"]
        )

    return {
        "milestone": MILESTONE,
        "generated_by": "tools/conformance/conformance.py",
        "method": (
            "name-level match of vanilla methods against Rust fn names: first in the file "
            "declaring the matched type, then anywhere in crates/. Absences are leads, not "
            "proven defects - see the module docstring for false-positive sources."
        ),
        "vanilla_classes_total": len(units),
        "vanilla_classes_covered": covered_classes,
        "classes_analysed": len(rows),
        "classes_unresolved": len(unresolved),
        "unresolved_classes": sorted(unresolved),
        "classes_resolved_without_fns": len(fnless),
        "resolved_without_fns": sorted(fnless),
        "classes_multi_file": sum(1 for r in rows if len(r["rust_files"]) > 1),
        "methods_scored": scored,
        "methods_name_matched": present,
        "methods_absent_workspace_wide": absent,
        "client_only_excluded": sum(r["client_only_skipped"] for r in rows),
        "data_modeled_excluded": sum(r["data_modeled_skipped"] for r in rows),
        "methods_matched_in_mapped_file": strict,
        "strict_match_pct": round(100 * strict / scored, 2) if scored else 0.0,
        "name_match_pct": round(100 * present / scored, 2) if scored else 0.0,
        "class_coverage_pct": round(100 * covered_classes / len(units), 2) if units else 0.0,
        "by_subsystem": {
            k: {
                "matched": v[0],
                "total": v[1],
                "pct": round(100 * v[0] / v[1], 2) if v[1] else 0.0,
            }
            for k, v in sorted(by_sub.items())
        },
        "classes": rows,
    }


# ---------------------------------------------------------------------------
# probes
# ---------------------------------------------------------------------------


def run_probes(units: list[dict], idx: dict, report: dict) -> list[str]:
    """Assert known-true and known-false facts at each stage. Raises on any failure."""
    passed = []
    by_name = {u["vanilla_class"]: u for u in units}

    # -- stage 1: the denominator ------------------------------------------------
    probe("decompile has the expected bulk", len(list(VANILLA_ROOT.rglob("*.java"))) > 4000)
    passed.append("decompile > 4000 java files")

    probe(
        "class count in a plausible band",
        2500 <= len(units) <= 4500,
        f"got {len(units)}",
    )
    passed.append(f"enumerated {len(units)} classes (band 2500-4500)")

    # known-true: Goal.java carries isInterruptable, read at Goal.java:19 this campaign.
    probe("Goal enumerated", "Goal" in by_name)
    probe(
        "Goal.isInterruptable enumerated",
        "isInterruptable" in by_name["Goal"]["methods"],
        "the method regex stopped matching interface bodies",
    )
    passed.append("Goal.isInterruptable enumerated (method regex alive)")

    # known-false: a constructor must never be recorded as a method.
    probe(
        "constructors excluded",
        all(u["simple_name"] not in u["methods"] for u in units),
    )
    passed.append("no class records its own constructor as a method")

    probe("package-info filtered", "package-info" not in by_name)
    passed.append("package-info noise filtered")

    # -- stage 1b: nested-class attribution, added 2026-08-21 --------------------
    # known-true: Item.Properties is its own unit and owns the builder calls.
    props = by_name.get("Item.Properties")
    probe("Item.Properties enumerated as its own class", props is not None)
    probe(
        "Item.Properties owns the builder calls",
        {"axe", "durability", "food", "fireResistant"} <= set(props["methods"]),
        f"got {props['methods'][:10]}",
    )
    # known-false: the outer class must NOT claim them. This was the largest measured
    # false-positive source - 41 of Item's 84 methods were Properties'.
    probe(
        "Item no longer claims its nested builder's methods",
        not ({"axe", "durability", "food", "fireResistant"} & set(by_name["Item"]["methods"])),
        f"got {by_name['Item']['methods']}",
    )
    probe(
        "Item still owns its own methods",
        "useOn" in by_name["Item"]["methods"] or "use" in by_name["Item"]["methods"],
        "the brace walker lost the outer body",
    )
    # known-false: a nested constructor is not a method of its own class either.
    probe("Properties does not record its constructor", "Properties" not in props["methods"])
    # known-false: braces inside string and char literals must not close a class body.
    sanity = 'public class A { public void f() { String s = "}"; char c = \'}\'; } }'
    _, tys = types_in_file(sanity)
    probe("literals do not close a class body", len(tys) == 1 and tys[0]["simple"] == "A",
          f"got {tys}")
    passed.append("nested attribution: Item.Properties owns axe/durability/food, Item does not")

    # -- stage 2: the mapping ----------------------------------------------------
    probe("struct index non-empty", len(idx["structs"]) > 500, f"got {len(idx['structs'])}")
    probe("fn index non-empty", len(idx["fns"]) > 2000, f"got {len(idx['fns'])}")
    passed.append(
        f"indexes: {len(idx['structs'])} types, {len(idx['fns'])} fns, "
        f"{len(idx['commands'])} command files"
    )

    # -- repo-agnostic: macro-declared methods, added 2026-08-21 -----------------
    # known-true: a derive-only struct has methods. SteelMC's packet structs contain no
    # literal `fn`, which scanned as an empty file and dropped 46 packet classes.
    probe(
        "derive names are read as methods",
        macro_fns("#[derive(Clone, Debug, WriteTo, ReadFrom)]\npub struct X;") == {"read", "write"},
        f"got {macro_fns('#[derive(WriteTo, ReadFrom)]')}",
    )
    probe(
        "attribute macros are read as methods",
        "to_id" in macro_fns("#[java_packet(BLOCK_UPDATE)]\npub struct CBlockUpdate;"),
    )
    # known-false: an unknown macro must not invent methods.
    probe(
        "unknown macros contribute nothing",
        macro_fns("#[derive(ZzzNotARealDerive)]\n#[zzz_not_a_real_attr(x)]") == set(),
    )
    probe(
        "a resolved file with no fn is analysed, not dropped",
        report["classes_unresolved"] < report["classes_analysed"] / 10,
        f"{report['classes_unresolved']} unresolved vs {report['classes_analysed']} analysed",
    )
    passed.append(
        f"macro-declared methods seen; {report['classes_resolved_without_fns']} classes "
        f"resolved to a fn-less file are analysed rather than dropped"
    )

    if not IS_PUMPKIN:
        passed.append("pumpkin-specific rename/mapping probes skipped (measuring another repo)")
        return passed

    probe("command index non-empty", len(idx["commands"]) > 40, f"got {len(idx['commands'])}")

    # known-true renames that plain name matching cannot recover.
    kind, matched, _ = map_class("FoodData", idx)
    probe("FoodData -> HungerManager", matched == "HungerManager", f"got {kind}/{matched}")
    kind, matched, refs = map_class("ListPlayersCommand", idx)
    probe(
        "ListPlayersCommand -> list.rs",
        any(r.endswith("commands/list.rs") for r in refs),
        f"got {kind}/{refs}",
    )
    # NB the packet rewrite recovers the C/S prefix but not a changed noun: this fork
    # names ClientboundAddEntityPacket CSpawnEntity (Yarn), so that class does NOT match.
    kind, matched, _ = map_class("ClientboundBlockUpdatePacket", idx)
    probe("Clientbound* -> C*", matched == "CBlockUpdate", f"got {kind}/{matched}")
    passed.append("rename probes: FoodData, ListPlayersCommand, ClientboundBlockUpdatePacket")

    # -- stage 2b: many-to-many, added 2026-08-21 with the mapper fix ------------
    # known-true: a name-but-no-file stage must not terminate the chain. `Pig` matches a
    # generated `pub const PIG` (no declaring file); entity/passive/pig.rs must still be
    # reached. This was 112 of the 126 silently-dropped classes.
    kind, _, refs = map_class("Pig", idx)
    probe(
        "Pig falls through a fileless name match to passive/pig.rs",
        any(r.endswith("entity/passive/pig.rs") for r in refs),
        f"got {kind}/{refs}",
    )
    # known-true: one class, several files.
    _, _, refs = map_class("Item", idx)
    probe("Item maps to more than one file", len(refs) >= 2, f"got {refs}")
    probe(
        "Item reaches item_stack, where hurtAndBreak/getDestroySpeed live",
        any(r.endswith("item_stack/mod.rs") for r in refs),
        f"got {refs}",
    )
    # known-true: several classes, one file - and a file that declares no type at all, so
    # only the hint table can reach it.
    _, _, slab = map_class("WeatheringCopperSlabBlock", idx)
    _, _, door = map_class("WeatheringCopperDoorBlock", idx)
    probe(
        "WeatheringCopper* classes share copper_weathering.rs",
        slab and slab == door and slab[0].endswith("copper_weathering.rs"),
        f"got {slab} / {door}",
    )
    # known-false: hints must not invent files. A pattern whose paths are absent (which is
    # every Pumpkin path when measuring Steel) contributes nothing.
    probe("hints never yield a nonexistent path", all((REPO_ROOT / h).is_file()
                                                      for c in ("Item", "MaceItem")
                                                      for h in hint_files(c)))
    # known-false: same-stem contamination. Vanilla Slime is an entity; block/blocks/slime.rs
    # is a different class that happens to share the stem, and its fns were credited to the mob.
    _, _, slime = map_class("Slime", idx, "entity")
    probe(
        "Slime does not pick up the slime BLOCK file",
        slime and all("/block/" not in r for r in slime),
        f"got {slime}",
    )
    # known-false: a Java-edition class must not land in a bedrock packet module.
    for cls, sub in (("Attribute", "entity"), ("LevelChunk", "chunk")):
        _, _, refs = map_class(cls, idx, sub)
        probe(f"{cls} avoids /bedrock/", refs and all("/bedrock/" not in r for r in refs),
              f"got {refs}")
    passed.append("many-to-many: Pig falls through, Item spans files, WeatheringCopper* share one")
    passed.append("wrong-map probes: Slime stays an entity, Attribute/LevelChunk avoid bedrock")

    # known-false: an invented class must not match anything.
    kind, matched, _ = map_class("ZzzNotARealVanillaClass", idx)
    probe("invented class stays unmatched", kind == "none", f"got {kind}/{matched}")
    # known-false: a command word with no Pumpkin file must not fall through to the
    # generic struct index (the ItemCommands -> struct Item collision).
    kind, _, _ = map_class("ZzzNotARealCommand", idx)
    probe("unknown command stays unmatched", kind == "none")
    passed.append("negative probes: invented class and invented command stay unmatched")

    # -- stage 3: the classification --------------------------------------------
    rows = {r["vanilla_class"]: r for r in report["classes"]}

    probe("Goal analysed", "Goal" in rows)
    # Deliberately checks the DECLARING-FILE tier, not merely "not absent": a plain
    # snake-case `can_use` exists somewhere in the workspace, so the weaker form of this
    # probe passed even with the alias deleted. Verified by deleting it (negative control,
    # 2026-08-21) - the strict form fails, the loose one did not.
    probe(
        "Goal.isInterruptable matched in the mapped file via can_stop",
        "isInterruptable" in rows["Goal"]["present_in_declaring_file"],
        "METHOD_ALIASES stopped being applied",
    )
    passed.append("Goal.isInterruptable matched in the mapped file (alias table live)")

    # KNOWN BLIND SPOT, asserted so it cannot change unnoticed: METHOD_RE requires a body,
    # so abstract and interface declarations (`public abstract boolean canUse();`) are not
    # enumerated at all. Goal.canUse is the worked example. Whoever widens the regex will
    # trip this probe and must re-measure precision, since the denominator moves.
    probe(
        "abstract declarations are still outside the denominator",
        "canUse" not in by_name["Goal"]["methods"],
        "the method regex now catches bodiless declarations - the denominator changed, "
        "re-measure precision and update the docstring",
    )
    passed.append("known blind spot pinned: bodiless abstract methods are not enumerated")

    probe(
        "hurtClient never counted as a gap",
        all("hurtClient" not in r["absent_workspace_wide"] for r in report["classes"]),
        "CLIENT_ONLY_METHODS stopped being applied",
    )
    passed.append("hurtClient excluded as client-only")

    # known-true gap: no lightning-strike-on-entity path exists (CLAUDE.md, still open).
    golem = rows.get("CopperGolem")
    probe("CopperGolem analysed", golem is not None)
    probe(
        "CopperGolem.thunderHit still flagged absent",
        "thunderHit" in golem["absent_workspace_wide"],
        "either it was implemented (good - retire this probe) or the classifier over-credits",
    )
    passed.append("CopperGolem.thunderHit flagged absent (known real gap)")

    # arithmetic sanity: the failure mode that once printed 220.9%.
    # The unresolved count is the mapper's own dropout rate: 126 before the many-to-many
    # fix, and each dropped class removes its whole method set from the denominator.
    probe(
        "unresolved dropout stays small",
        report["classes_unresolved"] < 40,  # 30 at the 2026-08-21 fix, 126 before it
        f"got {report['classes_unresolved']} - the fall-through in map_class regressed",
    )
    probe(
        "multi-file classes actually exist in the report",
        report["classes_multi_file"] >= 20,
        f"got {report['classes_multi_file']}",
    )
    passed.append(
        f"{report['classes_unresolved']} unresolved (<30), "
        f"{report['classes_multi_file']} classes span >1 file"
    )

    probe("no percentage over 100", report["name_match_pct"] <= 100)
    probe("class coverage <= 100", report["class_coverage_pct"] <= 100)
    probe(
        "tiers sum to the scored total",
        report["methods_name_matched"] + report["methods_absent_workspace_wide"]
        == report["methods_scored"],
    )
    probe("all subsystem pcts <= 100", all(v["pct"] <= 100 for v in report["by_subsystem"].values()))
    passed.append("arithmetic: tiers sum, no percentage over 100")

    return passed


# ---------------------------------------------------------------------------


def draw_sample(report: dict, count: int, seed: int) -> list[dict]:
    leads = [
        {"vanilla_class": r["vanilla_class"], "vanilla_path": r["vanilla_path"],
         "rust_file": r["rust_file"], "method": m}
        for r in report["classes"]
        for m in r["absent_workspace_wide"]
    ]
    rng = random.Random(seed)
    return rng.sample(leads, min(count, len(leads)))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=pathlib.Path, default=HERE / "conformance.json")
    parser.add_argument("--no-probes", action="store_true", help="skip self-validation (do not)")
    parser.add_argument("--sample", type=int, default=0, help="print N random flagged gaps")
    parser.add_argument("--seed", type=int, default=0, help="seed for --sample; record it")
    args = parser.parse_args()

    units = enumerate_vanilla()
    idx = build_indexes()
    report = classify(units, idx)
    report["probes"] = "skipped" if args.no_probes else []

    if not args.no_probes:
        try:
            report["probes"] = run_probes(units, idx, report)
        except ProbeFailure as exc:
            print(f"PROBE FAILED: {exc}", file=sys.stderr)
            print("refusing to emit a number from an instrument that fails its own check.",
                  file=sys.stderr)
            return 2

    args.out.write_text(json.dumps(report, indent=1))

    if args.sample:
        sample = draw_sample(report, args.sample, args.seed)
        report["sample"] = {"seed": args.seed, "leads": sample}
        args.out.write_text(json.dumps(report, indent=1))
        print(f"\nsample of {len(sample)} flagged gaps (seed {args.seed}):")
        for lead in sample:
            print(f"  {lead['vanilla_class']}.{lead['method']}")
            print(f"      {lead['vanilla_path']}  <->  {lead['rust_file']}")
        print()

    print(f"probes: {'SKIPPED' if args.no_probes else str(len(report['probes'])) + ' passed'}")
    print(f"vanilla classes enumerated      {report['vanilla_classes_total']}")
    print(f"  with a Rust counterpart       {report['vanilla_classes_covered']} "
          f"({report['class_coverage_pct']}%)")
    print(f"  analysed at method level      {report['classes_analysed']} "
          f"({report['classes_unresolved']} unresolved to a file)")
    print(f"methods scored                  {report['methods_scored']} "
          f"({report['client_only_excluded']} client-only, "
          f"{report['data_modeled_excluded']} data-modeled excluded)")
    print(f"  matched in the mapped file    {report['methods_matched_in_mapped_file']} "
          f"({report['strict_match_pct']}%)   <- strict")
    print(f"  matched anywhere in crates/   {report['methods_name_matched']} "
          f"({report['name_match_pct']}%)   <- loose, credits trait defaults")
    print(f"  absent workspace-wide (leads) {report['methods_absent_workspace_wide']}")
    print("\nby subsystem:")
    for name, value in report["by_subsystem"].items():
        print(f"  {name:<13} {value['matched']:5}/{value['total']:<5} {value['pct']:5.1f}%")
    print(f"\n-> {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
