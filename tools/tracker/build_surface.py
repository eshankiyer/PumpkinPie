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


def tag_members() -> dict[str, list[str]]:
    """Map `MINECRAFT_FOO` -> the block names in that tag, from the generated tag table."""
    text = read(ROOT / "crates/pumpkin-data/src/generated/tag.rs")
    members: dict[str, list[str]] = {}
    for match in re.finditer(
        r"pub const ((?:MINECRAFT|C)_[A-Z0-9_]+): super::Tag = \(\s*&\[(.*?)\]", text, re.S
    ):
        members[match.group(1)] = re.findall(r'"([a-z0-9_/]+)"', match.group(2))
    return members


def covered_blocks() -> set[str]:
    tags = tag_members()
    covered: set[str] = set()
    for path in (ROOT / "crates/pumpkin/src/block").rglob("*.rs"):
        text = read(path)
        # The namespace is optional in practice: wither_skull.rs writes
        # `#[pumpkin_block("wither_skeleton_skull")]` with no `minecraft:` prefix.
        for name in re.findall(r'#\[pumpkin_block\("(?:minecraft:)?([a-z0-9_]+)"\)\]', text):
            covered.add(name)
        # A third idiom: a hand-written `impl BlockMetadata { fn ids() }` that reads tag tables
        # directly, which neither attribute macro covers. PressurePlateBlock is one, and all 16
        # pressure plates counted as uncovered because of it.
        for match in re.finditer(r"impl (?:[\w:]+::)?BlockMetadata for \w+ \{", text):
            body, depth = "", 0
            for index in range(match.end() - 1, len(text)):
                body += text[index]
                if text[index] == "{":
                    depth += 1
                elif text[index] == "}":
                    depth -= 1
                    if depth == 0:
                        break
            for key in re.findall(r"tag::Block::([A-Z0-9_]+)", body):
                covered.update(tags.get(key, []))
            # An ids() body may delegate to a local helper - the coral blocks map each live
            # variant to its dead one through `get_dead_type` - so follow exactly one level of
            # indirection. Scanning the whole file instead would credit blocks a behaviour
            # merely mentions, like the FARMLAND in flowerbed's can_place_at.
            for callee in re.findall(r"\b([a-z_][a-z0-9_]*)\s*\(", body):
                fn = re.search(r"fn " + callee + r"\b[^{]*\{", text)
                if not fn:
                    continue
                inner, depth = "", 0
                for index in range(fn.end() - 1, len(text)):
                    inner += text[index]
                    if text[index] == "{":
                        depth += 1
                    elif text[index] == "}":
                        depth -= 1
                        if depth == 0:
                            break
                body += inner
            # Both `Block::NAME` and `BlockId::NAME` appear in these bodies.
            for name in re.findall(r"\bBlock(?:Id)?::([A-Z0-9_]+)", body):
                if not name.startswith(("MINECRAFT_", "C_")):
                    covered.add(name.lower())
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


def generated_id_list(file_name: str, fn_name: str) -> set[int]:
    """The ids a generated `fn NAME() -> Box<[u16]>` enumerator returns."""
    text = read(ROOT / "crates/pumpkin-data/src/generated" / file_name)
    start = text.find(f"pub fn {fn_name}(")
    if start < 0:
        return set()
    body, depth = "", 0
    for index in range(text.find("{", start), len(text)):
        body += text[index]
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                break
    return {int(n) for n in re.findall(r"(\d+)u16", body)}


def items_by_id(ids: set[int]) -> set[str]:
    """Item registry names for a set of item ids, from the Java table."""
    text = read(ROOT / "crates/pumpkin-data/src/generated/item.rs")
    out: set[str] = set()
    for match in re.finditer(r"pub const ([A-Z0-9_]+): Self = Self \{\s*id: (\d+),", text):
        if int(match.group(2)) in ids:
            out.add(match.group(1).lower())
    return out


def java_items() -> set[str]:
    """The Java item registry, separated from the Bedrock one.

    `item.rs` holds both editions in one table: a Bedrock entry carries a
    `BedrockItemVersion` and a namespaced `registry_key`, a Java entry neither. Counting their
    union put the denominator at 2051 and charged this server for 514 Bedrock-only entries -
    `agent_spawn_egg`, `allow`, the separate `*_double_slab` and `*_standing_sign` items - that
    have no Java counterpart at all. The Java registry is 1537, which is independently what
    SteelMC's tracker reports for a Java-only server.
    """
    text = read(ROOT / "crates/pumpkin-data/src/generated/item.rs")
    java: set[str] = set()
    for match in re.finditer(r"pub const ([A-Z0-9_]+): Self = Self \{", text):
        chunk, depth = "", 0
        for index in range(match.end() - 1, len(text)):
            chunk += text[index]
            if text[index] == "{":
                depth += 1
            elif text[index] == "}":
                depth -= 1
                if depth == 0:
                    break
        if "BedrockItemVersion" not in chunk:
            java.add(match.group(1).lower())
    return java


def data_driven_items() -> set[str]:
    """Items whose behaviour the server drives from their data components.

    `use_item_on`/`use_item` read these components straight off the stack, so an item carrying
    one behaves with no `ItemMetadata` of its own - every food is edible without a registration,
    every jukebox disc plays, every tool mines. Counting those as unimplemented understated
    items the same way ore experience understated blocks.

    Only components that give an item its OWN behaviour are counted. Enchantments and attribute
    modifiers are deliberately excluded: they modify an item that must already do something.
    """
    behaviour = (
        "Food", "Consumable", "Equippable", "Tool", "JukeboxPlayable", "Fireworks",
        "BlocksAttacks", "ChargedProjectiles", "WrittenBookContent", "WritableBookContent",
    )
    text = read(ROOT / "crates/pumpkin-data/src/generated/item.rs")
    covered: set[str] = set()
    for match in re.finditer(r"pub const ([A-Z0-9_]+): Self = Self \{", text):
        chunk, depth = "", 0
        for index in range(match.end() - 1, len(text)):
            chunk += text[index]
            if text[index] == "{":
                depth += 1
            elif text[index] == "}":
                depth -= 1
                if depth == 0:
                    break
        # The name may sit on its own line: `(\n    Food,\n    &FoodImpl {`.
        if any(re.search(r"\(\s*" + name + r"\s*,", chunk) for name in behaviour):
            covered.add(match.group(1).lower())
    return covered


def covered_items() -> set[str]:
    """Every `Item::X.id` named in an `ItemMetadata::ids` body, however it is formatted."""
    covered: set[str] = placeable_block_items() | data_driven_items()
    item_tags = tag_members()
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
            # Items register three ways: named individually, or via an item tag
            # (`tag::Item::MINECRAFT_SWORDS`), or by placing a block. Without the tag branch the
            # tag constant was also being mistaken for an item name.
            for key in re.findall(r"tag::Item::([A-Z0-9_]+)", body):
                covered.update(item_tags.get(key, []))
            # An ids() body may call a generated enumerator in `pumpkin-data`, which is a
            # different crate and so out of reach of the same-file helper follow above.
            # `spawn_egg_ids()` is the one in use; it returns a flat id list, so resolve the ids
            # against the item table rather than guessing from names.
            if "spawn_egg_ids()" in body:
                covered.update(items_by_id(generated_id_list("spawn_egg.rs", "spawn_egg_ids")))
            for name in re.findall(r"\bItem(?:Id)?::([A-Z0-9_]+)", body):
                if not name.startswith(("MINECRAFT_", "C_")):
                    covered.add(name.lower())
    return covered & java_items()


def total_items() -> int:
    return len(java_items())


def covered_entities() -> set[str]:
    """Entity types this server implements.

    Most are reachable through the generic `from_type` dispatch in `entity/type.rs`, but some
    are only ever constructed by the system that owns them - a player by the login path, a
    fishing bobber by the fishing rod, a leash knot by leashing - so they never appear there
    despite having a dedicated module. Counting a module whose file name is the entity picks
    those up without crediting the many types that are merely *referenced* by goals and
    targeting code.
    """
    text = read(ROOT / "crates/pumpkin/src/entity/type.rs")
    covered = {name.lower() for name in re.findall(r"EntityType::([A-Z0-9_]+)", text)}
    # Intersect with real entity names: the tree is full of modules like `mod`, `ai` and
    # `living` that are not entity types, and crediting those took the figure over 100%.
    real = {name.lower() for name in re.findall(
        r"pub const ([A-Z0-9_]+): EntityType = EntityType \{",
        read(ROOT / "crates/pumpkin-data/src/generated/entity_type.rs"),
    )}
    for path in (ROOT / "crates/pumpkin/src/entity").rglob("*.rs"):
        if path.stem.lower() in real:
            covered.add(path.stem.lower())
    return covered & real


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
