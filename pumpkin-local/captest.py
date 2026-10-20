import sys
MODEL = sys.argv[1] if len(sys.argv)>1 else "glm-xl:latest"
"""Does the token cap explain the incoherence? Test the simplest possible edit."""

import json
import urllib.request

SPEC = """Change one character in this Rust function.

The line `if age < self.max_age() && rand::rng().random_range(0..=10) == 0 {`
uses an inclusive range `0..=10`, which is 1 in 11. Vanilla is `nextInt(10)`,
which is exclusive, so 1 in 10.

Change `0..=10` to `0..10`. Change nothing else.

Reply with ONLY a unified diff in a ```diff fence.

### FILE: pumpkin/src/block/blocks/plant/crop/nether_wart.rs
```rust
    async fn random_tick(&self, world: &Arc<World>, pos: &BlockPos) {
        let (block, state_id) = world.get_block_and_state_id(pos);
        let age = self.get_age(state_id, block);
        if age < self.max_age() && rand::rng().random_range(0..=10) == 0 {
            world
                .set_block_state(
                    pos,
                    self.state_with_age(block, state_id, age + 1),
                    BlockFlags::NOTIFY_LISTENERS,
                )
                .await;
        }
    }
```
"""

for cap in (1500,):
    payload = {
        "model": MODEL,
        "messages": [{"role": "user", "content": SPEC}],
        "stream": False,
        "think": False,
        "options": {"temperature": 0.1, "num_ctx": 8192, "num_predict": cap},
    }
    req = urllib.request.Request(
        "http://127.0.0.1:11434/api/chat",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=2400) as r:
        out = json.load(r)["message"]["content"]
    print(f"\n{'=' * 70}\nnum_predict={cap}   chars={len(out)}\n{'=' * 70}")
    print(out[:1600])
