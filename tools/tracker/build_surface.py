#!/usr/bin/env python3
"""Count registry coverage: how many blocks, items and entity types have behaviour here.

This is the counterpart to the method-level view in `build_tracker.py`. It answers a
different and easier question — "does this block/item/entity have an implementation at
all" — which is the question other Rust servers' trackers put on their front page, so
having it makes the numbers comparable.

Coverage is read from the declarations the registries themselves use, so it cannot drift
from the code:

* blocks   `#[pumpkin_block("minecraft:x")]` and `#[pumpkin_block_from_tag("minecraft:tag")]`
* items    `impl ItemMetadata for X { fn ids() { [Item::A.id, ...] } }`
* entities the `EntityType::X =>` arms of `entity::type::from_type`

Denominators come from the generated registry data in `pumpkin-data`, i.e. the real 26.2
registries.

    python3 tools/tracker/build_surface.py            # writes docs/tracker/surface.json
"""

import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]


def read(path: pathlib.Path) -> str:
    return path.read_text(errors="ignore")


def block_tag_members() -> dict[str, list[str]]:
    """Map `MINECRAFT_FOO` -> the block names in that tag, from the generated tag table."""
    text = read(ROOT / "crates/pumpkin-data/src/generated/tag.rs")
    members: dict[str, list[str]] = {}
    for match in re.finditer(
        r"pub const ((?:MINECRAFT|C)_[A-Z0-9_]+): super::Tag = \(\s*&\[(.*?)\]", text, re.S
    ):
        members[match.group(1)] = re.findall(r'"([a-z0-9_/]+)"', match.group(2))
    return members


def covered_blocks() -> set[str]:
    tags = block_tag_members()
    covered: set[str] = set()
    for path in (ROOT / "crates/pumpkin/src/block").rglob("*.rs"):
        text = read(path)
        for name in re.findall(r'#\[pumpkin_block\("minecraft:([a-z0-9_]+)"\)\]', text):
            covered.add(name)
        for namespace, tag in re.findall(
            r'#\[pumpkin_block_from_tag\("([a-z]+):([a-z0-9_/]+)"\)\]', text
        ):
            key = namespace.upper() + "_" + tag.upper().replace("/", "_")
            covered.update(tags.get(key, []))
    return covered


def total_blocks() -> int:
    text = read(ROOT / "crates/pumpkin-data/src/generated/block.rs")
    return len(set(re.findall(r"pub const ([A-Z0-9_]+): Self = Block \{", text)))


def placeable_block_items() -> set[str]:
    """Items that place a block, which need no `ItemMetadata` of their own.

    `use_item_on` routes any item carrying a `Block::from_item_id` mapping straight to block
    placement, so those items behave without a registration of their own. Counting them as
    uncovered understated item support by about a thousand entries.

    Membership is decided by registry name, not by item id: `item.rs` is multi-version and the
    same item carries different ids in different version tables, so an id-based match pairs an
    item from one version against a block item from another. Name matching is what the
    generated mapping is built from and is stable across versions.
    """
    items = {name.lower() for name in re.findall(
        r"pub const ([A-Z0-9_]+): Self = Self \{", read(ROOT / "crates/pumpkin-data/src/generated/item.rs")
    )}
    blocks = {name.lower() for name in re.findall(
        r"pub const ([A-Z0-9_]+): Self = Block \{", read(ROOT / "crates/pumpkin-data/src/generated/block.rs")
    )}
    return items & blocks


def covered_items() -> set[str]:
    """Every `Item::X.id` named in an `ItemMetadata::ids` body, however it is formatted."""
    covered: set[str] = placeable_block_items()
    for path in (ROOT / "crates/pumpkin/src/item").rglob("*.rs"):
        text = read(path)
        for match in re.finditer(r"fn ids\(\)[^{]*\{", text):
            body = text[match.end() : match.end() + 2000]
            depth = 1
            for index, char in enumerate(body):
                if char == "{":
                    depth += 1
                elif char == "}":
                    depth -= 1
                    if depth == 0:
                        body = body[:index]
                        break
            covered.update(name.lower() for name in re.findall(r"Item::([A-Z0-9_]+)", body))
    return covered


def total_items() -> int:
    text = read(ROOT / "crates/pumpkin-data/src/generated/item.rs")
    return len(set(re.findall(r"pub const ([A-Z0-9_]+): Self = Self \{", text)))


def covered_entities() -> set[str]:
    text = read(ROOT / "crates/pumpkin/src/entity/type.rs")
    return {name.lower() for name in re.findall(r"EntityType::([A-Z0-9_]+)", text)}


def total_entities() -> int:
    text = read(ROOT / "crates/pumpkin-data/src/generated/entity_type.rs")
    return len(set(re.findall(r"pub const ([A-Z0-9_]+): EntityType = EntityType \{", text)))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=pathlib.Path, default=ROOT / "docs/tracker/surface.json")
    args = parser.parse_args()

    groups = {
        "blocks": (len(covered_blocks()), total_blocks()),
        "items": (len(covered_items()), total_items()),
        "entities": (len(covered_entities()), total_entities()),
    }

    covered_total = sum(covered for covered, _ in groups.values())
    registry_total = sum(total for _, total in groups.values())

    if registry_total == 0:
        print("found no registry entries — did the generated data move?", file=sys.stderr)
        return 1

    data = {
        "method": "registry entries with a behaviour implementation, read from the registration macros",
        "groups": {
            name: {"covered": covered, "total": total}
            for name, (covered, total) in groups.items()
        },
        "covered": covered_total,
        "total": registry_total,
    }

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(data, separators=(",", ":")))

    for name, (covered, total) in groups.items():
        print(f"{name:9} {covered:5}/{total:<5} {covered / total * 100:5.1f}%")
    print(f"{'total':9} {covered_total:5}/{registry_total:<5} {covered_total / registry_total * 100:5.1f}%")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
