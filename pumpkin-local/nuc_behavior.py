#!/usr/bin/env python3
"""
Behavioural differential oracle: place blocks, let them act, compare vanilla.

The command sweep found missing commands; this finds wrong BLOCK BEHAVIOUR,
observed live. It reuses the tricks the pumpkin-bot rig already proved:

  - Pumpkin keeps no chunks loaded and its forceload is broken, so a raw
    protocol client (pumpkin-bot/holder.js) stays connected to hold spawn
    chunks loaded. Start it before running this: `node holder.js Verifier`.
  - Pumpkin has no `data get` / `execute`, so block state is read with
    `clone <p> <p> <dst> filtered <block>` and parsing "N blocks cloned":
    N==1 means the block is there, N==0 means it is not. Works over RCON.

Each probe: setblock a scenario on BOTH servers, wait, then read a result
cell on both and diff. Vanilla is observed, never guessed, so a difference is
a real, evidenced parity gap. Vanilla holds its chunk with forceload (which
works there); Pumpkin relies on the bot. Probes stay in the spawn chunk.

    node ~/pumpkin-bot/holder.js Verifier &   # once, keeps pumpkin loaded
    python3 nuc_behavior.py
"""

import datetime
import os
import re
import time

from nuc_probe import rcon

V = ("192.168.1.93", 25576, "oracletest")
P = ("192.168.1.93", 25575, "pumpkintest")
HERE = os.path.dirname(os.path.abspath(__file__))
LEDGER = os.path.join(HERE, "DISCOVERED.md")
DST = "14 130 14"  # scratch clone dest, INSIDE the loaded spawn chunk (0-15)


def present(server, pos, block):
    """1 if `block` is at `pos`, 0 if not, via clone-count. -1 on error."""
    out = rcon(*server, f"clone {pos} {pos} {DST} filtered minecraft:{block}")
    if re.search(r"no blocks?\b.*clon|clon.*no blocks?\b", out, re.I):
        return 0
    # vanilla: "Successfully cloned 1 block(s)"; pumpkin: "1 blocks cloned".
    m = re.search(r"(\d+) block", out)
    return int(m.group(1)) if m else -1


# Each probe: (id, human desc, setup commands, (result_pos, result_block),
# expected-vanilla-present, wait seconds). Setup runs on both servers; then the
# result cell is read on both and compared. Coords sit in the spawn chunk.
# RANDOM-TICK behaviours only. These are what setblock CAN drive over RCON,
# given a long enough wait and random_tick_speed cranked (leaftest.js waits up
# to 210s - my first pass waited 18s and wrongly concluded RCON cannot do
# physics). Block-UPDATE physics (sand falling on support removal) still needs
# the bot rig and is NOT here. Waits are seconds; keep them realistic.
_SETUP = ["gamerule random_tick_speed 1000", "time set day", "weather clear"]
PROBES = [
    ("leaves_decay_lone",
     "leaves with no log nearby decay away",
     ["setblock 5 100 5 minecraft:stone",
      "setblock 5 102 5 minecraft:oak_leaves"],
     ("5 102 5", "oak_leaves"), 0, 75),
    ("leaves_keep_near_log",
     "leaves next to a log do NOT decay",
     ["setblock 8 102 5 minecraft:oak_log",
      "setblock 9 102 5 minecraft:oak_leaves"],
     ("9 102 5", "oak_leaves"), 1, 75),
    ("ice_melts_by_torch",
     "ice next to a torch melts (block light > 11)",
     ["setblock 12 100 5 minecraft:stone",
      "setblock 12 101 5 minecraft:ice",
      "setblock 13 101 5 minecraft:torch"],
     ("12 101 5", "ice"), 0, 60),
    ("sapling_stays",
     "a sapling placed on dirt stays placed (control)",
     ["setblock 15 100 5 minecraft:dirt",
      "setblock 15 101 5 minecraft:oak_sapling"],
     ("15 101 5", "oak_sapling"), 1, 5),
]


def run_probe(p):
    pid, desc, setup, (rpos, rblock), exp, wait = p
    for cmd in setup:
        rcon(*V, cmd)
        rcon(*P, cmd)
    if wait:
        time.sleep(wait)
    v = present(V, rpos, rblock)
    pk = present(P, rpos, rblock)
    match = v == pk
    return pid, desc, v, pk, exp, match


def file_gaps(gaps):
    if not gaps:
        return
    with open(LEDGER, "a") as fh:
        stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M")
        for pid, desc, v, pk in gaps:
            fh.write(
                f"- [ ] `behavior:{pid}` (high, {stamp}) - {desc}\n"
                f"    - vanilla: `result cell present={v}`\n"
                f"    - pumpkin: `result cell present={pk}`\n"
            )


def main():
    # sanity: bot must be holding pumpkin's spawn chunk
    if "Block placed" not in rcon(*P, "setblock 0 120 0 minecraft:air"):
        print("Pumpkin chunk not loaded - start: node ~/pumpkin-bot/holder.js Verifier")
        return
    rcon(*V, "forceload add 0 0")
    for cmd in _SETUP:  # random_tick_speed, day, clear - on both, once
        rcon(*V, cmd)
        rcon(*P, cmd)

    gaps = []
    for p in PROBES:
        pid, desc, v, pk, exp, match = run_probe(p)
        tag = "MATCH" if match else "DIFF"
        note = "" if v == exp else "  (vanilla != expected; probe or load suspect)"
        print(f"[{tag}] {pid:22} vanilla={v} pumpkin={pk}  ({desc}){note}")
        # A gap is real ONLY when vanilla did the expected thing (so the probe
        # is valid) AND pumpkin differs. If vanilla != expected, the probe
        # itself is unreliable (timing, setblock bypass) - never file that.
        if v == exp and pk != exp and pk != -1:
            gaps.append((pid, desc, v, pk))

    file_gaps(gaps)
    print(f"\n{len(gaps)} behavioural gap(s) filed to DISCOVERED.md"
          if gaps else "\nno behavioural gaps this pass")


if __name__ == "__main__":
    main()
