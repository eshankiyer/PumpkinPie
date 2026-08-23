#!/usr/bin/env python3
"""
Generic, repo-agnostic mechanical name-matching scanner. Same algorithm run unchanged
against any Rust repo's checkout - no per-repo hardcoded tables, so it's fair across
codebases with unrelated naming conventions (PumpkinPie/Pumpkin share ancestry; SteelMC
does not).

For each vanilla 26.2 class: a repo "covers" it if some struct/enum/trait declaration's
name, after stripping a common suffix (Entity/Block/Item/BlockEntity/Packet/Screen/Menu),
equals the vanilla class's simple name with the same suffix stripped (or matches exactly
without stripping). Method presence is checked by exact-or-snake_case substring search
across the file(s) declaring the matched type.
"""
import json
import re
import sys
from pathlib import Path

SUFFIXES = ["BlockEntity", "Entity", "Block", "Item", "Packet", "Screen", "Menu", "Recipe"]

TYPE_DECL_RE = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|trait)\s+(\w+)', re.M)
FN_DECL_RE = re.compile(r'\bfn\s+(\w+)')


def strip_suffix(name):
    for s in SUFFIXES:
        if name.endswith(s) and len(name) > len(s):
            return name[: -len(s)]
    return name


def camel_to_snake(name):
    s1 = re.sub(r'(.)([A-Z][a-z]+)', r'\1_\2', name)
    return re.sub(r'([a-z0-9])([A-Z])', r'\1_\2', s1).lower()


def build_index(repo_root):
    """Scan every .rs file once; return {type_name: [file_paths]} and {file_path: full_text}."""
    type_to_files = {}
    file_text = {}
    for rs in Path(repo_root).rglob("*.rs"):
        if "/target/" in str(rs) or "/.git/" in str(rs):
            continue
        try:
            text = rs.read_text(errors="ignore")
        except Exception:
            continue
        file_text[str(rs)] = text
        for m in TYPE_DECL_RE.finditer(text):
            type_to_files.setdefault(m.group(1), []).append(str(rs))
    return type_to_files, file_text


def find_match(vanilla_simple, type_to_files):
    if vanilla_simple in type_to_files:
        return vanilla_simple, type_to_files[vanilla_simple]
    stripped = strip_suffix(vanilla_simple)
    if stripped != vanilla_simple and stripped in type_to_files:
        return stripped, type_to_files[stripped]
    # try stripped-vs-stripped across all declared types (handles e.g. AmethystBlock vs Amethyst)
    for tname, files in type_to_files.items():
        if strip_suffix(tname) == stripped:
            return tname, files
    return None, None


def scan(repo_root, vanilla_units):
    type_to_files, file_text = build_index(repo_root)
    rows = []
    for unit in vanilla_units:
        simple = unit["vanilla_class"]
        matched_type, files = find_match(simple, type_to_files)
        if not matched_type:
            rows.append({
                "name": simple, "subsystem": unit.get("subsystem", "?"),
                "vanilla_path": unit["vanilla_path"], "matched_type": None,
                "present": 0, "total": len(unit["methods"]), "status": "todo",
            })
            continue
        scope_text = "\n".join(file_text[f] for f in files)
        declared_fns = set(FN_DECL_RE.findall(scope_text))
        declared_fns_lower = {f.lower() for f in declared_fns}
        present = 0
        for method in unit["methods"]:
            snake = camel_to_snake(method)
            if method in declared_fns or snake in declared_fns or method.lower() in declared_fns_lower:
                present += 1
            elif method in scope_text or snake in scope_text:
                present += 1
        total = len(unit["methods"])
        status = "complete" if present == total and total > 0 else ("partial" if present > 0 else "todo")
        rows.append({
            "name": simple, "subsystem": unit.get("subsystem", "?"),
            "vanilla_path": unit["vanilla_path"], "matched_type": matched_type,
            "present": present, "total": total, "status": status,
        })
    return rows


def main():
    repo_root = sys.argv[1]
    units_path = sys.argv[2]
    out_path = sys.argv[3]
    vanilla_units = json.loads(Path(units_path).read_text())
    rows = scan(repo_root, vanilla_units)
    classes = len(rows)
    complete = sum(1 for r in rows if r["status"] == "complete")
    partial = sum(1 for r in rows if r["status"] == "partial")
    todo = sum(1 for r in rows if r["status"] == "todo")
    methods_total = sum(r["total"] for r in rows)
    methods_present = sum(r["present"] for r in rows)
    out = {
        "repo": repo_root,
        "totals": {
            "classes": classes, "classes_with_analogue": complete + partial,
            "complete": complete, "partial": partial, "todo": todo,
            "methods_total": methods_total, "methods_present": methods_present,
        },
        "rows": rows,
    }
    Path(out_path).write_text(json.dumps(out, indent=1))
    print(f"{repo_root}: classes_with_analogue={complete+partial}/{classes} "
          f"methods={methods_present}/{methods_total} -> {out_path}")


if __name__ == "__main__":
    main()
