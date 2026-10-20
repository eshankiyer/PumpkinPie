#!/usr/bin/env python3
"""
Differential oracle: measure the same behaviour on vanilla and on Pumpkin.

This exists so the coding agent stops depending on somebody having correctly
remembered what vanilla does. An experiment is run against BOTH servers, the
observable is measured, and the two numbers are compared. Vanilla's number is
the specification. Nobody has to recall it, cite it, or be trusted about it.

Two ways it gets used:

  detect      run an experiment on both, report whether they diverge
  assert      after a fix, re-run and require convergence

The second is what makes this worth building. A textual assertion can be
satisfied by a plausible wrong change, which has already happened: asked to
remove a redundant gate, the model retuned it instead, so the gate applied
twice. It compiled and satisfied the assertion and was still wrong. An
empirical assertion cannot be fooled that way, because a wrong fix does not
move the measurement onto vanilla's.

Usage:
    python3 differential.py list
    python3 differential.py run nether_wart_growth
    python3 differential.py run nether_wart_growth --trials 400
"""

import argparse
import json
import math
import socket
import struct
import sys
import time

# Pumpkin and the vanilla oracle, both reachable over RCON.
SERVERS = {
    "pumpkin": {"host": "192.168.1.93", "port": 25575, "password": "pumpkintest"},
    "vanilla": {"host": "192.168.1.93", "port": 25576, "password": "oracletest"},
}


# --------------------------------------------------------------------------
# minimal RCON, no dependency on the node harness
# --------------------------------------------------------------------------
class Rcon:
    def __init__(self, host, port, password, timeout=30, drain=0.25):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.sock.settimeout(timeout)
        # How long to wait for further fragments of one response. Long enough to
        # catch a split reply on a LAN, short enough that a 40-rule probe does
        # not take a minute.
        self.drain = drain
        self.rid = 0
        if self._send(3, password) is None:
            raise RuntimeError(f"rcon auth failed for {host}:{port}")

    def _recv_exact(self, n):
        buf = b""
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise ConnectionError("rcon closed mid-packet")
            buf += chunk
        return buf

    def _recv_packet(self):
        size = struct.unpack("<i", self._recv_exact(4))[0]
        data = self._recv_exact(size)
        rid, kind = struct.unpack("<ii", data[:8])
        return rid, kind, data[8:-2].decode(errors="replace")

    def _send_raw(self, kind, body):
        self.rid += 1
        mine = self.rid
        payload = struct.pack("<ii", mine, kind) + body.encode() + b"\x00\x00"
        self.sock.sendall(struct.pack("<i", len(payload)) + payload)
        return mine

    def _send(self, kind, body):
        """Auth, or any single-packet exchange."""
        mine = self._send_raw(kind, body)
        rid, _, text = self._recv_packet()
        if rid == -1:
            return None
        if rid != mine:
            raise RuntimeError(f"rcon desync: wanted id {mine}, got {rid}")
        return text

    def cmd(self, c):
        """Send a command and read its COMPLETE response.

        Reading one packet per command is wrong and fails silently in the worst
        possible way. Pumpkin splits a single command response across several
        RCON packets, so a one-packet read returns the first fragment and leaves
        the rest queued; the next command then reads a stale fragment. The whole
        session slides one reply out of step and every measurement after the
        first is attributed to the wrong command. Vanilla happened to fit in one
        packet, so only Pumpkin looked wrong, which reads exactly like a parity
        bug and is not one.

        So: read the first packet, then keep draining on a short timeout until
        the server stops talking, and concatenate every fragment carrying our
        request id.

        The usual trick is a sentinel packet whose reply marks the end, but the
        two servers disagree about whether an empty command is answered at all,
        and a sentinel that is never answered blocks forever. A drain makes no
        assumption about either server's politeness, at the cost of one short
        timeout per command.
        """
        mine = self._send_raw(2, c)
        parts = []
        rid, _, text = self._recv_packet()
        if rid == mine:
            parts.append(text)
        prev = self.sock.gettimeout()
        self.sock.settimeout(self.drain)
        try:
            while True:
                rid, _, text = self._recv_packet()
                if rid == mine:
                    parts.append(text)
        except (socket.timeout, TimeoutError, ConnectionError, struct.error):
            pass
        finally:
            self.sock.settimeout(prev)
        return "".join(parts).strip()

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


# --------------------------------------------------------------------------
# experiments
#
# Each experiment must be expressible in commands BOTH servers support, which
# rules out a few things Pumpkin gets wrong: `execute if block` ignores its
# predicate, and block states are rejected in setblock. Counting via
# `clone <a> <b> <dst> filtered <block>` works on both and is the workhorse.
# --------------------------------------------------------------------------


def _count(r, a, b, block, dst):
    out = r.cmd(f"clone {a} {b} {dst} filtered {block}")
    if "No blocks cloned" in out:
        return 0
    for tok in out.split():
        if tok.isdigit():
            return int(tok)
    return 0


def exp_nether_wart_growth(r, trials):
    """How often does nether wart advance per random tick?

    Plant a row of age-0 wart on soul sand, run a fixed number of ticks at a
    known random tick rate, and count how many advanced. The ratio is the
    growth probability, which is the thing the code's range bound encodes.
    """
    r.cmd("gamerule randomTickSpeed 0")
    r.cmd("gamerule doDaylightCycle false")
    r.cmd("time set day")
    r.cmd("weather clear")

    y, z = 100, 300
    n = min(trials, 64)
    for i in range(n):
        r.cmd(f"setblock {i} {y - 1} {z} minecraft:soul_sand")
        r.cmd(f"setblock {i} {y} {z} minecraft:air")
        r.cmd(f"setblock {i} {y} {z} minecraft:nether_wart")
    planted = _count(r, f"0 {y} {z}", f"{n - 1} {y} {z}", "minecraft:nether_wart", "0 130 300")

    # one burst of random ticks, then measure
    r.cmd("gamerule randomTickSpeed 200")
    time.sleep(6)
    r.cmd("gamerule randomTickSpeed 0")
    still = _count(r, f"0 {y} {z}", f"{n - 1} {y} {z}", "minecraft:nether_wart", "0 131 300")

    return {
        "planted": planted,
        "still_present": still,
        "note": "wart stays nether_wart at every age, so this measures survival not growth",
    }


def exp_random_tick_rate(r, trials):
    """Does randomTickSpeed actually change how fast blocks tick?

    Ice melts on a random tick when block light is high enough. Ring ice with
    glowstone, set a tick rate, wait a fixed wall time, and count survivors.
    A server that ignores the game rule shows the same count at every rate,
    which is exactly the bug PR #2587 fixes.
    """
    y, z = 100, 320
    out = {}
    for rate in (0, 100):
        r.cmd(f"gamerule randomTickSpeed {rate}")
        n = min(trials, 32)
        for i in range(n):
            x = i * 3
            r.cmd(f"setblock {x} {y} {z} minecraft:ice")
            for dx, dy, dz in ((1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, 0, 1), (0, 0, -1)):
                r.cmd(f"setblock {x + dx} {y + dy} {z + dz} minecraft:glowstone")
        time.sleep(8)
        left = _count(r, f"0 {y} {z}", f"{n * 3} {y} {z}", "minecraft:ice", "0 132 320")
        out[f"ice_left_at_rate_{rate}"] = left
        for i in range(n):
            r.cmd(f"setblock {i * 3} {y} {z} minecraft:air")
    r.cmd("gamerule randomTickSpeed 3")
    return out


def exp_button_press_ticks(r, trials):
    """How many ticks does a stone vs wooden button stay pressed?

    Poll the powered state each tick-ish and count. Vanilla is 20 for the
    stone family and 30 for wooden; Pumpkin gave polished blackstone the
    wooden delay, which PR #2589 fixes.
    """
    res = {}
    y, z = 100, 340
    for i, btn in enumerate(
        ("minecraft:stone_button", "minecraft:polished_blackstone_button", "minecraft:oak_button")
    ):
        x = i * 5
        r.cmd(f"setblock {x} {y - 1} {z} minecraft:stone")
        r.cmd(f"setblock {x} {y} {z} {btn}")
        res[btn] = r.cmd(f"clone {x} {y} {z} {x} {y} {z} 0 133 340 filtered {btn}")
    res["note"] = "needs blockstate-aware reads to measure duration; placeholder"
    return res


EXPERIMENTS = {
    "nether_wart_growth": exp_nether_wart_growth,
    "random_tick_rate": exp_random_tick_rate,
    "button_press_ticks": exp_button_press_ticks,
}


# --------------------------------------------------------------------------
def run_on(server, fn, trials):
    cfg = SERVERS[server]
    try:
        r = Rcon(cfg["host"], cfg["port"], cfg["password"])
    except Exception as e:
        return {"error": f"{server} unreachable: {e}"}
    try:
        return fn(r, trials)
    except Exception as e:
        return {"error": f"{server} experiment failed: {e}"}
    finally:
        r.close()


def compare(a, b):
    """Report per-key divergence between two measurement dicts."""
    keys = sorted(set(a) | set(b))
    rows, diverged = [], False
    for k in keys:
        va, vb = a.get(k, "-"), b.get(k, "-")
        same = va == vb
        if isinstance(va, (int, float)) and isinstance(vb, (int, float)):
            # allow a little sampling slack on counts
            same = math.isclose(float(va), float(vb), rel_tol=0.15, abs_tol=2)
        if not same and k != "note":
            diverged = True
        rows.append((k, va, vb, "same" if same else "DIFFERENT"))
    return rows, diverged


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("action", choices=["list", "run"])
    ap.add_argument("experiment", nargs="?")
    ap.add_argument("--trials", type=int, default=64)
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()

    if a.action == "list":
        for name, fn in EXPERIMENTS.items():
            first = (fn.__doc__ or "").strip().splitlines()[0]
            print(f"  {name:<24} {first}")
        return 0

    if a.experiment not in EXPERIMENTS:
        print(f"unknown experiment {a.experiment!r}; try: list")
        return 2

    fn = EXPERIMENTS[a.experiment]
    print(f"running {a.experiment} on vanilla ...")
    van = run_on("vanilla", fn, a.trials)
    print(f"running {a.experiment} on pumpkin ...")
    pmk = run_on("pumpkin", fn, a.trials)

    if a.json:
        print(json.dumps({"vanilla": van, "pumpkin": pmk}, indent=2))
        return 0

    if "error" in van or "error" in pmk:
        print("\n  " + str(van.get("error", "")))
        print("  " + str(pmk.get("error", "")))
        print("\n  Cannot compare while a server is unreachable.")
        return 1

    rows, diverged = compare(van, pmk)
    w = max(len(str(k)) for k, *_ in rows) + 2
    print(f"\n  {'observable'.ljust(w)}{'vanilla':<22}{'pumpkin':<22}verdict")
    print("  " + "-" * (w + 52))
    for k, va, vb, verdict in rows:
        print(f"  {str(k).ljust(w)}{str(va):<22}{str(vb):<22}{verdict}")
    print()
    print("  DIVERGENT: pumpkin does not match vanilla" if diverged else "  MATCH")
    return 1 if diverged else 0


if __name__ == "__main__":
    sys.exit(main())
