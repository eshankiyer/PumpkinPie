# Pumpkin vanilla-parity tracker

Tracks how much of vanilla Minecraft (Java) is implemented in
[Pumpkin-MC/Pumpkin](https://github.com/Pumpkin-MC/Pumpkin), by measuring the
repository itself over time.

## Important caveat

**These are coverage proxies, not a parity score.** No honest single percentage
exists for "how much of Minecraft is implemented":

- Most vanilla blocks (stone, planks, wool…) need no behaviour code at all, so
  "blocks with a registered behaviour" *understates* parity.
- A registered behaviour can still be partial or wrong, which *overstates* it.

The useful signal is the direction of travel, not the absolute numbers.

## Usage

```sh
./collect.sh [path-to-pumpkin-checkout]   # appends a row to history.csv
./render.py                               # regenerates index.html
```

`collect.sh` uses `gh` for issue and PR counts if it is installed and
authenticated; without it those columns are left blank.

## What is measured

| Metric | Source |
| --- | --- |
| Blocks with behaviour | `manager.register(` in `pumpkin/src/block/registry.rs` |
| Total blocks | block constants in `pumpkin-data/src/generated/block.rs` |
| Mobs with AI | modules in `pumpkin/src/entity/mob/` |
| Total entity types | constants in `pumpkin-data/src/generated/entity_type.rs` |
| AI goals | modules in `pumpkin/src/entity/ai/goal/` |
| Commands | modules in `pumpkin/src/command/commands/` |
| Block entities | modules in `pumpkin/src/block/entities/` |
| Items with behaviour | modules in `pumpkin/src/item/items/` |
| TODO / FIXME | markers across `pumpkin/src` and `pumpkin-world/src` |
| Open issues / PRs | GitHub search API |

Counting modules and registrations is crude: it rewards a file existing rather
than that file being correct. It is chosen because it is cheap, reproducible
and hard to game accidentally, not because it is precise.
