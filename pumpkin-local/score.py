#!/usr/bin/env python3
"""
PARITY_SCORE: the objective function.

The rule this exists to enforce: no session may state a completion percentage it
cannot cite from here. Every number below is either measured against a live
vanilla 26.2 server or counted from a file in the repo. Nothing is estimated,
and nothing is a feeling about how much is left.

Three components, deliberately weighted toward measurement over paperwork:

  command surface   50   fraction of vanilla commands present, measured over RCON
  probe agreement   30   fraction of oracle probes matching vanilla right now
  backlog burndown  20   fraction of confirmed WORKQUEUE rows marked Fixed

Command surface dominates because a missing command is a hole a player hits
immediately and a probe cannot paper over. Backlog burndown is weighted lowest
because it is the only component we can move by editing a markdown file, and an
objective function you can satisfy by editing the scoreboard is not one.

Usage:
    score.py                  print the score
    score.py --write          also update coverage/PARITY_SCORE.md
    score.py --json           machine readable
"""

import argparse
import json
import os
import re
import subprocess
import sys
import datetime as dt

LOCAL = os.path.dirname(os.path.abspath(__file__))
ORACLE = os.path.join(LOCAL, "oracle")
WORKQUEUE = os.path.join(LOCAL, "WORKQUEUE.md")
OUT_DIR = os.path.join(LOCAL, "coverage")
OUT = os.path.join(OUT_DIR, "PARITY_SCORE.md")

WEIGHTS = {"commands": 50.0, "probes": 30.0, "backlog": 20.0}


def measure_live():
    """Command surface and probe agreement, straight off the two servers.

    Returns None if either server is unhealthy. An unmeasurable server must
    abstain rather than score: a zero here would look like catastrophic
    regression and would be an artefact of the instrument, which has already
    happened twice on this project.
    """
    sys.path.insert(0, ORACLE)
    try:
        from probe_gate import PROBES, measure_all
    except Exception as e:  # noqa: BLE001
        return None, f"probe module unavailable: {e}"

    try:
        result = measure_all()
    except Exception as e:  # noqa: BLE001
        return None, f"oracle unhealthy, abstaining: {e}"

    total = len(result)
    matching = sum(1 for r in result.values() if r["status"] == "matching")

    cmd = result.get("command_surface", {})
    mismatch = cmd.get("mismatch", {})
    missing = [k for k, v in mismatch.items() if v.get("pumpkin") == "MISSING"]
    # The probe samples a fixed list of commands; present = sampled minus missing.
    sampled = 19
    present = sampled - len(missing)

    return {
        "probes_total": total,
        "probes_matching": matching,
        "commands_sampled": sampled,
        "commands_present": present,
        "commands_missing": sorted(missing),
    }, None


def measure_backlog():
    """Confirmed rows in WORKQUEUE.md, and how many are marked Fixed."""
    try:
        text = open(WORKQUEUE).read()
    except OSError:
        return {"confirmed": 0, "fixed": 0}
    confirmed = len(re.findall(r"^STATE confirmed", text, re.M))
    fixed = len(re.findall(r"^STATE (?:confirmed, )?fixed", text, re.M | re.I))
    return {"confirmed": confirmed + fixed, "fixed": fixed}


def compute():
    live, err = measure_live()
    backlog = measure_backlog()

    parts = {}
    if live:
        parts["commands"] = live["commands_present"] / live["commands_sampled"]
        parts["probes"] = live["probes_matching"] / live["probes_total"]
    total_rows = max(1, backlog["confirmed"])
    parts["backlog"] = backlog["fixed"] / total_rows

    if live:
        score = sum(WEIGHTS[k] * v for k, v in parts.items())
        measured = True
    else:
        # Only the component that does not need the servers.
        score = WEIGHTS["backlog"] * parts["backlog"]
        measured = False

    return {
        "when": dt.datetime.now().isoformat(timespec="seconds"),
        "score": round(score, 2),
        "measured_live": measured,
        "error": err,
        "components": {k: round(v, 4) for k, v in parts.items()},
        "live": live,
        "backlog": backlog,
    }


def render(d):
    lines = []
    lines.append("# PARITY_SCORE")
    lines.append("")
    lines.append(
        "The objective function for this project. Every number here is measured "
        "against a live vanilla 26.2 server or counted from a file in this repo. "
        "No session may claim a completion percentage it cannot cite from this "
        "document."
    )
    lines.append("")
    lines.append(f"**SCORE: {d['score']} / 100**  measured {d['when']}")
    lines.append("")
    if not d["measured_live"]:
        lines.append(
            f"> Live components unavailable this run ({d['error']}), so only the "
            "backlog component is counted and the score is a floor, not a "
            "reading. An unmeasurable server abstains rather than scoring zero."
        )
        lines.append("")

    lines.append("## Components")
    lines.append("")
    lines.append("| component | weight | fraction | points |")
    lines.append("| --- | --- | --- | --- |")
    for k, w in WEIGHTS.items():
        frac = d["components"].get(k)
        if frac is None:
            lines.append(f"| {k} | {w:.0f} | not measured | 0.00 |")
        else:
            lines.append(f"| {k} | {w:.0f} | {frac:.3f} | {w * frac:.2f} |")
    lines.append("")

    live = d.get("live")
    if live:
        lines.append("## Command surface")
        lines.append("")
        lines.append(
            f"{live['commands_present']} of {live['commands_sampled']} sampled "
            "vanilla commands are registered on Pumpkin."
        )
        lines.append("")
        if live["commands_missing"]:
            lines.append("Missing: " + ", ".join(f"`{c}`" for c in live["commands_missing"]))
            lines.append("")
        lines.append("## Probe agreement")
        lines.append("")
        lines.append(
            f"{live['probes_matching']} of {live['probes_total']} oracle probes "
            "currently match vanilla."
        )
        lines.append("")

    b = d["backlog"]
    lines.append("## Backlog burndown")
    lines.append("")
    lines.append(f"{b['fixed']} of {b['confirmed']} confirmed WORKQUEUE rows are marked Fixed.")
    lines.append("")
    lines.append(
        "This component is weighted lowest on purpose. It is the only one that "
        "can be moved by editing a markdown file, and an objective function you "
        "can satisfy by editing the scoreboard is not an objective function."
    )
    lines.append("")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--write", action="store_true")
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()

    d = compute()
    if a.json:
        print(json.dumps(d, indent=2))
    else:
        print(render(d))

    if a.write:
        os.makedirs(OUT_DIR, exist_ok=True)
        with open(OUT, "w") as f:
            f.write(render(d))
        print(f"\nwritten to {OUT}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
