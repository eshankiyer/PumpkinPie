#!/usr/bin/env python3
"""
Mechanically determine, per enumerated vanilla class, whether an analogue exists
anywhere in the Pumpkin workspace. No AI calls -- pure name normalization + lookup
against an index of Rust struct/enum names and file stems. This is the "coverage"
half of the conformance score; it answers "does Pumpkin even attempt this," not
"does it behave the same" (that's fidelity, scored separately and only on covered
units).
"""
import json
import os
import re
import subprocess
from pathlib import Path

REPO_ROOT = Path(os.environ.get("PUMPKIN_REPO", Path(__file__).resolve().parent.parent))
# Crates moved under crates/ in the upstream restructure; the old flat paths matched
# nothing and silently scored every unit as uncovered (0 structs indexed -> 0.0%).
# Derived from the directory rather than hardcoded so the next move cannot repeat that.
CRATES = sorted(
    f"crates/{p.name}" for p in (REPO_ROOT / "crates").iterdir() if p.is_dir()
)

# suffixes vanilla appends that Pumpkin sometimes drops/renames (Block -> nothing,
# Item -> nothing, Entity kept). Try the class name as-is first, then with these
# suffixes stripped/swapped, in order.
SUFFIX_VARIANTS = ["", "Block", "Item", "Entity", "Impl", "Boss"]

# Curated, hand-verified vanilla-class -> Pumpkin-struct renames that exact/case-
# insensitive matching cannot recover (genuine semantic renames, not typos). Each
# entry was confirmed by reading both sides -- this is NOT fuzzy matching, it's a
# fixed lookup table, so it can't reintroduce the false-positive problem the fuzzy
# fallback had. Keep this small and justified; one line per entry below explains why.
KNOWN_ALIASES = {
    # vanilla FoodData -> Pumpkin HungerManager (hunger/food is modeled as "hunger" throughout Pumpkin)
    "FoodData": "HungerManager",
    "FoodConstants": "HungerManager",
    # AI goal renames, all Yarn-derived and confirmed by reading both sides during the
    # 2026-08-06 porting pass. Without these the goal subsystem looks far emptier than it
    # is -- 118 goal files exist and several vanilla names simply do not appear verbatim.
    "FloatGoal": "SwimGoal",
    "RandomStrollGoal": "WanderAroundGoal",
    "WaterAvoidingRandomStrollGoal": "WanderAroundGoal",
    "LookAtPlayerGoal": "LookAtEntityGoal",
    "RandomLookAroundGoal": "LookAroundGoal",
    "PanicGoal": "EscapeDangerGoal",
    "NearestAttackableTargetGoal": "ActiveTargetGoal",
    "HurtByTargetGoal": "RevengeGoal",
    "EatBlockGoal": "EatGrassGoal",
    "MeleeAttackGoal": "MeleeAttackGoal",
    # Skeletons and illusioners use vanilla's RangedBowAttackGoal, not the generic
    # RangedAttackGoal. The Rust port has existed since cb3660bc and is wired at
    # entity/mob/skeleton/mod.rs:107 -- the backlog claim that skeletons only ever get a
    # melee goal was stale, verified 2026-08-06.
    "RangedBowAttackGoal": "RangedBowAttackGoal",
}

# Command vanilla-class -> Pumpkin command-file-stem renames that the flatten-and-strip
# rule in command_names() cannot recover because the command word itself changed, not
# just its casing. Each confirmed by reading crates/pumpkin/src/command/commands/mod.rs's
# `mod` list plus the target file's `const NAME(S)` this session (2026-08-06).
KNOWN_COMMAND_ALIASES = {
    # vanilla's player-ban command class is BanPlayerCommands (distinct from
    # BanIpCommands/BanListCommands); Pumpkin's file is ban.rs, command name "ban".
    "BanPlayerCommands": "ban",
    # vanilla ListPlayersCommand ("/list") -> Pumpkin list.rs.
    "ListPlayersCommand": "list",
    # vanilla ClearInventoryCommands ("/clear") -> Pumpkin clear.rs.
    "ClearInventoryCommands": "clear",
    # vanilla SetSpawnCommand sets a PLAYER's individual spawn ("/spawnpoint"), distinct
    # from SetWorldSpawnCommand ("/setworldspawn", already matched by the flatten rule).
    "SetSpawnCommand": "spawnpoint",
    # vanilla SetPlayerIdleTimeoutCommand -> Pumpkin's shorter setidletimeout.rs.
    "SetPlayerIdleTimeoutCommand": "setidletimeout",
    # vanilla VersionCommand is folded into Pumpkin's own /pumpkin command
    # (pumpkin.rs, NAMES = ["pumpkin", "version"]) rather than a separate file.
    "VersionCommand": "pumpkin",
    # vanilla EmoteCommands registers "/me" (net/minecraft/server/commands/EmoteCommands.java);
    # Pumpkin's file is me.rs (NAMES = ["me"]) -- flattening "Emote" gives "emote", which
    # matches no Pumpkin command file and would otherwise wrongly land on an unrelated
    # bedrock protocol file named emote.rs via the generic stem index.
    "EmoteCommands": "me",
}

# Vanilla name-suffix -> Pumpkin name-suffix rewrites for entire naming families,
# distinct from KNOWN_ALIASES (single-name renames). Confirmed by grepping the actual
# Pumpkin source: vanilla's `*Menu` GUI classes (AnvilMenu, ChestMenu, FurnaceMenu, ...)
# are all implemented under Pumpkin's `*ScreenHandler` naming (Yarn-derived), e.g.
# pumpkin-inventory/src/anvil/anvil_screen_handler.rs::AnvilScreenHandler.
SUFFIX_REWRITES = [
    ("Menu", "ScreenHandler"),
]

# Vanilla classes that are pure Java doc-marker files with no code (package-info.java)
# or otherwise not real conformance units; filtered out before scoring so they don't
# inflate the denominator.
NOISE_CLASSES = {"package-info"}


# Vanilla names every packet Clientbound<Name>Packet / Serverbound<Name>Packet; Pumpkin
# names the same wire structs C<Name> / S<Name> (e.g. ClientboundAddEntityPacket ->
# CAddEntity, verified against crates/pumpkin-protocol/src/java/). Without this rewrite the
# protocol subsystem scored 2/282 = 0.71%, which is an artifact of naming, not of coverage.
PACKET_RE = re.compile(r"^(Clientbound|Serverbound)(\w+?)(Packet)?$")


def packet_names(class_name: str):
    m = PACKET_RE.match(class_name)
    if not m:
        return
    prefix = "C" if m.group(1) == "Clientbound" else "S"
    body = m.group(2)
    yield f"{prefix}{body}"
    # A few Pumpkin names keep the Packet suffix or drop a trailing noun; try the bare
    # body too so the stem index (add_entity.rs) can match.
    yield body


# Vanilla names every command class Foo(Command|Commands) (net/minecraft/server/commands/
# *.java, e.g. TeleportCommand, BanIpCommands, GameModeCommand). Pumpkin implements
# commands one-file-per-command under crates/pumpkin/src/command/commands/, registered
# by a CommandTree/ArgumentBuilder built inside an `init_command_tree()` function -- there
# is no struct carrying the vanilla class name at all (verified: teleport.rs defines
# EntitiesToEntityExecutor etc., never "TeleportCommand"). File stems are the vanilla
# command word with case and punctuation collapsed (BanIpCommands -> banip.rs,
# DefaultGameModeCommands -> defaultgamemode.rs, TeamMsgCommand -> teammsg.rs) -- read
# directly against crates/pumpkin/src/command/commands/*.rs and their `const NAMES`/`NAME`
# tables. Without this the commands subsystem scored 9/210 = 4.29%, almost entirely a
# naming-convention mismatch, not a real implementation gap.
COMMAND_RE = re.compile(r"^(\w+?)(Commands?)$")


def command_flatten(class_name: str):
    """Strip Command/Commands and collapse case, e.g. BanIpCommands -> 'banip'. Returns
    None for non-command classes. This is deliberately NOT fed into candidate_names /
    the generic struct index: a flattened single-word body like "Item" or "Random" would
    otherwise match an unrelated generated-data struct (ItemCommands -> struct Item is a
    real collision, confirmed 2026-08-06) and silently hide a missing command. It is only
    ever compared against build_command_stem_index(), a directory-scoped index of actual
    command files, mirroring how packet_names is scoped to protocol wire structs.
    """
    m = COMMAND_RE.match(class_name)
    if not m:
        return None
    return m.group(1).lower()


def build_command_stem_index():
    cmd_dir = REPO_ROOT / "crates/pumpkin/src/command/commands"
    stems = {}
    for p in sorted(cmd_dir.glob("*.rs")):
        if p.stem == "mod":
            continue
        stems[p.stem] = str(p.relative_to(REPO_ROOT))
    return stems


def camel_to_snake(name: str) -> str:
    s = re.sub(r"(.)([A-Z][a-z]+)", r"\1_\2", name)
    s = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", s)
    return s.lower()


def strip_suffix(name: str, suffix: str) -> str:
    if suffix and name.endswith(suffix) and len(name) > len(suffix):
        return name[: -len(suffix)]
    return name


# Vanilla abstract-base-class prefixes: Pumpkin has no reason to declare a Rust type for
# the Java abstraction itself when the concrete subclasses cover its behavior (Rust uses
# composition, not inheritance) -- e.g. AbstractBoat -> boat.rs, AbstractArrow -> arrow.rs,
# WanderingTrader's own file has no "Abstract" prefix so this only ever adds candidates,
# never removes the un-stripped name from the search. Verified 2026-09-01 by grep: all of
# AbstractBoat, AbstractMinecart, AbstractArrow already have a same-named-minus-prefix file.
PREFIX_VARIANTS = ["Abstract", "Base"]


def build_struct_index():
    # struct/enum/trait cover Rust's three ways to declare a named type-level thing;
    # trait is the honest analogue of a Java abstract class, and a large fraction of
    # the vanilla catalog IS abstract classes (AbstractArrow, BlockBehaviour, ...).
    # const/static catch vanilla's data-table classes (Foods, ArmorMaterials, MapColor)
    # that Pumpkin models as generated data rather than as a type.
    out = subprocess.run(
        ["rg", "-o", "--no-filename", "-g", "*.rs",
         r"^\s*pub (?:struct|enum|trait|const|static) (\w+)", "-r", "$1"] + CRATES,
        cwd=REPO_ROOT, capture_output=True, text=True, check=False,
    )
    names = set(out.stdout.split())
    return names


def build_filestem_index():
    out = subprocess.run(
        ["find"] + CRATES + ["-name", "*.rs"],
        cwd=REPO_ROOT, capture_output=True, text=True, check=False,
    )
    stems = {}
    for line in out.stdout.splitlines():
        p = Path(line)
        stem = p.stem
        if stem == "mod":
            stem = p.parent.name
        stems.setdefault(stem, line)
    return stems


def candidate_names(class_name: str):
    seen = set()
    bases = [class_name]
    for pre in PREFIX_VARIANTS:
        if class_name.startswith(pre) and len(class_name) > len(pre):
            bases.append(class_name[len(pre):])
    for base_name in bases:
        for suf in SUFFIX_VARIANTS:
            base = strip_suffix(base_name, suf)
            for n in (base_name, base):
                if n not in seen:
                    seen.add(n)
                    yield n
    for vanilla_suf, pumpkin_suf in SUFFIX_REWRITES:
        if class_name.endswith(vanilla_suf):
            rewritten = class_name[: -len(vanilla_suf)] + pumpkin_suf
            if rewritten not in seen:
                seen.add(rewritten)
                yield rewritten
    for n in packet_names(class_name):
        if n not in seen:
            seen.add(n)
            yield n


def find_coverage(class_name: str, struct_index, struct_index_ci, stem_index, command_stem_index):
    if class_name in KNOWN_ALIASES and KNOWN_ALIASES[class_name] in struct_index:
        return "known_alias", KNOWN_ALIASES[class_name], None
    # Command classes are resolved ONLY against the directory-scoped command index, before
    # any generic struct/filename matching runs -- see command_flatten()'s docstring for why.
    flat = command_flatten(class_name)
    if flat is not None:
        if class_name in KNOWN_COMMAND_ALIASES:
            alias = KNOWN_COMMAND_ALIASES[class_name]
            if alias in command_stem_index:
                return "known_alias", alias, command_stem_index[alias]
        if flat in command_stem_index:
            return "command_file_match", flat, command_stem_index[flat]
        return "none", None, None
    for cand in candidate_names(class_name):
        if cand in struct_index:
            return "struct_match", cand, None
    for cand in candidate_names(class_name):
        if cand.lower() in struct_index_ci:
            return "struct_match_ci", struct_index_ci[cand.lower()], None
    for cand in candidate_names(class_name):
        snake = camel_to_snake(cand)
        if snake in stem_index:
            return "filename_match", cand, stem_index[snake]
    return "none", None, None


def main():
    units_path = Path(__file__).parent / "vanilla_units.json"
    data = json.loads(units_path.read_text())
    before = len(data["units"])
    data["units"] = [u for u in data["units"] if u["vanilla_class"] not in NOISE_CLASSES]
    print(f"filtered {before - len(data['units'])} noise units (package-info etc.)")

    struct_index = build_struct_index()
    struct_index_ci = {s.lower(): s for s in struct_index}
    stem_index = build_filestem_index()
    command_stem_index = build_command_stem_index()
    print(
        f"indexed {len(struct_index)} rust structs/enums, {len(stem_index)} file stems, "
        f"{len(command_stem_index)} command files"
    )

    for unit in data["units"]:
        kind, matched_name, path = find_coverage(
            unit["vanilla_class"], struct_index, struct_index_ci, stem_index, command_stem_index
        )
        unit["coverage"] = "covered" if kind != "none" else "not_covered"
        unit["coverage_match_kind"] = kind
        unit["pumpkin_ref"] = path
        unit["pumpkin_matched_name"] = matched_name
        unit["fidelity"] = "unchecked"

    covered = sum(1 for u in data["units"] if u["coverage"] == "covered")
    total = len(data["units"])
    data["coverage_pct"] = round(100 * covered / total, 2)
    data["covered_classes"] = covered

    by_subsystem = {}
    for u in data["units"]:
        s = u["subsystem"]
        d = by_subsystem.setdefault(s, {"total": 0, "covered": 0})
        d["total"] += 1
        if u["coverage"] == "covered":
            d["covered"] += 1
    for s, d in by_subsystem.items():
        d["coverage_pct"] = round(100 * d["covered"] / d["total"], 2) if d["total"] else 0.0
    data["by_subsystem"] = by_subsystem

    out_path = Path(__file__).parent / "catalog.json"
    out_path.write_text(json.dumps(data, indent=1))
    print(f"coverage: {covered}/{total} = {data['coverage_pct']}%")
    for s, d in sorted(by_subsystem.items()):
        print(f"  {s}: {d['covered']}/{d['total']} = {d['coverage_pct']}%")
    print(f"-> {out_path}")


if __name__ == "__main__":
    main()
