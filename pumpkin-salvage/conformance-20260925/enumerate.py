#!/usr/bin/env python3
"""
Mechanically enumerate conformance units (vanilla classes + their public/protected
methods) from the decompiled 26.2 vanilla source. No AI calls. This defines the
denominator for the conformance catalog -- it must be built before any fidelity
comparison so the catalog isn't self-selected from prior audits.
"""
import json
import os
import re
import sys
from pathlib import Path

# Regenerate with tools/decompile-vanilla.sh. The old /tmp path did not survive a reboot,
# which silently broke this script; PUMPKIN_DECOMP overrides for a non-default location.
DECOMP_ROOT = Path(
    os.environ.get("PUMPKIN_DECOMP", Path.home() / "pumpkin-vanilla-26.2/decompiled")
) / "net/minecraft"
MILESTONE = "26.2"

# The original eight packages covered only gameplay -- 1598 of the decompile's 4839
# classes. That silently excluded five of the nine weighted subsystems in
# ROADMAP_26_2_PARITY.md, including the two heaviest (Protocol 15, Persistence 15), so
# every score computed from it described about a third of the surface while reading like
# a whole-codebase number. The roadmap-aligned packages are added below.
SUBSYSTEMS = {
    # gameplay (original set)
    "world/level/block": "block",
    "world/entity": "entity",
    "world/item": "item",
    "world/inventory": "inventory",
    "world/level/material": "material",
    "world/food": "food",
    "world/level/border": "border",
    "world/level/gameevent": "gameevent",
    # worldgen, dimensions, lighting
    "world/level/levelgen": "worldgen",
    "world/level/lighting": "lighting",
    "world/level/chunk": "chunk",
    # protocol and connectivity
    "network/protocol": "protocol",
    # commands and administration. net/minecraft/commands is Brigadier plumbing
    # (argument types, selectors); the actual /give, /tp, /fill implementations live in
    # server/commands, which is what Pumpkin's crates/pumpkin/src/command/commands mirrors.
    "commands": "commands",
    "server/commands": "commands",
    # persistence and world I/O
    "world/level/storage": "storage",
    "world/level/saveddata": "saveddata",
    # server core
    "server/level": "server_level",
}

# Deliberately excluded, with reasons -- these would inflate the denominator with code
# nobody intends to port:
#   util/datafix (396 classes) -- legacy save-format upgraders for every version since
#     1.0. Pumpkin targets 26.2 saves only; porting datafixers is not a parity goal.
#     Revisit only if backward-compatible world loading becomes a requirement.

CLASS_RE = re.compile(
    r"^\s*(?:public|protected)\s+(?:abstract\s+|final\s+|static\s+)*(?:class|interface|enum|record)\s+(\w+)",
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
# things that look like methods but aren't (constructors handled separately, control keywords)
KEYWORD_FALSE_POSITIVES = {
    "if", "for", "while", "switch", "catch", "synchronized", "return",
}


def extract_units(java_path: Path, subsystem: str):
    try:
        text = java_path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []

    class_match = CLASS_RE.search(text)
    class_name = class_match.group(1) if class_match else java_path.stem

    methods = set()
    for m in METHOD_RE.finditer(text):
        name = m.group(1)
        if name in KEYWORD_FALSE_POSITIVES or name == class_name:
            continue
        methods.add(name)

    rel_path = str(java_path.relative_to(DECOMP_ROOT.parent.parent))
    return {
        "milestone": MILESTONE,
        "subsystem": subsystem,
        "vanilla_class": class_name,
        "vanilla_path": rel_path,
        "method_count": len(methods),
        "methods": sorted(methods),
    }


def main():
    if not DECOMP_ROOT.exists():
        print(f"decomp root missing: {DECOMP_ROOT}", file=sys.stderr)
        sys.exit(1)

    units = []
    for subdir, subsystem in SUBSYSTEMS.items():
        pkg_dir = DECOMP_ROOT / subdir
        if not pkg_dir.exists():
            continue
        for java_file in sorted(pkg_dir.rglob("*.java")):
            unit = extract_units(java_file, subsystem)
            if unit:
                units.append(unit)

    out = {
        "milestone": MILESTONE,
        "generated_by": "conformance/enumerate.py",
        "total_classes": len(units),
        "total_methods": sum(u["method_count"] for u in units),
        "units": units,
    }
    out_path = Path(__file__).parent / "vanilla_units.json"
    out_path.write_text(json.dumps(out, indent=1))
    print(f"enumerated {out['total_classes']} classes, {out['total_methods']} methods -> {out_path}")


if __name__ == "__main__":
    main()
