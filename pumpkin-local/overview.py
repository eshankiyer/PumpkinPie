#!/usr/bin/env python3
"""
One-screen live overview of the whole campaign.

    python3 overview.py           # refresh every 10s
    python3 overview.py --once    # single snapshot
    python3 overview.py --no-net  # skip the GitHub call, much faster

Shows: the local agent run, GPU and model state, open PRs, worktrees, and the
backlog counts. Safe to run alongside a batch; it only reads.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time

STATUS = "/tmp/pumpkin_run_status.json"
LOG = "/tmp/overnight.log"
DONE = "/tmp/pumpkin_run_DONE.txt"
REPO = "/home/eshanki/Pumpkin"
LOCAL = "/home/eshanki/pumpkin-local"
TERMINAL = ("clean", "aborted", "failed")


def sh(cmd, timeout=30):
    try:
        p = subprocess.run(
            ["bash", "-c", cmd], capture_output=True, text=True, timeout=timeout
        )
        return p.stdout.strip()
    except Exception:
        return ""


def rule(title):
    return f"\n\033[1m{title}\033[0m\n" + "-" * 62


def agent_section():
    out = [rule("LOCAL AGENT")]
    running = bool(sh("pgrep -f 'python3 run_all.py' | head -1"))
    try:
        st = json.load(open(STATUS))
    except (OSError, json.JSONDecodeError):
        st = {}
    if not st:
        out.append("  no run recorded")
        return out
    done = sum(1 for v in st.values() if str(v.get("phase", "")).startswith(TERMINAL))
    txt = ""
    try:
        txt = open(LOG, errors="replace").read()
    except OSError:
        pass
    replies = [int(x) for x in re.findall(r"replied in (\d+)s", txt)]
    clean = txt.count("BUILD CLEAN")
    out.append(
        f"  {'RUNNING' if running else 'stopped'}   "
        f"{done}/{len(st)} finished   {clean} built clean   "
        f"{len(replies)} model calls"
        + (f", avg {sum(replies) // len(replies)}s" if replies else "")
    )
    for name in sorted(st):
        ph = str(st[name].get("phase", "?"))
        mark = (
            "OK" if ph.startswith("clean") else "XX" if ph.startswith(TERMINAL) else ".."
        )
        out.append(f"    {mark} {name:<26} {ph}")
    if os.path.exists(DONE):
        out.append("  DONE marker present")
    return out


def gpu_section():
    out = [rule("GPU AND MODEL")]
    g = sh(
        "nvidia-smi --query-gpu=memory.used,memory.total,utilization.gpu "
        "--format=csv,noheader 2>/dev/null"
    )
    out.append(f"  vram/util : {g or 'n/a'}")
    model = sh(
        "grep -h 'PUMPKIN_LOCAL_MODEL' " + os.path.join(LOCAL, "run_all.py") + " | head -1"
    )
    m = re.search(r'"([^"]+)"\s*\)\s*$', model or "")
    out.append(f"  model     : {m.group(1) if m else 'unknown'}")
    # -x matches the executable name, not the whole command line. With -f the
    # count includes this very shell, because "llama-server" appears in its own
    # command line, so it always read one too high and cried orphan. Same trap
    # that made `pkill -f ollama` kill its own wrapper. See DECISIONS.md.
    procs = sh("pgrep -c -x llama-server 2>/dev/null") or "0"
    note = "" if procs in ("0", "1") else "  <- ORPHAN, run serve.sh"
    out.append(f"  llama-server procs: {procs}{note}")
    return out


def pr_section(skip_net):
    out = [rule("UPSTREAM PRs")]
    if skip_net:
        out.append("  skipped (--no-net)")
        return out
    raw = sh(
        "cd %s && gh pr list --repo Pumpkin-MC/Pumpkin --author eshankiyer "
        "--state open --json number,title,reviewDecision --limit 60" % REPO,
        timeout=60,
    )
    try:
        prs = json.loads(raw)
    except Exception:
        out.append("  could not query GitHub")
        return out
    out.append(f"  {len(prs)} open")
    for p in sorted(prs, key=lambda x: -x["number"])[:8]:
        rd = p.get("reviewDecision") or "-"
        out.append(f"    #{p['number']}  {rd:<9} {p['title'][:44]}")
    if len(prs) > 8:
        out.append(f"    ... and {len(prs) - 8} more")
    return out


def backlog_section():
    out = [rule("BACKLOG")]
    for name, path in (
        ("WORKQUEUE.md", os.path.join(LOCAL, "WORKQUEUE.md")),
        ("PARITY.md", os.path.join(REPO, "PARITY.md")),
    ):
        try:
            text = open(path).read()
        except OSError:
            out.append(f"  {name}: missing")
            continue
        conf = len(re.findall(r"(?mi)^STATE\s+confirmed|\|\s*confirmed\s*\|", text))
        susp = len(re.findall(r"(?mi)^STATE\s+suspected|\|\s*suspected\s*\|", text))
        abst = len(re.findall(r"(?mi)^STATE\s+absent|\|\s*absent\s*\|", text))
        rows = text.count("\n|") if "|" in text else text.count("\n### ")
        out.append(
            f"  {name:<14} ~{rows} rows   confirmed {conf}  suspected {susp}  absent {abst}"
        )
    specs = sh(f"ls {LOCAL}/specs/*.md 2>/dev/null | wc -l")
    out.append(f"  specs ready    {specs}")
    wt = sh(f"cd {REPO} && git worktree list | wc -l")
    out.append(f"  worktrees      {wt}")
    return out


def bar(done, total, width=40):
    """A plain progress bar. No colour, so it survives being piped to a file."""
    total = max(1, total)
    filled = int(width * done / total)
    pct = 100.0 * done / total
    return f"[{'#' * filled}{'.' * (width - filled)}] {done}/{total}  {pct:5.1f}%"


def score_section():
    """The objective function, and the queue burndown, as bars."""
    out = [rule("PROGRESS")]

    path = os.path.join(LOCAL, "coverage", "PARITY_SCORE.md")
    score, when = None, ""
    try:
        text = open(path).read()
        m = re.search(r"\*\*SCORE: ([\d.]+) / 100\*\*\s+measured (\S+)", text)
        if m:
            score, when = float(m.group(1)), m.group(2)
    except OSError:
        pass
    if score is None:
        out.append("  PARITY_SCORE   not measured yet")
    else:
        out.append(f"  PARITY_SCORE   {bar(score, 100)}")
        out.append(f"                 measured {when}")
        try:
            for line in open(path).read().splitlines():
                m = re.match(r"\| (\w+) \| (\d+) \| ([\d.]+) \| ([\d.]+) \|", line)
                if m:
                    name, weight, _, pts = m.groups()
                    out.append(
                        f"    {name:<10} {bar(float(pts), float(weight), 24)}"
                    )
        except OSError:
            pass

    # Spec queue burndown: how many have cleared the gate.
    try:
        st = json.load(open(STATUS))
    except (OSError, json.JSONDecodeError):
        st = {}
    specs = sorted(
        os.path.basename(p)[:-3]
        for p in __import__("glob").glob(os.path.join(LOCAL, "specs", "[0-9]*.md"))
    )
    passed = sum(
        1 for s in specs if st.get(s, {}).get("phase") in ("candidate", "clean")
    )
    out.append(f"  spec queue     {bar(passed, len(specs))}")
    return out


def budget_section():
    """What is left of today's Claude allowance, and what spent it."""
    out = [rule("CLAUDE BUDGET (the only thing that costs money)")]
    try:
        data = json.load(open(os.path.join(LOCAL, "BUDGET.json")))
    except (OSError, json.JSONDecodeError):
        out.append("  nothing spent today")
        return out
    spent = data.get("spent", {})
    total = sum(spent.values())
    out.append(f"  today {data.get('date', '?')}   {bar(total, 8, 24)}")
    for tier, cap in (("scaffold", 2), ("escalate", 3)):
        out.append(f"    {tier:<10} {bar(spent.get(tier, 0), cap, 12)}")
    out.append("  the local 30B is free and does the bulk of the work")
    return out


def loop_section():
    """Where the autonomous loop currently is."""
    out = [rule("LOOP")]
    alive = bool(sh("pgrep -f 'bash /home/eshanki/pumpkin-local/nightly' | head -1"))
    out.append(f"  nightly        {'RUNNING' if alive else 'stopped'}")
    tail = sh("tail -400 /tmp/pumpkin_nightly.log 2>/dev/null")
    rounds = re.findall(r"---- round (\d+) of (\d+)", tail)
    if rounds:
        cur, tot = rounds[-1]
        out.append(f"  round          {bar(int(cur), int(tot), 24)}")
    phase = "unknown"
    for marker in (
        "[health]", "[sync]", "[sweep]", "[scaffold]", "[agent]",
        "[escalate]", "[ship]", "[score]",
    ):
        if marker in tail:
            idx = tail.rfind(marker)
            phase = marker.strip("[]")
            _ = idx
    last = [ln for ln in tail.splitlines() if ln.startswith("[")]
    if last:
        phase = last[-1].strip("[]").split("]")[0]
    out.append(f"  phase          {phase}")
    return out


def render(skip_net):
    parts = []
    parts += [f"\033[1mPUMPKIN CAMPAIGN\033[0m   {time.strftime('%H:%M:%S')}"]
    parts += score_section()
    parts += loop_section()
    parts += budget_section()
    parts += agent_section()
    parts += gpu_section()
    parts += backlog_section()
    parts += pr_section(skip_net)
    return "\n".join(parts)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--once", action="store_true")
    ap.add_argument("--no-net", action="store_true")
    ap.add_argument("--interval", type=float, default=10.0)
    a = ap.parse_args()
    while True:
        text = render(a.no_net)
        if a.once:
            print(text)
            return 0
        os.system("clear")
        print(text)
        time.sleep(a.interval)


if __name__ == "__main__":
    sys.exit(main())
