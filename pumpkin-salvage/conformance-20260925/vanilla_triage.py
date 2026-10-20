#!/usr/bin/env python3
"""
Deterministically triage method_gaps.py leads using ONLY the vanilla decompile, before
any human or LLM reads them. Every rule here is mechanical and cheap; the point is to
stop paying judgment cost on leads that provably need no port.

Rules, each measured on the 2026-08-06 run (4591 leads):

  VANILLA_STUB (393, 8.6%) -- the vanilla method body is empty, or a bare `return false/
                         true/null/0`, or nothing but a super call. There is no behaviour
                         to port; Pumpkin omitting it is correct, not a gap.
  NO_CALLER    (0)    -- CURRENTLY INERT, deliberately. See below.

VALIDATION (the reason NO_CALLER is inert)
------------------------------------------
Both rules were checked against the 18-lead sample whose ground truth was established by
reading both sides. The first NO_CALLER implementation matched only `.foo(` and so scored
FlowerBlock.makeEffectList as uncalled -- it is a private static helper invoked
unqualified as `makeEffectList(...)` -- which would have SILENTLY DISCARDED a confirmed
real gap. Widening the regex to catch unqualified calls fixes that false negative but the
pattern then also matches each method's own declaration, so every method has >=1 "caller"
and the rule now fires on nothing.

It is left in place, inert, rather than deleted: a filter that drops true positives is
strictly worse than no filter, and 8.6% of free elimination is not worth reintroducing
that risk. To revive it, exclude the declaration site itself (and remember the decompile
contains only the server jar, so client-side callers are invisible and absence of callers
does NOT by itself prove dead code).

Current honest yield: 8.6% of leads removed, with zero false negatives on the three known
real gaps (FlowerBlock.makeEffectList, EnderDragon.hurtServer, CopperGolem.thunderHit).

Output: method_gaps_triaged.json, same shape as method_gaps.json plus a `triage` field
per lead. Leads marked NEEDS_REVIEW are the ones actually worth spending attention on.

This does NOT decide whether Pumpkin implements something -- it only decides whether
vanilla has anything worth implementing. The Rust side is method_gaps.py's job.
"""
import json
import os
import re
import sys
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
DECOMP = Path(
    os.environ.get("PUMPKIN_DECOMP", Path.home() / "pumpkin-vanilla-26.2/decompiled")
)

TRIVIAL_BODY_RE = re.compile(
    r"return (false|true|null|0|0\.0[FfDd]?|this|super\.\w+\([^;]*\));?$"
)
# Must match UNQUALIFIED calls too, not just `.foo(`. Requiring the dot marked
# FlowerBlock.makeEffectList as NO_CALLER -- it is a private static helper invoked as
# `makeEffectList(...)` -- which would have discarded a confirmed real gap. A filter that
# drops true positives is worse than no filter, so this errs toward over-counting callers.
CALL_RE = re.compile(r"\b(\w+)\s*\(")
COMMENT_RE = re.compile(r"//.*|/\*.*?\*/", re.S)

_text_cache: dict[str, str] = {}


def java_text(rel: str) -> str:
    t = _text_cache.get(rel)
    if t is None:
        p = DECOMP / "net/minecraft" / rel if not rel.startswith("net/") else DECOMP / rel
        t = p.read_text(errors="replace") if p.exists() else ""
        _text_cache[rel] = t
    return t


def method_body(rel: str, method: str) -> str | None:
    """Source text inside the first brace-balanced body of `method` in `rel`."""
    t = java_text(rel)
    for m in re.finditer(r"\b" + re.escape(method) + r"\s*\(", t):
        open_i = t.find("{", m.end())
        semi_i = t.find(";", m.end())
        # abstract/interface declaration ends in ';' before any '{'
        if open_i == -1 or (semi_i != -1 and semi_i < open_i):
            continue
        depth = 0
        for k in range(open_i, len(t)):
            if t[k] == "{":
                depth += 1
            elif t[k] == "}":
                depth -= 1
                if depth == 0:
                    return t[open_i + 1 : k].strip()
    return None


def build_call_counts(names: set[str]) -> Counter:
    counts = Counter()
    for p in (DECOMP / "net").rglob("*.java"):
        for m in CALL_RE.finditer(p.read_text(errors="replace")):
            n = m.group(1)
            if n in names:
                counts[n] += 1
    return counts


def main() -> int:
    src = HERE / "method_gaps.json"
    if not src.exists():
        print("method_gaps.json missing -- run method_gaps.py first", file=sys.stderr)
        return 1
    report = json.loads(src.read_text())

    names = {m for c in report["classes"] for m in c["absent_workspace_wide"]}
    print(f"scanning decompile for callers of {len(names)} distinct method names...")
    calls = build_call_counts(names)

    tally = Counter()
    for cls in report["classes"]:
        triaged = []
        for m in cls["absent_workspace_wide"]:
            body = method_body(cls["vanilla_path"], m)
            if body is not None:
                stripped = COMMENT_RE.sub("", body).strip()
                if stripped == "" or TRIVIAL_BODY_RE.fullmatch(stripped):
                    triaged.append({"method": m, "triage": "VANILLA_STUB"})
                    tally["VANILLA_STUB"] += 1
                    continue
            if calls[m] == 0:
                triaged.append({"method": m, "triage": "NO_CALLER"})
                tally["NO_CALLER"] += 1
                continue
            triaged.append({"method": m, "triage": "NEEDS_REVIEW"})
            tally["NEEDS_REVIEW"] += 1
        cls["absent_triaged"] = triaged

    total = sum(tally.values())
    report["triage"] = {
        "generated_by": "conformance/vanilla_triage.py",
        "totals": dict(tally),
        "method": (
            "VANILLA_STUB: vanilla body empty/trivial, nothing to port. "
            "NO_CALLER: name never invoked in the decompiled server tree (client-only or "
            "dead). NEEDS_REVIEW: everything else."
        ),
    }
    out = HERE / "method_gaps_triaged.json"
    out.write_text(json.dumps(report, indent=1))

    print(f"triaged {total} leads:")
    for k, v in tally.most_common():
        print(f"  {k:<14} {v:>5}  ({100 * v / total:.1f}%)")
    print(f"\n-> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
