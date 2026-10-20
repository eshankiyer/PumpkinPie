"""Add exact ASSERT_PRESENT / ASSERT_ABSENT lines to every spec.

ASSERT_ABSENT alone only catches "did nothing". It cannot catch "did the wrong
thing": on growth-rates the model changed 0..=25 to 0..5 instead of removing the
gate, which satisfied ASSERT_ABSENT, compiled, and was still wrong. Pairing it
with an ASSERT_PRESENT naming the exact expected result closes that hole.
"""

import os

SPECS = "/home/eshanki/pumpkin-local/specs"

# spec file -> list of assertion lines to insert after CRATES:
ASSERTS = {
    "01-skylight-bounds.md": [
        # the fix must introduce a signed negative check; an unsigned cast must go
        "ASSERT_ABSENT: as usize / BlockPalette::SIZE",
        "ASSERT_PRESENT: < 0",
    ],
    "02-growth-rates.md": [
        # the inner gate must be GONE, not retuned. Any random_range in this
        # override means the gate is still there in some form.
        "ASSERT_ABSENT: random_range(0..=25)",
        "ASSERT_ABSENT: random_range(0..5)",
        "ASSERT_ABSENT: random_range(0..=5)",
    ],
    "03-nether-wart.md": [
        "ASSERT_ABSENT: random_range(0..=10)",
        "ASSERT_PRESENT: random_range(0..10)",
    ],
    "04-diagonal-largemob.md": [
        # vanilla's early-out tests width against 1.0 and a positive malus
        "ASSERT_PRESENT: cost_malus",
        "ASSERT_PRESENT: 1.0",
    ],
    "05-anvil-helmet.md": [
        # the loop must become conditional on damage type
        "ASSERT_PRESENT: DamageType",
    ],
    "06-crit-conditions.md": [
        # at least one of the four new exclusions must actually appear
        "ASSERT_PRESENT: BLINDNESS",
    ],
}


def patch(path, lines):
    src = open(path).read()
    head, sep, body = src.partition("\n---\n")
    kept = [
        ln
        for ln in head.splitlines()
        if not ln.strip().upper().startswith(("ASSERT_ABSENT", "ASSERT_PRESENT"))
    ]
    new_head = "\n".join(kept + lines)
    open(path, "w").write(new_head + sep + body)
    return len(lines)


total = 0
for name, lines in ASSERTS.items():
    p = os.path.join(SPECS, name)
    if not os.path.exists(p):
        print("MISSING", name)
        continue
    total += patch(p, lines)
    print(f"{name}: {len(lines)} assertions")
print(f"\n{total} assertions across {len(ASSERTS)} specs")
