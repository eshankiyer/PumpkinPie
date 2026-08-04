# Design: per-block `rotate`/`mirror` for structure-template placement

Status: design only, no code changed by this document.

## 1. Summary of the gap

A census across the block implementations found the same missing piece of
per-block logic independently in at least seven blocks: `RedstoneWireBlock`,
`StairBlock`, `LeafLitterBlock`, `FenceBlock` (related but distinct, see
§3.4), `BigDripleafStemBlock`, `MangrovePropaguleBlock`, `FlowerbedBlock`.
Vanilla's `Block.rotate(BlockState, Rotation)` / `Block.mirror(BlockState,
Mirror)` are overridden per-block to transform directional state (`FACING`,
`AXIS`, connection-side properties, `SHAPE`, etc.) whenever a block is placed
as part of a rotated/mirrored structure template (villages, shipwrecks,
ruined portals, pillager outposts, bastions, trial chambers, ancient cities,
and any other NBT structure piece). The read below shows this is **not** a
"trait method is missing" problem — the trait method and the plumbing to it
already exist in one code path — it is that:

1. The generic default implementation is a total no-op, and no block
   overrides it.
2. There are **two parallel, disconnected placement code paths**, and only
   one of them is wired to the trait method at all. The other path
   (structure *templates*, i.e. the NBT-based structures that are the
   actual subject of the six-plus block bugs) never calls the trait method —
   it runs its own separate, weaker, string-based transform instead.

Both problems have to be fixed for the census bugs to go away. Fixing only
#1 (adding per-block overrides) does nothing for the blocks placed via NBT
templates until #2 is also fixed, because that path doesn't call the trait
method in the first place.

## 2. What already exists today (read precisely, so we don't re-invent it)

### 2.1 `pumpkin-data/src/block_rotation.rs`

This file is in good shape and does **not** need architectural changes. It
defines:

- `Rotation` (`None`, `Clockwise90`, `Rotate180`, `CounterClockwise90`) with
  `transform_pos`, `rotate_offset`, `transform_size`, `rotate_facing(&str)`,
  `rotate_axis(&str)`, `rotate_block_rotation(i32)` (signs/banners 0-15),
  `then()` (composition), `inverse()`, `to_axis()`.
- `Mirror` (`None`, `LeftRight`, `FrontBack`) with `transform_pos`,
  `mirror_facing(&str)`, `mirror_block_rotation(i32)`, and
  `get_rotation(Rotation) -> Rotation` — this is the Rust equivalent of
  vanilla's `Mirror.getRotation(Direction)`/the mirror-as-rotation trick used
  by `RedStoneWireBlock`/`StairBlock`'s `mirror()` overrides (mirror is
  implemented in vanilla as "rotate 180, then optionally reinterpret which
  corner/shape it is" — see §4).

These are string-keyed helpers (`"north"`, `"facing"`, `"x"`/`"z"`), which is
why they're consumed today by the raw-NBT-property code in
`BlockStateResolver` (§2.3) rather than by typed `BlockProperties` structs.
They are perfectly reusable from the new trait method too — a per-block
`rotate`/`mirror` override on a typed property struct can still call
`rotation.rotate_facing(...)` etc. by converting the enum to/from its string
form, or (preferably, see §5) by adding small typed convenience methods
later. No changes to this file are required for this design; it is listed in
§6 only as an optional follow-up file.

### 2.2 The trait method already exists — and is already wired for one placement path

`pumpkin/src/block/mod.rs` (`BlockBehaviour` trait, lines ~224-235):

```rust
fn mirror(&self, block: &Block, state_id: BlockStateId, mirror: Mirror) -> &'static BlockState {
    block.mirror(state_id, mirror)
}

fn rotate(&self, block: &Block, state_id: BlockStateId, rotation: Rotation) -> &'static BlockState {
    block.rotate(state_id, rotation)
}
```

So the trait hook the census asked for **already exists**, with exactly the
shape the task expects (default delegates to a block-level default, override
per block). The problem is entirely downstream:

`pumpkin-data/src/blocks.rs` (lines 180-196), the generic default that every
block falls back to:

```rust
pub const fn mirror(&self, id: BlockStateId, _mirror: crate::block_rotation::Mirror) -> &'static BlockState {
    BlockState::from_id(id)
}

pub const fn rotate(&self, id: BlockStateId, _rotation: crate::block_rotation::Rotation) -> &'static BlockState {
    BlockState::from_id(id)
}
```

This is a hard no-op: it ignores the rotation/mirror argument and returns the
state unchanged. Since **none** of `RedstoneWireBlock`, `StairBlock`,
`LeafLitterBlock`, `BigDripleafStemBlock`, `MangrovePropaguleBlock`,
`FlowerbedBlock` override `BlockBehaviour::rotate`/`mirror` (confirmed by
reading `pumpkin/src/block/blocks/stairs.rs` and
`pumpkin/src/block/blocks/redstone/redstone_wire.rs` in full — their `impl
BlockBehaviour` blocks have no `rotate`/`mirror` fn), every call into this
path for these blocks is currently a no-op regardless of caller.

The plumbing that *does* reach this trait method:

- `pumpkin/src/block/registry.rs` (`BlockRegistry::mirror`/`::rotate`,
  ~line 1352-1377) looks up the block's `Arc<dyn BlockBehaviour>` and calls
  the trait method, falling back to `Block::mirror`/`Block::rotate` if the
  block has no registered behaviour at all.
- `pumpkin-world/src/generation/structure/structures/mod.rs`, `place_block`
  (~line 185-207), the **hand-written structure piece placer** used by
  procedurally-coded generators (woodland mansion, ocean monument,
  strongholds, and similar Rust-coded piece placers that call
  `place_block(...)` directly per block), *does* call
  `block_registry.mirror(...)` / `block_registry.rotate(...)` before placing
  each block. **This path is correctly wired end-to-end already** — it is
  just defeated by the no-op default in §2.2, because no block overrides it.
- `pumpkin/src/command/commands/place.rs` (`/place` command / structure
  block manual placement) also calls `block.mirror`/`block.rotate` directly
  the same way.
- `pumpkin/src/world/mod.rs` (~line 6230/6239) exposes
  `World::mirror_block_state`/`rotate_block_state`-style wrappers around
  `block_registry.mirror`/`rotate` for other callers (piston push-back of
  rotated pistons is not relevant here, but any future caller gets the fix
  for free once the per-block overrides exist).

**Conclusion for this half:** the fix for the "manual piece placer" and
`/place` code paths is *only* §3 (implement the per-block overrides). The
trait, the registry dispatch, and the callers are already correct.

### 2.3 The other, disconnected path: NBT structure templates

`pumpkin-world/src/generation/structure/template/template_piece.rs`
(`TemplatePiece::place_blocks`, called from `StructurePieceBase::place`) is
the placer used for **NBT-template structures** — this is the code path that
actually places villages, shipwrecks, ruined portals, pillager outposts,
bastion remnants, trial chambers, etc. from `.nbt` structure files, which is
exactly the kind of structure the task description calls out ("mansions,
villages, shipwrecks — anything using structure NBT with rotation/mirror
parameters" — note: the woodland mansion is *not* actually NBT-based in
either vanilla or Pumpkin, it is the hand-coded `mansion/placer.rs`
generator using the §2.2 path; the NBT-driven structures are villages,
shipwrecks, ruined portals, outposts, bastions, trial chambers and similar).

For each block in the template it calls:

```rust
let Some(state) = BlockStateResolver::resolve(palette_entry, self.rotation, self.mirror) else { ... };
```

`BlockStateResolver::resolve`
(`pumpkin-world/src/generation/structure/template/block_state_resolver.rs`)
does **not** call `block_registry.rotate`/`mirror` or
`BlockBehaviour::rotate`/`mirror` at all. It runs its own separate, from-NBT
transform directly on the raw `(String, String)` property pairs read out of
the structure's palette entry, before the block state has even been
resolved to a `BlockState`/typed properties struct:

```rust
let transformed_key = match key.as_str() {
    "north" | "south" | "east" | "west" => rotation.rotate_facing(mirror.mirror_facing(key)).to_string(),
    _ => key.clone(),
};
let new_value = Self::transform_property(key, value, rotation, mirror);
```

and `transform_property` only special-cases `"facing"`, `"axis"`, and
`"rotation"` (signs/banners); every other key (including stairs' `"shape"`)
passes through unchanged.

This is why the census still finds these bugs "unfixed" even though a
rotate/mirror mechanism nominally exists elsewhere in the codebase:

- `RedstoneWireBlock`'s `north`/`south`/`east`/`west` connection properties
  *happen* to work by accident for pure key-renaming under 90°/180°/270°
  rotation, because vanilla's `RedStoneWireBlock.rotate()` is also just a
  same-shaped key permutation (see §4.1) and the connection property values
  (`up`/`side`/`none`) don't need transforming, only the key does. But
  vanilla's `mirror()` does two *specific* pairwise swaps
  (`LEFT_RIGHT`: swap `north`/`south`; `FRONT_BACK`: swap `east`/`west`) —
  the generic `key -> rotate_facing(mirror_facing(key))` composition in
  `BlockStateResolver` happens to reduce to the same net permutation *for
  redstone wire specifically*, so this one is likely fine by coincidence of
  the transform being an involution on this particular property set. It
  should still move onto the trait method in §3 so it's driven by the same
  logic as every other call site instead of relying on that coincidence.
- `StairBlock`'s `shape` property (`straight` / `inner_left` / `inner_right`
  / `outer_left` / `outer_right`) is **never transformed** by
  `BlockStateResolver` — mirroring a staircase currently keeps the same
  corner shape label, which after the mirror is the *wrong* corner (see the
  exact vanilla swap table in §4.2). This is the "genuinely complex shape
  logic" the task calls out.
- `LeafLitterBlock`/`BigDripleafStemBlock`/`MangrovePropaguleBlock`/
  `FlowerbedBlock` typically key off `facing` and/or per-segment boolean
  flags that are not simple `facing`/`axis`/`rotation` strings, so they
  silently pass through `transform_property`'s `_ => value.to_string()`
  fallback unchanged.

**Conclusion for this half:** fixing `TemplatePiece`/`BlockStateResolver` to
call through to `BlockBehaviour::rotate`/`mirror` (via `block_registry`) is
required, or the per-block overrides in §3 will have zero effect on NBT
structures, which are the structures actually named in the task and in the
census.

## 3. The fix

### 3.1 No new trait method needed — the signature already exists and is right

Do **not** add a second `rotate`/`mirror` method to `BlockBehaviour`. The
existing one already matches the pattern the task asks for (a default
that most blocks never need to touch, overridden per-block, mirroring how
`on_place`/`get_state_for_neighbor_update` work):

```rust
// pumpkin/src/block/mod.rs — already present, unchanged
fn mirror(&self, block: &Block, state_id: BlockStateId, mirror: Mirror) -> &'static BlockState {
    block.mirror(state_id, mirror)
}
fn rotate(&self, block: &Block, state_id: BlockStateId, rotation: Rotation) -> &'static BlockState {
    block.rotate(state_id, rotation)
}
```

These are synchronous, non-async, unlike most of `BlockBehaviour` — that's
correct and should stay that way: vanilla's `Block.rotate`/`mirror` are also
plain synchronous state transforms with no world access, and structure
generation runs off the main tick loop under `ProtoChunk`, not against a
live `World`, so keeping this non-async is a hard requirement, not a
stylistic choice — an `async fn` signature here would not compile against
`ProtoChunk`'s synchronous generation path in `TemplatePiece::place_blocks`.

The only change needed to the trait file is none — §3.2/§3.3 are pure
`impl BlockBehaviour for XBlock { fn rotate(...) / fn mirror(...) }`
overrides in each block's existing file, exactly like any other trait
method override already in the codebase (e.g. `on_neighbor_update` in
`stairs.rs`).

### 3.2 Fix the integration point: wire `TemplatePiece` to the trait method

This is the change that actually unblocks the census bugs. In
`pumpkin-world/src/generation/structure/template/template_piece.rs`,
`place_blocks` currently does:

```rust
let Some(state) = BlockStateResolver::resolve(palette_entry, self.rotation, self.mirror) else { ... };
```

Change the resolution to a two-step process, mirroring what
`structures/mod.rs::place_block` already does correctly:

1. `BlockStateResolver::resolve_simple(palette_entry)` — resolve the palette
   entry's block name + literal NBT properties to an **un-rotated**
   `&'static BlockState` (this keeps `resolve`'s existing job of turning
   strings into a real state, but drops its own ad-hoc rotate/mirror
   string-transform logic entirely — see §3.3).
2. Feed that resolved state through the same dispatch `structures/mod.rs`
   uses:
   ```rust
   let block = Block::from_state_id(state.id);
   let mut state = state;
   if self.mirror != Mirror::None {
       state = block_registry.mirror(block, state.id, self.mirror);
   }
   if self.rotation != Rotation::None {
       state = block_registry.rotate(block, state.id, self.rotation);
   }
   ```

`TemplatePiece::place_blocks` and `StructurePieceBase::place` do not
currently receive a `&dyn WorldPortalExt`/block registry with the needed
`mirror`/`rotate` methods reachable at that point — check
`StructurePieceBase::place`'s existing `_block_registry:
&dyn WorldPortalExt` parameter (already present in the trait signature,
currently unused/prefixed `_` in `TemplatePiece::place`). `WorldPortalExt`
would need `mirror`/`rotate` added to its trait surface (or `TemplatePiece`
needs the concrete `Arc<BlockRegistry>` threaded through instead of the
`WorldPortalExt` trait object) so `place_blocks` can call into
`BlockRegistry::mirror`/`rotate`. This is a small plumbing change scoped to
`pumpkin-world`'s structure-generation module boundary with
`pumpkin`'s `BlockRegistry` — check how `structures/mod.rs::place_block`
itself receives its `block_registry: &dyn WorldPortalExt` argument (it
already takes one, from its caller) and thread `TemplatePiece::place`'s
already-present `_block_registry` parameter the same way instead of
discarding it.

### 3.3 Simplify `BlockStateResolver`: drop the parallel rotate/mirror logic

Once §3.2 lands, `BlockStateResolver::resolve`'s `rotation`/`mirror`
parameters and its `transform_property`/key-remap logic become dead code —
delete them and keep only `resolve_simple`'s job (name + literal properties
-> `BlockState`, no transform). Keep the file's existing unit tests but
retarget them at `resolve_simple` plus a couple of new tests at the
`TemplatePiece` level that check a rotated/mirrored stair/redstone-wire
palette entry round-trips through `block_registry.rotate`/`mirror` (these
can be added once the per-block overrides in §4 exist; they are the
regression tests for the census bugs).

### 3.4 `FenceBlock` — scope note, not part of this fix

Vanilla's `FenceBlock` (checked in
`net/minecraft/world/level/block/FenceBlock.java`) has **no**
`rotate`/`mirror` override at all — fence connection state
(`north`/`south`/`east`/`west` booleans) is not carried through structure
rotation as stored state; it is recomputed live from neighbor blocks the
same way it is after any neighbor update (`updateShape`/`getStateForPlacement`
logic keyed off actual neighboring blocks in the *rotated* world, which are
already in their rotated positions once each neighboring block has itself
been placed and rotated). The census's "connection-side exception list" note
for `FenceBlock` refers to the separate list of blocks a fence is allowed to
visually connect to (fence gates, walls, glass panes, etc.) — a
`can_connect`-style predicate bug, not a rotate/mirror bug. It does not need
a `BlockBehaviour::rotate`/`mirror` override and is out of scope for this
design; file it as its own row if the connect-predicate list is wrong.

## 4. Worked examples

### 4.1 `RedstoneWireBlock`

Vanilla (`RedStoneWireBlock.java`):

```java
protected BlockState rotate(BlockState state, Rotation rotation) {
   return switch (rotation) {
      case CLOCKWISE_180 -> state.setValue(NORTH, state.getValue(SOUTH))
                                 .setValue(EAST,  state.getValue(WEST))
                                 .setValue(SOUTH, state.getValue(NORTH))
                                 .setValue(WEST,  state.getValue(EAST));
      case COUNTERCLOCKWISE_90 -> state.setValue(NORTH, state.getValue(EAST))
                                        .setValue(EAST,  state.getValue(SOUTH))
                                        .setValue(SOUTH, state.getValue(WEST))
                                        .setValue(WEST,  state.getValue(NORTH));
      case CLOCKWISE_90 -> state.setValue(NORTH, state.getValue(WEST))
                                .setValue(EAST,  state.getValue(NORTH))
                                .setValue(SOUTH, state.getValue(EAST))
                                .setValue(WEST,  state.getValue(SOUTH));
      default -> state;
   };
}

protected BlockState mirror(BlockState state, Mirror mirror) {
   return switch (mirror) {
      case LEFT_RIGHT -> state.setValue(NORTH, state.getValue(SOUTH)).setValue(SOUTH, state.getValue(NORTH));
      case FRONT_BACK -> state.setValue(EAST, state.getValue(WEST)).setValue(WEST, state.getValue(EAST));
      default -> super.mirror(state, mirror);
   };
}
```

Pumpkin equivalent, in `pumpkin/src/block/blocks/redstone/redstone_wire.rs`,
added to the existing `impl BlockBehaviour for RedstoneWireBlock` block
(reusing the file's existing `RedstoneWireProperties` type alias for
`RedstoneWireLikeProperties`):

```rust
fn rotate(&self, block: &Block, state_id: BlockStateId, rotation: Rotation) -> &'static BlockState {
    let mut wire = RedstoneWireProperties::from_state_id(state_id, block);
    let (n, e, s, w) = (wire.north, wire.east, wire.south, wire.west);
    match rotation {
        Rotation::Rotate180 => {
            wire.north = s.to_north(); wire.east = w.to_east();
            wire.south = n.to_south(); wire.west = e.to_west();
        }
        Rotation::CounterClockwise90 => {
            wire.north = e.to_north(); wire.east = s.to_east();
            wire.south = w.to_south(); wire.west = n.to_west();
        }
        Rotation::Clockwise90 => {
            wire.north = w.to_north(); wire.east = n.to_east();
            wire.south = e.to_south(); wire.west = s.to_west();
        }
        Rotation::None => {}
    }
    BlockState::from_id(wire.to_state_id(block))
}

fn mirror(&self, block: &Block, state_id: BlockStateId, mirror: Mirror) -> &'static BlockState {
    let mut wire = RedstoneWireProperties::from_state_id(state_id, block);
    match mirror {
        Mirror::LeftRight => {
            let (n, s) = (wire.north.to_wire_connection(), wire.south.to_wire_connection());
            wire.north = s.to_north();
            wire.south = n.to_south();
        }
        Mirror::FrontBack => {
            let (e, w) = (wire.east.to_wire_connection(), wire.west.to_wire_connection());
            wire.east = w.to_east();
            wire.west = e.to_west();
        }
        Mirror::None => {}
    }
    BlockState::from_id(wire.to_state_id(block))
}
```

Note: `north`/`south`/`east`/`west` are four *distinct* generated types
(`NorthRedstone`, `SouthRedstone`, `EastRedstone`, `WestRedstone`), not one
shared enum, so a direct field swap won't typecheck — the implementation
above converts each side to the shared `WireConnection` enum (the file
already has `NorthRedstone::to_wire_connection()` / `WireConnection::to_north()`-style
conversions for exactly this purpose, see lines ~434-517 of the current
file) and reassigns through that. The `rotate` example above the fold has
the same issue for its four-way permutation and should be rewritten the same
way (via `to_wire_connection()`/`to_{north,south,east,west}()`) rather than
assigning mismatched property types directly — this is a mechanical rewrite
using helpers already in the file; no new conversion code is needed.

### 4.2 `StairBlock`

Vanilla (`StairBlock.java`):

```java
protected BlockState rotate(BlockState state, Rotation rotation) {
   return state.setValue(FACING, rotation.rotate(state.getValue(FACING)));
}

protected BlockState mirror(BlockState state, Mirror mirror) {
   Direction facing = state.getValue(FACING);
   StairsShape shape = state.getValue(SHAPE);
   return switch (mirror) {
      case LEFT_RIGHT -> switch (facing.getAxis() == Direction.Axis.Z ? shape : null) {
         // (abbreviated — the real switch is keyed on facing+shape, see full source)
      };
      // see full table below
   };
}
```

The actual vanilla table (both `LEFT_RIGHT` and `FRONT_BACK` branches key off
`facing` and swap `OUTER_LEFT<->OUTER_RIGHT` / `INNER_LEFT<->INNER_RIGHT`
*only* when the mirror axis is perpendicular to the facing's axis, and always
apply a 180° rotation to `FACING` when the shape needed swapping):

- `Mirror::LeftRight` (mirrors across the X axis, i.e. flips
  north/south-facing stairs' left/right sense): if `facing` is `North` or
  `South`, `OUTER_LEFT -> {rotate180, OUTER_RIGHT}`,
  `INNER_LEFT -> {rotate180, INNER_RIGHT}` (and the symmetric right->left),
  `STRAIGHT -> rotate180` only. If `facing` is `East`/`West`, no shape swap,
  just fall through to `super.mirror` (i.e. `state` unchanged — `LeftRight`
  doesn't rotate a facing already on the X axis).
- `Mirror::FrontBack` is the East/West-axis analogue with the same shape
  swap logic and the opposite facing-axis test.

Pumpkin equivalent, in `pumpkin/src/block/blocks/stairs.rs`, added to the
existing `impl BlockBehaviour for StairBlock` (reusing the file's existing
`StairsProperties` alias for `OakStairsLikeProperties`):

```rust
fn rotate(&self, block: &Block, state_id: BlockStateId, rotation: Rotation) -> &'static BlockState {
    let mut props = StairsProperties::from_state_id(state_id, block);
    props.facing = props.facing.rotate(rotation); // HorizontalFacing needs a `rotate(Rotation)` helper — see §6
    BlockState::from_id(props.to_state_id(block))
}

fn mirror(&self, block: &Block, state_id: BlockStateId, mirror: Mirror) -> &'static BlockState {
    let mut props = StairsProperties::from_state_id(state_id, block);
    let axis_matches = matches!(
        (mirror, props.facing),
        (Mirror::LeftRight, HorizontalFacing::North | HorizontalFacing::South)
            | (Mirror::FrontBack, HorizontalFacing::East | HorizontalFacing::West)
    );
    if axis_matches {
        props.shape = match props.shape {
            StairsShape::OuterLeft => StairsShape::OuterRight,
            StairsShape::OuterRight => StairsShape::OuterLeft,
            StairsShape::InnerLeft => StairsShape::InnerRight,
            StairsShape::InnerRight => StairsShape::InnerLeft,
            StairsShape::Straight => StairsShape::Straight,
        };
        props.facing = props.facing.rotate(Rotation::Rotate180);
    }
    BlockState::from_id(props.to_state_id(block))
}
```

This is the worked example the task calls "genuinely complex shape logic":
the mirror is not a pure per-property transform, it's conditional on the
*combination* of `facing`'s axis and which mirror axis is being applied,
which is exactly why this can't live in the generic string-keyed
`BlockStateResolver` code and needs a real per-block override.

## 5. `OctahedralGroup` / `Mirror::rotation()` / `Mirror::symbol()` — separate, lower-priority piece

`Mirror.rotation()` in vanilla returns a `com.mojang.math.OctahedralGroup`
value (one of 24 elements of the full cube symmetry group, each carrying a
3x3 rotation matrix) and `Mirror.symbol()` returns a translatable
`Component` for GUI text. Grepping this codebase confirms **no
`OctahedralGroup` type exists anywhere in Rust** currently.

These two members are used exclusively by:

- The **`StructureBlockEntity`**/structure-block GUI feature (arbitrary
  player-driven rotate/mirror/save/load of a structure region with full 3D
  corner-block integrity data, which needs the full 24-element octahedral
  group rather than just 4 rotations x 3 mirrors because structure blocks
  also support flipping saved structures that were captured at an angle,
  and because `Mirror.symbol()` feeds the in-game GUI button label).
- `Mirror.rotation()`'s only other call site in vanilla is
  `BlockGetter`/model-baking code (client-side block model rotation for
  rendering), which is not applicable to a server.

Neither of these is needed to fix the six-plus census block bugs: those are
all fixed by plain `Rotation`/`Mirror` (§3-§4) acting during worldgen
structure placement, which never constructs an `OctahedralGroup`. The
`OctahedralGroup`/`Mirror::rotation()`/`Mirror::symbol()` piece is therefore
**scoped out of this design** as lower priority, gated entirely behind
whether/when `StructureBlock` itself gets implemented as a player-facing
feature. If a separate "unimplemented blocks" design doc for `StructureBlock`
exists or is written, `OctahedralGroup` belongs there, not here — this
document's fix is complete without it. (No such doc currently exists in
`designs/`; note this cross-reference for whoever writes it.)

## 6. Files to create/modify, in dependency order

1. **No trait signature change** — `pumpkin/src/block/mod.rs`'s
   `BlockBehaviour::rotate`/`::mirror` already exist with the right shape;
   nothing to do here structurally. (Optional: add doc comments pointing at
   this design doc and at `structures/mod.rs::place_block` /
   `template_piece.rs::place_blocks` as the two call sites, for the next
   person who looks for callers.)
2. **`pumpkin-data/src/block_properties.rs` (generated) /
   `pumpkin-data/src/block_direction.rs`** — add a `HorizontalFacing::rotate(Rotation)`
   (and/or `Mirror`) convenience helper if one doesn't already exist next to
   the existing `rotate_clockwise`/`rotate_counter_clockwise` methods on
   `BlockDirection` in `block_direction.rs` — check whether `HorizontalFacing`
   (used by `StairsProperties.facing`) has an equivalent before adding a new
   one; this is a small prerequisite used by every per-block override in
   step 5 that has a plain `facing` property.
3. **`pumpkin-world/src/generation/structure/structures/mod.rs`** — no
   change needed; this path is already correctly wired (§2.2), listed here
   only as the reference implementation to copy in step 4.
4. **`pumpkin-world/src/generation/structure/template/template_piece.rs`**
   — thread the block registry's `rotate`/`mirror` into `place_blocks`,
   replacing the call to the rotating variant of `BlockStateResolver::resolve`
   with `resolve_simple` + `block_registry.mirror`/`.rotate`, matching
   `structures/mod.rs::place_block`'s existing pattern (§3.2). Requires
   exposing `mirror`/`rotate` on whatever registry-handle type
   `StructurePieceBase::place`'s `_block_registry: &dyn WorldPortalExt`
   parameter resolves to (or threading the concrete `BlockRegistry` through
   instead — whichever `structures/mod.rs`'s call site already does, copy
   that).
5. **`pumpkin-world/src/generation/structure/template/block_state_resolver.rs`**
   — delete the `rotation`/`mirror` parameters and `transform_property`/key
   remap logic from `resolve`; keep only the literal-property resolution
   (§3.3). Update/retarget the existing unit tests.
6. **Per-block overrides**, each a self-contained addition to an existing
   `impl BlockBehaviour for XBlock` block, no other files touched per block:
   - `pumpkin/src/block/blocks/redstone/redstone_wire.rs` — `rotate`/`mirror`
     via the four-way key permutation (§4.1).
   - `pumpkin/src/block/blocks/stairs.rs` — `rotate`/`mirror` with the
     facing + shape swap table (§4.2). `StairBlock` is `#[pumpkin_block_from_tag]`-based
     covering *all* stair variants, so this one override fixes every wood
     type at once.
   - `pumpkin/src/block/blocks/plant/leaf_litter.rs`,
     `pumpkin/src/block/blocks/plant/big_dripleaf_stem.rs`,
     `pumpkin/src/block/blocks/plant/mangrove_propagule.rs`,
     `pumpkin/src/block/blocks/plant/flowerbed.rs` — read each block's
     vanilla source (`LeafLitterBlock.java`, `BigDripleafStemBlock.java`
     extends `HorizontalDirectionalBlock` generic rotate/mirror,
     `MangrovePropaguleBlock.java` extends `HorizontalDirectionalBlock` too
     — likely all reduce to a plain `facing.rotate(rotation)`/
     `facing.mirror(mirror)` one-liner once step 2's helper exists, i.e. the
     same shape as `StairBlock.rotate` without the shape-swap complexity) and
     add the matching one-line-to-few-line override each. These four are
     expected to be simple once steps 2 and 4 land — no further design
     needed per-block, they follow the `StairBlock.rotate` half of §4.2
     without the mirror shape table.
7. **Regression tests** — add to `template_piece.rs` (or a sibling test
   module) a test per fixed block: build a minimal one-block
   `StructureTemplate`/palette entry with a known rotation+mirror, run it
   through `TemplatePiece::place_blocks` (or the smallest exposed seam),
   assert the resulting `BlockState`'s properties match hand-computed
   vanilla output. These are the tests that would have caught all six
   original census bugs and should stay red until steps 4-6 land together —
   fixing only the per-block overrides without step 4 will leave these
   tests failing, which is the intended tripwire against merging a partial
   fix.

## 7. Priority ordering

1. Step 4 (wire `TemplatePiece` to `block_registry.rotate`/`mirror`) is the
   highest-priority single change: without it, every per-block override in
   step 6 is inert for the NBT structures the census bugs are actually about.
2. Steps 5 (simplify `BlockStateResolver`) and 2 (facing rotate helper) are
   small prerequisites/cleanups that can land alongside step 4.
3. Step 6 (`StairBlock`, `RedstoneWireBlock` first — they're the ones with
   worked examples above and the ones most likely to show up rotated in
   villages/shipwrecks/outposts) then the four plant blocks.
4. §5 (`OctahedralGroup`) is explicitly deprioritized and only relevant once
   someone starts implementing `StructureBlock` as a player-facing feature.
