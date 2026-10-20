#!/usr/bin/env python3
"""
One-screen status for the parity loop. Designed for `watch`:

    watch -n15 python3 ~/pumpkin-local/status.py

Honest about scope: the bar tracks KNOWN, confirmed gaps, not true vanilla
parity (which is unbounded). It shows what the loop can actually act on and how
far through it is, plus what the oracle has discovered but not yet confirmed.
"""

import glob
import os
import re
import socket
import subprocess
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)


def bar(done, total, width=32):
    if total <= 0:
        return "[" + " " * width + "] n/a"
    filled = round(width * done / total)
    pct = 100 * done / total
    return "[" + "#" * filled + "-" * (width - filled) + f"] {done}/{total} ({pct:.0f}%)"


def up(url):
    try:
        urllib.request.urlopen(url, timeout=2)
        return True
    except Exception:
        return False


def rcon_ok(port):
    try:
        with socket.create_connection(("192.168.1.93", port), timeout=2):
            return True
    except OSError:
        return False


def tok_s():
    try:
        out = open(os.path.join(HERE, "logs", "llama.log")).read()
        hits = re.findall(r"tg =\s*([0-9.]+) t/s", out)
        return hits[-1] if hits else "?"
    except OSError:
        return "?"


def spree_now():
    try:
        lines = open(os.path.join(HERE, "logs", "spree.log")).read().splitlines()
    except OSError:
        return "idle"
    for ln in reversed(lines):
        if ln.startswith("=== ") or "iter" in ln or "VERIFIED" in ln or "CANDIDATE" in ln:
            return ln.strip("= ").strip()
    return "idle"


def next_run():
    try:
        out = subprocess.run(
            ["systemctl", "--user", "list-timers", "pumpkin-parity.timer",
             "--no-pager"], capture_output=True, text=True, timeout=5
        ).stdout
        m = re.search(r"(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d)", out)
        return m.group(1) if m else "not scheduled"
    except Exception:
        return "?"


def main():
    import mini_spree as m

    items = m.parse_queue()
    confirmed = [i for i in items if m.eligible(i)[0]]
    cand = glob.glob(os.path.join(HERE, "candidates", "*.diff"))
    # A confirmed item counts as addressed if a candidate diff exists for it.
    cand_ids = {os.path.basename(c).split("-")[0] for c in cand}
    addressed = [i for i in confirmed if i["id"] in cand_ids]
    disc = 0
    dpath = os.path.join(HERE, "DISCOVERED.md")
    if os.path.exists(dpath):
        disc = len(re.findall(r"^- \[ \]", open(dpath).read(), re.M))

    llama = up("http://127.0.0.1:11434/health")
    van, pump = rcon_ok(25576), rcon_ok(25575)

    print("PUMPKIN PARITY LOOP")
    print("-" * 52)
    print("confirmed queue " + bar(len(addressed), len(confirmed)))
    print(f"candidates      {len(cand)} diffs on disk (incl. legacy)")
    print(f"discovered      {disc} gaps, unconfirmed (need review before fixing)")
    print("-" * 52)
    print(f"model    {'up, ' + tok_s() + ' tok/s' if llama else 'DOWN'}")
    print(f"oracle   vanilla {'ok' if van else 'DOWN'} | pumpkin {'ok' if pump else 'DOWN'}")
    print(f"spree    {spree_now()[:46]}")
    print(f"nightly  next {next_run()}")


if __name__ == "__main__":
    main()
