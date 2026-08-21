# Differential parity probes

This directory contains the first executable slice of the parity harness. It
connects to isolated RCON endpoints, runs the same probes against vanilla and
Pumpkin, normalizes only nondeterministic presentation fields, and writes a
JSON artifact containing both outputs and the SHA-256 digests of the decompiled
Java source files used by the probe.

The harness is intentionally strict: a missing target or unequal probe fails.
It is not a claim that the whole server is covered. Add a probe only when its
commands and normalizer are justified by the corresponding decompiled Java
source and the result is deterministic on repeated vanilla runs.

Example:

```sh
python3 tools/differential/run.py \
  --vanilla 192.168.1.93:25576 \
  --pumpkin 192.168.1.93:25575 \
  --allow-destructive \
  --out target/differential/latest.json
```

The endpoints must be disposable test worlds. The probes change weather,
difficulty, blocks, and force-load state, and clean up their mutations after
each probe. The runner also verifies that both RCON endpoints report the
expected Minecraft version before running probes and records the exact version
responses plus a digest manifest for the Java source files. RCON does not
expose a cryptographic build identity, so the report is evidence of the
endpoint and source inputs, not proof that a remote process was built from
those exact files.
