#!/usr/bin/env python3
"""Build the implementation-tracker dataset served at /tracker/.

Reads the local conformance run (`method_gaps.json`, produced by
`conformance/method_gaps.py` against a 26.2 decompile) and emits the JSON the
tracker page fetches. The conformance directory is a local instrument and is not
checked in, so its path is passed on the command line:

    python3 tools/tracker/build_tracker.py ~/Pumpkin/conformance/method_gaps.json

What the numbers mean, so the page can be read honestly:

* A row is one vanilla class that Pumpkin declares an analogue of.
* `present` counts the class's vanilla methods that have a plausibly-named
  counterpart somewhere in the workspace; `absent` counts those that do not.
* Name matching is mechanical. An absent method may still be implemented under a
  different name or inlined into its caller, so `absent` is a lead count, not a
  defect count. The page says so.
"""

import argparse
import json
import pathlib
import sys

SUBSYSTEM_GROUPS = {
    "block": "blocks",
    "material": "blocks",
    "item": "items",
    "inventory": "items",
    "entity": "entities",
    "gameevent": "entities",
}


def group_for(subsystem: str) -> str:
    return SUBSYSTEM_GROUPS.get(subsystem, "other")


def status_for(present: int, absent: int) -> str:
    if absent == 0:
        return "complete"
    if present == 0:
        return "todo"
    return "partial"


def build(source: pathlib.Path) -> dict:
    raw = json.loads(source.read_text())
    classes = raw["classes"] if isinstance(raw, dict) and "classes" in raw else raw
    entries = classes.values() if isinstance(classes, dict) else classes

    rows = []
    for entry in entries:
        present = entry.get("in_declaring_file", 0) + entry.get("elsewhere_in_workspace", 0)
        absent = len(entry.get("absent_workspace_wide", []))
        rows.append(
            {
                "name": entry.get("vanilla_class"),
                "group": group_for(entry.get("subsystem", "")),
                "subsystem": entry.get("subsystem"),
                "vanilla_path": entry.get("vanilla_path"),
                "rust_file": entry.get("rust_file"),
                "present": present,
                "absent": absent,
                "total": present + absent,
                "status": status_for(present, absent),
            }
        )

    rows.sort(key=lambda row: (row["group"], row["name"] or ""))

    totals = {
        "classes": len(rows),
        "complete": sum(1 for row in rows if row["status"] == "complete"),
        "partial": sum(1 for row in rows if row["status"] == "partial"),
        "todo": sum(1 for row in rows if row["status"] == "todo"),
        "methods_total": sum(row["total"] for row in rows),
        "methods_present": sum(row["present"] for row in rows),
    }

    return {
        "milestone": raw.get("milestone") if isinstance(raw, dict) else None,
        "method": "mechanical name matching against a 26.2 decompile; absent counts are leads, not defects",
        "totals": totals,
        "rows": rows,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=pathlib.Path, help="path to method_gaps.json")
    parser.add_argument(
        "--out",
        type=pathlib.Path,
        default=pathlib.Path("docs/tracker/data.json"),
        help="where to write the tracker dataset",
    )
    args = parser.parse_args()

    if not args.source.is_file():
        print(f"no such file: {args.source}", file=sys.stderr)
        return 1

    data = build(args.source)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(data, separators=(",", ":")))
    totals = data["totals"]
    print(
        f"{totals['classes']} classes -> {args.out} "
        f"({totals['complete']} complete, {totals['partial']} partial, {totals['todo']} todo)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
