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

MEASURED PRECISION (2026-08-21, this instrument, sample recorded in sample_verdicts.json)
-----------------------------------------------------------------------------------------
15 leads drawn with `--sample 15 --seed 815` and verified by reading BOTH sides:

    REAL_GAP        4   LootTable.createStackSplitter, LivingEntity.setStingerCount,
                        StonecutterMenu.removed, SideChainPartBlock.connectToTheLeft
    RENAME          4   ignoreExplosion, getMinZ, doHurtTarget, createMenu
    DATA/INLINED    5   modeled as a struct field, a registry entry or a component
    CLIENT/DEBUG    2   displayFireAnimation, GateBehavior.debugString

So about ONE IN FOUR flagged leads is a real gap (4/15 = 27%, and with n=15 the interval
around that is wide - roughly 9-55%). Scale the lead count by ~0.27 for an expected
backlog, never read it as a defect count.

An earlier 18-lead sample of this same run BEFORE the 2026-08-21 alias and data-modeled
additions scored 2/18 = 11%; the additions raised it. The two samples come from different
runs, so treat that as directional, not a controlled comparison.

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
4. One class maps to one file. Pumpkin splits Entity behaviour across mod.rs, living.rs
   and player.rs, so the strict tier under-credits large classes badly.
5. 126 covered classes resolve to no readable Rust file and are dropped entirely, and
   some mappings are simply wrong (LevelChunk lands on a bedrock packet file). A wrong
   mapping moves methods between the two present tiers; it does not create absences.
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


def enumerate_vanilla() -> list[dict]:
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
            text = java.read_text(encoding="utf-8", errors="replace")
            match = CLASS_RE.search(text)
            name = match.group(1) if match else java.stem
            if name in NOISE_CLASSES:
                continue
            methods = {
                m.group(1)
                for m in METHOD_RE.finditer(text)
                if m.group(1) not in KEYWORD_FALSE_POSITIVES and m.group(1) != name
            }
            methods -= STRUCTURAL_EXCLUSIONS
            units.append(
                {
                    "subsystem": subsystem,
                    "vanilla_class": name,
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

    stems: dict[str, str] = {}
    all_rs = [q for root in SRC_ROOTS for q in (REPO_ROOT / root).rglob("*.rs")]
    for path in sorted(all_rs):
        rel = str(path.relative_to(REPO_ROOT))
        stem = path.stem if path.stem != "mod" else path.parent.name
        stems.setdefault(stem, rel)

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


def candidate_names(class_name: str):
    seen = set()
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


def map_class(class_name: str, idx: dict) -> tuple[str, str | None, str | None]:
    """-> (match kind, matched Rust name, Rust file if known)."""
    if class_name in KNOWN_ALIASES and KNOWN_ALIASES[class_name] in idx["structs"]:
        alias = KNOWN_ALIASES[class_name]
        return "known_alias", alias, idx["decls"].get(alias)

    flat = command_flatten(class_name)
    if flat is not None:
        alias = KNOWN_COMMAND_ALIASES.get(class_name)
        if alias and alias in idx["commands"]:
            return "known_alias", alias, idx["commands"][alias]
        if flat in idx["commands"]:
            return "command_file_match", flat, idx["commands"][flat]
        return "none", None, None

    for cand in candidate_names(class_name):
        if cand in idx["structs"]:
            return "struct_match", cand, idx["decls"].get(cand)
    for cand in candidate_names(class_name):
        if cand.lower() in idx["structs_ci"]:
            hit = idx["structs_ci"][cand.lower()]
            return "struct_match_ci", hit, idx["decls"].get(hit)
    for cand in candidate_names(class_name):
        snake = camel_to_snake(cand)
        if snake in idx["stems"]:
            return "filename_match", cand, idx["stems"][snake]
    return "none", None, None


# ---------------------------------------------------------------------------
# stage 3: classify methods
# ---------------------------------------------------------------------------


def method_spellings(java_name: str) -> set[str]:
    out = {camel_to_snake(java_name)}
    m = GETTER_RE.match(java_name)
    if m:  # getShape -> shape as well as get_shape
        out.add(camel_to_snake(m.group(1)))
    out |= METHOD_ALIASES.get(java_name, set())
    return out


def fns_in_file(rel: str) -> set[str]:
    path = REPO_ROOT / rel
    try:
        return set(re.findall(r"\bfn\s+(\w+)", path.read_text(encoding="utf-8", errors="replace")))
    except OSError:
        return set()


def classify(units: list[dict], idx: dict) -> dict:
    rows, unresolved = [], 0
    covered_classes = 0
    for unit in units:
        kind, matched, ref = map_class(unit["vanilla_class"], idx)
        unit["coverage_match_kind"] = kind
        unit["rust_file"] = ref
        unit["matched_name"] = matched
        if kind == "none":
            continue
        covered_classes += 1
        if not ref:
            unresolved += 1
            continue
        local = fns_in_file(ref)
        if not local:
            unresolved += 1
            continue

        in_file, elsewhere, absent, client_only, data_modeled = [], [], [], [], []
        for method in unit["methods"]:
            if method in CLIENT_ONLY_METHODS:
                client_only.append(method)
                continue
            if method in DATA_MODELED_METHODS:
                data_modeled.append(method)
                continue
            spellings = method_spellings(method)
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
                "rust_file": ref,
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
        "classes_unresolved": unresolved,
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
        1000 <= len(units) <= 3000,
        f"got {len(units)}",
    )
    passed.append(f"enumerated {len(units)} classes (band 1000-3000)")

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
        all(u["vanilla_class"] not in u["methods"] for u in units),
    )
    passed.append("no class records its own constructor as a method")

    probe("package-info filtered", "package-info" not in by_name)
    passed.append("package-info noise filtered")

    # -- stage 2: the mapping ----------------------------------------------------
    probe("struct index non-empty", len(idx["structs"]) > 500, f"got {len(idx['structs'])}")
    probe("fn index non-empty", len(idx["fns"]) > 2000, f"got {len(idx['fns'])}")
    probe("command index non-empty", len(idx["commands"]) > 40, f"got {len(idx['commands'])}")
    passed.append(
        f"indexes: {len(idx['structs'])} types, {len(idx['fns'])} fns, "
        f"{len(idx['commands'])} command files"
    )

    # known-true renames that plain name matching cannot recover.
    kind, matched, _ = map_class("FoodData", idx)
    probe("FoodData -> HungerManager", matched == "HungerManager", f"got {kind}/{matched}")
    kind, matched, ref = map_class("ListPlayersCommand", idx)
    probe(
        "ListPlayersCommand -> list.rs",
        ref is not None and ref.endswith("commands/list.rs"),
        f"got {kind}/{ref}",
    )
    # NB the packet rewrite recovers the C/S prefix but not a changed noun: this fork
    # names ClientboundAddEntityPacket CSpawnEntity (Yarn), so that class does NOT match.
    kind, matched, _ = map_class("ClientboundBlockUpdatePacket", idx)
    probe("Clientbound* -> C*", matched == "CBlockUpdate", f"got {kind}/{matched}")
    passed.append("rename probes: FoodData, ListPlayersCommand, ClientboundBlockUpdatePacket")

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
