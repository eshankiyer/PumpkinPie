# Rulebook

Rules distilled from repeated gate failures. Each was paid for by the same mistake landing at least twice. night_run.py injects this whole file into every local and sonnet prompt, so the loop learns once and never relearns.

Do not delete a rule to make a diff pass; that is fixing the code to dodge the process. Rules are keyed by an HTML comment so they are appended once.

<!-- rule:check:cargo_check_error -->
- Avoid the failure captured below; a candidate that reproduces it will be rejected by the gate, so pre-empt it.
  - seen 5x. example: `check: cargo_check_error`

<!-- rule:static:escape_artifact:u00xx:'u0026' -->
- Avoid the failure captured below; a candidate that reproduces it will be rejected by the gate, so pre-empt it.
  - seen 4x. example: `static: escape_artifact:u00xx:'u0026'`

<!-- rule:structure:orphan_code:<file>:curly_unbalanced_by_-<n> -->
- Avoid the failure captured below; a candidate that reproduces it will be rejected by the gate, so pre-empt it.
  - seen 3x. example: `structure: orphan_code:pumpkin/src/item/potion.rs:curly_unbalanced_by_-2`

<!-- rule:apply:git_apply_check_failed -->
- Avoid the failure captured below; a candidate that reproduces it will be rejected by the gate, so pre-empt it.
  - seen 3x. example: `apply: git_apply_check_failed`

<!-- rule:clippy:clippy_warning -->
- Avoid the failure captured below; a candidate that reproduces it will be rejected by the gate, so pre-empt it.
  - seen 2x. example: `clippy: clippy_warning`
