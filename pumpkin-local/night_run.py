#!/usr/bin/env python3
"""
Unattended entrypoint for the Pumpkin parity port loop.

One command, run from cron, that turns a night of idle GPU into a handful of
reviewed, gate-passing commits on a fork branch and a report a human can read in
the morning. It is the assembly line, not a new machine: every station already
exists (mini_spree for the local pass, mini_escalate for the sonnet tier,
gate.py for the referee, ref_lookup.py for clean-room references, coverage.py
for the score) and this file only sequences them and enforces the discipline
between them.

Methodology, lifted straight from the migration-loop playbook and not
negotiable here:

  * the queue is rebuilt from disk every run. An item is DONE iff a passing gate
    verdict exists for one of its candidate diffs. Nothing is remembered between
    runs except what is written on disk, so a crash mid-night resumes cleanly.
  * the gate is a MECHANICAL referee. This file never inspects a diff and
    decides it looks fine; a change ships iff gate.py exits 0 on it.
  * fix the PROCESS, not the code. When the same failure reason recurs, this
    file writes a RULEBOOK rule that future prompts inherit. It never edits a
    candidate to paper over a pattern.

Model discipline is a hard project rule and is asserted at startup: the local
27B is free and does the grinding, sonnet is the ONLY escalation tier, and
nothing above sonnet may appear in any runtime path. Pushes go to the `fork`
remote only, never origin/upstream, never force.

    python3 night_run.py --dry-run     # parse, check state, print plan, touch nothing
    python3 night_run.py               # the real night
    python3 night_run.py --limit 3     # cap items attempted (a supervised trial)
"""

import argparse
import datetime as dt
import fcntl
import glob
import json
import os
import re
import subprocess
import sys
import time
import urllib.request

import mini_spree as ms        # parse_queue, eligible, run_item, task_text, sh, WORKTREE
import mini_escalate as me      # escalate_one, probe_quota, MODEL (must be sonnet)
import budget                    # shared per-day claude spend cap

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.environ.get("PUMPKIN_REPO", os.path.expanduser("~/Pumpkin"))
CAND = os.path.join(HERE, "candidates")
LOGS = os.path.join(HERE, "logs")
REPORTS = os.path.join(HERE, "reports")
RULEBOOK = os.path.join(HERE, "RULEBOOK.md")
GATE = os.path.join(HERE, "gate.py")
REF_LOOKUP = os.path.join(HERE, "ref_lookup.py")
GATE_FAILURES = os.path.join(HERE, "gate_failures.log")
COVSTATE = os.path.join(REPORTS, ".coverage_state.json")
LOCKFILE = "/tmp/pumpkin_night_run.lock"

# The one branch this loop ships to, and its base. Never reset the branch to the
# base once it exists: each night stacks commits on top of the last.
BRANCH = "autoport/nightly"
BASE = "verify/all"
REMOTE = "fork"                 # github.com/eshankiyer/Pumpkin, HARD: never origin/upstream

MODEL_HEALTH = "http://127.0.0.1:11434/health"
SERVE = os.path.join(HERE, "serve.sh")

# Same attributable identity ship.py commits under, so a maintainer can tell in
# one glance which commits came out of the automated loop.
AUTHOR_NAME = "Pumpkin Parity Pipeline"
AUTHOR_EMAIL = "rdravida+pumpkin-pipeline@gmail.com"

RULEBOOK_TEXT = ""              # loaded once at run start, injected into every prompt


# --------------------------------------------------------------------------- #
# small process helpers - every subprocess in this file has a timeout, always. #
# --------------------------------------------------------------------------- #

def sh(cmd, cwd=None, timeout=600):
    """bash -c with a hard timeout. Returns (rc, combined_output)."""
    try:
        p = subprocess.run(
            ["bash", "-c", cmd], cwd=cwd, capture_output=True, text=True,
            timeout=timeout,
        )
        return p.returncode, (p.stdout or "") + (p.stderr or "")
    except subprocess.TimeoutExpired as e:
        return 124, f"TIMEOUT after {timeout}s: {e}"


def git(cmd, cwd=REPO, timeout=600):
    """git under the pipeline identity, without touching any global config."""
    ident = f'-c user.name="{AUTHOR_NAME}" -c user.email="{AUTHOR_EMAIL}"'
    return sh(f"git {ident} {cmd}", cwd=cwd, timeout=timeout)


def load_json(path, default):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return default


def save_json(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(data, f, indent=2)


def slug(text, n=30):
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")[:n]


# --------------------------------------------------------------------------- #
# model server - the whole night is worthless if inference silently falls to   #
# the CPU, so bring it up per serve.sh conventions before any spree.           #
# --------------------------------------------------------------------------- #

def model_up():
    try:
        with urllib.request.urlopen(MODEL_HEALTH, timeout=10) as r:
            return r.status == 200
    except Exception:
        return False


def ensure_model():
    """True if the model answers /health, starting it via serve.sh if not.

    serve.sh re-runs the stray-kill and the VRAM assertion and refuses to run
    the 27B on the CPU, so calling it unconditionally when the port is cold is
    the only thing standing between us and a night of CPU inference.
    """
    if model_up():
        return True, "already serving"
    rc, out = sh(f"bash {SERVE}", timeout=300)
    if model_up():
        return True, "started via serve.sh"
    return False, f"serve.sh did not bring the model up (rc={rc}): {out[-400:]}"


# --------------------------------------------------------------------------- #
# clean-room references - ref_lookup.py turns an item's keywords into a        #
# REFERENCE section from the Apache-2.0 repos in refs/. Contract: it prints    #
# that text on stdout. Absent or empty is fine; the item just gets no refs.    #
# --------------------------------------------------------------------------- #

def references_for(item):
    if not os.path.exists(REF_LOOKUP):
        return ""
    keywords = " ".join([item["id"], item["name"], item.get("where", "")])
    rc, out = sh(f"python3 {REF_LOOKUP} {ms.shq(keywords)}", cwd=HERE, timeout=120)
    return out.strip() if rc == 0 else ""


# --------------------------------------------------------------------------- #
# prompt enrichment - fold the rulebook and the item's references into the     #
# problem statement mini and sonnet both see. We wrap mini_spree.task_text     #
# rather than fork it, so mini_escalate (which calls ms.task_text) inherits    #
# the same enrichment for free.                                                #
# --------------------------------------------------------------------------- #

_ORIG_TASK_TEXT = ms.task_text


def _enriched_task_text(item):
    parts = [_ORIG_TASK_TEXT(item)]
    if RULEBOOK_TEXT.strip():
        parts.append(
            "\n\n--- RULEBOOK (lessons paid for by past failures; every rule "
            "here is mandatory) ---\n" + RULEBOOK_TEXT.strip()
        )
    refs = item.get("_references", "")
    if refs.strip():
        parts.append(
            "\n\n--- REFERENCE (clean-room, Apache-2.0 only; describes behaviour "
            "to reimplement, never to copy) ---\n" + refs.strip()
        )
    return "".join(parts)


def install_prompt_enrichment():
    ms.task_text = _enriched_task_text


# --------------------------------------------------------------------------- #
# gate verdicts - gate.py is the referee. Contract: `python3 gate.py <diff>    #
# <repo>` applies the diff in a throwaway worktree, runs static sanity +       #
# cargo check/clippy/test, writes a JSON verdict next to the candidate, exits  #
# 0 on pass, and appends failures to gate_failures.log. The verdict path and   #
# its exact keys are not pinned by the contract, so we read defensively.       #
# --------------------------------------------------------------------------- #

def verdict_path(diff):
    # gate.py writes its verdict to <candidate>.gate.json (confirmed against the
    # real gate.py, which a sibling worker landed mid-build).
    return diff + ".gate.json"


def verdict_passed(verdict):
    """Tolerant read of a gate verdict dict: pass iff any obvious pass key is
    truthy (pass / passed / ok), or a 'verdict'/'result' field says so."""
    if not isinstance(verdict, dict):
        return False
    for key in ("pass", "passed", "ok"):
        if key in verdict:
            return bool(verdict[key])
    v = str(verdict.get("verdict", verdict.get("result", ""))).lower()
    return v in ("pass", "passed", "ok", "clean")


def diff_has_passing_verdict(diff):
    """Done-detection reads only what is on disk: a verdict file that passes."""
    v = load_json(verdict_path(diff), None)
    return verdict_passed(v)


def verdict_reason(diff):
    v = load_json(verdict_path(diff), {}) or {}
    return str(v.get("reason") or v.get("detail") or v.get("verdict") or "").strip()


def run_gate(diff):
    """Run the referee on one candidate diff. Returns (passed, reason).

    Exit 0 is the authoritative pass signal; the verdict json only supplies a
    human reason. If gate.py is absent we cannot certify anything, so nothing
    ships - failing closed is the safe direction.
    """
    if not os.path.exists(GATE):
        return False, "gate.py not present; cannot certify"
    rc, out = sh(f"python3 {GATE} {ms.shq(diff)} {ms.shq(REPO)}", cwd=HERE, timeout=3600)
    reason = verdict_reason(diff)
    if rc == 0:
        return True, reason or "check+clippy+test pass"
    if not reason:
        reason = f"gate exit {rc}: {out.strip()[-200:]}"
    return False, reason


# --------------------------------------------------------------------------- #
# the queue, rebuilt from disk. Eligible = confirmed + vanilla-verified        #
# (mini_spree.eligible is the single source of that truth). Done = a candidate #
# diff for the item carries a passing gate verdict.                            #
# --------------------------------------------------------------------------- #

def item_candidates(item):
    return sorted(glob.glob(os.path.join(CAND, f"{item['id']}-*.diff")))


def item_done(item):
    return any(diff_has_passing_verdict(d) for d in item_candidates(item))


def passing_diff_for(item):
    for d in item_candidates(item):
        if diff_has_passing_verdict(d):
            return d
    return None


def build_queue(limit=0, only=None):
    """(eligible_not_done, skipped, already_done) rebuilt from WORKQUEUE + disk."""
    eligible, skipped, done = [], [], []
    for item in ms.parse_queue():
        if only and item["id"] not in only:
            continue
        ok, why = ms.eligible(item)
        if not ok:
            skipped.append((item["id"], why))
            continue
        if item_done(item):
            done.append(item)
            continue
        eligible.append(item)
    if limit:
        eligible = eligible[:limit]
    return eligible, skipped, done


# --------------------------------------------------------------------------- #
# the per-item pipeline: local spree -> gate; on failure, sonnet escalation    #
# (budget permitting) -> gate. Returns a result dict for the report.           #
# --------------------------------------------------------------------------- #

def gate_new_candidates(item, seen_before):
    """Gate every candidate diff for the item that we have not gated yet.
    Returns (passing_diff_or_None, [(diff, passed, reason), ...])."""
    results = []
    passing = None
    for d in item_candidates(item):
        if d in seen_before:
            continue
        passed, reason = run_gate(d)
        results.append((d, passed, reason))
        if passed and passing is None:
            passing = d
    return passing, results


def process_item(item, mini_timeout, allow_escalate):
    res = {"id": item["id"], "name": item["name"], "tier": None,
           "status": "attempted", "gate": [], "passing_diff": None,
           "escalated": False, "escalate_note": ""}

    item["_references"] = references_for(item)
    before = set(item_candidates(item))

    # --- local tier: the free 27B grinds the item in its worktree ------------
    ms.run_item(item, mini_timeout)
    passing, gres = gate_new_candidates(item, before)
    res["gate"].extend([(os.path.basename(d), ok, r) for d, ok, r in gres])
    if passing:
        res.update(tier="local", status="PASS", passing_diff=passing)
        return res

    # --- sonnet tier: only the hard subset, only if budget allows ------------
    if not allow_escalate:
        res["status"] = "FAIL-LOCAL (escalation disabled)"
        return res
    ok, why = budget.take("escalate")   # shared per-day cap; also records spend
    if not ok:
        res["status"] = f"FAIL-LOCAL (no escalation: {why})"
        return res

    assert me.MODEL == "sonnet", f"escalation model is {me.MODEL!r}, must be sonnet"
    res["escalated"] = True
    quota_ok, quota_why = me.probe_quota()
    if not quota_ok:
        res["status"] = f"FAIL-LOCAL (sonnet unavailable: {quota_why})"
        res["escalate_note"] = quota_why
        return res

    seen = set(item_candidates(item))
    me.escalate_one(item)
    passing, gres = gate_new_candidates(item, seen)
    res["gate"].extend([(os.path.basename(d), ok, r) for d, ok, r in gres])
    if passing:
        res.update(tier="sonnet", status="PASS", passing_diff=passing)
    else:
        res.update(tier="sonnet", status="FAIL-SONNET")
    return res


# --------------------------------------------------------------------------- #
# commit + push. One commit per passing item onto autoport/nightly, created    #
# from verify/all the first time and never reset thereafter. Push to `fork`    #
# only; fetch first and abort the whole push stage if the branch diverged.     #
# HARD RULES enforced here: no origin/upstream, no force-push, ever.           #
# --------------------------------------------------------------------------- #

def repo_tracked_dirty():
    """True if TRACKED files are modified/staged. Untracked (??) is ignored:
    a stray *.bak must not block the run."""
    rc, out = git("status --porcelain --untracked-files=no")
    return bool(out.strip())


def ensure_branch():
    """Check out autoport/nightly, creating it from verify/all if absent.
    Never `checkout -B` - that would discard prior nights' commits."""
    rc, out = git(f"rev-parse --verify --quiet refs/heads/{BRANCH}")
    if rc == 0:
        rc, out = git(f"checkout {BRANCH}")
    else:
        rc, out = git(f"checkout -b {BRANCH} {BASE}")
    return rc == 0, out.strip()


def commit_message(item, tier):
    summary = item.get("what") or item.get("do") or item["name"]
    summary = " ".join(summary.split())[:120]
    subject = f"{item['id']} {slug(item['name'], 40)}: {summary}"
    body = (
        f"gate: check+clippy+test pass (tier: {tier})\n\n"
        f"Automated port of confirmed parity item {item['id']} ({item['name']}).\n"
    )
    return subject + "\n\n" + body


def apply_and_commit(item, diff, tier):
    """Apply one passing diff and commit it. Returns (ok, note)."""
    rc, out = git(f"apply --index --check {ms.shq(diff)}")
    if rc != 0:
        # fall back to a 3-way apply before giving up
        rc2, out2 = git(f"apply --3way {ms.shq(diff)}")
        if rc2 != 0:
            return False, f"diff would not apply: {out.strip()[-160:]}"
    else:
        git(f"apply --index {ms.shq(diff)}")
    git("add -A")
    msg_file = f"/tmp/pumpkin_night_msg_{item['id']}.txt"
    with open(msg_file, "w") as f:
        f.write(commit_message(item, tier))
    rc, out = git(f"commit -q -F {ms.shq(msg_file)}")
    if rc != 0:
        return False, f"commit failed: {out.strip()[-160:]}"
    return True, "committed"


def branch_diverged():
    """After fetching fork, True if the remote branch carries commits we do not
    have locally. That is a human's problem, never a retry's."""
    rc, out = git(f"rev-parse --verify --quiet refs/remotes/{REMOTE}/{BRANCH}")
    if rc != 0:
        return False, "remote branch absent (first push)"
    rc, out = git(f"rev-list --count {REMOTE}/{BRANCH} ^{BRANCH}")
    ahead = out.strip()
    try:
        remote_only = int(ahead)
    except ValueError:
        return True, f"could not compare with remote ({ahead!r})"
    if remote_only > 0:
        return True, f"remote is {remote_only} commit(s) ahead of local"
    return False, "fast-forward safe"


def push_stage(dry):
    """Fetch fork, verify no divergence, push. Never force, never origin."""
    rc, out = git(f"fetch {REMOTE} {BRANCH}", timeout=300)
    # a missing remote branch makes fetch non-zero; that is fine (first push).
    diverged, why = branch_diverged()
    if diverged:
        return False, f"ABORT push: {why} - branch diverged, needs a human"
    if dry:
        return True, f"would push {BRANCH} -> {REMOTE} ({why})"
    # HARD: explicit refspec, named remote, no --force anywhere.
    rc, out = git(f"push {REMOTE} {BRANCH}:{BRANCH}", timeout=900)
    if rc != 0:
        return False, f"push failed: {out.strip()[-200:]}"
    return True, f"pushed {BRANCH} -> {REMOTE}"


def commit_and_push(passing_items, dry):
    """passing_items: list of (item, diff, tier). Returns a report dict."""
    out = {"branch_ok": False, "commits": [], "push": ""}
    if not passing_items:
        out["push"] = "nothing passed the gate; no commit or push"
        return out
    if repo_tracked_dirty():
        out["push"] = ("ABORT commit stage: repo has modified tracked files; "
                       "refusing to commit on a dirty tree")
        return out
    ok, note = ensure_branch()
    out["branch_ok"] = ok
    if not ok:
        out["push"] = f"ABORT: could not check out {BRANCH}: {note}"
        return out
    for item, diff, tier in passing_items:
        if dry:
            out["commits"].append((item["id"], f"would commit {os.path.basename(diff)}"))
            continue
        cok, cnote = apply_and_commit(item, diff, tier)
        out["commits"].append((item["id"], cnote))
    ok, note = push_stage(dry)
    out["push"] = note
    return out


# --------------------------------------------------------------------------- #
# the rulebook loop - the heart of "fix the process, not the code". When the   #
# same normalised failure reason shows up >= 2 times across items/nights, we   #
# write ONE imperative rule (+ example) so every future prompt inherits the    #
# lesson. We never edit a candidate to dodge the pattern.                      #
# --------------------------------------------------------------------------- #

def normalise_reason(text):
    """Collapse a failure line to a stable signature: drop timestamps, ids,
    paths, line/col numbers and specific literals, keep the shape. A rustc code
    like E0308 is the strongest signature and is preserved verbatim."""
    t = text.lower()
    m = re.search(r"e\d{3,4}", t)          # rustc error code, the best key
    if m:
        return f"rustc:{m.group(0)}"
    t = re.sub(r"[a-z0-9_./~-]+\.rs(:\d+)?(:\d+)?", "<file>", t)
    t = re.sub(r"\b\d+\b", "<n>", t)
    t = re.sub(r"[0-9a-f]{7,40}", "<hash>", t)
    t = re.sub(r"\s+", " ", t).strip()
    return t[:120]


def parse_gate_failures():
    """Read gate_failures.log. gate.py writes tab-separated lines
    `ts<TAB>item<TAB>stage<TAB>reason`; we key the signature on stage+reason,
    which is the finest signal the log carries. We stay tolerant of a JSON line
    or a free-form line in case the format ever changes. Returns list of
    (raw_reason, normalised_signature)."""
    if not os.path.exists(GATE_FAILURES):
        return []
    out = []
    for line in open(GATE_FAILURES):
        line = line.rstrip("\n")
        if not line.strip():
            continue
        cols = line.split("\t")
        if len(cols) >= 4:                       # the real gate.py format
            stage, reason = cols[2], cols[3]
            raw = f"{stage}: {reason}"
            out.append((raw, normalise_reason(f"{stage}:{reason}")))
            continue
        raw = line
        try:
            obj = json.loads(line)
            if isinstance(obj, dict):
                stage = str(obj.get("stage_failed") or obj.get("stage") or "")
                reason = str(obj.get("reason") or obj.get("detail") or obj.get("error") or line)
                raw = f"{stage}: {reason}" if stage else reason
        except ValueError:
            m = re.search(r"(?:reason|fail(?:ed)?|error)[:=]\s*(.+)$", line, re.I)
            if m:
                raw = m.group(1)
        out.append((raw, normalise_reason(raw)))
    return out


RULEBOOK_HEADER = (
    "# Rulebook\n\n"
    "Rules distilled from repeated gate failures. Each was paid for by the same "
    "mistake landing at least twice. night_run.py injects this whole file into "
    "every local and sonnet prompt, so the loop learns once and never relearns.\n\n"
    "Do not delete a rule to make a diff pass; that is fixing the code to dodge "
    "the process. Rules are keyed by an HTML comment so they are appended once.\n"
)


def load_rulebook():
    if os.path.exists(RULEBOOK):
        return open(RULEBOOK).read()
    return RULEBOOK_HEADER


def existing_rule_keys(text):
    return set(re.findall(r"<!--\s*rule:(.*?)\s*-->", text))


def rule_sentence(sig, example):
    """One imperative sentence for a recurring signature. Kept deliberately
    generic - the example carries the specifics."""
    if sig.startswith("rustc:"):
        return (f"Do not ship a candidate that triggers {sig.split(':')[1].upper()}; "
                f"resolve the type/borrow error before the diff leaves the model.")
    return ("Avoid the failure captured below; a candidate that reproduces it "
            "will be rejected by the gate, so pre-empt it.")


def update_rulebook(dry):
    """Append a rule for every recurring (>=2) failure signature not already
    encoded. Returns list of new rule signatures."""
    global RULEBOOK_TEXT
    failures = parse_gate_failures()
    counts, example = {}, {}
    for raw, sig in failures:
        counts[sig] = counts.get(sig, 0) + 1
        example.setdefault(sig, raw)
    text = load_rulebook()
    have = existing_rule_keys(text)
    added = []
    for sig, n in sorted(counts.items(), key=lambda kv: -kv[1]):
        if n < 2 or sig in have:
            continue
        block = (
            f"\n<!-- rule:{sig} -->\n"
            f"- {rule_sentence(sig, example[sig])}\n"
            f"  - seen {n}x. example: `{example[sig][:200]}`\n"
        )
        text = text.rstrip("\n") + "\n" + block
        added.append(sig)
    if added and not dry:
        with open(RULEBOOK, "w") as f:
            f.write(text)
    RULEBOOK_TEXT = text
    return added


# --------------------------------------------------------------------------- #
# coverage + report. coverage.py refreshes COVERAGE.md and prints a summary    #
# line we parse for a delta against the previous night.                        #
# --------------------------------------------------------------------------- #

def run_coverage():
    rc, out = sh("python3 coverage.py", cwd=HERE, timeout=600)
    m = re.search(r"(\d+)\s+gap families,\s*(\d+)\s+standalone", out)
    fams = int(m.group(1)) if m else None
    standalone = int(m.group(2)) if m else None
    prev = load_json(COVSTATE, {})
    delta = {}
    if fams is not None:
        if prev.get("families") is not None:
            delta = {"families": fams - prev["families"],
                     "standalone": standalone - prev["standalone"]}
        save_json(COVSTATE, {"date": dt.date.today().isoformat(),
                             "families": fams, "standalone": standalone})
    return {"summary": out.strip().splitlines()[0] if out.strip() else "coverage.py produced no output",
            "families": fams, "standalone": standalone, "delta": delta}


def budget_line():
    text, _, _ = budget.status()
    return text


def write_report(ctx):
    os.makedirs(REPORTS, exist_ok=True)
    stamp = dt.date.today().strftime("%Y%m%d")
    path = os.path.join(REPORTS, f"night-{stamp}.md")
    L = []
    L.append(f"# Night run {dt.date.today().isoformat()}")
    L.append("")
    L.append(f"- mode: {'DRY RUN' if ctx['dry'] else 'LIVE'}")
    L.append(f"- model server: {ctx['model_note']}")
    L.append(f"- eligible-not-done: {len(ctx['eligible'])}   "
             f"already-done: {len(ctx['done'])}   skipped: {len(ctx['skipped'])}")
    if ctx.get("fatal"):
        L.append(f"- **run ended early:** {ctx['fatal']}")
    L.append("")

    L.append("## Items attempted")
    L.append("")
    if not ctx["results"]:
        L.append("_none attempted this run._")
    for r in ctx["results"]:
        L.append(f"### {r['id']} {r['name']}")
        L.append(f"- tier: {r['tier'] or '-'}   status: **{r['status']}**")
        if r.get("escalate_note"):
            L.append(f"- escalation note: {r['escalate_note']}")
        for base, ok, reason in r["gate"]:
            L.append(f"- gate `{base}`: {'PASS' if ok else 'FAIL'} - {reason}")
        L.append("")
    cp = ctx["commit"]
    L.append("## Commits and push")
    L.append("")
    L.append(f"- branch `{BRANCH}` checked out: {cp['branch_ok']}")
    for iid, note in cp["commits"]:
        L.append(f"- {iid}: {note}")
    L.append(f"- push: {cp['push']}")
    L.append("")
    cov = ctx["coverage"]
    L.append("## Coverage")
    L.append("")
    L.append(f"- {cov['summary']}")
    if cov["delta"]:
        L.append(f"- delta vs last night: families {cov['delta']['families']:+d}, "
                 f"standalone {cov['delta']['standalone']:+d}")
    L.append("")
    L.append("## Budget")
    L.append("")
    L.append("```")
    L.append(ctx["budget"])
    L.append("```")
    L.append("")
    L.append("## New rulebook rules")
    L.append("")
    if ctx["new_rules"]:
        for sig in ctx["new_rules"]:
            L.append(f"- `{sig}`")
    else:
        L.append("_none; no failure reason recurred >=2x._")
    L.append("")
    with open(path, "w") as f:
        f.write("\n".join(L) + "\n")
    update_index(path, ctx)
    return path


def update_index(report_path, ctx):
    """Maintain reports/INDEX.md: one rolling line per night, newest first."""
    idx = os.path.join(REPORTS, "INDEX.md")
    passed = sum(1 for r in ctx["results"] if r["status"] == "PASS")
    line = (f"- [{dt.date.today().isoformat()}]({os.path.basename(report_path)}) - "
            f"{passed}/{len(ctx['results'])} passed, "
            f"{len(ctx['done'])} already-done, push: {ctx['commit']['push'][:60]}\n")
    header = "# Night run index\n\nNewest first.\n\n"
    body = ""
    if os.path.exists(idx):
        existing = open(idx).read()
        body = existing.split("\n\n", 2)[-1] if existing.count("\n\n") >= 2 else ""
    # de-dupe today's line if the run is repeated
    body = "".join(l for l in body.splitlines(keepends=True)
                   if f"night-{dt.date.today().strftime('%Y%m%d')}" not in l)
    with open(idx, "w") as f:
        f.write(header + line + body)


# --------------------------------------------------------------------------- #
# dry run - the smoke test. Parse the queue, check the model server, the git   #
# state and divergence, the presence of the sibling contracts, and print the   #
# plan. Touches nothing: no spree, no escalate, no commit, no push.            #
# --------------------------------------------------------------------------- #

def dry_run(limit, only):
    print("=== night_run.py --dry-run ===\n")

    print("[model] server health:")
    print(f"  {MODEL_HEALTH} -> {'UP' if model_up() else 'DOWN (serve.sh would run)'}")

    print("\n[contracts] sibling scripts:")
    for label, path in (("gate.py", GATE), ("ref_lookup.py", REF_LOOKUP)):
        print(f"  {label:14} {'present' if os.path.exists(path) else 'ABSENT (coded against contract)'}")
    print(f"  RULEBOOK.md    {'present' if os.path.exists(RULEBOOK) else 'absent (will be created)'}")

    print("\n[git] repo state:")
    rc, branch = git("rev-parse --abbrev-ref HEAD")
    rc, remotes = sh("git remote -v", cwd=REPO)
    has_fork = f"{REMOTE}\t" in remotes and "eshankiyer/Pumpkin" in remotes
    print(f"  repo: {REPO}")
    print(f"  current branch: {branch.strip()}")
    print(f"  tracked-dirty: {repo_tracked_dirty()}")
    print(f"  fork remote ok: {has_fork}")
    rc, exists = git(f"rev-parse --verify --quiet refs/heads/{BRANCH}")
    print(f"  {BRANCH}: {'exists' if exists.strip() else f'absent (would branch from {BASE})'}")
    git(f"fetch {REMOTE} {BRANCH}", timeout=120)
    diverged, why = branch_diverged()
    print(f"  divergence check: {'DIVERGED - push would abort' if diverged else 'safe'} ({why})")

    print("\n[queue] rebuilt from disk:")
    eligible, skipped, done = build_queue(limit, only)
    print(f"  eligible-not-done: {len(eligible)}   already-done: {len(done)}   skipped: {len(skipped)}")
    for it in eligible:
        print(f"    PLAN {it['id']:4} {it['name'][:52]}")
    for iid, why in skipped[:8]:
        print(f"    skip {iid:4} {why}")

    print("\n[budget]")
    print("  " + budget_line().replace("\n", "\n  "))

    print("\n[plan] this run WOULD, per eligible item:")
    print("  1. enrich prompt with RULEBOOK + ref_lookup REFERENCE")
    print("  2. run local mini_spree pass, gate every candidate via gate.py")
    print("  3. on gate failure, escalate to sonnet (budget permitting), gate again")
    print(f"  4. commit each passing diff to {BRANCH}, push to {REMOTE} only (no force)")
    print("  5. scan gate_failures.log, append recurring-failure rules to RULEBOOK.md")
    print("  6. run coverage.py and write reports/night-YYYYMMDD.md + INDEX.md")
    print("\nNo spree, escalation, commit, or push happened (dry run).")


# --------------------------------------------------------------------------- #
# main. A lockfile stops overlapping nights; the whole pipeline is wrapped so  #
# that even an unexpected exception still writes a report before exiting.      #
# --------------------------------------------------------------------------- #

def acquire_lock():
    """Non-blocking exclusive flock. Returns the fd (kept open) or None."""
    fd = open(LOCKFILE, "w")
    try:
        fcntl.flock(fd.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except (BlockingIOError, OSError):
        return None
    fd.write(f"{os.getpid()} {dt.datetime.now().isoformat()}\n")
    fd.flush()
    return fd


def run_night(limit, only, dry):
    global RULEBOOK_TEXT
    RULEBOOK_TEXT = load_rulebook()      # feed prompts the rules earned so far
    install_prompt_enrichment()

    ctx = {"dry": dry, "results": [], "fatal": None,
           "commit": {"branch_ok": False, "commits": [], "push": "not reached"},
           "new_rules": [], "model_note": ""}
    try:
        up, note = ensure_model()
        ctx["model_note"] = note
        if not up:
            ctx["fatal"] = f"model server unavailable: {note}"

        eligible, skipped, done = build_queue(limit, only)
        ctx.update(eligible=eligible, skipped=skipped, done=done)
        print(f"eligible: {len(eligible)}  done: {len(done)}  skipped: {len(skipped)}",
              flush=True)

        if up and not dry:
            allow_esc = os.path.exists(me.CLI)
            for item in eligible:
                print(f"\n=== {item['id']} {item['name']} ===", flush=True)
                r = process_item(item, mini_timeout=3600, allow_escalate=allow_esc)
                ctx["results"].append(r)
        elif up and dry:
            pass  # dry never runs the model tiers

        # commit + push every item that earned a passing gate verdict
        id2item = {it["id"]: it for it in eligible}
        passing = [(id2item[r["id"]], r["passing_diff"], r["tier"])
                   for r in ctx["results"]
                   if r["status"] == "PASS" and r["passing_diff"]]
        ctx["commit"] = commit_and_push(passing, dry)

        # rulebook loop: encode any failure reason that has now recurred >=2x
        ctx["new_rules"] = update_rulebook(dry)

    except Exception as e:      # never lose the report to an exception
        import traceback
        ctx["fatal"] = f"{type(e).__name__}: {e}"
        ctx["traceback"] = traceback.format_exc()
        print(ctx["traceback"], file=sys.stderr, flush=True)
    finally:
        ctx["coverage"] = run_coverage()
        ctx["budget"] = budget_line()
        report = write_report(ctx)
        print(f"\nreport: {report}", flush=True)
    return ctx


def main():
    ap = argparse.ArgumentParser(description="Unattended Pumpkin parity port loop.")
    ap.add_argument("--dry-run", action="store_true",
                    help="parse, check state, print plan; touch nothing")
    ap.add_argument("--limit", type=int, default=0, help="cap items attempted")
    ap.add_argument("--only", default="", help="comma-separated item ids")
    a = ap.parse_args()
    only = {s.strip() for s in a.only.split(",") if s.strip()}

    if a.dry_run:
        dry_run(a.limit, only)
        return 0

    lock = acquire_lock()
    if lock is None:
        print("another night_run is already holding the lock; exiting", file=sys.stderr)
        return 1
    try:
        run_night(a.limit, only, dry=False)
    finally:
        fcntl.flock(lock.fileno(), fcntl.LOCK_UN)
        lock.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
