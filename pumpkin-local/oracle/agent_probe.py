#!/usr/bin/env python3
"""
Read-only probe the coding agent may use to ground its decisions in fact.

The agent has repeatedly failed by inventing what vanilla does. This gives it a
way to look instead of guess, while keeping it on a short leash: every operation
here is read-only or confined to a scratch area, and anything that could damage
a world, a container, or the host is simply not reachable from this file.

Allowed:
    logs [N]                  tail Pumpkin's server log
    logs-grep <pattern>       search Pumpkin's log
    ask <server> <query>      one read-only query, from a fixed whitelist
    gamerule <name>           read the rule on BOTH servers and compare
    blocks <a> <b> <block>    count matching blocks on BOTH, in the scratch area
    diff-gamerules            compare a standard set of rules across both

Not allowed, by construction: setblock outside scratch, kill, stop, op, ban,
save-off, arbitrary command passthrough, anything touching the real world or
the containers.
"""

import argparse
import re
import subprocess
import sys

sys.path.insert(0, "/home/eshanki/pumpkin-local/oracle")
from differential import Rcon, SERVERS

NUC = "eshan@192.168.1.93"
PUMPKIN_CONTAINER = "pumpkin-live"

# Queries the agent may run verbatim. Read-only, no world mutation.
SAFE_QUERIES = {
    "seed",
    "list",
    "time query daytime",
    "time query gametime",
    "difficulty",
    "worldborder get",
}
# Read-only prefixes: a gamerule with no value is a read, with a value is a write.
SAFE_PREFIXES = ("gamerule ",)

# The agent may only touch blocks inside this box, far from anything real.
SCRATCH = {"x": (900, 960), "y": (100, 110), "z": (900, 960)}

STANDARD_RULES = [
    "random_tick_speed",
    "advance_weather",
    "advance_time",
    "block_drops",
    "mob_griefing",
    "keep_inventory",
    "fire_damage",
    "max_entity_cramming",
    "spawn_radius",
    "natural_regeneration",
]


def is_safe(query):
    q = query.strip()
    if q in SAFE_QUERIES:
        return True
    for p in SAFE_PREFIXES:
        if q.startswith(p):
            # `gamerule x` reads; `gamerule x true` writes. Only allow the read.
            return len(q.split()) == 2
    return False


def in_scratch(coord):
    try:
        x, y, z = (int(v) for v in coord.split())
    except ValueError:
        return False
    return all(
        lo <= v <= hi
        for v, (lo, hi) in zip((x, y, z), (SCRATCH["x"], SCRATCH["y"], SCRATCH["z"]))
    )


def connect(name, health_check=True):
    """Connect, and by default refuse to hand back a sick server.

    This matters more than it looks. A measurement was taken against Pumpkin
    while it had exhausted its file descriptors; RCON answered, but with
    nonsense, and the run produced six confident "divergences" that vanished
    entirely after a restart. An oracle that does not check its instrument
    just automates being wrong. Every reading goes through here.
    """
    cfg = SERVERS[name]
    r = Rcon(cfg["host"], cfg["port"], cfg["password"], timeout=20)
    if health_check:
        probe = r.cmd("seed")
        if "Seed:" not in probe:
            r.close()
            raise RuntimeError(
                f"{name} is answering RCON but looks unhealthy: seed returned "
                f"{probe[:80]!r}. Refusing to measure against it."
            )
    return r


def cmd_logs(n):
    out = subprocess.run(
        ["bash", "-c", f"ssh {NUC} 'docker logs --tail {int(n)} {PUMPKIN_CONTAINER} 2>&1'"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    print(out.stdout or out.stderr)


def cmd_logs_grep(pattern):
    if not re.fullmatch(r"[\w \-\.:/\[\]\(\)\*\+\?]{1,80}", pattern):
        sys.exit("pattern rejected: keep it simple and short")
    out = subprocess.run(
        [
            "bash",
            "-c",
            f"ssh {NUC} \"docker logs --tail 4000 {PUMPKIN_CONTAINER} 2>&1 | grep -i -- '{pattern}' | tail -40\"",
        ],
        capture_output=True,
        text=True,
        timeout=90,
    )
    print(out.stdout.strip() or "  no matching log lines")


def cmd_ask(server, query):
    if server not in SERVERS:
        sys.exit(f"unknown server {server}")
    if not is_safe(query):
        sys.exit(
            f"query refused: {query!r} is not in the read-only whitelist.\n"
            "This probe deliberately cannot mutate a world."
        )
    r = connect(server)
    print(r.cmd(query))
    r.close()


def cmd_gamerule(name):
    if not re.fullmatch(r"[A-Za-z_]{1,40}", name):
        sys.exit("bad rule name")
    rows = {}
    for s in SERVERS:
        try:
            r = connect(s)
            rows[s] = r.cmd(f"gamerule {name}")
            r.close()
        except Exception as e:
            rows[s] = f"unreachable: {e}"
    for s, v in rows.items():
        print(f"  {s:<9} {v}")


def cmd_diff_gamerules():
    conns = {}
    for s in SERVERS:
        try:
            conns[s] = connect(s)
        except Exception as e:
            sys.exit(f"{s} unreachable: {e}")
    bad = ("Incorrect argument", "Unknown", "Expected", "<--[HERE]", "Invalid")
    print(f"\n  {'rule':<24}{'vanilla':<10}{'pumpkin':<10}verdict")
    print("  " + "-" * 56)
    for rule in STANDARD_RULES:
        vals = {}
        for s, c in conns.items():
            resp = c.cmd(f"gamerule {rule}")
            vals[s] = "absent" if any(b in resp for b in bad) else "present"
        verdict = "same" if vals["vanilla"] == vals["pumpkin"] else "DIVERGENT"
        print(f"  {rule:<24}{vals['vanilla']:<10}{vals['pumpkin']:<10}{verdict}")
    for c in conns.values():
        c.close()


def cmd_blocks(a, b, block):
    if not (in_scratch(a) and in_scratch(b)):
        sys.exit(
            "coordinates refused: counting is confined to the scratch box "
            f"x{SCRATCH['x']} y{SCRATCH['y']} z{SCRATCH['z']}"
        )
    if not re.fullmatch(r"minecraft:[a-z_]{1,40}", block):
        sys.exit("block must look like minecraft:some_block")
    for s in SERVERS:
        try:
            r = connect(s)
            out = r.cmd(f"clone {a} {b} 940 108 940 filtered {block}")
            n = 0 if "No blocks cloned" in out else next(
                (int(t) for t in out.split() if t.isdigit()), -1
            )
            print(f"  {s:<9} {n}")
            r.close()
        except Exception as e:
            print(f"  {s:<9} unreachable: {e}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="action", required=True)
    p = sub.add_parser("logs"); p.add_argument("n", nargs="?", default=60)
    p = sub.add_parser("logs-grep"); p.add_argument("pattern")
    p = sub.add_parser("ask"); p.add_argument("server"); p.add_argument("query")
    p = sub.add_parser("gamerule"); p.add_argument("name")
    sub.add_parser("diff-gamerules")
    p = sub.add_parser("blocks")
    p.add_argument("a"); p.add_argument("b"); p.add_argument("block")
    a = ap.parse_args()

    {
        "logs": lambda: cmd_logs(a.n),
        "logs-grep": lambda: cmd_logs_grep(a.pattern),
        "ask": lambda: cmd_ask(a.server, a.query),
        "gamerule": lambda: cmd_gamerule(a.name),
        "diff-gamerules": cmd_diff_gamerules,
        "blocks": lambda: cmd_blocks(a.a, a.b, a.block),
    }[a.action]()


if __name__ == "__main__":
    sys.exit(main())
