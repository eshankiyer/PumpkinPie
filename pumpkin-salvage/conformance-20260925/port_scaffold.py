#!/usr/bin/env python3
"""
Emit a minimal porting scaffold per vanilla class: ONLY the methods still missing, in
Pumpkin's naming, with the vanilla body inlined as a reference comment.

WHY THIS EXISTS (it is a token-cost lever, not a code generator)
---------------------------------------------------------------
The raw transliteration in tools/transpile_vanilla.py emits every method of a class --
Entity has 422. Handing that to a model to port means paying for 422 methods of context
to work on the handful that are actually absent. This joins three sources so the unit of
work is right-sized:

    tools/transpile_vanilla.py  -- structural skeleton, signatures already in Rust shape
    conformance/method_gaps.py  -- which methods have no Rust counterpart
    conformance/vanilla_triage.py -- which of those are worth reviewing at all

and writes only the NEEDS_REVIEW absent methods. On the 2026-08-06 data that is 4198
methods instead of 17309, and they arrive grouped by class with the vanilla source
attached, so no decompile spelunking is needed per method.

CALIBRATION -- READ BEFORE TRUSTING OUTPUT
A measured 18-lead sample put the gap detector's precision near 1 in 6 (see
method_gaps.py's docstring). These scaffolds are therefore LEADS: expect most to turn out
already implemented under a Yarn rename or inlined into a caller. Confirm against the
decompile before writing code, and when a lead proves false, add the rename to
METHOD_ALIASES so it is never paid for twice.
"""
import json
import os
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DECOMP = Path(
    os.environ.get("PUMPKIN_DECOMP", Path.home() / "pumpkin-vanilla-26.2/decompiled")
)
DEFAULT_OUT = Path.home() / "pumpkin-vanilla-26.2/scaffold"

sys.path.insert(0, str(HERE))
from method_gaps import METHOD_ALIASES, camel_to_snake  # noqa: E402
from vanilla_triage import method_body  # noqa: E402


def pumpkin_name(java_method: str) -> tuple[str, str | None]:
    """Preferred Rust fn name, plus the alias source when one is known."""
    alias = METHOD_ALIASES.get(java_method)
    if alias:
        chosen = sorted(alias)[0]
        return chosen, f"Pumpkin names this `{chosen}` (METHOD_ALIASES)"
    return camel_to_snake(java_method), None


def main() -> int:
    src = HERE / "method_gaps_triaged.json"
    if not src.exists():
        print("method_gaps_triaged.json missing -- run method_gaps.py then "
              "vanilla_triage.py", file=sys.stderr)
        return 1
    report = json.loads(src.read_text())
    out_root = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_OUT

    classes = written = methods_out = 0
    for cls in report["classes"]:
        todo = [t["method"] for t in cls.get("absent_triaged", [])
                if t["triage"] == "NEEDS_REVIEW"]
        if not todo:
            continue
        classes += 1

        lines = [
            f"// Porting scaffold for vanilla `{cls['vanilla_class']}`.",
            f"// Vanilla : {cls['vanilla_path']}",
            f"// Pumpkin : {cls['rust_file']}",
            "//",
            f"// {len(todo)} of {cls['total_methods']} methods have no Rust counterpart by",
            "// name. Roughly 1 in 6 such leads is a real gap -- the rest are Yarn renames",
            "// or behaviour inlined into a caller. Verify before implementing, and record",
            "// any rename in conformance/method_gaps.py METHOD_ALIASES.",
            "",
        ]
        for jm in todo:
            rust_fn, note = pumpkin_name(jm)
            body = method_body(cls["vanilla_path"], jm)
            lines.append(f"// --- vanilla {cls['vanilla_class']}.{jm} ---")
            if note:
                lines.append(f"// {note}")
            if body:
                snippet = [l for l in body.splitlines() if l.strip()][:20]
                lines += [f"//   {l.strip()}" for l in snippet]
                if len(body.splitlines()) > 20:
                    lines.append("//   ... (truncated; read the decompile for the rest)")
            lines.append(f"fn {rust_fn}() {{ todo!() }}")
            lines.append("")
            methods_out += 1

        rel = Path(cls["vanilla_path"]).with_suffix("")
        dst = out_root / rel.parent / f"{camel_to_snake(rel.name)}.rs"
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_text("\n".join(lines))
        written += 1

    print(f"scaffolded {methods_out} methods across {written} classes -> {out_root}")
    print(f"(vs {report['methods_total']} methods if whole classes were emitted)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
