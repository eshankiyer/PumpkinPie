#!/usr/bin/env python3
"""Move a file's `#[cfg(test)] mod tests` block to the end of the file."""
import sys, pathlib
for path in sys.argv[1:]:
    p = pathlib.Path(path); s = p.read_text()
    marker = "#[cfg(test)]\nmod tests {"
    if marker not in s:
        print("no test module:", path); continue
    i = s.index(marker)
    j = s.index("{", i + len("#[cfg(test)]\nmod tests"))
    depth, k = 0, j
    while True:
        if s[k] == '{': depth += 1
        elif s[k] == '}':
            depth -= 1
            if depth == 0: break
        k += 1
    block = s[i:k + 1]
    rest = (s[:i] + s[k + 1:]).rstrip('\n')
    p.write_text(rest + "\n\n" + block + "\n")
    print("moved:", path)
