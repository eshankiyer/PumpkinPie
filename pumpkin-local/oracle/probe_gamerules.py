#!/usr/bin/env python3
"""First real differential: does Pumpkin accept the same game rules as vanilla?

Game rule naming has already cost this project real time. They were renamed to
snake_case in 25w44a, and every claim about it so far has rested on a wiki page.
Asking both servers directly settles it in one round trip and needs nobody to
remember anything.
"""

import sys

sys.path.insert(0, "/home/eshanki/pumpkin-local/oracle")
from differential import Rcon, SERVERS

RULES = [
    "random_tick_speed",
    "randomTickSpeed",
    "advance_weather",
    "doWeatherCycle",
    "advance_time",
    "doDaylightCycle",
    "block_drops",
    "doTileDrops",
    "mob_griefing",
    "mobGriefing",
    "keep_inventory",
    "keepInventory",
    "fire_damage",
    "natural_regeneration",
    "spawn_radius",
    "max_entity_cramming",
]


def ok(resp):
    """A rule exists if the server reports a value rather than an argument error."""
    bad = ("Incorrect argument", "Unknown", "Expected", "<--[HERE]", "Invalid")
    return not any(b in resp for b in bad)


conns = {}
for name, cfg in SERVERS.items():
    try:
        conns[name] = Rcon(cfg["host"], cfg["port"], cfg["password"], timeout=15)
    except Exception as e:
        print(f"{name} unreachable: {e}")
        sys.exit(1)

print(f"\n  {'game rule':<24}{'vanilla':<12}{'pumpkin':<12}verdict")
print("  " + "-" * 60)

agree = disagree = 0
for rule in RULES:
    v = ok(conns["vanilla"].cmd(f"gamerule {rule}"))
    p = ok(conns["pumpkin"].cmd(f"gamerule {rule}"))
    if v == p:
        verdict, agree = "same", agree + 1
    else:
        verdict = "MISMATCH" if v else "pumpkin accepts a rule vanilla rejects"
        disagree += 1
    print(f"  {rule:<24}{str(v):<12}{str(p):<12}{verdict}")

print(f"\n  {agree} agree, {disagree} differ")
for c in conns.values():
    c.close()
