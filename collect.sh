#!/bin/bash
# Collects vanilla-parity coverage proxies from a Pumpkin checkout and appends
# them to a CSV history. These are PROXIES, not a parity percentage: many
# vanilla blocks legitimately need no behaviour code, so "blocks with a
# registered behaviour" undercounts real parity. Trends over time are the
# useful signal, not the absolute numbers.
set -e
REPO="${1:-$HOME/Pumpkin}"
OUT="${2:-$HOME/pumpkin-parity/history.csv}"
cd "$REPO"

DATE=$(date -u +%Y-%m-%d)
COMMIT=$(git rev-parse --short HEAD)

blocks_total=$(grep -c "pub const [A-Z_0-9]*: Self = Block {" pumpkin-data/src/generated/block.rs)
blocks_impl=$(grep -c "manager.register(" pumpkin/src/block/registry.rs)
entities_total=$(grep -c "^    pub const " pumpkin-data/src/generated/entity_type.rs)
mobs_ai=$(ls pumpkin/src/entity/mob/*.rs 2>/dev/null | wc -l)
ai_goals=$(ls pumpkin/src/entity/ai/goal/*.rs 2>/dev/null | wc -l)
commands=$(ls pumpkin/src/command/commands/*.rs | wc -l)
block_entities=$(ls pumpkin/src/block/entities/*.rs 2>/dev/null | wc -l)
items=$(ls pumpkin/src/item/items/*.rs 2>/dev/null | wc -l)
todos=$(grep -rn "TODO\|FIXME" --include="*.rs" pumpkin/src pumpkin-world/src | wc -l)

if command -v gh >/dev/null 2>&1; then
  issues=$(gh api "search/issues?q=repo:Pumpkin-MC/Pumpkin+is:issue+is:open&per_page=1" --jq ".total_count" 2>/dev/null || echo "")
  prs=$(gh api "search/issues?q=repo:Pumpkin-MC/Pumpkin+is:pr+is:open&per_page=1" --jq ".total_count" 2>/dev/null || echo "")
else
  issues=""; prs=""
fi

[ -f "$OUT" ] || echo "date,commit,blocks_total,blocks_impl,entities_total,mobs_ai,ai_goals,commands,block_entities,items,todos,open_issues,open_prs" > "$OUT"
echo "$DATE,$COMMIT,$blocks_total,$blocks_impl,$entities_total,$mobs_ai,$ai_goals,$commands,$block_entities,$items,$todos,$issues,$prs" >> "$OUT"
echo "collected $DATE $COMMIT"
