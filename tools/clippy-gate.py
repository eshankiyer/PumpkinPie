"""Report every clippy error on stdin's JSON stream, span or no span.

Exists because the obvious gate -- `--message-format short` piped through
`grep ": error"` -- silently passes a tree CI rejects. Not every diagnostic
carries a usable span: `too_long_first_doc_paragraph` on a `#[path]` module
reported line_start 137 with line_end 4, which short format renders as a bare
`error: ...` with no file prefix, so the grep matched nothing and the gate said
clean. That shipped a red CI on 2026-08-22.
"""

import json
import sys

seen: set[tuple[str, str]] = set()

for line in sys.stdin:
    try:
        payload = json.loads(line)
    except ValueError:
        continue
    message = payload.get("message")
    if not message or message.get("level") != "error":
        continue
    text = message.get("message", "")
    # A summary of errors already reported individually.
    if text.startswith("could not compile"):
        continue
    spans = message.get("spans") or []
    if spans:
        where = f"{spans[0]['file_name']}:{spans[0]['line_start']}"
    else:
        where = "NO-SPAN"
    key = (where, text)
    if key in seen:
        continue
    seen.add(key)
    print(f"{where}: error: {text}")

sys.exit(1 if seen else 0)
