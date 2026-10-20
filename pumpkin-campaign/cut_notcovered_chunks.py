#!/usr/bin/env python3
"""Cut worklist chunks from genuinely NOT-covered vanilla classes (no Rust analogue at
all), for real-feature dispatch rather than gap-filling an existing file. Filters out
likely false negatives (an abstract/renamed vanilla class whose concrete behavior
already has a Rust file under a different stem) using the same suffix/prefix stripping
cut_chunks.py's covered-side matching already relies on, applied here to the raw
not_covered list.
"""
import json, os, re, subprocess, pathlib

C = pathlib.Path("/home/eshanki/pumpkin-campaign")
REPO = pathlib.Path(os.environ.get("PUMPKIN_REPO", "/home/eshanki/pumpkin-wt/staging"))
PREFIX = os.environ.get("PREFIX", "nc")
NCHUNKS = int(os.environ.get("NCHUNKS", "4"))
PER = int(os.environ.get("PER_CHUNK", "1"))
MIN_METHODS = int(os.environ.get("MIN_METHODS", "6"))
MAX_METHODS = int(os.environ.get("MAX_METHODS", "30"))
OUT = pathlib.Path(os.environ.get("OUTDIR", str(C / "chunks")))
OUT.mkdir(parents=True, exist_ok=True)

done_path = C / "done_notcovered_classes.txt"
done = set(done_path.read_text().split()) if done_path.exists() else set()

catalog = json.loads((REPO / "conformance/catalog.json").read_text())
units = [u for u in catalog["units"] if u["coverage"] == "not_covered"]

rs_files = subprocess.run(["rg", "--files", "-g", "*.rs", "crates"], cwd=REPO,
                           capture_output=True, text=True).stdout.splitlines()
stems = set(pathlib.Path(f).stem.lower() for f in rs_files)

def camel_to_snake(name):
    return re.sub(r'(?<!^)(?=[A-Z])', '_', name).lower()

def strip_prefixes(name):
    variants = [name]
    for pre in ("Abstract", "Base"):
        if name.startswith(pre):
            variants.append(name[len(pre):])
    return variants

def has_hit(name):
    for v in strip_prefixes(name):
        snake = camel_to_snake(v)
        if snake in stems or v.lower() in stems:
            return True
        for suf in ("Block", "Item", "Entity", "Impl", "Boss", "Menu", "Fluid", "Argument", "Goal", "Mob"):
            if v.endswith(suf) and len(v) > len(suf):
                if camel_to_snake(v[:-len(suf)]) in stems:
                    return True
    return False

cands = [
    u for u in units
    if MIN_METHODS <= u["method_count"] <= MAX_METHODS
    and u["vanilla_class"] not in done
    and not has_hit(u["vanilla_class"])
]
cands.sort(key=lambda u: -u["method_count"])

taken = []
for i in range(NCHUNKS):
    chunk = cands[i * PER:(i + 1) * PER]
    if not chunk:
        break
    payload = [{
        "subsystem": u["subsystem"],
        "vanilla_class": u["vanilla_class"],
        "vanilla_path": u["vanilla_path"],
        "method_count": u["method_count"],
        "methods": u["methods"],
    } for u in chunk]
    p = OUT / f"{PREFIX}_chunk{i + 1}.json"
    p.write_text(json.dumps(payload, indent=2))
    taken += [u["vanilla_class"] for u in chunk]
    print(f"{p}  {[c['vanilla_class'] for c in chunk]}")

with done_path.open("a") as f:
    for name in taken:
        f.write(name + "\n")
print(f"marked {len(taken)} classes done; {len(cands) - len(taken)} remain in band")
