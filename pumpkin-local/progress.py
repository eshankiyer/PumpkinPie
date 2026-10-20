#!/usr/bin/env python3
"""
Live progress view for the local agent run.

    python3 progress.py            # refresh every 5s until the run ends
    python3 progress.py --once     # print one snapshot and exit

Reads the status file run_all.py writes plus the log, so it is safe to run
alongside the batch and costs it nothing.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time

STATUS = "/tmp/pumpkin_run_status.json"
LOG = "/tmp/overnight.log"
DONE = "/tmp/pumpkin_run_DONE.txt"

# a spec is finished when it reaches one of these
TERMINAL = ("clean", "aborted", "failed")
BAR_W = 28


def running():
    p = subprocess.run(
        ["bash", "-c", "pgrep -f 'python3 run_all.py' | head -1"],
        capture_output=True,
        text=True,
    )
    return bool(p.stdout.strip())


def load_status():
    try:
        with open(STATUS) as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError):
        return {}


def log_text():
    try:
        return open(LOG, errors="replace").read()
    except OSError:
        return ""


def bar(done, total, width=BAR_W):
    if total <= 0:
        return "[" + " " * width + "]"
    filled = int(round(width * done / total))
    return "[" + "#" * filled + "-" * (width - filled) + "]"


def phase_icon(phase):
    if phase.startswith("clean"):
        return "OK "
    if phase.startswith(("aborted", "failed")):
        return "XX "
    if phase.startswith("generating"):
        return " > "
    if phase.startswith("building"):
        return " B "
    if phase.startswith("queue"):
        return " . "
    return "   "


def iter_of(phase):
    m = re.search(r"iter (\d+)", phase)
    return int(m.group(1)) if m else 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--once", action="store_true")
    ap.add_argument("--interval", type=float, default=5.0)
    ap.add_argument("--max-iters", type=int, default=6)
    a = ap.parse_args()

    start = time.time()
    while True:
        st = load_status()
        txt = log_text()

        m = re.search(r"running (\d+) specs", txt)
        total = int(m.group(1)) if m else len(st) or 1

        # measured reply times, used for the estimate
        replies = [int(x) for x in re.findall(r"replied in (\d+)s", txt)]
        avg_reply = sum(replies) / len(replies) if replies else 60.0
        n_builds = txt.count(": building")
        # builds are the long pole and overlap two at a time
        avg_build = 150.0

        done = sum(
            1 for v in st.values() if str(v.get("phase", "")).startswith(TERMINAL)
        )
        # progress within the unfinished specs, by iteration consumed
        partial = 0.0
        for v in st.values():
            ph = str(v.get("phase", ""))
            if ph.startswith(TERMINAL):
                continue
            partial += min(iter_of(ph), a.max_iters) / a.max_iters
        frac_done = (done + partial) / total if total else 0.0

        elapsed = time.time() - start
        lines = []
        lines.append(
            f"  Pumpkin local agent   {bar(frac_done * total, total)}  "
            f"{frac_done * 100:5.1f}%   {done}/{total} finished"
        )
        lines.append("")
        for name in sorted(st):
            ph = str(st[name].get("phase", "?"))
            it = iter_of(ph)
            it_s = f"iter {it}/{a.max_iters}" if it else " " * 10
            lines.append(f"  {phase_icon(ph)}{name:<24} {it_s:<12} {ph}")
        lines.append("")

        # Estimate: remaining iterations, throttled by the single GPU slot.
        remaining_iters = 0.0
        for v in st.values():
            ph = str(v.get("phase", ""))
            if ph.startswith(TERMINAL):
                continue
            remaining_iters += max(0, a.max_iters - iter_of(ph))
        # GPU is serial, so generation time is the floor; builds mostly overlap
        eta_s = remaining_iters * avg_reply + (remaining_iters / 2.0) * avg_build * 0.5
        if replies:
            lines.append(
                f"  avg reply {avg_reply:.0f}s over {len(replies)} calls, "
                f"{n_builds} builds so far"
            )
        if not running():
            lines.append("  run has ENDED")
        else:
            lines.append(
                f"  worst case if every spec uses all {a.max_iters} iterations: "
                f"~{eta_s / 60:.0f} min"
            )
            lines.append(
                "  most specs finish or fail earlier, so treat that as an upper bound"
            )

        out = "\n".join(lines)
        if a.once:
            print(out)
            return 0
        os.system("clear")
        print(out)
        if os.path.exists(DONE):
            print("\n  DONE marker written:\n")
            print(open(DONE).read())
            return 0
        if not running():
            return 0
        time.sleep(a.interval)


if __name__ == "__main__":
    sys.exit(main())
