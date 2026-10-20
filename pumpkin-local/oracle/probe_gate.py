#!/usr/bin/env python3
"""
Probe gate: the acceptance half of the Phase 2 pass criterion.

A spec passes only if the oracle probe that motivated it flips from divergent to
matching AND no other probe regresses. That is a measurement against a real
vanilla 26.2 server, not a sentence in a spec file, which is the whole point:
a plausible-looking wrong edit compiles and satisfies textual assertions, and
has already done so here (a growth gate got retuned instead of removed).

    probe_gate.py --list
    probe_gate.py --sweep                  # measure every probe, print a table
    probe_gate.py --baseline FILE          # measure all, write JSON
    probe_gate.py --gate NAME --baseline FILE
        exit 0  -> NAME flipped divergent->matching and nothing else regressed
        exit 2  -> NAME did not flip
        exit 3  -> NAME flipped but another probe regressed (names it)
        exit 4  -> a server was unhealthy, so no measurement is trustworthy

Exit 4 matters. Six "divergences" were once recorded while Pumpkin had exhausted
its file descriptors and was answering RCON with garbage. Every one was false.
An unhealthy server must produce no verdict at all, never a verdict of divergent.
"""

import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from differential import SERVERS, Rcon

BASELINE = "/tmp/pumpkin_probe_baseline.json"

# Scratch volume both servers agree is empty air, far from spawn structures.
SCRATCH = (900, 100, 900)


def connect(name):
    """Connect and refuse to return a handle to a server that is not sane.

    The health probe has a known answer. A server that is up enough to accept a
    TCP connection but has run out of file descriptors will still answer, with
    nonsense, and nonsense measured against vanilla reads as a divergence.
    """
    cfg = SERVERS[name]
    r = Rcon(cfg["host"], cfg["port"], cfg["password"], timeout=20)
    probe = r.cmd("seed")
    if "Seed:" not in probe:
        r.close()
        raise RuntimeError(f"{name} answered RCON but is unhealthy: {probe[:120]!r}")
    return r


# ---------------------------------------------------------------------------
# Probes.
#
# Each is measure(rcon) -> a small JSON-able value. Compared by equality between
# the two servers. Keep them cheap and side-effect-free where possible; anything
# that writes must write inside SCRATCH and clean up after itself.
# ---------------------------------------------------------------------------


# Both servers answer an unrecognised literal differently, and neither wording is
# a substring of the other, so match on both. Pumpkin's own text carries a literal
# "%s" because the format argument is never substituted, which is itself a bug and
# is tracked separately; do not "normalise" it away here.
UNKNOWN_MARKERS = ("Unknown or incomplete command", "Unknown command:")


def _gamerule(r, rule):
    out = r.cmd(f"gamerule {rule}")
    if any(t in out for t in UNKNOWN_MARKERS) or "Incorrect argument" in out:
        return "UNSUPPORTED"
    # "Gamerule fall_damage is currently set to: true"
    return out.rpartition(":")[2].strip() or "EMPTY"


def p_gamerule_defaults(r):
    """Default value of every game rule both servers should agree on."""
    rules = [
        "random_tick_speed",
        "advance_weather",
        "advance_time",
        "block_drops",
        "mob_griefing",
        "keep_inventory",
        "fire_damage",
        "max_entity_cramming",
        "spawn_chunk_radius",
        "players_sleeping_percentage",
        "universal_anger",
        "forgive_dead_players",
        "disable_raids",
        "do_patrol_spawning",
        "do_trader_spawning",
        "do_warden_spawning",
        "do_fire_tick",
        "do_mob_spawning",
        "natural_regeneration",
        "fall_damage",
        "drowning_damage",
        "freeze_damage",
        "show_death_messages",
        "announce_advancements",
        "command_block_output",
        "log_admin_commands",
        "send_command_feedback",
        "max_command_chain_length",
        "spectators_generate_chunks",
        "do_immediate_respawn",
        "do_limited_crafting",
        "global_sound_events",
        "lava_source_conversion",
        "water_source_conversion",
        "ender_pearls_vanish_on_death",
        "projectiles_can_break_blocks",
        "minecart_max_speed",
        "tnt_explosion_drop_decay",
        "snow_accumulation_height",
        "locator_bar",
    ]
    return {rule: _gamerule(r, rule) for rule in rules}


def p_command_surface(r):
    """Which commands exist at all. Missing commands are parity gaps by themselves."""
    cmds = [
        "locate structure minecraft:village_plains",
        "place feature minecraft:oak",
        "function minecraft:nonexistent",
        "ride @s dismount",
        "damage @s 1",
        "return 0",
        "random value 1..6",
        "tick query",
        "jfr stop",
        "debug stop",
        "spawnpoint",
        "worldborder get",
        "attribute @s minecraft:scale get",
        "loot give @s loot minecraft:blocks/dirt",
        "item get entity @s weapon",
        "fillbiome ~ ~ ~ ~ ~ ~ minecraft:plains",
        "waypoint list",
        "dialog clear @s",
        "test runall",
    ]
    out = {}
    for c in cmds:
        res = r.cmd(c)
        head = c.split()[0]
        # An unknown-literal reply means the command is not registered. Anything
        # else, including an argument error or "that position is not loaded",
        # means it exists.
        missing = any(t in res for t in UNKNOWN_MARKERS)
        out[head] = "MISSING" if missing else "PRESENT"
    return out


def p_setblock_roundtrip(r):
    """Write a block into scratch and read it back through a filtered clone count."""
    x, y, z = SCRATCH
    r.cmd(f"setblock {x} {y} {z} minecraft:stone")
    # clone ... filtered reports an exact count and is one of the two primitives
    # that behaves identically on both servers, unlike `execute if block`.
    res = r.cmd(f"clone {x} {y} {z} {x} {y} {z} {x} {y} {z + 4} filtered minecraft:stone")
    r.cmd(f"setblock {x} {y} {z} minecraft:air")
    r.cmd(f"setblock {x} {y} {z + 4} minecraft:air")
    digits = [t for t in res.replace(",", " ").split() if t.isdigit()]
    return {"cloned": digits[0] if digits else "NONE"}


def p_difficulty_and_time(r):
    """Difficulty and the shape of a time query.

    Only the difficulty VALUE is compared, not the sentence, because the two
    servers legitimately word messages differently and a wording difference is
    not a parity bug worth an agent's iteration budget.
    """
    diff = r.cmd("difficulty")
    value = diff.rstrip(".").split()[-1] if diff.split() else "EMPTY"
    day = r.cmd("time query day")
    return {
        "difficulty": value,
        "time_query_supported": not any(t in day for t in UNKNOWN_MARKERS),
        "time_query_has_number": any(t.isdigit() for t in day.replace("(", " ").split()),
    }



# --- Block support/survival rules -------------------------------------------
#
# Each case builds a support context in scratch, writes a test block, and reports
# whether the test block survived. Survival is read back with `clone ... filtered`,
# which is one of the two primitives that behaves identically on both servers.
#
# This probe exists because the original four probes sampled gamerules, command
# names, one setblock and difficulty - none of which touch block behaviour, which
# is where most of this project's parity work actually lives. A live-testing pass
# found a real stalactite placement bug that static review had passed as correct;
# the dripstone cases below are that bug, encoded so the harness catches it.
#
# (support_block, d_support_y, test_block, expected_note)
SUPPORT_CASES = [
    ("minecraft:dripstone_block", +1, "minecraft:pointed_dripstone[vertical_direction=down,thickness=tip]", "stalactite hangs from dripstone block"),
    ("minecraft:stone", -1, "minecraft:pointed_dripstone[vertical_direction=up,thickness=tip]", "stalagmite stands on stone"),
    ("minecraft:stone", -1, "minecraft:torch", "torch on solid floor"),
    ("minecraft:air", -1, "minecraft:torch", "torch with no floor must not survive"),
    ("minecraft:stone", -1, "minecraft:lantern", "lantern on solid floor"),
    ("minecraft:stone", -1, "minecraft:redstone_wire", "redstone dust on solid floor"),
    ("minecraft:air", -1, "minecraft:redstone_wire", "redstone dust with no floor must not survive"),
    ("minecraft:dirt", -1, "minecraft:poppy", "flower on dirt"),
    ("minecraft:stone", -1, "minecraft:poppy", "flower on stone must not survive"),
    ("minecraft:sand", -1, "minecraft:cactus", "cactus on sand"),
    ("minecraft:stone", -1, "minecraft:cactus", "cactus on stone must not survive"),
    ("minecraft:stone", -1, "minecraft:rail", "rail on solid floor"),
]


def _survives(r, x, y, z, test_block):
    """True if a block written at (x,y,z) is still there afterwards."""
    # Compare against the block id without its state, since `filtered` matches by block.
    bare = test_block.split("[")[0]
    res = r.cmd(f"clone {x} {y} {z} {x} {y} {z} {x} {y} {z + 6} filtered {bare}")
    digits = [t for t in res.replace(",", " ").split() if t.isdigit()]
    got = digits[0] if digits else "0"
    r.cmd(f"setblock {x} {y} {z + 6} minecraft:air")
    return got != "0"


def p_block_support_rules(r):
    """Whether each block survives placement in a defined support context.

    Covers block-behaviour parity, which the original probe set did not sample at
    all. Only survival is compared - never the message wording, which the two
    servers legitimately phrase differently.

    IMPORTANT on reading the results: the "must not survive" wording in the case
    labels is the AUTHOR'S EXPECTATION, not the pass condition. This is a
    DIFFERENTIAL probe - it passes when Pumpkin's answer equals vanilla's answer,
    whatever that answer is. Measured on vanilla, `torch with no floor` and
    `cactus on stone` both SURVIVE a setblock plus neighbour nudge, because
    /setblock bypasses validation and not every block re-checks support on a
    neighbour update (some wait for a scheduled or random tick). That is vanilla's
    real behaviour under this stimulus, so it is exactly what Pumpkin must match.
    Do not "fix" Pumpkin to satisfy the label.
    """
    x, y, z = SCRATCH
    # Nothing is loaded when no player is online, so forceload the scratch chunks first.
    # The cases are spaced 3 blocks apart over 12 cases, so span the whole x range.
    cx0, cz0 = x >> 4, z >> 4
    cx1 = (x + len(SUPPORT_CASES) * 3) >> 4
    for cx in range(cx0, cx1 + 1):
        r.cmd(f"forceload add {cx * 16} {cz0 * 16}")

    # Refuse to report anything if scratch is still not loaded. Without this guard every
    # case reads GONE on BOTH servers, the comparison reports "matching", and the
    # harness advertises parity it never measured. That false-OK is exactly how the
    # pre-existing setblock_roundtrip probe has been passing - both servers failing
    # identically is not agreement.
    #
    # NOTE: forceload itself was broken on Pumpkin until a90c95fa - it registered the
    # ticket but never raised the chunk level, so it reported success and loaded nothing.
    # If this returns UNMEASURABLE against a Pumpkin build, check it postdates that fix
    # before assuming the probe is at fault.
    probe = r.cmd(f"setblock {x} {y} {z} minecraft:stone")
    if "not loaded" in probe:
        return {"UNMEASURABLE": "scratch chunk not loaded even after forceload"}
    r.cmd(f"setblock {x} {y} {z} minecraft:air")

    out = {}
    for i, (support, dy, test_block, note) in enumerate(SUPPORT_CASES):
        # Space the cases out so neighbouring cases cannot support each other.
        cx, cy, cz = x + (i * 3), y, z
        # Clear the column first so a previous run cannot mask a failure.
        for c in (-1, 0, 1):
            r.cmd(f"setblock {cx} {cy + c} {cz} minecraft:air")
        if support != "minecraft:air":
            r.cmd(f"setblock {cx} {cy + dy} {cz} {support}")
        r.cmd(f"setblock {cx} {cy} {cz} {test_block}")
        # /setblock BYPASSES placement validation - an unsupported block is written and only
        # breaks on the next block update. Without this nudge the probe measures "can setblock
        # write this block" (nearly always yes) rather than "does this block's support rule
        # match vanilla", and scores ~11/12 on both servers while testing nothing. Poking an
        # adjacent block and removing it forces the neighbour update that applies the rule.
        r.cmd(f"setblock {cx + 1} {cy} {cz} minecraft:stone")
        r.cmd(f"setblock {cx + 1} {cy} {cz} minecraft:air")
        out[note] = "SURVIVED" if _survives(r, cx, cy, cz, test_block) else "GONE"
        # Clean up so repeat runs start from bare scratch.
        for c in (-1, 0, 1):
            r.cmd(f"setblock {cx} {cy + c} {cz} minecraft:air")
    return out


def p_mob_attributes(r):
    """Base attribute values for a few mobs, read through the attribute command.

    Movement speed is included deliberately: a live report of 'spider and zombie
    move way too fast' was root-caused to the attribute being applied twice on the
    producer side, and nothing in the original probe set could have caught it.
    Reads base values only, so it does not depend on entities ticking - which they
    do not do with no player online.
    """
    out = {}
    for mob in ["zombie", "spider", "pig", "creeper"]:
        for attr in ["movement_speed", "max_health"]:
            res = r.cmd(f"attribute @e[type=minecraft:{mob},limit=1] minecraft:{attr} base get")
            if any(t in res for t in UNKNOWN_MARKERS):
                out[f"{mob}.{attr}"] = "UNSUPPORTED"
                continue
            nums = [t for t in res.replace(",", " ").split() if _isnum(t)]
            out[f"{mob}.{attr}"] = nums[-1] if nums else "NO_ENTITY"
    return out


def _isnum(t):
    try:
        float(t)
        return True
    except ValueError:
        return False


PROBES = {
    "gamerule_defaults": p_gamerule_defaults,
    "command_surface": p_command_surface,
    "setblock_roundtrip": p_setblock_roundtrip,
    "difficulty_and_time": p_difficulty_and_time,
    "block_support_rules": p_block_support_rules,
    "mob_attributes": p_mob_attributes,
}


def measure_all(only=None):
    """Measure every probe on both servers. Raises if either is unhealthy."""
    van, pum = connect("vanilla"), connect("pumpkin")
    try:
        result = {}
        for pname, fn in PROBES.items():
            if only and pname not in only:
                continue
            v = fn(van)
            p = fn(pum)
            # Report per-key so a probe that covers forty game rules tells you
            # which one moved rather than just "different".
            if isinstance(v, dict) and isinstance(p, dict):
                mismatch = {
                    k: {"vanilla": v.get(k), "pumpkin": p.get(k)}
                    for k in sorted(set(v) | set(p))
                    if v.get(k) != p.get(k)
                }
            else:
                mismatch = {} if v == p else {"_": {"vanilla": v, "pumpkin": p}}
            result[pname] = {
                "status": "matching" if not mismatch else "divergent",
                "mismatch": mismatch,
            }
        return result
    finally:
        van.close()
        pum.close()


def load(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--sweep", action="store_true")
    ap.add_argument("--baseline", default=BASELINE)
    ap.add_argument("--write-baseline", action="store_true")
    ap.add_argument("--gate")
    a = ap.parse_args()

    if a.list:
        for k in PROBES:
            print(k)
        return 0

    try:
        now = measure_all()
    except Exception as e:
        print(f"UNHEALTHY: {e}", file=sys.stderr)
        # Deliberately not a divergence verdict. An unmeasurable server must
        # abstain, because "divergent" here would send the agent off to fix
        # code that was never wrong. That mistake has been made once already.
        return 4

    if a.write_baseline or a.sweep:
        for name, r in now.items():
            flag = "OK " if r["status"] == "matching" else "DIFF"
            print(f"{flag} {name}")
            for k, d in list(r["mismatch"].items())[:12]:
                print(f"       {k}: vanilla={d['vanilla']!r} pumpkin={d['pumpkin']!r}")
        if a.write_baseline:
            with open(a.baseline, "w") as f:
                json.dump({"t": time.time(), "probes": now}, f, indent=2)
            print(f"\nbaseline written to {a.baseline}")
        return 0

    if not a.gate:
        ap.error("need --gate NAME, --sweep, --write-baseline or --list")

    if a.gate not in PROBES:
        print(f"no such probe: {a.gate}", file=sys.stderr)
        return 2

    base = load(a.baseline).get("probes", {})
    if not base:
        print(f"no baseline at {a.baseline}; run --write-baseline first", file=sys.stderr)
        return 2

    # 1. The named probe must have flipped.
    was = base.get(a.gate, {}).get("status")
    is_ = now[a.gate]["status"]
    if was != "divergent":
        print(f"FAIL {a.gate}: baseline was {was!r}, not divergent; nothing to fix")
        return 2
    if is_ != "matching":
        print(f"FAIL {a.gate}: still divergent")
        for k, d in list(now[a.gate]["mismatch"].items())[:8]:
            print(f"     {k}: vanilla={d['vanilla']!r} pumpkin={d['pumpkin']!r}")
        return 2

    # 2. Nothing else may have regressed. A fix that flips its own probe while
    #    breaking another is a net loss and must not be allowed to open a PR.
    regressed = [
        n
        for n in now
        if n != a.gate
        and base.get(n, {}).get("status") == "matching"
        and now[n]["status"] != "matching"
    ]
    if regressed:
        print(f"FAIL {a.gate}: flipped, but these regressed: {', '.join(regressed)}")
        for n in regressed:
            for k, d in list(now[n]["mismatch"].items())[:6]:
                print(f"     {n}.{k}: vanilla={d['vanilla']!r} pumpkin={d['pumpkin']!r}")
        return 3

    print(f"PASS {a.gate}: divergent -> matching, no regressions")
    return 0


if __name__ == "__main__":
    sys.exit(main())
