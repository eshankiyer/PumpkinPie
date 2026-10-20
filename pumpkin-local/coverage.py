#!/usr/bin/env python3
"""
Source-coverage enumerator: walk the block registry, list unimplemented
behaviour families as features for the bot to translate.

RELIABILITY WARNING (2026-07-30). This is a WEAK, high-false-positive signal.
The block->behaviour mapping in Pumpkin runs through registry.rs registrations
+ macro attributes + tag expansions + nested modules, and every static parser
tried here false-flagged IMPLEMENTED families as gaps (pressure_plate, skull,
cauldron, doors, signs were all called gaps but are implemented). Four distinct
causes: macro-only detection, case sensitivity, \b never matching oak_door,
and attribute-placement. Only concrete_powder was a gap under every method.
TREAT EVERY FAMILY HERE AS UNCONFIRMED - verify against registry.rs before
believing it. The reliable gap-finder is the RUNTIME oracle (nuc_probe /
verify), which observes real behaviour instead of guessing from source.

The runtime oracle finds behaviour that is wrong. This finds behaviour that is
ABSENT, systematically, from the data itself - which is the "go through it one
by one" engine. It is data-driven, never source-translating: it reads
Pumpkin's own generated registry (pumpkin-data, itself extracted from vanilla's
data generator) and Pumpkin's behaviour registrations, and reports the gap.
No Minecraft code is read or copied; that clean-room line is the whole reason
Pumpkin exists.

Key move: it does not dump ~1000 bare blocks (most are plain cubes needing no
behaviour). It groups the gap into behavioural FAMILIES by name suffix (doors,
buttons, trapdoors, signs, ...). A family with many members and no behaviour
impl is one high-value feature. Standalone gap blocks are listed separately and
de-prioritised, since a lone plain block usually needs nothing.

    python3 coverage.py            # write COVERAGE.md
    python3 coverage.py --top 20   # also print the top families
"""

import argparse
import os
import re
from collections import defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.expanduser("~/pumpkin-wt/spree")
if not os.path.isdir(REPO):
    REPO = os.path.expanduser("~/Pumpkin")
BLOCK_DATA = os.path.join(REPO, "pumpkin-data/src/generated/block.rs")
BLOCKS_DIR = os.path.join(REPO, "pumpkin/src/block/blocks")
OUT = os.path.join(HERE, "COVERAGE.md")

# Behavioural families - blocks that need shared INTERACTIVE behaviour. Plain
# families (planks, wood, log, stem, leaves, slab, stairs, wall - solid or
# near-solid cubes) are deliberately excluded: they need no custom behaviour,
# and including them was pure noise in the first pass.
FAMILIES = [
    "door", "trapdoor", "fence_gate", "button", "pressure_plate",
    "wall_sign", "hanging_sign", "sign", "candle", "shulker_box",
    "wall_torch", "sapling", "concrete_powder", "coral_fan", "coral",
    "pot", "skull", "head", "rail", "lantern", "campfire", "command_block",
    "beehive", "banner", "bed", "cauldron", "chest", "hopper", "dispenser",
    "dropper", "note_block", "jukebox", "lectern", "composter", "beacon",
]


def all_blocks():
    """Every block id in the generated registry (bare snake_case names)."""
    text = open(BLOCK_DATA).read()
    return sorted(set(re.findall(r'name: "([a-z_]+)"', text)))


REGISTRY = os.path.join(REPO, "pumpkin/src/block/registry.rs")


def covered_declarations():
    """Block ids + tags declared by structs that are actually REGISTERED.

    registry.rs is ground truth: `manager.register(FooBlock)` is the only way
    a behaviour takes effect. So the authoritative "implemented" set is the
    #[pumpkin_block(...)] / #[pumpkin_block_from_tag(...)] declarations on the
    structs that appear in a register() call - not any grep of the source,
    which produced false positives (macro-only) and false negatives (\bdoor\b
    never matching oak_door) three different ways. Returns the joined text of
    those declarations for keyword checks.
    """
    registered = set(re.findall(r"register\((\w+)\)", open(REGISTRY).read()))
    decls = []
    for root, _, files in os.walk(BLOCKS_DIR):
        for fn in files:
            if not fn.endswith(".rs"):
                continue
            src = open(os.path.join(root, fn)).read()
            # Pair each pumpkin_block* attribute with the struct name below it.
            for m in re.finditer(
                r'#\[pumpkin_block(?:_from_tag)?\("minecraft:([a-z_]+)"\)\]\s*'
                r"pub struct (\w+)",
                src,
            ):
                decl, struct = m.group(1), m.group(2)
                if struct in registered:
                    decls.append(decl)
    return " ".join(decls), registered


def family_of(block):
    for fam in FAMILIES:
        if block.endswith(fam) or block == fam:
            return fam
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--top", type=int, default=15)
    a = ap.parse_args()

    blocks = all_blocks()
    covered, registered = covered_declarations()

    # A family is implemented if a REGISTERED struct declares a block/tag whose
    # name contains the family keyword; a standalone block if it is declared
    # by name. Authoritative because it is gated on registry.rs.
    impl_fams = {fam for fam in FAMILIES if fam in covered}

    gap_families = defaultdict(list)
    standalone = []
    for b in blocks:
        fam = family_of(b)
        if fam:
            if fam not in impl_fams:
                gap_families[fam].append(b)
            continue
        if b not in covered:
            standalone.append(b)

    ranked = sorted(gap_families.items(), key=lambda kv: -len(kv[1]))

    with open(OUT, "w") as fh:
        fh.write("# Block behaviour coverage gaps\n\n")
        fh.write(
            f"{len(blocks)} blocks in the registry; {len(registered)} block "
            f"behaviours registered; {len(impl_fams)} of {len(FAMILIES)} "
            "interactive families implemented.\n\n"
            "Families below have members with no behaviour implementation. Each "
            "family is ONE feature to translate (shared logic). Standalone gap "
            "blocks are listed last and are lower priority - many are plain "
            "cubes that need no behaviour. Vanilla behaviour must still be "
            "confirmed from the wiki/oracle before implementing; this only says "
            "WHAT is missing, not HOW it behaves.\n\n"
        )
        fh.write("## Families (ranked by member count)\n\n")
        for fam, members in ranked:
            fh.write(f"- [ ] **{fam}** ({len(members)} blocks) e.g. "
                     f"{', '.join(members[:4])}\n")
        fh.write(f"\n## Standalone gap blocks ({len(standalone)})\n\n")
        fh.write(", ".join(standalone) + "\n")

    print(f"{len(blocks)} blocks, {len(registered)} behaviours registered. "
          f"{len(ranked)} gap families, {len(standalone)} standalone.")
    print(f"wrote {OUT}\n\ntop families:")
    for fam, members in ranked[: a.top]:
        print(f"  {fam:18} {len(members)} blocks")


if __name__ == "__main__":
    main()
