---
name: pumpkin-leaves-verify-and-pr
description: One-shot: read leaf decay test result, open the leaves PR if it passed
---

You are finishing an automated verification for Eshan's Pumpkin-MC contribution work. Context: a leaf-decay test was armed to run automatically against a Minecraft server on his NUC after a rebuild; its output was written to /tmp/leaftest3.txt on this laptop. Use Desktop Commander to run shell commands.

1. Run: cat /tmp/leaftest3.txt
   Interpret: lines look like "t=45s lone=AIR nearLog=OCCUPIED". PASS means at some timestamp lone=AIR while nearLog=OCCUPIED (lone leaves decayed, leaves next to a log survived). If every line shows lone=OCCUPIED, or the file is missing/empty, the test FAILED or has not finished.

2. If PASS: open the pull request with: export PATH=$HOME/.local/bin:$PATH && gh pr create --repo Pumpkin-MC/Pumpkin --head eshankiyer:fix/leaves-distance-updates --title "feat: leaf distance updates and decay" --body-file /tmp/leaves_pr_body.md
   First write /tmp/leaves_pr_body.md with exactly this content:

## Description

Fixes #2445.

Leaves blocks previously had no behaviour registered, so the distance property always stayed at its default of 7 regardless of nearby logs, and leaves never decayed.

This PR adds a LeavesBlock behaviour covering every block in the minecraft:leaves tag: on_place computes the real distance and sets persistent like vanilla player-placed leaves; neighbor updates schedule a 1-tick recalculation so cascades through a canopy stay cheap; the scheduled tick recomputes distance as min(neighbor)+1 with logs as distance 0, capped at 7; random ticks decay non-persistent leaves whose distance stays 7.

## Testing

- cargo clippy -p pumpkin --all-targets and cargo fmt --check pass with the workspace deny-level lints
- Verified live on a server built from this branch plus the #2481 setblock fix (headless client keeping chunks ticking, RCON probes, random_tick_speed raised): command-placed lone oak leaves (persistent=false, distance 7) decayed, while oak leaves adjacent to an oak log did not
- Note from testing: random tick sampling only reaches sections whose randomly_ticking_mask is set and the per-section attempt count is hardcoded at 3 ignoring the random_tick_speed gamerule; that looks like a separate pre-existing issue

3. Notify Eshan with the result either way: if PASS, give him the new PR URL and say all five PRs for today are open (#2474, #2475, #2476, #2481, plus this one). If FAIL or unfinished, tell him the leaf test did not pass, show the last few lines of the file, and say the leaves PR was NOT opened and the branch fix/leaves-distance-updates is pushed and waiting. Do not open the PR on a failed or missing result. Keep everything brief; do not re-run builds or start servers.