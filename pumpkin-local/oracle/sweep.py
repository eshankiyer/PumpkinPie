#!/usr/bin/env python3
"""
Oracle sweep: measure behaviours on both servers, and turn any divergence into
a spec the coding agent can act on without a human in the loop.

The chain this closes:

    experiment -> measure vanilla and pumpkin -> diverged? -> write a spec
    -> agent edits -> build gate -> re-measure -> converged?

The important property is that the acceptance test is a MEASUREMENT, not a
sentence someone wrote. A plausible-looking wrong edit does not move the number
onto vanilla's, so it fails. That is the failure this whole setup exists to
close: the agent once "fixed" a growth gate by retuning it instead of removing
it, which compiled and satisfied a textual assertion and was still wrong.

An experiment can only auto-generate a spec if it declares the code site that
governs it. Without that, a divergence is still reported, but localising it is
a human job and the sweep says so rather than guessing.

Usage:
    python3 sweep.py                 # measure everything, report
    python3 sweep.py --emit-specs    # also write specs for divergences
"""

import argparse
import json
import os
import sys
import time

sys.path.insert(0, "/home/eshanki/pumpkin-local/oracle")
from differential import SERVERS, Rcon

SPEC_DIR = "/home/eshanki/pumpkin-local/specs/auto"
REPORT = "/tmp/pumpkin_sweep.json"


def healthy(name):
    cfg = SERVERS[name]
    try:
        r = Rcon(cfg["host"], cfg["port"], cfg["password"], timeout=15)
        ok = "Seed:" in r.cmd("seed")
        r.close()
        return ok
    except Exception:
        return False


# --------------------------------------------------------------------------
# Experiments.
#
# measure(r) -> dict of observable -> value
# site: optional. If present, a divergence can be turned into a line-edit spec
#       automatically. Without it the sweep reports and stops.
# --------------------------------------------------------------------------


def m_gamerule_names(r):
    """Which game rule identifiers does this server accept?"""
    bad = ("Incorrect argument", "Unknown", "Expected", "<--[HERE]", "Invalid")
    out = {}
    for rule in (
        "random_tick_speed",
        "advance_weather",
        "advance_time",
        "block_drops",
        "mob_griefing",
        "keep_inventory",
        "fire_damage",
        "max_entity_cramming",
        "reduced_debug_info",
        "spawn_chunk_radius",
        "players_sleeping_percentage",
        "universal_anger",
        "forgive_dead_players",
        "disable_raids",
        "do_patrol_spawning",
        "do_trader_spawning",
        "do_warden_spawning",
        "do_insomnia",
        "global_sound_events",
        "lava_source_conversion",
        "water_source_conversion",
    ):
        out[rule] = "absent" if any(b in r.cmd(f"gamerule {rule}") for b in bad) else "present"
    return out


def m_command_surface(r):
    """Which commands does this server know about at all?"""
    out = {}
    for c in (
        "locate",
        "place",
        "loot",
        "data",
        "function",
        "datapack",
        "attribute",
        "ride",
        "damage",
        "random",
        "return",
        "rotate",
        "tick",
        "transfer",
        "spreadplayers",
        "spectate",
        "trigger",
        "jfr",
        "debug",
        "perf",
        "fillbiome",
        "item",
        "recipe",
        "advancement",
        "bossbar",
        "team",
        "scoreboard",
        "worldborder",
        "schedule",
        "particle",
        "playsound",
        "stopsound",
        "title",
        "tellraw",
    ):
        resp = r.cmd(f"help {c}")
        known = "Unknown" not in resp and "Incorrect" not in resp and resp.strip() != ""
        out[c] = "present" if known else "absent"
    return out


def m_default_gamerule_values(r):
    """What are the shipped defaults for rules both servers have?"""
    out = {}
    for rule in ("random_tick_speed", "max_entity_cramming", "players_sleeping_percentage"):
        resp = r.cmd(f"gamerule {rule}")
        val = resp.split()[-1] if resp and ":" in resp else "n/a"
        out[rule] = val
    return out


EXPERIMENTS = {
    "gamerule_names": {"fn": m_gamerule_names, "site": None},
    "command_surface": {"fn": m_command_surface, "site": None},
    "gamerule_defaults": {"fn": m_default_gamerule_values, "site": None},
}


def run(name, exp):
    res = {}
    for server in ("vanilla", "pumpkin"):
        cfg = SERVERS[server]
        try:
            r = Rcon(cfg["host"], cfg["port"], cfg["password"], timeout=25)
            res[server] = exp["fn"](r)
            r.close()
        except Exception as e:
            res[server] = {"error": str(e)[:120]}
    return res


def diverge(res):
    v, p = res.get("vanilla", {}), res.get("pumpkin", {})
    if "error" in v or "error" in p:
        return []
    return [(k, v.get(k, "-"), p.get(k, "-")) for k in sorted(set(v) | set(p)) if v.get(k) != p.get(k)]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--emit-specs", action="store_true")
    a = ap.parse_args()

    for s in ("vanilla", "pumpkin"):
        if not healthy(s):
            print(f"  {s} is not healthy. Refusing to measure; a sick server "
                  f"produces confident nonsense.")
            return 1

    report, total = {}, 0
    for name, exp in EXPERIMENTS.items():
        print(f"\n=== {name} ===")
        res = run(name, exp)
        d = diverge(res)
        report[name] = {"result": res, "divergences": d, "site": exp["site"]}
        if not d:
            print("  no divergence")
            continue
        total += len(d)
        print(f"  {len(d)} divergence(s)")
        print(f"  {'observable':<32}{'vanilla':<14}pumpkin")
        for k, vv, pv in d:
            print(f"  {k:<32}{str(vv):<14}{pv}")
        if a.emit_specs and exp["site"]:
            os.makedirs(SPEC_DIR, exist_ok=True)
            print(f"  spec emitted to {SPEC_DIR}")
        elif a.emit_specs:
            print("  no code site declared, so this needs a human to localise")

    with open(REPORT, "w") as f:
        json.dump({"when": time.strftime("%F %T"), "report": report}, f, indent=2)
    print(f"\n  {total} divergence(s) total; full detail in {REPORT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
