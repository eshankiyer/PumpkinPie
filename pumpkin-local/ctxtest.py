"""Verify the context applier handles qwen's real output, which omits ---/+++ headers."""

import os
import subprocess
import sys

sys.path.insert(0, "/home/eshanki/pumpkin-local")
from harness import apply_by_context

WT = "/tmp/ctxtest"
subprocess.run(["bash", "-c", f"rm -rf {WT} && mkdir -p {WT}/sub"], capture_output=True)
orig = """    async fn random_tick(&self, world: &Arc<World>, pos: &BlockPos) {
        let (block, state_id) = world.get_block_and_state_id(pos);
        let age = self.get_age(state_id, block);
        if age < self.max_age() && rand::rng().random_range(0..=10) == 0 {
            world.set_block_state(pos).await;
        }
    }
"""
open(f"{WT}/sub/wart.rs", "w").write(orig)

# exactly what qwen2.5-coder emitted: a fenced hunk with no file headers
reply = """```diff
    async fn random_tick(&self, world: &Arc<World>, pos: &BlockPos) {
        let (block, state_id) = world.get_block_and_state_id(pos);
        let age = self.get_age(state_id, block);
-       if age < self.max_age() && rand::rng().random_range(0..=10) == 0 {
+       if age < self.max_age() && rand::rng().random_range(0..10) == 0 {
            world.set_block_state(pos).await;
        }
    }
```"""

applied, failed = apply_by_context(WT, ["sub/wart.rs"], reply)
result = open(f"{WT}/sub/wart.rs").read()
print("applied:", applied, "failed:", failed)
assert applied == 1, f"did not apply: {failed}"
assert "0..10" in result and "0..=10" not in result, result
assert "world.set_block_state(pos).await;" in result, "context was damaged"
print("context applier OK")
print(result)
