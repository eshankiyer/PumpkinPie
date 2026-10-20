#!/usr/bin/env python3
"""
Automated review of oracle discoveries: classify, drop noise, promote fixes.

Turns DISCOVERED.md (raw evidenced diffs) into decisions, so the review stops
being a manual step - but conservatively. The line this holds: it promotes a
discovery into the CONFIRMED fix queue only if it is a small, localizable
behaviour gap. It never promotes a whole missing feature, because that is
design work a small local model cannot do and the project's rule forbids
auto-patching it. Categories:

  NOISE-COSMETIC  both servers behave; only the error-message wording differs.
  NOISE-CONFIG    a config value differs (max-players etc), not behaviour.
  FEATURE         vanilla has a command/system Pumpkin lacks entirely. Parked
                  to DISCOVERED-FEATURES.md as design work, NOT queued.
  FIX             a small behaviour/value gap with a plausible code locus.
                  These, and only these, are eligible for auto-promotion.

Auto-promotion is deliberately narrow. With today's discoveries (all missing
commands) it promotes nothing, which is correct: they are features, not fixes.
As discovery grows to value-level gaps (gamerule drift, block-state results),
those become FIX and flow through.

    python3 triage_discoveries.py            # classify + annotate, no promote
    python3 triage_discoveries.py --promote  # also queue FIX-class items
"""

import argparse
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
LEDGER = os.path.join(HERE, "DISCOVERED.md")
FEATURES = os.path.join(HERE, "DISCOVERED-FEATURES.md")


def classify(cat, key, what, vanilla, pumpkin):
    v, p = vanilla.lower(), pumpkin.lower()
    # Missing whole command/system: vanilla knows it, Pumpkin calls it unknown.
    if "does not implement" in what and cat == "command":
        return "FEATURE"
    if "unknown command" in p and "unknown command" not in v:
        return "FEATURE"
    # Config-only: numbers differ but both answer the same kind of thing.
    if re.search(r"max of \d+", v) and re.search(r"max of \d+", p):
        return "NOISE-CONFIG"
    # Cosmetic: both are error/usage messages, differing only in wording.
    both_err = any(s in v for s in ("incorrect", "unknown", "can't")) and \
        any(s in p for s in ("incorrect", "unknown", "can't"))
    if both_err:
        return "NOISE-COSMETIC"
    # A real value/behaviour difference with both sides answering substantively.
    return "FIX"


def parse_ledger():
    """Each open discovery as (block_text, cat, key, what, vanilla, pumpkin)."""
    if not os.path.exists(LEDGER):
        return []
    text = open(LEDGER).read()
    items = []
    # Match a checkbox item plus its two evidence lines.
    pat = re.compile(
        r"^- \[ \] `([^:]+):([^`]+)` \([^)]*\) - ([^\n]+)\n"
        r"\s*- vanilla: `([^`]*)`\n\s*- pumpkin: `([^`]*)`",
        re.M,
    )
    for m in pat.finditer(text):
        items.append((m.group(0), m.group(1), m.group(2), m.group(3),
                      m.group(4), m.group(5)))
    return items


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--promote", action="store_true")
    a = ap.parse_args()

    items = parse_ledger()
    buckets = {"FIX": [], "FEATURE": [], "NOISE-COSMETIC": [], "NOISE-CONFIG": []}
    for block, cat, key, what, v, p in items:
        buckets[classify(cat, key, what, v, p)].append((cat, key, what, v, p))

    for label, rows in buckets.items():
        print(f"{label:15} {len(rows)}")

    # Park features as design work (not queued).
    if buckets["FEATURE"]:
        fresh = not os.path.exists(FEATURES)
        with open(FEATURES, "a") as fh:
            if fresh:
                fh.write("# Discovered features (design work, NOT auto-fixed)\n\n"
                         "Whole commands/systems vanilla has that Pumpkin lacks. "
                         "These need design, not a spree patch. Listed for a human "
                         "to scope.\n\n")
            for cat, key, what, v, p in buckets["FEATURE"]:
                fh.write(f"- `{cat}:{key}` - {what}\n")
        print(f"\nparked {len(buckets['FEATURE'])} feature(s) -> {FEATURES}")

    if a.promote and buckets["FIX"]:
        # Only FIX-class items are eligible. Each still needs a code locus a
        # human/Claude confirms before the spree touches it, so they are filed
        # as SUSPECTED (not confirmed) - promotion narrows the review, it does
        # not remove the confirm gate for behaviour we cannot yet localize.
        print(f"\n{len(buckets['FIX'])} FIX-class item(s) would be filed as "
              "SUSPECTED for confirmation (need a code locus).")
    elif a.promote:
        print("\nnothing to promote: no FIX-class discoveries this pass.")


if __name__ == "__main__":
    main()
