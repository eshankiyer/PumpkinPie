#!/usr/bin/env python3
"""
Use a LOCAL model to propose vanilla-method -> Pumpkin-fn renames, then verify every
proposal mechanically. Costs no API tokens.

WHY THIS JOB AND NOT TRANSLATION
--------------------------------
A measured 18-lead sample found 10 of 18 "missing" methods were actually implemented under
a Yarn rename or inlined into a caller -- renames are the single largest error source in
the gap detector. Each rename found once and written into METHOD_ALIASES is never paid for
again.

This task suits a small local model precisely because it is verifiable:
  * the answer must be one of a fixed candidate list, so a hallucinated name is caught by
    a set membership test, not by judgement;
  * a wrong answer is cheap (a lead stays in the pile) and cannot corrupt code;
  * the prompt is tiny -- one method signature plus a shortlist.

It does NOT ask the model to write Rust. Small local models do not produce
Pumpkin-idiomatic Rust, and code that has to be repaired costs more than it saves.

VERIFICATION (mandatory -- do not remove)
A proposal is accepted only if the name exists in the workspace fn index. Local workers in
this project have previously reported confident results that did not survive checking, so
every suggestion is treated as untrusted until the grep agrees. Accepted proposals are
still LEADS: spot-check a sample before pasting them into METHOD_ALIASES.
"""
import json
import os
import subprocess
import sys
import urllib.request
from difflib import get_close_matches
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent
OLLAMA = os.environ.get("OLLAMA_URL", "http://localhost:11434/api/generate")
MODEL = os.environ.get("PUMPKIN_LLM", "qwen2.5-coder:7b")

sys.path.insert(0, str(HERE))
from method_gaps import CRATES, all_fn_names, camel_to_snake  # noqa: E402
from vanilla_triage import method_body  # noqa: E402

PROMPT = """You map Java method names from Minecraft to their Rust equivalents in a \
reimplementation that uses Yarn-style naming.

Java method: {cls}.{meth}
Java body:
{body}

Candidate Rust function names actually present in the codebase:
{cands}

Which candidate implements the same behaviour? Answer with JSON only:
{{"match": "<candidate name or null>"}}
Answer null unless you are confident. A wrong guess is worse than null."""


def build_fn_locations() -> dict[str, list[str]]:
    """Rust fn name -> every file declaring it, for the same-subtree verification gate."""
    out = subprocess.run(
        ["rg", "--no-heading", "--with-filename", "-g", "*.rs", "-o",
         r"\bfn\s+(\w+)", "-r", "$1"] + CRATES,
        cwd=REPO_ROOT, capture_output=True, text=True, check=False,
    )
    locs: dict[str, list[str]] = {}
    for line in out.stdout.splitlines():
        path, _, name = line.partition(":")
        locs.setdefault(name, []).append(path)
    return locs


def ask(prompt: str) -> str:
    """Query the local ollama server, or OpenRouter when OPENROUTER_API_KEY is set.

    OpenRouter free-tier models (`:free` suffix) are capped at 50 requests/day and
    20/min, so a full sweep of the lead pool takes days there. That is fine for a
    slow background alias-mining loop -- the table it produces is permanent -- but it
    is not a way to do interactive work.
    """
    # Read from a mode-600 file when the env var is absent. This repo's shell is fish,
    # whose config.fish is not sourced by the bash/python this runs under, and it keeps
    # the secret out of process listings and shell history.
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        key_file = Path.home() / ".config/openrouter.key"
        if key_file.exists():
            key = key_file.read_text().strip()
    if key:
        payload = json.dumps({
            "model": os.environ.get("PUMPKIN_LLM_REMOTE",
                                    "qwen/qwen3-coder-480b:free"),
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0, "max_tokens": 64,
            "response_format": {"type": "json_object"},
        }).encode()
        req = urllib.request.Request(
            "https://openrouter.ai/api/v1/chat/completions", data=payload,
            headers={"Content-Type": "application/json",
                     "Authorization": f"Bearer {key}"})
        with urllib.request.urlopen(req, timeout=180) as r:
            return json.loads(r.read())["choices"][0]["message"]["content"]

    payload = json.dumps({
        "model": MODEL, "prompt": prompt, "stream": False, "format": "json",
        "options": {"temperature": 0, "num_predict": 64},
    }).encode()
    req = urllib.request.Request(OLLAMA, data=payload,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=180) as r:
        return json.loads(r.read())["response"]


def main() -> int:
    limit = int(sys.argv[1]) if len(sys.argv) > 1 else 40
    triaged = HERE / "method_gaps_triaged.json"
    if not triaged.exists():
        print("run method_gaps.py then vanilla_triage.py first", file=sys.stderr)
        return 1
    report = json.loads(triaged.read_text())
    fns = all_fn_names()
    fn_locations = build_fn_locations()
    cls_refs = {c["vanilla_class"]: c["rust_file"] for c in report["classes"]}

    leads = []
    for cls in report["classes"]:
        for t in cls.get("absent_triaged", []):
            if t["triage"] == "NEEDS_REVIEW":
                leads.append((cls["vanilla_class"], cls["vanilla_path"], t["method"]))
    leads = leads[:limit]
    print(f"asking {MODEL} about {len(leads)} leads (local, no API cost)")

    accepted, rejected, nulls = [], 0, 0
    for cls, path, meth in leads:
        snake = camel_to_snake(meth)
        # Shortlist keeps the prompt small and bounds what the model may answer with.
        cands = get_close_matches(snake, fns, n=12, cutoff=0.55)
        if not cands:
            nulls += 1
            continue
        body = (method_body(path, meth) or "")[:400]
        try:
            raw = ask(PROMPT.format(cls=cls, meth=meth, body=body,
                                    cands="\n".join(cands)))
            pick = (json.loads(raw) or {}).get("match")
        except Exception:
            pick = None
        if not pick or pick == "null":
            nulls += 1
            continue
        # VERIFICATION, two gates. Existence alone is NOT enough: the first version of
        # this script accepted 5 proposals whose names all existed and all of which were
        # wrong -- absSnapRotationTo -> set_rotation (a WASM plugin binding),
        # baseTick -> mob_tick (a mob AI hook), applyImplicitComponent -> set_component
        # (a WASM item_stack binding). Existence catches hallucination, not wrongness.
        #
        # Gate 2 requires the candidate to live in the same module subtree as the Rust
        # file this vanilla class already maps to, which rejects cross-subsystem matches
        # like an Entity method resolving into plugin/loader/wasm.
        if not (pick in fns and pick in cands):
            rejected += 1
            continue
        where = fn_locations.get(pick, [])
        scope = str(Path(cls_ref).parent) if (cls_ref := cls_refs.get(cls)) else None
        if scope and not any(w.startswith(scope) for w in where):
            rejected += 1
            continue
        accepted.append({"vanilla": meth, "class": cls, "rust": pick,
                         "rust_file": next((w for w in where if scope and w.startswith(scope)), where[0] if where else None)})

    out = HERE / "llm_alias_proposals.json"
    out.write_text(json.dumps({
        "model": MODEL,
        "note": "LEADS ONLY -- spot-check before adding to METHOD_ALIASES.",
        "accepted": accepted, "rejected_hallucinations": rejected, "declined": nulls,
    }, indent=1))
    print(f"accepted {len(accepted)}  rejected(not in codebase) {rejected}  declined {nulls}")
    for a in accepted[:15]:
        print(f"  {a['class']}.{a['vanilla']} -> {a['rust']}")
    print(f"\n-> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
