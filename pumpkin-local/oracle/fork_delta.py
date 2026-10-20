#!/usr/bin/env python3
"""Three-way differential parity probe: vanilla vs upstream Pumpkin vs PumpkinPie.

Why this exists
---------------
`score.py` reports 46.58/100 for BOTH upstream Pumpkin and PumpkinPie, component for
component. That is a fact about the instrument, not the code: its three components are
command registration over a 19-command sample, four probes (gamerules, command names,
one setblock, difficulty), and a markdown checklist. The fork's ~788 commits are mob AI,
worldgen, block behaviour, chunk serialization and entity metadata - none of which those
components sample.

This probe samples the domains the fork actually changed, against the same vanilla
reference (26.2), so the delta between the two subjects is attributable to the work.

Method
------
Every probe returns a flat dict of field -> value. A subject scores a field if its value
equals vanilla's for that field. Score = matched fields / comparable fields.

Fields where VANILLA itself cannot answer are dropped from the denominator entirely
rather than counted against anyone - otherwise a broken probe inflates or deflates both
subjects and looks like a result. This is enforced at FIELD granularity (see DEAD), not
just at probe granularity: two servers both answering UNSUPPORTED is not agreement.

Traps this deliberately guards against, all of which produced a false finding earlier:
  * Nothing is loaded with no players online, so a setblock probe silently no-ops and
    reads as "block did not survive". Every block probe forceloads first and ABORTS the
    whole probe if the scratch chunk is still unloaded, rather than reporting data.
  * /setblock BYPASSES placement validation - an unsupported block is written anyway and
    only breaks on a neighbour update. Support probes must poke a neighbour afterwards.
  * Leftover /forceload state makes unrelated positions read as loaded. Cleared at start.
  * Both servers failing identically is NOT agreement. Vanilla is the reference; if
    vanilla cannot do it, the field is dropped, not scored as a match.
  * The oracle runs random_tick_speed=1000 for other tests while the Pumpkin rigs run 3.
    NOTHING random-tick-driven may be probed here (crop growth, farmland hydration, ice
    and snow melt, fire spread, leaf decay, sapling growth) - a difference there traces
    to rig config, not to code. Probes below are all command-response or explicit-write.
  * Loot tables with a random component (leaves -> sapling/stick, gravel -> flint, crop
    seed counts, fortune) differ between two runs of the SAME server. Only deterministic
    tables are sampled, and --repeat cross-checks stability.
  * Message WORDING differs legitimately between implementations. Every probe compares an
    extracted value (a number, a PASS/FAIL, a PRESENT/ABSENT), never prose.

Usage
-----
    python3 fork_delta.py [subject ...]      # default: upstream pie
    python3 fork_delta.py --repeat pie       # run twice, flag unstable fields
"""

import json
import re
import sys
import time

from differential import Rcon

ENDPOINTS = {
    "vanilla": ("192.168.1.93", 25576, "oracletest"),
    "upstream": ("127.0.0.1", 25599, "upstreamtest"),
    "pie": ("127.0.0.1", 25598, "pietest"),
}

SCRATCH = (2000, 100, 2000)
UNKNOWN = ("Unknown or incomplete command", "Incorrect argument", "Unknown command",
           "Invalid or unknown", "Expected ", "Can't find element")
# Sentinels meaning "no answer". If VANILLA produces one, the field is dropped entirely.
DEAD = {"UNSUPPORTED", "NO_VALUE", "ABORTED", "", "NOT_LOADED"}


def unsupported(res):
    return any(t in res for t in UNKNOWN)


def _num(text):
    """Last number appearing in a message, normalised. None if there is none."""
    m = re.findall(r"-?\d+(?:\.\d+)?", text.replace(",", " "))
    return f"{float(m[-1]):.4f}" if m else None


def _first_num(text):
    m = re.findall(r"-?\d+(?:\.\d+)?", text.replace(",", " "))
    return f"{float(m[0]):.4f}" if m else None


def _ensure_loaded(r, x, z, tries=8, delay=1.5):
    """Forceload a chunk. True only once a write actually lands.

    Retries: forceload registers a ticket, but the chunk still has to be GENERATED, which
    is not instant. Probing immediately reports "not loaded" and aborts the whole probe -
    a race that looked exactly like the forceload bug it was meant to detect.
    """
    r.cmd(f"forceload add {x} {z}")
    for _ in range(tries):
        probe = r.cmd(f"setblock {x} 100 {z} minecraft:stone")
        if "not loaded" not in probe:
            r.cmd(f"setblock {x} 100 {z} minecraft:air")
            return True
        time.sleep(delay)
    return False


def _clear_items(r):
    r.cmd("kill @e[type=item]")


# --- probes ----------------------------------------------------------------

SUPPORT_CASES = [
    ("minecraft:dripstone_block", +1, "minecraft:pointed_dripstone[vertical_direction=down,thickness=tip]"),
    ("minecraft:stone", -1, "minecraft:pointed_dripstone[vertical_direction=up,thickness=tip]"),
    ("minecraft:stone", -1, "minecraft:torch"),
    ("minecraft:air", -1, "minecraft:torch"),
    ("minecraft:stone", -1, "minecraft:lantern"),
    ("minecraft:air", -1, "minecraft:lantern"),
    ("minecraft:stone", -1, "minecraft:redstone_wire"),
    ("minecraft:air", -1, "minecraft:redstone_wire"),
    ("minecraft:dirt", -1, "minecraft:poppy"),
    ("minecraft:stone", -1, "minecraft:poppy"),
    ("minecraft:sand", -1, "minecraft:cactus"),
    ("minecraft:stone", -1, "minecraft:cactus"),
    ("minecraft:stone", -1, "minecraft:rail"),
    ("minecraft:stone", -1, "minecraft:ladder"),
    ("minecraft:dirt", -1, "minecraft:wheat"),
    ("minecraft:stone", -1, "minecraft:wheat"),
    ("minecraft:farmland", -1, "minecraft:wheat"),
    ("minecraft:stone", -1, "minecraft:sugar_cane"),
    ("minecraft:stone", -1, "minecraft:snow"),
    ("minecraft:air", -1, "minecraft:snow"),
    ("minecraft:stone", -1, "minecraft:lever"),
    ("minecraft:air", -1, "minecraft:lever"),
    ("minecraft:stone", -1, "minecraft:stone_pressure_plate"),
    ("minecraft:air", -1, "minecraft:stone_pressure_plate"),
    ("minecraft:stone", -1, "minecraft:comparator"),
    ("minecraft:air", -1, "minecraft:comparator"),
    ("minecraft:stone", -1, "minecraft:repeater"),
    ("minecraft:air", -1, "minecraft:repeater"),
    ("minecraft:stone", -1, "minecraft:oak_door"),
    ("minecraft:air", -1, "minecraft:oak_door"),
    ("minecraft:dirt", -1, "minecraft:bamboo"),
    ("minecraft:stone", -1, "minecraft:bamboo"),
    ("minecraft:stone", -1, "minecraft:nether_wart"),
    ("minecraft:soul_sand", -1, "minecraft:nether_wart"),
    ("minecraft:end_stone", -1, "minecraft:chorus_flower"),
    ("minecraft:stone", -1, "minecraft:chorus_flower"),
]


def p_block_support(r):
    """Does each block survive placement + a neighbour update, as vanilla decides?"""
    x, y, z = SCRATCH
    if not _ensure_loaded(r, x, z):
        return {"__abort__": "scratch not loaded even after forceload"}
    out = {}
    for i, (support, dy, test_block) in enumerate(SUPPORT_CASES):
        cx, cy, cz = x + (i % 5) * 3, y + (i // 5) * 4, z
        for c in (-1, 0, 1):
            r.cmd(f"setblock {cx} {cy + c} {cz} minecraft:air")
        if support != "minecraft:air":
            r.cmd(f"setblock {cx} {cy + dy} {cz} {support}")
        r.cmd(f"setblock {cx} {cy} {cz} {test_block}")
        # /setblock bypasses validation; force the neighbour update that applies the rule.
        r.cmd(f"setblock {cx + 1} {cy} {cz} minecraft:stone")
        r.cmd(f"setblock {cx + 1} {cy} {cz} minecraft:air")
        bare = test_block.split("[")[0]
        res = r.cmd(f"clone {cx} {cy} {cz} {cx} {cy} {cz} {cx} {cy} {cz + 6} filtered {bare}")
        if unsupported(res) or "not loaded" in res:
            val = "UNSUPPORTED"
        else:
            digits = [t for t in res.replace(",", " ").split() if t.isdigit()]
            val = "SURVIVED" if (digits and digits[0] != "0") else "GONE"
        out[f"support:{bare.split(':')[1]}:{support.split(':')[1]}{dy:+d}"] = val
        r.cmd(f"setblock {cx} {cy} {cz + 6} minecraft:air")
        for c in (-1, 0, 1):
            r.cmd(f"setblock {cx} {cy + c} {cz} minecraft:air")
    return out


MOBS = [
    "zombie", "spider", "pig", "creeper", "slime", "magma_cube", "skeleton", "enderman",
    "witch", "blaze", "ghast", "iron_golem", "wolf", "cow", "sheep", "chicken", "villager",
    "cave_spider", "silverfish", "endermite", "guardian", "elder_guardian", "shulker",
    "wither_skeleton", "husk", "drowned", "stray", "phantom", "vindicator", "evoker",
    "pillager", "ravager", "hoglin", "zoglin", "piglin", "piglin_brute", "warden",
    "axolotl", "goat", "bee", "dolphin", "turtle", "panda", "polar_bear", "llama",
    "horse", "donkey", "camel", "fox", "cat", "ocelot", "rabbit", "bat", "squid",
    "glow_squid", "cod", "salmon", "pufferfish", "tropical_fish", "strider", "allay",
    "frog", "tadpole", "sniffer", "armadillo", "breeze", "bogged", "creaking",
]
ATTRS = ["movement_speed", "max_health", "attack_damage", "armor", "follow_range",
         "knockback_resistance", "attack_knockback", "step_height", "scale",
         "safe_fall_distance", "gravity", "oxygen_bonus", "burning_time"]


def p_mob_attributes(r):
    """Base attributes per mob. Summons into forceloaded scratch, reads, kills.

    Base values, not observed motion - entities do not tick with no player online, so
    anything motion-derived would be unmeasurable rather than merely slow.

    NOTE: selectors use the BARE type name. Namespaced `type=minecraft:zombie` is
    rejected by Pumpkin (see p_selector_surface), which would make every field here read
    UNSUPPORTED on both subjects - dozens of dead fields masquerading as data, exactly
    the failure this harness exists to prevent.
    """
    x, y, z = SCRATCH
    if not _ensure_loaded(r, x, z):
        return {"__abort__": "scratch not loaded even after forceload"}
    out = {}
    for mob in MOBS:
        r.cmd(f"kill @e[type={mob}]")
        spawned = r.cmd(f"summon minecraft:{mob} {x} {y} {z}")
        if unsupported(spawned) or "Unable" in spawned:
            out[f"{mob}.summon"] = "UNSUPPORTED"
            continue
        out[f"{mob}.summon"] = "OK"
        for attr in ATTRS:
            res = r.cmd(f"attribute @e[type={mob},limit=1] minecraft:{attr} base get")
            if unsupported(res) or "No entity" in res:
                out[f"{mob}.{attr}"] = "UNSUPPORTED"
            else:
                out[f"{mob}.{attr}"] = _num(res) or "NO_VALUE"
        r.cmd(f"kill @e[type={mob}]")
    return out


# Deterministic loot tables only - no fortune, no random-count or random-chance rolls.
LOOT_BLOCKS = [
    "stone", "granite", "cobblestone", "deepslate", "iron_ore", "deepslate_iron_ore",
    "gold_ore", "deepslate_gold_ore", "copper_ore", "deepslate_copper_ore",
    "coal_ore", "deepslate_coal_ore", "diamond_ore", "deepslate_diamond_ore",
    "emerald_ore", "nether_quartz_ore", "nether_gold_ore", "ancient_debris",
    "glass", "white_stained_glass", "glass_pane", "ice", "packed_ice", "blue_ice",
    "grass_block", "dirt", "podzol", "farmland", "sand", "red_sand", "clay",
    "oak_log", "stripped_oak_log", "bookshelf", "glowstone", "sea_lantern",
    "melon", "sponge", "wet_sponge", "cobweb", "snow_block", "obsidian",
    "raw_iron_block", "amethyst_cluster", "budding_amethyst", "spawner",
    "infested_stone", "chorus_flower", "bamboo", "cactus", "sugar_cane",
    "torch", "redstone_torch", "ladder", "rail", "chest", "furnace", "hopper",
    "sculk", "sculk_catalyst", "pointed_dripstone", "moss_block", "mud",
    "netherrack", "soul_sand", "magma_block", "end_stone", "purpur_block",
]


def p_block_drops(r):
    """Item count dropped by `loot spawn ... mine <pos>` for deterministic loot tables.

    Only the COUNT is compared: vanilla names the item in its message and Pumpkin does
    not, so item identity is not observable on both sides and is therefore not scored.
    """
    x, y, z = 2000, 110, 2000
    if not _ensure_loaded(r, x, z):
        return {"__abort__": "scratch not loaded even after forceload"}
    out = {}
    for b in LOOT_BLOCKS:
        src = (x, y + 2, z)
        placed = r.cmd(f"setblock {src[0]} {src[1]} {src[2]} minecraft:{b}")
        if unsupported(placed):
            out[f"drop:{b}"] = "UNSUPPORTED"
            continue
        res = r.cmd(f"loot spawn {x} {y} {z} mine {src[0]} {src[1]} {src[2]}")
        if unsupported(res):
            out[f"drop:{b}"] = "UNSUPPORTED"
        elif "No items" in res or "no items" in res:
            out[f"drop:{b}"] = "0.0000"
        else:
            out[f"drop:{b}"] = _first_num(res) or "NO_VALUE"
        _clear_items(r)
        r.cmd(f"setblock {src[0]} {src[1]} {src[2]} minecraft:air")
    return out


def p_fill_clone_semantics(r):
    """Counts returned by fill/clone/setblock, including the no-op and replace cases.

    Vanilla counts only blocks it actually CHANGED; a fill over identical blocks reports
    zero. That is a value, not wording, so it is comparable.
    """
    x, y, z = 2100, 100, 2100
    if not _ensure_loaded(r, x, z):
        return {"__abort__": "scratch not loaded even after forceload"}
    out = {}

    def count(res, key):
        if unsupported(res) or "not loaded" in res:
            out[key] = "UNSUPPORTED"
        elif "No blocks" in res or "no blocks" in res:
            out[key] = "0.0000"
        else:
            out[key] = _first_num(res) or "NO_VALUE"

    r.cmd(f"fill {x} {y} {z} {x+2} {y} {z+2} minecraft:air")
    count(r.cmd(f"fill {x} {y} {z} {x+2} {y} {z+2} minecraft:stone"), "fill.fresh_9")
    count(r.cmd(f"fill {x} {y} {z} {x+2} {y} {z+2} minecraft:stone"), "fill.noop_same_block")
    count(r.cmd(f"fill {x} {y} {z} {x+2} {y} {z+2} minecraft:dirt replace minecraft:stone"),
          "fill.replace_matching")
    count(r.cmd(f"fill {x} {y} {z} {x+2} {y} {z+2} minecraft:gold_block replace minecraft:stone"),
          "fill.replace_nonmatching")
    count(r.cmd(f"fill {x} {y} {z} {x+2} {y} {z+2} minecraft:stone keep"), "fill.keep_over_solid")
    r.cmd(f"fill {x} {y} {z} {x+2} {y} {z+2} minecraft:air")
    count(r.cmd(f"fill {x} {y} {z} {x+2} {y} {z+2} minecraft:stone keep"), "fill.keep_over_air")
    count(r.cmd(f"fill {x} {y} {z} {x+2} {y} {z+2} minecraft:air outline"), "fill.outline")
    r.cmd(f"fill {x} {y} {z} {x+2} {y} {z+2} minecraft:stone")
    r.cmd(f"setblock {x+1} {y} {z+1} minecraft:gold_block")
    count(r.cmd(f"clone {x} {y} {z} {x+2} {y} {z+2} {x+10} {y} {z} filtered minecraft:gold_block"),
          "clone.filtered_one")
    count(r.cmd(f"clone {x} {y} {z} {x+2} {y} {z+2} {x+20} {y} {z} filtered minecraft:stone"),
          "clone.filtered_eight")
    count(r.cmd(f"clone {x} {y} {z} {x+2} {y} {z+2} {x+30} {y} {z} replace"), "clone.replace_all")
    count(r.cmd(f"clone {x} {y} {z} {x+2} {y} {z+2} {x+40} {y} {z} masked"), "clone.masked")

    sb = r.cmd(f"setblock {x} {y} {z} minecraft:stone")
    out["setblock.same_block_again"] = "FAILED" if ("Could not" in sb or "not change" in sb) else "OK"
    sb = r.cmd(f"setblock {x} {y} {z} minecraft:dirt keep")
    out["setblock.keep_over_solid"] = "FAILED" if ("Could not" in sb or "not change" in sb) else "OK"

    for k in range(0, 50, 10):
        r.cmd(f"fill {x+k} {y} {z} {x+k+2} {y} {z+2} minecraft:air")
    return out


def p_command_results(r):
    """World-state commands: compare the VALUE they read back, not their message."""
    out = {}

    def val(key, cmd, extract=_num):
        res = r.cmd(cmd)
        out[key] = "UNSUPPORTED" if unsupported(res) else (extract(res) or "NO_VALUE")

    r.cmd("worldborder set 200")
    val("worldborder.set_get", "worldborder get")
    r.cmd("worldborder add 100")
    val("worldborder.after_add", "worldborder get")
    r.cmd("worldborder set 59999968")
    val("worldborder.reset", "worldborder get")
    for sub in ["worldborder damage amount 0.2", "worldborder damage buffer 5",
                "worldborder warning distance 5", "worldborder warning time 15",
                "worldborder center 0 0"]:
        res = r.cmd(sub)
        out["cmd:" + sub.replace(" ", "_")] = "UNSUPPORTED" if unsupported(res) else "OK"

    for t in ["0", "6000", "13000", "18000", "23999"]:
        r.cmd(f"time set {t}")
        val(f"time.set_{t}_query_day", "time query day")
    r.cmd("time set 1000")

    for d in ["peaceful", "easy", "normal", "hard"]:
        r.cmd(f"difficulty {d}")
        res = r.cmd("difficulty")
        out[f"difficulty.roundtrip_{d}"] = (
            "UNSUPPORTED" if unsupported(res) else res.rstrip(".").split()[-1].lower()
        )
    r.cmd("difficulty normal")

    for w in ["clear", "rain", "thunder"]:
        res = r.cmd(f"weather {w}")
        out[f"weather.{w}"] = "UNSUPPORTED" if unsupported(res) else "OK"
    r.cmd("weather clear")

    r.cmd("scoreboard objectives remove fdz")
    r.cmd("scoreboard objectives add fdz dummy")
    r.cmd("scoreboard players set A fdz 17")
    r.cmd("scoreboard players set B fdz 5")
    val("scoreboard.get", "scoreboard players get A fdz")
    r.cmd("scoreboard players add A fdz 3")
    val("scoreboard.after_add", "scoreboard players get A fdz")
    r.cmd("scoreboard players remove A fdz 4")
    val("scoreboard.after_remove", "scoreboard players get A fdz")
    for op, key in [("+=", "op_add"), ("-=", "op_sub"), ("*=", "op_mul"),
                    ("/=", "op_div"), ("%=", "op_mod"), ("<", "op_min"),
                    (">", "op_max"), ("><", "op_swap")]:
        r.cmd("scoreboard players set A fdz 17")
        r.cmd("scoreboard players set B fdz 5")
        res = r.cmd(f"scoreboard players operation A fdz {op} B fdz")
        if unsupported(res):
            out[f"scoreboard.{key}"] = "UNSUPPORTED"
        else:
            val(f"scoreboard.{key}", "scoreboard players get A fdz")
    r.cmd("scoreboard players set A fdz -9")
    r.cmd("scoreboard players set B fdz 4")
    r.cmd("scoreboard players operation A fdz /= B fdz")
    val("scoreboard.negative_floor_div", "scoreboard players get A fdz")
    r.cmd("scoreboard players set A fdz -9")
    r.cmd("scoreboard players operation A fdz %= B fdz")
    val("scoreboard.negative_mod", "scoreboard players get A fdz")
    res = r.cmd("scoreboard players reset A fdz")
    out["scoreboard.reset_then_get"] = (
        "NONE" if "No score" in r.cmd("scoreboard players get A fdz") else "STILL_SET"
    )
    r.cmd("scoreboard objectives remove fdz")
    return out


def p_execute_surface(r):
    """`execute if/unless` subcommands: does the PREDICATE actually discriminate?

    Every field is a TRUE case and a FALSE case. A server that always passes, always
    fails, or does not know the subcommand is distinguishable from one that works.
    """
    x, y, z = 2200, 100, 2200
    if not _ensure_loaded(r, x, z):
        return {"__abort__": "scratch not loaded even after forceload"}
    r.cmd(f"setblock {x} {y} {z} minecraft:stone")
    r.cmd(f"setblock {x} {y+1} {z} minecraft:air")
    r.cmd(f"setblock {x} {y+2} {z} minecraft:oak_door[half=lower,open=true]")
    r.cmd("kill @e[type=zombie]")
    r.cmd(f"summon minecraft:zombie {x} {y+4} {z}")
    out = {}

    def verdict(key, true_cmd, false_cmd):
        a, b = r.cmd(true_cmd), r.cmd(false_cmd)
        if unsupported(a) or unsupported(b):
            out[key] = "UNSUPPORTED"
        else:
            ta = "passed" in a.lower() or "Count" in a
            tb = "passed" in b.lower() or "Count" in b
            out[key] = {(True, False): "DISCRIMINATES", (True, True): "ALWAYS_PASS",
                        (False, False): "ALWAYS_FAIL",
                        (False, True): "INVERTED"}[(ta, tb)]

    verdict("execute.if_block",
            f"execute if block {x} {y} {z} minecraft:stone",
            f"execute if block {x} {y} {z} minecraft:air")
    verdict("execute.if_block_unnamespaced",
            f"execute if block {x} {y} {z} stone",
            f"execute if block {x} {y} {z} air")
    verdict("execute.if_block_blockstate",
            f"execute if block {x} {y+2} {z} minecraft:oak_door[open=true]",
            f"execute if block {x} {y+2} {z} minecraft:oak_door[open=false]")
    verdict("execute.if_block_tag",
            f"execute if block {x} {y} {z} #minecraft:base_stone_overworld",
            f"execute if block {x} {y+1} {z} #minecraft:base_stone_overworld")
    verdict("execute.unless_block",
            f"execute unless block {x} {y} {z} minecraft:air",
            f"execute unless block {x} {y} {z} minecraft:stone")
    verdict("execute.if_blocks",
            f"execute if blocks {x} {y} {z} {x} {y} {z} {x} {y} {z} all",
            f"execute if blocks {x} {y} {z} {x} {y} {z} {x} {y+1} {z} all")
    verdict("execute.if_entity_type",
            "execute if entity @e[type=zombie]",
            "execute if entity @e[type=creeper]")
    verdict("execute.if_entity_type_namespaced",
            "execute if entity @e[type=minecraft:zombie]",
            "execute if entity @e[type=minecraft:creeper]")
    verdict("execute.if_entity_tag_selector",
            "execute if entity @e[type=#minecraft:zombies]",
            "execute if entity @e[type=#minecraft:skeletons]")
    verdict("execute.if_entity_nbt",
            "execute if entity @e[type=zombie,nbt={OnGround:0b}]",
            "execute if entity @e[type=zombie,nbt={Invulnerable:1b}]")
    verdict("execute.if_score",
            "execute if score A fdzx matches 1..",
            "execute if score A fdzx matches 1..")

    # `run` must actually gate the inner command, not merely parse.
    r.cmd(f"setblock {x+1} {y} {z} minecraft:air")
    r.cmd(f"execute if block {x} {y} {z} minecraft:stone run setblock {x+1} {y} {z} minecraft:gold_block")
    res = r.cmd(f"clone {x+1} {y} {z} {x+1} {y} {z} {x+1} {y} {z+8} filtered minecraft:gold_block")
    ran_true = "UNSUPPORTED" if unsupported(res) else (
        "RAN" if (_first_num(res) or "0") not in ("0", "0.0000") else "SKIPPED")
    out["execute.run_gated_true"] = ran_true
    r.cmd(f"setblock {x+1} {y} {z+8} minecraft:air")
    r.cmd(f"setblock {x+1} {y} {z} minecraft:air")
    r.cmd(f"execute if block {x} {y} {z} minecraft:diamond_block run setblock {x+1} {y} {z} minecraft:gold_block")
    res = r.cmd(f"clone {x+1} {y} {z} {x+1} {y} {z} {x+1} {y} {z+8} filtered minecraft:gold_block")
    out["execute.run_gated_false"] = "UNSUPPORTED" if unsupported(res) else (
        "RAN" if (_first_num(res) or "0") not in ("0", "0.0000") else "SKIPPED")
    r.cmd(f"setblock {x+1} {y} {z+8} minecraft:air")
    r.cmd("kill @e[type=zombie]")
    return out


def p_selector_surface(r):
    """Entity-selector syntax acceptance, each verified to actually select."""
    x, y, z = 2200, 100, 2200
    if not _ensure_loaded(r, x, z):
        return {"__abort__": "scratch not loaded even after forceload"}
    r.cmd("kill @e[type=zombie]")
    r.cmd("kill @e[type=skeleton]")
    r.cmd(f"summon minecraft:zombie {x} {y+4} {z}")
    r.cmd(f"summon minecraft:zombie {x} {y+4} {z}")
    r.cmd(f"summon minecraft:skeleton {x} {y+4} {z}")
    out = {}
    sels = {
        "bare_type": "@e[type=zombie]",
        "namespaced_type": "@e[type=minecraft:zombie]",
        "tag_type": "@e[type=#minecraft:zombies]",
        "negated_type": "@e[type=!zombie,type=!item]",
        "limit": "@e[type=zombie,limit=1]",
        "sort_nearest": "@e[type=zombie,sort=nearest,limit=1]",
        "distance": f"@e[type=zombie,distance=..100000]",
        "nbt": "@e[type=zombie,nbt={OnGround:0b}]",
        "name": "@e[type=zombie,name=nope]",
        "scores": "@e[type=zombie,scores={fdzx=1..}]",
        "tag_attr": "@e[type=zombie,tag=nope]",
        "predicate": "@e[type=zombie,predicate=minecraft:nope]",
        "x_y_z_dx": f"@e[type=zombie,x={x-5},y={y},z={z-5},dx=10,dy=10,dz=10]",
    }
    for key, sel in sels.items():
        # `attribute ... get` is the read primitive: it errors on a bad selector and
        # reports "No entity was found" on a well-formed selector that matches nothing.
        res = r.cmd(f"attribute {sel.replace(']', ',limit=1]')} minecraft:max_health get")
        if unsupported(res):
            out[f"selector.{key}"] = "REJECTED"
        elif "No entity" in res:
            out[f"selector.{key}"] = "ACCEPTED_EMPTY"
        else:
            out[f"selector.{key}"] = "ACCEPTED_MATCH"
    r.cmd("kill @e[type=zombie]")
    r.cmd("kill @e[type=skeleton]")
    return out


def p_effects_and_modifiers(r):
    """effect give -> read the resulting attribute total, and modifier arithmetic.

    Compares NUMBERS (post-modifier attribute totals), which is the only part of the
    effect system observable over RCON with no players online.
    """
    x, y, z = 2200, 100, 2200
    if not _ensure_loaded(r, x, z):
        return {"__abort__": "scratch not loaded even after forceload"}
    out = {}
    r.cmd("kill @e[type=zombie]")
    r.cmd(f"summon minecraft:zombie {x} {y+4} {z}")
    sel = "@e[type=zombie,limit=1]"

    def get(key, attr, mode="get"):
        res = r.cmd(f"attribute {sel} minecraft:{attr} {mode}")
        out[key] = "UNSUPPORTED" if (unsupported(res) or "No entity" in res) else (
            _num(res) or "NO_VALUE")

    get("effect.speed_base_before", "movement_speed", "base get")
    get("effect.speed_total_before", "movement_speed")
    for amp, key in [(0, "I"), (1, "II"), (4, "V")]:
        res = r.cmd(f"effect give {sel} minecraft:speed 100 {amp}")
        if unsupported(res):
            out[f"effect.speed_{key}_total"] = "UNSUPPORTED"
        else:
            get(f"effect.speed_{key}_total", "movement_speed")
    r.cmd(f"effect clear {sel} minecraft:speed")
    get("effect.speed_total_after_clear", "movement_speed")
    # vanilla merge rule: a WEAKER reapplication must not overwrite a stronger effect
    r.cmd(f"effect give {sel} minecraft:speed 100 4")
    r.cmd(f"effect give {sel} minecraft:speed 100 0")
    get("effect.speed_weak_over_strong", "movement_speed")
    r.cmd(f"effect clear {sel}")
    for eff, attr, key in [("health_boost", "max_health", "healthboost"),
                           ("resistance", "armor", "resistance_armor"),
                           ("slowness", "movement_speed", "slowness"),
                           ("strength", "attack_damage", "strength"),
                           ("weakness", "attack_damage", "weakness"),
                           ("luck", "luck", "luck")]:
        r.cmd(f"effect clear {sel}")
        res = r.cmd(f"effect give {sel} minecraft:{eff} 100 0")
        if unsupported(res):
            out[f"effect.{key}"] = "UNSUPPORTED"
        else:
            get(f"effect.{key}", attr)
    r.cmd(f"effect clear {sel}")

    mres = r.cmd(f"attribute {sel} minecraft:max_health modifier add "
                 "minecraft:fdz_test 10 add_value")
    if unsupported(mres):
        out["modifier.add_value"] = "UNSUPPORTED"
    else:
        get("modifier.add_value", "max_health")
        get("modifier.base_unchanged", "max_health", "base get")
        res = r.cmd(f"attribute {sel} minecraft:max_health modifier value get minecraft:fdz_test")
        out["modifier.value_get"] = "UNSUPPORTED" if unsupported(res) else (_num(res) or "NO_VALUE")
        r.cmd(f"attribute {sel} minecraft:max_health modifier remove minecraft:fdz_test")
        get("modifier.after_remove", "max_health")
        r.cmd(f"attribute {sel} minecraft:max_health modifier add minecraft:fdz2 0.5 add_multiplied_base")
        get("modifier.add_multiplied_base", "max_health")
        r.cmd(f"attribute {sel} minecraft:max_health modifier remove minecraft:fdz2")
        r.cmd(f"attribute {sel} minecraft:max_health modifier add minecraft:fdz3 0.5 add_multiplied_total")
        get("modifier.add_multiplied_total", "max_health")
        r.cmd(f"attribute {sel} minecraft:max_health modifier remove minecraft:fdz3")
    r.cmd(f"attribute {sel} minecraft:max_health base set 30")
    get("modifier.base_set", "max_health", "base get")
    r.cmd("kill @e[type=zombie]")
    return out


def p_gamerule_surface(r):
    """Which gamerules exist, and with what value."""
    # 26.2 gamerules are snake_case. camelCase names are rejected by VANILLA too, which
    # makes every subject report ABSENT and score a free match - the exact false-agreement
    # this script exists to prevent.
    rules = [
        "fire_damage", "mob_griefing", "do_insomnia",
        "players_sleeping_percentage", "do_vines_spread", "tnt_explosion_drop_decay",
        "snow_accumulation_height", "water_source_conversion", "lava_source_conversion",
        "block_explosion_drop_decay", "do_warden_spawning", "do_fire_tick",
        "keep_inventory", "do_mob_spawning", "do_daylight_cycle", "fall_damage",
        "natural_regeneration", "reduced_debug_info", "spawn_radius",
        "do_mob_loot", "do_tile_drops", "do_entity_drops", "command_block_output",
        "log_admin_commands", "show_death_messages", "send_command_feedback",
        "spectators_generate_chunks", "disable_raids", "do_immediate_respawn",
        "drowning_damage", "freeze_damage", "do_patrol_spawning", "do_trader_spawning",
        "forgive_dead_players", "universal_anger", "max_entity_cramming",
        "max_command_chain_length", "max_command_fork_count", "command_modification_block_limit",
        "global_sound_events", "do_limited_crafting", "announce_advancements",
        "projectiles_can_break_blocks", "player_nether_portal_default_delay",
        "player_nether_portal_creative_delay", "ender_pearls_vanish_on_death",
        "minecart_max_speed", "spawn_chunk_radius", "block_explosion_drop_decay",
        "allow_fire_ticks_away_from_player", "locator_bar", "tnt_explodes",
    ]
    out = {}
    # random_tick_speed is deliberately NOT sampled: the oracle runs 1000 for other
    # tests, so its value is rig config, not parity.
    for rule in rules:
        res = r.cmd(f"gamerule {rule}")
        absent = unsupported(res)
        out[f"gamerule:{rule}"] = "ABSENT" if absent else "PRESENT"
        if not absent:
            out[f"gamerule_value:{rule}"] = res.rstrip(".").split()[-1]
    # a set/get roundtrip catches a gamerule that is registered but not stored
    res = r.cmd("gamerule max_entity_cramming 42")
    if not unsupported(res):
        out["gamerule.roundtrip_set"] = _num(r.cmd("gamerule max_entity_cramming")) or "NO_VALUE"
        r.cmd("gamerule max_entity_cramming 24")
    return out


def p_forceload(r):
    """Does /forceload actually load, or merely claim to?"""
    x, z = 4000, 4000
    r.cmd(f"forceload add {x} {z}")
    q = r.cmd("forceload query")
    wrote = r.cmd(f"setblock {x} 100 {z} minecraft:stone")
    r.cmd(f"setblock {x} 100 {z} minecraft:air")
    r.cmd(f"forceload remove {x} {z}")
    after = r.cmd(f"setblock {x} 100 {z} minecraft:stone")
    return {
        "forceload.command_exists": "NO" if unsupported(q) else "YES",
        "forceload.chunk_actually_loads": "NO" if "not loaded" in wrote else "YES",
        "forceload.remove_releases": "NO" if "not loaded" not in after else "YES",
    }


def p_item_commands(r):
    """give / item / enchant results against a summoned entity (no players online)."""
    x, y, z = 2200, 100, 2200
    if not _ensure_loaded(r, x, z):
        return {"__abort__": "scratch not loaded even after forceload"}
    out = {}
    r.cmd("kill @e[type=zombie]")
    r.cmd(f"summon minecraft:zombie {x} {y+4} {z}")
    sel = "@e[type=zombie,limit=1]"
    for key, cmd in [
        ("item.replace_mainhand", f"item replace entity {sel} weapon.mainhand with minecraft:diamond_sword"),
        ("item.replace_count", f"item replace entity {sel} weapon.offhand with minecraft:stone 17"),
        ("item.replace_head", f"item replace entity {sel} armor.head with minecraft:diamond_helmet"),
        ("item.replace_component", f'item replace entity {sel} weapon.mainhand with minecraft:diamond_sword[minecraft:damage=5]'),
        ("item.modify", f"item modify entity {sel} weapon.mainhand minecraft:nonexistent"),
        ("enchant.valid", f"enchant {sel} minecraft:sharpness 3"),
        ("enchant.too_high_level", f"enchant {sel} minecraft:sharpness 9"),
        ("enchant.wrong_item", f"enchant {sel} minecraft:respiration 1"),
        ("enchant.nonexistent", f"enchant {sel} minecraft:not_an_enchant 1"),
        ("data.get_entity_root", f"data get entity {sel}"),
        ("data.get_entity_path", f"data get entity {sel} Health"),
        ("data.merge_entity", f"data merge entity {sel} {{NoAI:1b}}"),
        ("data.modify_entity", f"data modify entity {sel} Invulnerable set value 1b"),
        ("ride.mount", f"ride {sel} mount {sel}"),
        ("tag.add", f"tag {sel} add fdztag"),
        ("tag.list", f"tag {sel} list"),
        ("damage.cmd", f"damage {sel} 1"),
        ("tp.entity", f"tp {sel} {x} {y+4} {z}"),
        ("rotate.cmd", f"rotate {sel} 90 0"),
        ("spawnpoint.cmd", "spawnpoint @e[limit=1] 0 100 0"),
    ]:
        res = r.cmd(cmd)
        if unsupported(res):
            out[key] = "UNSUPPORTED"
        elif any(t in res for t in ("Failed", "failed", "No entity", "Nothing changed",
                                    "cannot", "Cannot", "not a valid", "does not")):
            out[key] = "REJECTED"
        else:
            out[key] = "OK"
    # data get with a path must return the VALUE, comparable across implementations
    res = r.cmd(f"data get entity {sel} Health")
    out["data.health_value"] = "UNSUPPORTED" if unsupported(res) else (_num(res) or "NO_VALUE")
    res = r.cmd(f"data get entity {sel} Fire")
    out["data.fire_value"] = "UNSUPPORTED" if unsupported(res) else (_num(res) or "NO_VALUE")
    r.cmd("kill @e[type=zombie]")
    return out


PROBES = {
    "block_support": p_block_support,
    "block_drops": p_block_drops,
    "mob_attributes": p_mob_attributes,
    "fill_clone": p_fill_clone_semantics,
    "command_results": p_command_results,
    "execute_surface": p_execute_surface,
    "selector_surface": p_selector_surface,
    "effects": p_effects_and_modifiers,
    "item_commands": p_item_commands,
    "gamerule_surface": p_gamerule_surface,
    "forceload": p_forceload,
}


def measure(name):
    host, port, pw = ENDPOINTS[name]
    r = Rcon(host, port, pw)
    r.cmd("forceload remove all")  # stale tickets contaminate load-state probes
    result = {}
    for pname, fn in PROBES.items():
        try:
            got = fn(r)
        except Exception as exc:  # noqa: BLE001 - report, do not mask
            got = {"__abort__": f"{type(exc).__name__}: {exc}"}
        result[pname] = got
    r.cmd("forceload remove all")
    return result


def flatten(run):
    flat = {}
    for pname, fields in run.items():
        if "__abort__" in fields:
            continue
        for k, v in fields.items():
            flat[f"{pname}/{k}"] = v
    return flat


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    repeat = "--repeat" in sys.argv
    subjects = args or ["upstream", "pie"]
    names = ["vanilla"] + subjects

    runs = [{n: measure(n) for n in names}]
    if repeat:
        runs.append({n: measure(n) for n in names})

    flats = [{n: flatten(runs[i][n]) for n in names} for i in range(len(runs))]

    # A field is UNSTABLE if any single server disagreed with itself between runs. Those
    # are nondeterministic (loot rolls, tick races) and are dropped, not reported.
    unstable = set()
    if repeat:
        for n in names:
            for k, v in flats[0][n].items():
                if flats[1][n].get(k) != v:
                    unstable.add(k)

    van = flats[0]["vanilla"]
    totals = {s: [0, 0] for s in subjects}
    dropped = 0
    mismatches = []
    print(f"{'field':<58} {'vanilla':<14} " + " ".join(f"{s:<14}" for s in subjects))
    for field in sorted(van):
        vval = van[field]
        if vval in DEAD or field in unstable:
            dropped += 1
            continue
        cells = []
        bad = False
        for s in subjects:
            sval = flats[0][s].get(field, "ABORTED")
            cells.append(sval)
            totals[s][1] += 1
            if sval == vval:
                totals[s][0] += 1
            else:
                bad = True
        line = f"{field:<58} {vval:<14} " + " ".join(f"{c:<14}" for c in cells)
        print(line + ("   <-- MISMATCH" if bad else ""))
        if bad:
            mismatches.append((field, vval, dict(zip(subjects, cells))))

    print(f"\ndropped {dropped} fields (vanilla could not answer, or unstable across runs)")
    if repeat and unstable:
        print("unstable fields:", ", ".join(sorted(unstable)[:40]))
    for s in subjects:
        hit, tot = totals[s]
        pct = (100.0 * hit / tot) if tot else 0.0
        print(f"{s:<12} matches vanilla on {hit}/{tot} comparable fields = {pct:.1f}%")

    print(f"\n{len(mismatches)} MISMATCHES")
    for field, vval, cells in mismatches:
        print(f"  {field:<56} vanilla={vval:<14} " +
              " ".join(f"{s}={v}" for s, v in cells.items()))

    json.dump({"runs": runs, "unstable": sorted(unstable),
               "mismatches": [(f, v, c) for f, v, c in mismatches]},
              open("/tmp/fork_delta.json", "w"), indent=1)
    print("raw -> /tmp/fork_delta.json")


if __name__ == "__main__":
    main()
