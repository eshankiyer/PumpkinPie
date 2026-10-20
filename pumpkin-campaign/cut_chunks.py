#!/usr/bin/env python3
"""Cut method-gap worklist chunks for parity dispatch lanes."""
import json, os, sys, pathlib

C = pathlib.Path("/home/eshanki/pumpkin-campaign")
GAPS = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "conformance/method_gaps.json")
PREFIX = os.environ.get("PREFIX", "mg")
NCHUNKS = int(os.environ.get("NCHUNKS", "4"))
PER = int(os.environ.get("PER_CHUNK", "5"))
MIN_ABSENT = int(os.environ.get("MIN_ABSENT", "8"))
MAX_ABSENT = int(os.environ.get("MAX_ABSENT", "40"))
OUT = pathlib.Path(os.environ.get("OUTDIR", str(C / "chunks")))
OUT.mkdir(parents=True, exist_ok=True)

done_path = C / "done_classes.txt"
done = set(done_path.read_text().split()) if done_path.exists() else set()

classes = json.loads(GAPS.read_text())["classes"]
cands = [
    c for c in classes
    if MIN_ABSENT <= len(c.get("absent_workspace_wide", [])) <= MAX_ABSENT
    and c["vanilla_class"] not in done
]
cands.sort(key=lambda c: -len(c["absent_workspace_wide"]))

taken = []
for i in range(NCHUNKS):
    chunk = cands[i * PER:(i + 1) * PER]
    if not chunk:
        break
    p = OUT / f"{PREFIX}_chunk{i + 1}.json"
    p.write_text(json.dumps(chunk, indent=2))
    taken += [c["vanilla_class"] for c in chunk]
    print(f"{p}  {len(chunk)} classes  {sum(len(c['absent_workspace_wide']) for c in chunk)} leads")

with done_path.open("a") as f:
    for name in taken:
        f.write(name + "\n")
print(f"marked {len(taken)} classes done; {len(cands) - len(taken)} candidates remain in band")
