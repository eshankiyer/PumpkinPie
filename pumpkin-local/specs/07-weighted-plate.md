WORKTREE: /home/eshanki/pumpkin-wt/wplate
FILES: pumpkin/src/block/blocks/redstone/pressure_plate/weighted.rs
CRATES: pumpkin
TARGET:             let f = (weight.min(len) / weight) as f32;
ASSERT_ABSENT: (weight.min(len) / weight) as f32
ASSERT_PRESENT: as f32 / weight as f32
---
This line does integer division and then casts, so it always yields 0 or 1.

`weight` and `len` are both `usize`. `weight.min(len) / weight` is therefore
integer division, and because `len` was already clamped to `weight` on the line
above, the quotient is 0 for every count below the maximum and 1 at the maximum.
The cast to `f32` happens after the division, far too late to help.

Vanilla casts each operand to float BEFORE dividing:

    float f = (float)Math.min(this.weight, i) / (float)this.weight;

Rewrite the line so both operands are cast to `f32` first and the division is
floating point. Keep the variable name `f`, keep `weight.min(len)` as the
numerator and `weight` as the denominator, and keep the trailing semicolon.

Do not touch the surrounding `if len > 0 {` or the `return` on the next line.
