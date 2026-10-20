import sys

sys.path.insert(0, "/home/eshanki/pumpkin-local")
from harness import extract_diffs

# Unterminated fence: exactly what truncation at the token cap produces.
r = """Here is the fix:
```diff
--- a/t.rs
+++ b/t.rs
@@
 fn main() {
-    let x = 3;
+    let x = 4;
 }"""
d = extract_diffs(r)
print("extracted:", len(d))
assert d, "nothing extracted"
print(repr(d[0][:60]))
assert not d[0].lstrip().startswith("```"), "fence leaked into patch"
assert d[0].startswith("--- a/t.rs"), "did not start at the diff header"
print("fence-strip OK")
