#!/bin/bash
cd "$1" || exit 1
echo "=== dead-code audit $1"
git diff | grep -E "^\+.*fn " | sed 's/.*fn \([a-z_0-9]*\).*/\1/' | sort -u | while read n; do
  [ -z "$n" ] && continue
  c=$(grep -rn "\b$n(" --include='*.rs' crates/ tools/ 2>/dev/null | grep -v "fn $n" | wc -l)
  [ "$c" -eq 0 ] && echo "  ZERO CALLERS: $n"
done
echo "=== end"
