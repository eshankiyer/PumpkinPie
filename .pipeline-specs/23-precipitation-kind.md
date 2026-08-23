WORKTREE: /home/eshanki/pumpkin-wt/subm5
FILES: pumpkin/src/world/precipitation.rs:1-37
CRATES: pumpkin
FUNC: pub fn precipitation_kind(weather: &Weather, x: f64, y: i32, z: f64, sea_level: i32) -> Precipitation {
TEST: cargo test -p pumpkin world::precipitation::tests::precipitation_kind
CHAIN: none
ASSERT_ABSENT: todo!("STUB: see tests::precipitation_kind_none_when_biome_has_no_precipitation")
---
This is M5's F1: the gate that everything else in this feature (water
freezing, snow accumulation, cauldron rain fill-up) is built on top of. Every
other stub in this batch (specs 24, 25, 26) takes a `Precipitation` value as
an input parameter rather than recomputing it, so getting this classification
right is what makes the rest of the feature correct.

`precipitation_kind` mirrors vanilla's `Biome#getPrecipitationAt`. Given a
biome's `Weather` and a world position, it decides whether that position is
currently getting no precipitation, rain, or snow:

- If `weather` has no precipitation at all (deserts, badlands, nether biomes,
  etc. - `has_precipitation == false` in the biome's data), the answer is
  always `Precipitation::None`, regardless of temperature.
- Otherwise, compute the position-adjusted temperature via
  `weather.compute_temperature(x, y, z, sea_level)` (this method already
  exists on `pumpkin_util::biome::Weather` and is not part of this stub -
  call it, do not reimplement temperature math). If that temperature is
  strictly less than `FREEZE_AND_SNOW_TEMPERATURE` (0.15), the answer is
  `Precipitation::Snow`. Otherwise it is `Precipitation::Rain`.

The 0.15 threshold is not invented for this stub: it is the exact same
constant already used by the worldgen `FreezeTopLayerFeature` at
`pumpkin-world/src/generation/feature/features/freeze_top_layer.rs:37,49`,
which freezes water and lays snow at chunk-generation time using this same
cutoff. This runtime feature is the missing counterpart to that worldgen
feature - same threshold, evaluated every tick instead of once at generation.

`has_precipitation` is a private field on `Weather` (in
`pumpkin-util/src/biome.rs`) currently marked `#[expect(dead_code)]` because
nothing reads it yet. `Weather` and `precipitation_kind` live in different
crates, so you cannot add a public accessor as part of this stub (this
stub's `FILES` line and `CRATES: pumpkin` scope you to
`pumpkin/src/world/precipitation.rs` only). Instead, read the field directly:
`precipitation_kind` is defined in the `pumpkin` crate, but `has_precipitation`
is a private field of a type defined in the `pumpkin-util` crate, so you
cannot reach it from here without a public getter or another public
accessor. Check `pumpkin-util/src/biome.rs` first: if `Weather` already
exposes a way to read `has_precipitation` (a getter, or the field already
made `pub`), use that. If it does not, you need to add a small `#[must_use]
pub const fn has_precipitation(&self) -> bool` accessor to `Weather` in
`pumpkin-util/src/biome.rs` and remove the now-inaccurate
`#[expect(dead_code)]` on that field (leaving it in place will fail the
build once the field is actually read, because the expectation becomes
unfulfilled and `unfulfilled_lint_expectations` is denied). That is the one
piece of this stub that reaches outside `pumpkin/src/world/precipitation.rs`;
everything else stays local to this file.

Leave `compute_temperature` and the rest of `Weather` alone - it already has
the noise-sampling logic for above-sea-level positions and is out of scope
here. Do not add a live `World`/chunk/RNG dependency to this function: it
must stay a pure function of its five parameters so the unit tests can call
it directly, and it must not be confused with `World::is_raining_at` (which
would additionally need sky-visibility and a live chunk heightmap, and is
intentionally not part of this scaffold).

The specific way this goes wrong: using `<=` instead of `<` at the 0.15
boundary (vanilla's own biome data never actually lands exactly on 0.15, but
match the strict `<` used by the sibling worldgen feature for the snow
check), or checking `has_precipitation` after the temperature check instead
of before (a cold desert-like biome with `has_precipitation == false` must
still return `Precipitation::None`, not `Precipitation::Snow`).
