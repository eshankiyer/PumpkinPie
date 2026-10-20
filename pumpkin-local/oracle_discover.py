#!/usr/bin/env python3
"""
Autonomous gap discovery via the differential oracle.

This is the engine that grows the parity list without guessing. It asks the
same thing of vanilla and Pumpkin over RCON and records where they differ.
Because vanilla's answer is OBSERVED, not assumed, a discovered gap satisfies
the project's cardinal rule (never guess vanilla) by construction - the
evidence is the vanilla server's own live response.

Output goes to DISCOVERED.md, NOT straight into WORKQUEUE.md. Two reasons:
the curated queue stays hand-trusted, and not every diff is a bug - config
values (max-players), cosmetic error-message formatting, and unimplemented
commands are triaged differently from real behaviour gaps. A human or a
stronger model promotes DISCOVERED items into WORKQUEUE. Discovery proposes
with evidence; it does not self-confirm and does not self-fix.

Categories are probe generators; add more over time and breadth grows. Start
with the two that need no world state: gamerules and the command surface.

    python3 oracle_discover.py                 # all categories
    python3 oracle_discover.py --category gamerule
"""

import argparse
import datetime
import os
import re

from nuc_probe import VANILLA, rcon

PUMPKIN = ("192.168.1.93", 25575, "pumpkintest")
HERE = os.path.dirname(os.path.abspath(__file__))
LEDGER = os.path.join(HERE, "DISCOVERED.md")

# The MC 26.2 gamerule set. Probing each on both servers finds rules Pumpkin
# does not know (vanilla answers, Pumpkin errors) and default-value drift.
GAMERULES = [
    "announceAdvancements", "blockExplosionDropDecay", "commandBlockOutput",
    "commandModificationBlockLimit", "disableElytraMovementCheck",
    "disablePlayerMovementCheck", "disableRaids", "doDaylightCycle",
    "doEntityDrops", "doFireTick", "doImmediateRespawn", "doInsomnia",
    "doLimitedCrafting", "doMobLoot", "doMobSpawning", "doPatrolSpawning",
    "doTileDrops", "doTraderSpawning", "doVinesSpread", "doWardenSpawning",
    "doWeatherCycle", "drowningDamage", "enderPearlsVanishOnDeath",
    "fallDamage", "fireDamage", "forgiveDeadPlayers", "freezeDamage",
    "globalSoundEvents", "keepInventory", "lavaSourceConversion",
    "logAdminCommands", "maxCommandChainLength", "maxCommandForkCount",
    "maxEntityCramming", "mobExplosionDropDecay", "mobGriefing",
    "naturalRegeneration", "playersNetherPortalCreativeDelay",
    "playersNetherPortalDefaultDelay", "playersSleepingPercentage",
    "projectilesCanBreakBlocks", "pvp", "randomTickSpeed", "reducedDebugInfo",
    "sendCommandFeedback", "showDeathMessages", "snowAccumulationHeight",
    "spawnChunkRadius", "spawnRadius", "spectatorsGenerateChunks",
    "tntExplosionDropDecay", "universalAnger", "waterSourceConversion",
]

# Command surface: send each name alone. Vanilla knowing it while Pumpkin
# says "Unknown command" is a missing-command gap. Both erroring the same way
# is parity.
COMMANDS = [
    "advancement", "attribute", "bossbar", "clear", "clone", "damage", "data",
    "datapack", "debug", "defaultgamemode", "difficulty", "effect", "enchant",
    "execute", "experience", "fill", "fillbiome", "forceload", "function",
    "gamemode", "gamerule", "give", "item", "jfr", "kill", "list", "locate",
    "loot", "particle", "place", "playsound", "random", "recipe", "ride",
    "rotate", "schedule", "scoreboard", "seed", "setblock", "setworldspawn",
    "spawnpoint", "spectate", "spreadplayers", "stopsound", "summon", "tag",
    "team", "teleport", "tellraw", "tick", "time", "title", "transfer",
    "weather", "worldborder", "xp",
]


UNKNOWN = re.compile(r"unknown command|incorrect argument|can't find|no such", re.I)


def _both(cmd):
    try:
        v = rcon(*VANILLA, cmd)
    except OSError as e:
        v = f"<vanilla unreachable: {e}>"
    try:
        p = rcon(*PUMPKIN, cmd)
    except OSError as e:
        p = f"<pumpkin unreachable: {e}>"
    return v, p


def discover_gamerules():
    """Rules vanilla knows that Pumpkin does not, or whose values drift."""
    out = []
    for g in GAMERULES:
        v, p = _both(f"gamerule {g}")
        v_known = "unknown" not in v.lower() and "incorrect" not in v.lower()
        p_known = "unknown" not in p.lower() and "incorrect" not in p.lower()
        if v_known and not p_known:
            out.append(("gamerule", g, "Pumpkin does not implement this gamerule",
                        v, p, "high"))
        elif v_known and p_known and v != p:
            out.append(("gamerule", g, "gamerule value/output differs", v, p, "med"))
    return out


def discover_commands():
    """Commands vanilla recognises that Pumpkin reports as unknown."""
    out = []
    for c in COMMANDS:
        v, p = _both(c)
        v_unknown = "unknown command" in v.lower()
        p_unknown = "unknown command" in p.lower()
        if not v_unknown and p_unknown:
            out.append(("command", c, "Pumpkin does not implement this command",
                        v, p, "high"))
    return out


CATEGORIES = {"gamerule": discover_gamerules, "command": discover_commands}


def load_seen():
    if not os.path.exists(LEDGER):
        return set()
    return set(re.findall(r"^- \[[ x]\] `([^`]+)`", open(LEDGER).read(), re.M))


def append(findings):
    """Append only genuinely new findings, as an evidenced, unchecked list."""
    seen = load_seen()
    new = [f for f in findings if f"{f[0]}:{f[1]}" not in seen]
    if not new:
        return 0
    fresh = not os.path.exists(LEDGER)
    with open(LEDGER, "a") as fh:
        if fresh:
            fh.write(
                "# Discovered parity gaps (oracle-observed)\n\n"
                "Auto-filed by oracle_discover.py. Each line is a live "
                "vanilla-vs-Pumpkin difference; vanilla output is observed, "
                "not guessed. Promote real behaviour gaps into WORKQUEUE.md as "
                "confirmed (the evidence is already here); drop cosmetic or "
                "config-only diffs. NOT auto-fixed.\n\n"
            )
        stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M")
        for cat, key, what, v, p, sev in new:
            fh.write(
                f"- [ ] `{cat}:{key}` ({sev}, {stamp}) - {what}\n"
                f"    - vanilla: `{v[:160]}`\n"
                f"    - pumpkin: `{p[:160]}`\n"
            )
    return len(new)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--category", choices=list(CATEGORIES) + ["all"], default="all")
    a = ap.parse_args()
    cats = list(CATEGORIES) if a.category == "all" else [a.category]

    findings = []
    for c in cats:
        f = CATEGORIES[c]()
        print(f"{c}: {len(f)} difference(s)", flush=True)
        findings += f
    n = append(findings)
    print(f"\n{len(findings)} diffs found, {n} new filed to {LEDGER}", flush=True)


if __name__ == "__main__":
    main()
