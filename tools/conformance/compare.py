#!/usr/bin/env python3
"""Compare two conformance.json runs on a like-for-like basis.

The headline percentages from conformance.py are NOT comparable between repos: the
denominator is "methods of classes this repo maps", so a repo that maps fewer classes
gets an easier denominator. This script joins two runs on vanilla_class and reports:

  * strict/loose for both repos restricted to the classes BOTH analysed
  * the class sets each repo covers uniquely
  * an absolute score over every enumerated vanilla class (unmapped == absent)

    python3 tools/conformance/compare.py A.json B.json --labels pumpkin steel
"""

import argparse
import json
import pathlib
import random
import sys
from collections import defaultdict

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))


def load(path):
    r = json.loads(pathlib.Path(path).read_text())
    return r, {c["vanilla_class"]: c for c in r["classes"]}


def scored(c):
    return c["total_methods"] - c["client_only_skipped"] - c["data_modeled_skipped"]


def tally(rows):
    s = sum(scored(c) for c in rows)
    strict = sum(c["in_declaring_file"] for c in rows)
    loose = sum(c["in_declaring_file"] + c["elsewhere_in_workspace"] for c in rows)
    return s, strict, loose


def pct(n, d):
    return round(100 * n / d, 2) if d else 0.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("a")
    ap.add_argument("b")
    ap.add_argument("--labels", nargs=2, default=["A", "B"])
    ap.add_argument("--sample", type=int, default=0)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--out", type=pathlib.Path)
    args = ap.parse_args()
    la, lb = args.labels

    ra, ca = load(args.a)
    rb, cb = load(args.b)

    common = sorted(set(ca) & set(cb))
    only_a = sorted(set(ca) - set(cb))
    only_b = sorted(set(cb) - set(ca))

    out = {"labels": [la, lb], "headline": {}, "common": {}, "sets": {}}
    for lab, r in ((la, ra), (lb, rb)):
        out["headline"][lab] = {
            "classes_analysed": r["classes_analysed"],
            "methods_scored": r["methods_scored"],
            "strict_pct": r["strict_match_pct"],
            "loose_pct": r["name_match_pct"],
            "absent": r["methods_absent_workspace_wide"],
        }

    for lab, idx in ((la, ca), (lb, cb)):
        rows = [idx[k] for k in common]
        s, st, lo = tally(rows)
        out["common"][lab] = {
            "classes": len(common),
            "methods_scored": s,
            "strict": st,
            "strict_pct": pct(st, s),
            "loose": lo,
            "loose_pct": pct(lo, s),
            "absent": s - lo,
        }

    out["sets"] = {
        f"only_{la}": len(only_a),
        f"only_{lb}": len(only_b),
        "common": len(common),
    }

    # absolute: every enumerated vanilla class, unmapped methods counted absent
    from conformance import enumerate_vanilla  # noqa: E402

    units = enumerate_vanilla()
    total_methods = sum(len(u["methods"]) for u in units)
    out["absolute"] = {"vanilla_classes": len(units), "vanilla_methods": total_methods}
    for lab, idx in ((la, ca), (lb, cb)):
        st = sum(c["in_declaring_file"] for c in idx.values())
        lo = sum(c["in_declaring_file"] + c["elsewhere_in_workspace"] for c in idx.values())
        out["absolute"][lab] = {
            "strict_pct_of_all_vanilla": pct(st, total_methods),
            "loose_pct_of_all_vanilla": pct(lo, total_methods),
        }

    # per-subsystem on the common set
    sub = defaultdict(lambda: defaultdict(lambda: [0, 0, 0]))
    for k in common:
        for lab, idx in ((la, ca), (lb, cb)):
            c = idx[k]
            v = sub[c["subsystem"]][lab]
            v[0] += c["in_declaring_file"]
            v[1] += c["in_declaring_file"] + c["elsewhere_in_workspace"]
            v[2] += scored(c)
    out["common_by_subsystem"] = {
        s: {lab: {"strict_pct": pct(v[0], v[2]), "loose_pct": pct(v[1], v[2]), "scored": v[2]}
            for lab, v in d.items()}
        for s, d in sorted(sub.items())
    }

    # classes only B covers, ranked by method count
    out[f"classes_only_{lb}"] = sorted(
        ({"vanilla_class": cb[k]["vanilla_class"], "subsystem": cb[k]["subsystem"],
          "vanilla_path": cb[k]["vanilla_path"], "total_methods": cb[k]["total_methods"]}
         for k in only_b),
        key=lambda r: -r["total_methods"],
    )
    out[f"classes_only_{la}"] = sorted(
        ({"vanilla_class": ca[k]["vanilla_class"], "subsystem": ca[k]["subsystem"],
          "total_methods": ca[k]["total_methods"]} for k in only_a),
        key=lambda r: -r["total_methods"],
    )

    # depth gaps: common classes where A is much thinner than B (loose tier)
    depth = []
    for k in common:
        x, y = ca[k], cb[k]
        sa, sb = scored(x), scored(y)
        a_loose = x["in_declaring_file"] + x["elsewhere_in_workspace"]
        b_loose = y["in_declaring_file"] + y["elsewhere_in_workspace"]
        if sa and sb:
            delta = pct(b_loose, sb) - pct(a_loose, sa)
            if delta > 0:
                depth.append({"vanilla_class": k, "subsystem": x["subsystem"],
                              "vanilla_path": x["vanilla_path"],
                              f"{la}_loose_pct": pct(a_loose, sa),
                              f"{lb}_loose_pct": pct(b_loose, sb),
                              "delta": round(delta, 1),
                              f"{la}_absent": len(x["absent_workspace_wide"]),
                              "scored": sa})
    out["depth_gaps"] = sorted(depth, key=lambda r: (-r[f"{la}_absent"]))

    if args.sample:
        leads = []
        for k in common:
            x, y = ca[k], cb[k]
            present_b = set(y["present_in_declaring_file"]) | set(y["present_elsewhere"])
            for m in x["absent_workspace_wide"]:
                if m in present_b:
                    leads.append({"vanilla_class": k, "method": m,
                                  "vanilla_path": x["vanilla_path"],
                                  f"{la}_file": x["rust_file"]})
        out["lead_pool_size"] = len(leads)
        rng = random.Random(args.seed)
        out["sample"] = {"seed": args.seed,
                         "leads": rng.sample(leads, min(args.sample, len(leads)))}

    text = json.dumps(out, indent=1)
    if args.out:
        args.out.write_text(text)

    h = out["headline"]
    print("HEADLINE (different denominators - NOT comparable)")
    for lab in (la, lb):
        print(f"  {lab:<10} {h[lab]['classes_analysed']:4} classes  {h[lab]['methods_scored']:5} methods"
              f"  strict {h[lab]['strict_pct']:5.2f}%  loose {h[lab]['loose_pct']:5.2f}%")
    print(f"\nCLASS SETS: common {len(common)}, only {la} {len(only_a)}, only {lb} {len(only_b)}")
    print("\nCOMMON-CLASS COMPARISON (apples to apples)")
    for lab in (la, lb):
        c = out["common"][lab]
        print(f"  {lab:<10} {c['methods_scored']:5} methods  strict {c['strict_pct']:5.2f}%"
              f"  loose {c['loose_pct']:5.2f}%  absent {c['absent']}")
    print("\nABSOLUTE (all %d enumerated vanilla methods)" % total_methods)
    for lab in (la, lb):
        a = out["absolute"][lab]
        print(f"  {lab:<10} strict {a['strict_pct_of_all_vanilla']:5.2f}%"
              f"  loose {a['loose_pct_of_all_vanilla']:5.2f}%")
    print("\nCOMMON-SET BY SUBSYSTEM (loose %)")
    for s, d in out["common_by_subsystem"].items():
        print(f"  {s:<13} scored {d[la]['scored']:5}  {la} {d[la]['loose_pct']:5.1f}%"
              f"   {lb} {d[lb]['loose_pct']:5.1f}%")
    if args.sample:
        print(f"\nlead pool (absent in {la}, present in {lb}, common classes): {out['lead_pool_size']}")
        for l in out["sample"]["leads"]:
            print(f"  {l['vanilla_class']}.{l['method']}  <-> {l[f'{la}_file']}")
    if args.out:
        print(f"\n-> {args.out}")


if __name__ == "__main__":
    main()
