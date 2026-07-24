#!/usr/bin/env python3
"""Renders history.csv into a self-contained HTML dashboard (inline SVG, no CDN).

Read the caveat in the page: these are coverage proxies, not a parity score.
"""
import csv, os, sys, html
from datetime import datetime

HERE = os.path.dirname(os.path.abspath(__file__))
CSV = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "history.csv")
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "index.html")

rows = list(csv.DictReader(open(CSV)))
if not rows:
    sys.exit("no data")
latest = rows[-1]

def i(row, key):
    try:
        return int(row[key])
    except (ValueError, KeyError, TypeError):
        return 0

CARDS = [
    ("Blocks with behaviour", i(latest, "blocks_impl"), i(latest, "blocks_total"),
     "Blocks registered in the block registry, over every block in the game data. "
     "Many vanilla blocks need no behaviour code at all, so this undercounts real parity."),
    ("Mobs with AI", i(latest, "mobs_ai"), i(latest, "entities_total"),
     "Mob types with an AI module, over every entity type (which includes projectiles, "
     "vehicles and display entities that need no AI)."),
    ("Commands", i(latest, "commands"), 75,
     "Command modules implemented, against roughly 75 player-facing commands in vanilla 1.21."),
]

OTHER = [
    ("AI goals", i(latest, "ai_goals")),
    ("Block entities", i(latest, "block_entities")),
    ("Items with behaviour", i(latest, "items")),
    ("TODO / FIXME markers", i(latest, "todos")),
    ("Open issues", i(latest, "open_issues")),
    ("Open PRs", i(latest, "open_prs")),
]


def donut(done, total, label):
    pct = (done / total * 100) if total else 0
    r, c = 54, 2 * 3.14159 * 54
    filled = c * min(pct, 100) / 100
    return f"""<div class="card">
  <svg viewBox="0 0 140 140" width="140" height="140">
    <circle cx="70" cy="70" r="{r}" fill="none" stroke="#2a2f3a" stroke-width="16"/>
    <circle cx="70" cy="70" r="{r}" fill="none" stroke="#f0932b" stroke-width="16"
            stroke-dasharray="{filled:.1f} {c:.1f}" stroke-linecap="round"
            transform="rotate(-90 70 70)"/>
    <text x="70" y="66" text-anchor="middle" fill="#e8eaed" font-size="26"
          font-weight="600">{pct:.0f}%</text>
    <text x="70" y="88" text-anchor="middle" fill="#9aa0a6" font-size="15">{done}/{total}</text>
  </svg>
  <h3>{html.escape(label)}</h3>
</div>"""

def sparkline(key, label):
    vals = [i(r, key) for r in rows]
    if len(vals) < 2:
        return f'<div class="spark"><span>{html.escape(label)}</span><em>{vals[-1] if vals else 0}</em>' \
               f'<small>needs 2+ data points to trend</small></div>'
    lo, hi = min(vals), max(vals)
    span = (hi - lo) or 1
    w, h = 260, 46
    pts = " ".join(
        f"{x / (len(vals) - 1) * w:.1f},{h - (v - lo) / span * h:.1f}"
        for x, v in enumerate(vals)
    )
    delta = vals[-1] - vals[0]
    sign = "+" if delta > 0 else ""
    return f"""<div class="spark">
  <span>{html.escape(label)}</span><em>{vals[-1]}</em>
  <svg viewBox="0 0 {w} {h}" width="{w}" height="{h}"><polyline points="{pts}"
    fill="none" stroke="#f0932b" stroke-width="2"/></svg>
  <small>{sign}{delta} since {rows[0]['date']}</small>
</div>"""

cards = "\n".join(donut(d, t, n) for n, d, t, _ in CARDS)
notes = "\n".join(f"<li><b>{html.escape(n)}</b> — {html.escape(note)}</li>" for n, _, _, note in CARDS)
sparks = "\n".join(sparkline(k, l) for l, k in [
    ("Blocks with behaviour", "blocks_impl"), ("Mobs with AI", "mobs_ai"),
    ("Commands", "commands"), ("TODO / FIXME", "todos"),
    ("Open issues", "open_issues"), ("Open PRs", "open_prs"),
])
stats = "\n".join(f"<tr><td>{html.escape(n)}</td><td>{v}</td></tr>" for n, v in OTHER)


PAGE = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Pumpkin vanilla-parity tracker</title>
<style>
:root {{ color-scheme: dark; }}
body {{ margin:0; padding:32px; background:#15171c; color:#e8eaed;
  font:15px/1.6 ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif; }}
.wrap {{ max-width:980px; margin:0 auto; }}
h1 {{ font-size:26px; margin:0 0 4px; }}
.sub {{ color:#9aa0a6; margin-bottom:28px; }}
.caveat {{ background:#241f16; border:1px solid #5c4a22; border-radius:10px;
  padding:14px 18px; margin-bottom:28px; color:#e0c9a0; }}
.cards {{ display:flex; gap:20px; flex-wrap:wrap; margin-bottom:28px; }}
.card {{ background:#1c1f26; border:1px solid #2a2f3a; border-radius:12px;
  padding:18px; text-align:center; flex:1; min-width:190px; }}
.card h3 {{ font-size:14px; font-weight:500; color:#9aa0a6; margin:10px 0 0; }}
.sparks {{ display:flex; gap:16px; flex-wrap:wrap; margin-bottom:28px; }}
.spark {{ background:#1c1f26; border:1px solid #2a2f3a; border-radius:12px;
  padding:14px 16px; flex:1; min-width:280px; }}
.spark span {{ color:#9aa0a6; font-size:13px; display:block; }}
.spark em {{ font-style:normal; font-size:24px; font-weight:600; display:block; margin:2px 0 6px; }}
.spark small {{ color:#6b7280; font-size:12px; }}
table {{ border-collapse:collapse; width:100%; background:#1c1f26;
  border:1px solid #2a2f3a; border-radius:12px; overflow:hidden; }}
td {{ padding:10px 16px; border-bottom:1px solid #2a2f3a; }}
tr:last-child td {{ border-bottom:none; }}
td:last-child {{ text-align:right; font-variant-numeric:tabular-nums; color:#f0932b; }}
ul {{ color:#9aa0a6; font-size:13px; padding-left:20px; }}
footer {{ margin-top:32px; color:#6b7280; font-size:13px; }}
a {{ color:#f0932b; }}
</style></head><body><div class="wrap">
<h1>Pumpkin vanilla-parity tracker</h1>
<div class="sub">Pumpkin-MC/Pumpkin at <code>{html.escape(latest['commit'])}</code> &middot;
  collected {html.escape(latest['date'])}</div>

<div class="caveat"><b>These are coverage proxies, not a parity score.</b>
Nobody can put a meaningful single percentage on "how much of Minecraft is implemented".
Most vanilla blocks need no behaviour code, so the block figure understates parity;
conversely a registered behaviour may still be incomplete, which overstates it.
Treat the direction of travel as the signal, not the absolute numbers.</div>

<div class="cards">{cards}</div>
<h2 style="font-size:18px">Trend</h2>
<div class="sparks">{sparks}</div>
<h2 style="font-size:18px">Raw counts</h2>
<table>{stats}</table>
<h2 style="font-size:18px;margin-top:28px">What each figure means</h2>
<ul>{notes}</ul>
<footer>Regenerate: <code>collect.sh &amp;&amp; render.py</code>.
History in <code>history.csv</code> ({len(rows)} sample{'s' if len(rows) != 1 else ''}).
Source: <a href="https://github.com/Pumpkin-MC/Pumpkin">Pumpkin-MC/Pumpkin</a>.</footer>
</div></body></html>"""

open(OUT, "w").write(PAGE)
print(f"wrote {OUT} ({len(rows)} samples)")
