#!/usr/bin/env python3
"""
Oracle sweep: broaden discovery to MORE behaviour categories per cycle.

oracle_discover.py probes two surfaces (bare gamerule names, bare command
names). This adds categories that go a level deeper and stay mechanically
diffable, so a human never has to eyeball wording:

  selector      target-selector parsing edge cases (@e[type=..], scores, nbt)
  argument      command ARGUMENT validation, not just command existence
  summon_nbt    whether NBT tags on /summon are parsed at all
  block_support world blocks that must pop off without support
  block_state   setblock a block, read it straight back (normalisation)
  worldgen      natural terrain the shared seed must reproduce identically

The trick that keeps this safe to automate is the SAME one the rest of the rig
uses: vanilla is OBSERVED, never guessed. Every probe sends one command
sequence to BOTH servers and records both raw replies. We then normalise each
reply to an OUTCOME CLASS (OK / BAD_ARG / UNKNOWN_ENTITY / NOT_LOADED / ...).
Comparing CLASSES, not text, means "Summoned new Pig" vs "Object successfully
summoned" is correctly read as parity (both OK), while "Killed Pig" vs
"Invalid or unknown entity type 'minecraft'" is correctly read as a gap.

Output, exactly like the rest of discovery:
  - confirmed-shape evidence lines APPEND to DISCOVERED.md (same format as
    oracle_discover / nuc_behavior; prefix `sweep:<cat>:<id>`).
  - WORKQUEUE-format STUBS (STATE suspected, never confirmed) go to
    oracle/sweep-proposals.md. This file NEVER writes into WORKQUEUE.md.

Two probe tiers:
  - chunk-INDEPENDENT categories (selector, argument, summon_nbt) run any time
    both servers answer RCON. They need no loaded world.
  - chunk-DEPENDENT categories (block_support, block_state, worldgen) need
    Pumpkin's spawn chunk held loaded, which on Pumpkin means the holder bot
    (`node ~/pumpkin-bot/holder.js Verifier`) because its forceload is broken.
    If the chunk is not loaded they are SKIPPED and reported, never filed as a
    gap (a NOT_LOADED reply is a rig state, not a parity difference).

Usage:
    python3 oracle_sweep.py                 # all categories, report only
    python3 oracle_sweep.py --file          # also append findings + stubs
    python3 oracle_sweep.py --category selector
    python3 oracle_sweep.py --list
"""

import argparse
import datetime
import os
import re
import sys
import time

# Reuse the robust draining RCON from the oracle package; do NOT reimplement
# it. differential.Rcon concatenates multi-packet Pumpkin replies, which a
# naive one-packet read gets silently wrong (see its cmd() docstring).
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "oracle"))
from differential import SERVERS, Rcon  # noqa: E402

LEDGER = os.path.join(HERE, "DISCOVERED.md")
PROPOSALS = os.path.join(HERE, "oracle", "sweep-proposals.md")

# A scratch cell + dst well inside the spawn chunk (x,z in 0..15), high enough
# to miss terrain. Mirrors nuc_behavior's convention.
SCRATCH = "1 110 1"
DST = "13 200 13"


# --------------------------------------------------------------------------
# Outcome classification. The point of the whole sweep: compare CLASSES, not
# raw text, so cosmetic wording never fabricates a gap and a genuine class
# change (accepted vs rejected vs mis-parsed) is always caught.
#
# Order matters and error markers are checked BEFORE success. RCON rejections
# ECHO the offending command with a `<--[HERE]` caret, and that echo carries
# the command's own digits and `key=value` pairs. An early "looks successful"
# check keyed on digits therefore mislabels rejections as OK (a real bug this
# code hit: `kill @e[...,limit=0]<--[HERE]` matched an `=0` success pattern).
# So: decide rejection first, and treat "no error marker at all" as success -
# an accepted command returns a normal informational line, never a caret.
# --------------------------------------------------------------------------
# A transport failure (timeout, reset, our own <server error: ...> wrapper) is
# NOT a game outcome. It must never read as SUCCESS or seed a divergence, or a
# flaky LAN packet becomes a phantom parity gap.
_ERROR = re.compile(r"<\w+ error:|timed out|timeout|unreachable|connection|desync", re.I)
_NOT_LOADED = re.compile(r"not loaded|could not load", re.I)
_UNKNOWN_ENTITY = re.compile(r"invalid or unknown entity|unknown entity type", re.I)
_UNKNOWN_CMD = re.compile(r"unknown command|unknown or incomplete", re.I)
# Any signal the command was refused, at parse time or run time. Lumped on
# purpose: distinguishing parse-reject from runtime-reject produced cosmetic
# divergences (both servers refusing bad input in different words), whereas the
# signal we want is SUCCESS-vs-REJECTED and REJECTED-vs-UNKNOWN_*.
_REJECTED = re.compile(
    r"<--\[HERE\]|incorrect argument|expected|must be|does not exist|"
    r"cannot|can't|could not|no such|not a valid|invalid|failed|"
    r"out of range|not allowed|permission|unparsed|parse", re.I)


def classify(text):
    """Normalise an RCON reply to a coarse outcome class.

    Buckets: NOT_LOADED, UNKNOWN_ENTITY, UNKNOWN_CMD, REJECTED, EMPTY, SUCCESS.
    SUCCESS is the default for any non-empty reply carrying no error marker,
    because an accepted command answers with an ordinary informational line.
    """
    t = (text or "").strip()
    if not t:
        return "EMPTY"
    if _ERROR.search(t):
        return "ERROR"
    if _NOT_LOADED.search(t):
        return "NOT_LOADED"
    if _UNKNOWN_ENTITY.search(t):
        return "UNKNOWN_ENTITY"
    if _UNKNOWN_CMD.search(t):
        return "UNKNOWN_CMD"
    if _REJECTED.search(t):
        return "REJECTED"
    return "SUCCESS"


def _cmd_once(R, name, cmd):
    """One command on one server, with a single retry: a lone LAN timeout must
    not masquerade as a behaviour difference, so give it a second try before
    recording a transport ERROR."""
    for attempt in (1, 2):
        try:
            return R[name].cmd(cmd)
        except Exception as e:  # noqa: BLE001 - report, never crash the sweep
            if attempt == 2:
                return f"<{name} error: {e}>"
            time.sleep(0.5)


def probe(R, cmd):
    """Run one command on both servers; return the paired raw+class record."""
    rec = {}
    for name in ("vanilla", "pumpkin"):
        raw = _cmd_once(R, name, cmd)
        rec[name] = {"raw": raw, "class": classify(raw)}
    rec["cmd"] = cmd
    rec["diverged"] = rec["vanilla"]["class"] != rec["pumpkin"]["class"]
    return rec


def present(R, name, pos, block):
    """Block-presence as an integer observable via clone-count (the rig's
    workhorse; blockstate reads are unreliable, see WORKQUEUE B1). 1 present,
    0 absent, -1 unreadable (e.g. chunk not loaded or a transport error)."""
    raw = _cmd_once(R, name, f"clone {pos} {pos} {DST} filtered {block}")
    if re.search(r"not loaded", raw, re.I) or _ERROR.search(raw):
        return -1, raw
    if re.search(r"no blocks? .*clon|clon.* no blocks?", raw, re.I):
        return 0, raw
    m = re.search(r"(\d+)\s+block", raw)
    return (int(m.group(1)), raw) if m else (-1, raw)


def _finding(cat, pid, desc, seq, v_raw, v_obs, p_raw, p_obs):
    return {
        "category": cat, "id": pid, "desc": desc, "seq": seq,
        "vanilla": {"raw": v_raw, "obs": v_obs},
        "pumpkin": {"raw": p_raw, "obs": p_obs},
        "diverged": v_obs != p_obs,
    }


def _cmd_finding(R, cat, pid, desc, cmd):
    r = probe(R, cmd)
    return _finding(cat, pid, desc, [cmd],
                    r["vanilla"]["raw"], r["vanilla"]["class"],
                    r["pumpkin"]["raw"], r["pumpkin"]["class"])


# --------------------------------------------------------------------------
# Chunk-INDEPENDENT categories: no world state needed, always runnable.
# --------------------------------------------------------------------------

# Target-selector parsing. oracle_discover proves /kill exists; this proves the
# selector GRAMMAR behind it matches. Live-observed: Pumpkin rejects a
# namespaced type in @e[type=minecraft:pig] as "unknown entity 'minecraft'".
SELECTOR_PROBES = [
    ("type_namespaced", "@e[type=minecraft:pig] namespaced entity type",
     "kill @e[type=minecraft:pig,limit=0]"),
    ("type_bare", "@e[type=pig] bare entity type",
     "kill @e[type=pig,limit=0]"),
    ("type_negated", "@e[type=!minecraft:player] negated type",
     "kill @e[type=!minecraft:player,limit=0]"),
    ("type_tag", "@e[type=#minecraft:skeletons] entity type tag",
     "kill @e[type=#minecraft:skeletons,limit=0]"),
    ("distance_range", "@e[distance=..5] bounded range",
     "kill @e[distance=..5,limit=0]"),
    ("scores", "@e[scores={obj=1..}] score selector",
     "kill @e[scores={foo=1..},limit=0]"),
    ("nbt", "@e[nbt={OnGround:1b}] nbt selector",
     "kill @e[nbt={OnGround:1b},limit=0]"),
    ("sort_limit", "@e[sort=nearest,limit=1] sort+limit",
     "kill @e[sort=nearest,limit=1,type=item]"),
    ("gamemode", "@a[gamemode=survival] player gamemode filter",
     "kill @a[gamemode=survival,limit=0]"),
    ("tag_filter", "@e[tag=foo] tag filter", "kill @e[tag=foo,limit=0]"),
]

# Command ARGUMENT validation. Bare-name existence is oracle_discover's job;
# this drives one level in, where Pumpkin often accepts a command name but
# mishandles its arguments.
ARGUMENT_PROBES = [
    ("time_bad_unit", "time set with a bad keyword", "time set notaphase"),
    ("gamerule_oob", "numeric gamerule out of range", "gamerule randomTickSpeed -5"),
    ("difficulty_bad", "unknown difficulty name", "difficulty ultra"),
    ("effect_infinite", "effect give infinite duration keyword",
     "effect give @e[type=item,limit=0] minecraft:speed infinite 1"),
    ("summon_rel_norun", "summon at relative coords via rcon (no origin)",
     "summon minecraft:pig ~ ~ ~"),
    ("tp_selector_to_pos", "teleport a selector to coords",
     "teleport @e[type=item,limit=0] 0 100 0"),
    ("give_component", "give with a data-component argument",
     "give @a[limit=0] minecraft:stone[minecraft:custom_name='x'] 1"),
    ("particle_named", "particle by name with coords",
     "particle minecraft:flame 0 200 0 0 0 0 0 1 force"),
    ("fillbiome_named", "fillbiome to a named biome",
     "fillbiome 0 200 0 0 200 0 minecraft:plains"),
    ("ride_mount", "ride subcommand form", "ride @e[type=item,limit=0] mount @e[type=boat,limit=0]"),
]

# Whether /summon parses an NBT/data tag at all. summon works on unloaded
# chunks (observed), so the tag is the only variable.
SUMMON_NBT_PROBES = [
    ("nogravity", "summon armor_stand with {NoGravity:1b}",
     "summon minecraft:armor_stand 2 200 2 {NoGravity:1b}"),
    ("customname", "summon with a CustomName component",
     'summon minecraft:armor_stand 2 200 2 {CustomName:\'{"text":"x"}\'}'),
    ("tags", "summon carrying scoreboard Tags",
     "summon minecraft:armor_stand 2 200 2 {Tags:[\"sweep\"]}"),
    ("passengers", "summon with a Passengers stack",
     "summon minecraft:armor_stand 2 200 2 {Passengers:[{id:\"minecraft:pig\"}]}"),
    ("badtag", "summon with malformed NBT (both should reject)",
     "summon minecraft:armor_stand 2 200 2 {NoGravity:}"),
]


def gen_selector(R):
    return [_cmd_finding(R, "selector", i, d, c) for i, d, c in SELECTOR_PROBES]


def gen_argument(R):
    return [_cmd_finding(R, "argument", i, d, c) for i, d, c in ARGUMENT_PROBES]


def gen_summon_nbt(R):
    return [_cmd_finding(R, "summon_nbt", i, d, c) for i, d, c in SUMMON_NBT_PROBES]


# --------------------------------------------------------------------------
# Chunk-DEPENDENT categories: need Pumpkin's spawn chunk held loaded.
# Each probe is setblock-a-scenario then read-a-cell, so vanilla is observed
# with no dependence on worldgen (except `worldgen`, which is the point).
# --------------------------------------------------------------------------

# (id, desc, [setup cmds], read_pos, read_block, correct_vanilla_present)
# Support blocks placed with nothing under them: vanilla removes them.
BLOCK_SUPPORT = [
    ("torch_midair", "wall/floor torch with no support pops off",
     ["setblock 3 108 3 minecraft:air", "setblock 3 108 3 minecraft:torch"],
     "3 108 3", "minecraft:torch", 0),
    ("sapling_on_stone", "sapling on stone (not dirt) pops off",
     ["setblock 4 108 3 minecraft:stone", "setblock 4 108 4 minecraft:oak_sapling"],
     "4 108 4", "minecraft:oak_sapling", 0),
    ("flower_midair", "dandelion with no soil pops off",
     ["setblock 5 108 3 minecraft:air", "setblock 5 108 3 minecraft:dandelion"],
     "5 108 3", "minecraft:dandelion", 0),
    ("rail_midair", "rail with no support pops off",
     ["setblock 6 108 3 minecraft:air", "setblock 6 108 3 minecraft:rail"],
     "6 108 3", "minecraft:rail", 0),
    ("redstone_midair", "redstone dust with no support pops off",
     ["setblock 7 108 3 minecraft:air", "setblock 7 108 3 minecraft:redstone_wire"],
     "7 108 3", "minecraft:redstone_wire", 0),
    ("torch_on_stone", "control: torch ON a full block stays",
     ["setblock 8 108 3 minecraft:stone", "setblock 8 109 3 minecraft:torch"],
     "8 109 3", "minecraft:torch", 1),
]

# setblock then read straight back: does the block persist and normalise?
BLOCK_STATE = [
    ("plain_stone", "setblock stone persists", "minecraft:stone"),
    ("stairs", "setblock oak_stairs persists", "minecraft:oak_stairs"),
    ("slab", "setblock stone_slab persists", "minecraft:stone_slab"),
    ("chest", "setblock chest (block entity) persists", "minecraft:chest"),
    ("waterlogged_fence", "setblock a fence persists", "minecraft:oak_fence"),
    ("wall_sign", "setblock oak_sign persists", "minecraft:oak_sign"),
]

# Natural terrain the shared seed 1784236824834363978 must reproduce. Compares
# clone-counts of expected blocks in the spawn column; a mismatch is a worldgen
# parity gap. Read-only apart from the scratch dst.
WORLDGEN = [
    ("bedrock_floor", "bedrock count in the y=-64..-60 spawn column",
     "0 -64 0", "5 -60 5", "minecraft:bedrock"),
    ("deepslate_band", "deepslate count in the y=-40..-36 spawn column",
     "0 -40 0", "5 -36 5", "minecraft:deepslate"),
    ("stone_band", "stone count in the y=10..14 spawn column",
     "0 10 0", "5 14 5", "minecraft:stone"),
]


def gen_block_support(R):
    out = []
    for pid, desc, setup, rpos, rblock, _exp in BLOCK_SUPPORT:
        for c in setup:
            R["vanilla"].cmd(c)
            R["pumpkin"].cmd(c)
        vo, vr = present(R, "vanilla", rpos, rblock)
        po, pr = present(R, "pumpkin", rpos, rblock)
        out.append(_finding("block_support", pid, desc, setup, vr, vo, pr, po))
    return out


def gen_block_state(R):
    out = []
    for pid, desc, block in BLOCK_STATE:
        seq = [f"setblock {SCRATCH} minecraft:air", f"setblock {SCRATCH} {block}"]
        for c in seq:
            R["vanilla"].cmd(c)
            R["pumpkin"].cmd(c)
        vo, vr = present(R, "vanilla", SCRATCH, block)
        po, pr = present(R, "pumpkin", SCRATCH, block)
        out.append(_finding("block_state", pid, desc, seq, vr, vo, pr, po))
    return out


def gen_worldgen(R):
    out = []
    for pid, desc, a, b, block in WORLDGEN:
        cmd = f"clone {a} {b} {DST} filtered {block}"
        vr = R["vanilla"].cmd(cmd)
        pr = R["pumpkin"].cmd(cmd)
        vo = -1 if "not loaded" in vr.lower() else _num(vr)
        po = -1 if "not loaded" in pr.lower() else _num(pr)
        out.append(_finding("worldgen", pid, desc, [cmd], vr, vo, pr, po))
    return out


def _num(s):
    m = re.search(r"(\d+)\s+block", s)
    if m:
        return int(m.group(1))
    return 0 if re.search(r"no blocks", s, re.I) else -1


# category name -> (needs_loaded_chunk, generator)
CATEGORIES = {
    "selector": (False, gen_selector),
    "argument": (False, gen_argument),
    "summon_nbt": (False, gen_summon_nbt),
    "block_support": (True, gen_block_support),
    "block_state": (True, gen_block_state),
    "worldgen": (True, gen_worldgen),
}


def _connect():
    R = {}
    for name in ("vanilla", "pumpkin"):
        cfg = SERVERS[name]
        R[name] = Rcon(cfg["host"], cfg["port"], cfg["password"], timeout=20)
    return R


def pumpkin_chunk_loaded(R):
    """The chunk-dependent tier only means anything if Pumpkin actually holds
    its spawn chunk. Its forceload is broken, so this is normally the holder
    bot's job; without it every block probe returns NOT_LOADED, which is a rig
    state and must never be filed as a parity gap."""
    raw = R["pumpkin"].cmd(f"setblock {SCRATCH} minecraft:air")
    return "not loaded" not in raw.lower()


# --------------------------------------------------------------------------
# Filing. Two sinks, mirroring the rest of the rig, and NEVER WORKQUEUE.md.
# --------------------------------------------------------------------------
# Outcomes that are rig/transport state, not a game behaviour. A divergence
# touching one of these is never filed: it is a chunk that was not held or a
# packet that timed out, not something Pumpkin gets wrong.
_NON_OUTCOME = {"ERROR", "NOT_LOADED"}


def _filable(f):
    # -1 is the block-category "unreadable" observable (unloaded chunk / bad
    # clone); treat it exactly like the ERROR/NOT_LOADED string classes.
    bad = _NON_OUTCOME | {-1}
    return (f["diverged"]
            and f["vanilla"]["obs"] not in bad
            and f["pumpkin"]["obs"] not in bad)


def _seen_ids(path, prefix):
    if not os.path.exists(path):
        return set()
    return set(re.findall(rf"`({re.escape(prefix)}[^`]+)`", open(path).read()))


def file_discovered(findings):
    """Append diverged findings to DISCOVERED.md in its existing evidence
    format. Vanilla output is observed, so this is real evidence, not a guess.
    Deduped by the `sweep:<cat>:<id>` key."""
    seen = _seen_ids(LEDGER, "sweep:")
    new = [f for f in findings
           if _filable(f) and f"sweep:{f['category']}:{f['id']}" not in seen]
    if not new:
        return 0
    stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M")
    with open(LEDGER, "a") as fh:
        for f in new:
            key = f"sweep:{f['category']}:{f['id']}"
            fh.write(
                f"- [ ] `{key}` (suspected, {stamp}) - {f['desc']}\n"
                f"    - seq: `{' ; '.join(f['seq'])[:200]}`\n"
                f"    - vanilla: `[{f['vanilla']['obs']}] {str(f['vanilla']['raw'])[:130]}`\n"
                f"    - pumpkin: `[{f['pumpkin']['obs']}] {str(f['pumpkin']['raw'])[:130]}`\n"
            )
    return len(new)


PROP_HEADER = (
    "# Sweep proposals (WORKQUEUE stubs, STATE suspected)\n\n"
    "Auto-filed by oracle_sweep.py. These are candidate WORKQUEUE items in the\n"
    "queue's own ID/STATE/WHERE/WHAT/VANILLA/DO/CARE shape, but they are\n"
    "NEVER promoted here: every one is STATE suspected. A human or Claude reads\n"
    "the evidence in DISCOVERED.md, confirms the code site, and only then\n"
    "copies a hardened item into WORKQUEUE.md. This file is a staging area, not\n"
    "the queue. The vanilla line is oracle-OBSERVED (live RCON), which is why\n"
    "these are worth a look, but 'suspected' still holds until the WHERE is\n"
    "pinned to real code.\n\n---\n"
)


def file_proposals(findings):
    seen = _seen_ids(PROPOSALS, "SWEEP-")
    new = [f for f in findings
           if _filable(f) and f"SWEEP-{f['category']}-{f['id']}".upper() not in
           {s.upper() for s in seen}]
    if not new:
        return 0
    fresh = not os.path.exists(PROPOSALS)
    os.makedirs(os.path.dirname(PROPOSALS), exist_ok=True)
    stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M")
    with open(PROPOSALS, "a") as fh:
        if fresh:
            fh.write(PROP_HEADER)
        for f in new:
            iid = f"SWEEP-{f['category']}-{f['id']}"
            fh.write(
                f"\n### {iid}  {f['desc']}\n"
                f"STATE suspected\n"
                f"WHERE `TBD - localise from the command sequence below`\n"
                f"WHAT vanilla and Pumpkin give different observable outcomes "
                f"for this {f['category']} probe (filed {stamp}).\n"
                f"SEQ `{' ; '.join(f['seq'])}`\n"
                f"VANILLA (oracle-observed) obs=`{f['vanilla']['obs']}` "
                f"reply=`{str(f['vanilla']['raw'])[:120]}`\n"
                f"PUMPKIN obs=`{f['pumpkin']['obs']}` "
                f"reply=`{str(f['pumpkin']['raw'])[:120]}`\n"
                f"DO Verify the vanilla side against 26.2, pin WHERE to code, "
                f"then promote to WORKQUEUE.md as confirmed.\n"
                f"CARE Outcome-class diff only; wording differences are already "
                f"normalised out, so a divergence here is a real class change.\n"
            )
    return len(new)


# --------------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--category", choices=list(CATEGORIES) + ["all"], default="all")
    ap.add_argument("--file", action="store_true",
                    help="append findings to DISCOVERED.md and stubs to sweep-proposals.md")
    ap.add_argument("--list", action="store_true", help="list categories and exit")
    a = ap.parse_args()

    if a.list:
        for name, (needs, _) in CATEGORIES.items():
            tier = "chunk-dependent" if needs else "chunk-independent"
            print(f"  {name:<14} {tier}")
        return 0

    try:
        R = _connect()
    except Exception as e:  # noqa: BLE001
        print(f"cannot reach a server over RCON: {e}")
        print("nothing measured; a sick server produces confident nonsense.")
        return 1

    loaded = pumpkin_chunk_loaded(R)
    if not loaded:
        print("NOTE: Pumpkin spawn chunk not loaded (forceload is broken; start "
              "`node ~/pumpkin-bot/holder.js Verifier`). Chunk-dependent "
              "categories are SKIPPED, not filed.\n")

    cats = list(CATEGORIES) if a.category == "all" else [a.category]
    findings, skipped = [], []
    for c in cats:
        needs, gen = CATEGORIES[c]
        if needs and not loaded:
            skipped.append(c)
            continue
        fs = gen(R)
        findings += fs
        d = sum(1 for f in fs if f["diverged"])
        print(f"=== {c:<14} {len(fs)} probe(s), {d} divergence(s) ===")
        for f in fs:
            mark = "DIFF " if f["diverged"] else "match"
            print(f"  [{mark}] {f['id']:<18} "
                  f"v={f['vanilla']['obs']!s:<12} p={f['pumpkin']['obs']!s}")

    for name in ("vanilla", "pumpkin"):
        R[name].close()

    total = sum(1 for f in findings if f["diverged"])
    print(f"\n{len(findings)} probe(s) run, {total} divergence(s) found.")
    if skipped:
        print(f"skipped (chunk not loaded): {', '.join(skipped)}")

    if a.file:
        n1 = file_discovered(findings)
        n2 = file_proposals(findings)
        print(f"filed {n1} new evidence line(s) to {LEDGER}")
        print(f"filed {n2} new stub(s) to {PROPOSALS}")
    else:
        print("(dry run; pass --file to append to DISCOVERED.md and sweep-proposals.md)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
