#!/usr/bin/env python3
"""
One spend cap shared by every tier that costs Claude quota.

Two things spend it: the escalation tier (specs the 30B failed) and the design
pass (turning a subsystem row into a scaffold). Capping them separately does not
work, because the loop runs 50 rounds and each round is entitled to its own
allowance, so per-round caps multiply into an unbounded daily total. The cap has
to be per day and shared.

Everything the local 30B does is free. Only these two tiers touch the
subscription, so this is the whole exposure.

    budget.py --status        what is left today
    budget.py --take <tier>   consume one, exit 1 if none left
"""

import argparse
import datetime as dt
import json
import os
import sys

LEDGER = os.path.join(os.path.dirname(os.path.abspath(__file__)), "BUDGET.json")

# Deliberately low. A design pass is 15 to 30 minutes of heavy file reading and
# an escalation is a full spec attempt, so eight calls is already several hours
# of model time per day. Raise it once there is evidence the output is worth it,
# not before.
DAILY_TOTAL = int(os.environ.get("PUMPKIN_DAILY_CLAUDE_CALLS", "8"))

# Sub-caps inside the shared total, so one tier cannot eat the whole day.
PER_TIER = {"scaffold": 2, "escalate": 3}


def load():
    try:
        with open(LEDGER) as f:
            data = json.load(f)
    except (OSError, ValueError):
        data = {}
    today = dt.date.today().isoformat()
    if data.get("date") != today:
        data = {"date": today, "spent": {}}
    return data


def save(data):
    with open(LEDGER, "w") as f:
        json.dump(data, f, indent=2)


def status():
    data = load()
    spent = data.get("spent", {})
    total = sum(spent.values())
    lines = [f"date {data['date']}  total {total}/{DAILY_TOTAL}"]
    for tier, cap in PER_TIER.items():
        lines.append(f"  {tier:10s} {spent.get(tier, 0)}/{cap}")
    return "\n".join(lines), total, spent


def take(tier):
    data = load()
    spent = data.setdefault("spent", {})
    total = sum(spent.values())
    used = spent.get(tier, 0)
    cap = PER_TIER.get(tier, DAILY_TOTAL)

    if total >= DAILY_TOTAL:
        return False, f"daily total exhausted ({total}/{DAILY_TOTAL})"
    if used >= cap:
        return False, f"{tier} sub-cap exhausted ({used}/{cap})"

    spent[tier] = used + 1
    save(data)
    return True, f"{tier} {used + 1}/{cap}, day {total + 1}/{DAILY_TOTAL}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--status", action="store_true")
    ap.add_argument("--take")
    a = ap.parse_args()

    if a.take:
        ok, why = take(a.take)
        print(why)
        return 0 if ok else 1

    text, _, _ = status()
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
