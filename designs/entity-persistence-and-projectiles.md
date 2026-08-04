# Design: entity persistence and projectile parity gaps

Scope: 8 subsystems flagged by the entity/projectile census as missing or stubbed.
Vanilla source: `/tmp/pumpkin-vanilla-26.2/decompiled/`. Rust source: `pumpkin/src/`.
This document designs the fix; it does not implement it.

---

## 1. EnderDragon NBTStorage (phase state lost on save/reload)

### Vanilla source

`net/minecraft/world/entity/boss/enderdragon/EnderDragon.java:734-748`:

```java
protected void addAdditionalSaveData(final ValueOutput output) {
   super.addAdditionalSaveData(output);
   output.putInt("DragonPhase", this.phaseManager.getCurrentPhase().getPhase().getId());
   output.putInt("DragonDeathTime", this.dragonDeathTime);
   output.putFloat("sitting_damage_received", this.sittingDamageReceived);
}

protected void readAdditionalSaveData(final ValueInput input) {
   super.readAdditionalSaveData(input);
   input.getInt("DragonPhase").ifPresent(phaseId -> this.phaseManager.setPhase(EnderDragonPhase.getById(phaseId)));
   this.dragonDeathTime = input.getIntOr("DragonDeathTime", 0);
   this.sittingDamageReceived = input.getFloatOr("sitting_damage_received", 0.0F);
}
```

Three keys only: `DragonPhase` (int id), `DragonDeathTime` (int), `sitting_damage_received`
(float). `EnderDragonPart.java:34-40` has genuinely empty `readAdditionalSaveData`/
`addAdditionalSaveData` overrides in vanilla too — `shouldBeSaved()` returns `false`
(line 73-75) because parts are recreated by the dragon on load, so the Rust
`impl NBTStorage for EnderDragonPart {}` empty stub is **already correct** and does not
need a change. The dragon fight's crystal/spawn state itself lives in
`EnderDragonFight`/`level.dat`'s `DragonFight` compound, out of scope here.

### Current Rust state

`pumpkin/src/entity/boss/ender_dragon.rs:807`: `impl NBTStorage for EnderDragonEntity {}`
— fully empty. The struct already carries every field vanilla persists and more:
`phase: Mutex<EnderDragonPhase>` (maps to `DragonPhase`), `dragon_death_time: Mutex<i32>`,
`sitting_damage_received: Mutex<f32>`. `EnderDragonPhase` (in `phase/mod.rs`, not read yet
but referenced throughout) needs a stable `id()`/`from_id()` round-trip matching vanilla's
`EnderDragonPhase.getId()`/`getById(int)` — check that mapping (`HOVERING=0`,
`STRAFING=1`, ... per vanilla) before wiring `read`/`write`.

### Target design

Implement `NBTStorage` for `EnderDragonEntity` mirroring the two other `Mutex`-guarded
entity NBTStorage impls in the codebase (e.g. look at `wither.rs` or `tnt.rs` for the
async-lock-then-put pattern). Because these fields live behind `tokio::sync::Mutex`,
`write_nbt`/`read_nbt_non_mut` must `.lock().await` each one — no blocking locks in an
async fn.

```rust
impl NBTStorage for EnderDragonEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            let phase = *self.phase.lock().await;
            nbt.put_int("DragonPhase", phase.id());
            nbt.put_int("DragonDeathTime", *self.dragon_death_time.lock().await);
            nbt.put_float("sitting_damage_received", *self.sitting_damage_received.lock().await);
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            if let Some(id) = nbt.get_int("DragonPhase")
                && let Some(phase) = EnderDragonPhase::from_id(id)
            {
                // Do not go through `set_phase` here: that method calls the phase's
                // begin()/end() hooks (spawning targets, sounds, etc.), which vanilla's
                // load path explicitly bypasses by writing straight to the phase field.
                *self.phase.lock().await = phase;
            }
            *self.dragon_death_time.lock().await = nbt.get_int("DragonDeathTime").unwrap_or(0);
            *self.sitting_damage_received.lock().await =
                nbt.get_float("sitting_damage_received").unwrap_or(0.0);
        })
    }
}
```

Also confirm the call site: whatever currently drives `EnderDragonEntity`'s
`as_nbt_storage()` (already implemented, returns `self`) must be invoked from the normal
entity save/load path (`Entity`'s NBT round-trip in `chunk`/`world` persistence) the same
way other mobs are — this is presumably already wired generically since `MobEntity`/
`LivingEntity` NBTStorage delegates through `as_nbt_storage()`; only the trait impl body
is missing.

### Files to modify

- `pumpkin/src/entity/boss/ender_dragon.rs` — implement `NBTStorage for EnderDragonEntity`
  (the `impl NBTStorage for EnderDragonPart {}` empty stub stays empty, matching vanilla).
- `pumpkin/src/entity/boss/ender_dragon/phase/mod.rs` (not read in this pass — verify it
  exists and that `EnderDragonPhase` has, or gains, `id()`/`from_id()` matching vanilla's
  `EnderDragonPhase` registry order before wiring the read path).

### Needs verification

- Exact vanilla phase-id ordering (`EnderDragonPhase.java`'s static registration order)
  to make sure `from_id`/`id()` round-trips match — wiki does not document this reliably,
  cite `EnderDragonPhase.java`'s field order directly when implementing.

---

## 2. FallingBlockEntity landing/despawn/damage/tile-entity gaps

### Vanilla source

`net/minecraft/world/entity/item/FallingBlockEntity.java`:

- `tick()` (lines 147-243) is the whole state machine. Key vanilla behaviors currently
  missing in Rust:
  - **100/600-tick despawn** (lines 172-179): if not on ground/stuck-in-water and
    `time > 100` while outside world Y bounds, or unconditionally `time > 600`, drop the
    item (if `dropItem` and the `ENTITY_DROPS` gamerule) and discard — this prevents
    blocks falling forever into the void.
  - **mayReplace / wouldSurvive gate** (lines 184-232): on landing, vanilla does *not*
    unconditionally set the block. It checks:
    - `mayReplace = currentState.canBeReplaced(...)` (down-facing placement context)
    - `wouldContinueFalling = FallingBlock.isFree(state below) && !(isConcrete && stuck in water)`
    - `wouldSurvive = blockState.canSurvive(level, pos) && !wouldContinueFalling`
    - only if `mayReplace && wouldSurvive` does it call `level.setBlock` and fire
      `Fallable.onLand`; otherwise (unless `cancelDrop`) it discards and, if
      `dropItem && ENTITY_DROPS`, calls `callOnBrokenAfterFall` then `spawnAtLocation`.
  - **cancelDrop** (line 233-236): if `cancelDrop` is set (e.g., an anvil that broke),
    skip the item drop entirely, just call `callOnBrokenAfterFall`.
  - **blockData / tile-entity restoration** (lines 204-220): if the falling block carried
    `blockData` (from a spawner or NBT-holding block that got dislodged) and the landed
    state `hasBlockEntity()`, merge the stored `blockData` compound into the placed block
    entity's own save-without-metadata output and reload it.
  - **causeFallDamage** (lines 252-279): currently hardcoded to `false` in Rust; vanilla
    computes `damage = min(floor(fallDistanceInt * fallDamagePerDistance), fallDamageMax)`,
    applies it to living entities in the bounding box (excluding creative/spectator), and
    for anvils specifically has a chance (`0.05 + fallDistanceInt*0.05`) to advance the
    anvil's damage stage or set `cancelDrop` if it would fully break.
  - `hurtEntities`/`fallDamagePerDistance`/`fallDamageMax` are set via `setHurtsEntities`,
    called by block placement code (anvils call this with specific values) — this needs a
    Rust equivalent hook, likely from wherever `FallingEntity::replace_spawn` is invoked
    for anvils today (grep for anvil block logic).

### Current Rust state

`pumpkin/src/entity/falling.rs`. The whole `EntityBase::tick` is ~45 lines vs. vanilla's
~100: it applies gravity, moves, and on `on_ground` **unconditionally** calls
`world.set_block_state(&landing_pos, final_state_id, ...)` via
`crate::block::blocks::falling::on_land_state` (handles only the concrete-powder
water-solidify case) then immediately `entity.remove()`. There is no time counter, no
despawn timeout, no `mayReplace`/`wouldSurvive` check, no `cancelDrop`, no `blockData`.
`damage()` is hardcoded `Box::pin(async move { false })`. `write_nbt`/`read_nbt_non_mut`
only round-trip `BlockState`; none of `Time`, `DropItem`, `HurtEntities`,
`FallHurtAmount`, `FallHurtMax`, `TileEntityData`, `CancelDrop` are read or written.

### Target design

Expand `FallingEntity`'s fields to match vanilla's persisted state:

```rust
pub struct FallingEntity {
    entity: Entity,
    block_state_id: AtomicCell<BlockStateId>,
    time: AtomicI32,
    drop_item: AtomicBool,
    cancel_drop: AtomicBool,
    hurt_entities: AtomicBool,
    fall_damage_per_distance: AtomicCell<f32>,
    fall_damage_max: AtomicI32,
    block_data: Mutex<Option<NbtCompound>>,
}
```

`tick()` restructure to mirror vanilla's branches:

1. increment `time`; apply gravity/move (existing).
2. compute `is_concrete`, `is_stuck_in_water` (needs a fluid-state check on landing pos;
   port `ConcretePowderBlock` water-stuck logic — likely partially present in
   `block::blocks::falling::on_land_state` already, reuse it).
3. if `!on_ground && !is_stuck_in_water`: check the 100/600 despawn window; on timeout,
   drop item (respecting `ENTITY_DROPS` gamerule + `drop_item` flag) and `entity.remove()`.
4. else (landed or stuck): halve/negate velocity as vanilla does
   (`multiply(0.7, -0.5, 0.7)`), then gate on `!cancel_drop`:
   - compute `may_replace` (reuse/extend whatever block-replaceability check
     `block::registry` already exposes for placement context) and `would_survive`
     (needs a `can_survive`-equivalent per-block hook — likely already exists for gravity
     blocks' scheduled tick logic; reuse it rather than duplicating).
   - if both true: `set_block_state`, call the block's `on_land`/`Fallable`-equivalent
     hook (only concrete powder currently implements this — expand `falling::on_land_state`
     or add an explicit call), then if `block_data` is `Some` and the new state
     `has_block_entity()`, merge it into the fresh block entity's NBT and mark it dirty.
   - else if `drop_item && ENTITY_DROPS`: `callOnBrokenAfterFall` equivalent (currently
     absent — likely a no-op hook for most blocks, meaningful for anvils/concrete) then
     spawn a dropped item entity for the block.
   - `cancel_drop` path: just discard, call the broken-after-fall hook, no item.

`damage()` (the `EntityBase::damage` override, corresponds to vanilla's
`causeFallDamage`, NOT `hurtServer` which is unrelated and stays `false`/no-op per
vanilla's `isAttackable() == false`): implement the real fall-damage computation when
`hurt_entities` is set, including the anvil-damage-stage chance. This requires plumbing
through however `causeFallDamage` is invoked in the Rust movement/fall-distance system
(grep `causeFallDamage`/`cause_fall_damage` equivalents in `Entity`'s movement code) —
today `FallingEntity::damage` is a distinct, unrelated no-op for the `hurtServer` path;
the real hook point is wherever the generic entity movement code calls into fall damage
after `on_ground` transitions. This is the largest unknown — needs an Explore pass on how
`Entity` currently computes/dispatches fall damage generically before adding an
override point for `FallingEntity`.

`write_nbt`/`read_nbt_non_mut`: add the six missing keys (`Time`, `DropItem`,
`HurtEntities`, `FallHurtAmount`, `FallHurtMax`, `TileEntityData` as raw NBT compound,
`CancelDrop`), matching vanilla defaults (`DropItem` defaults `true`, `HurtEntities`
defaults to `blockState.is(BlockTags.ANVIL)` — i.e., anvils hurt entities by default even
without an explicit `setHurtsEntities` call).

Add a `FallingEntity::spawn_with_hurt(...)` or a `set_hurts_entities(damage_per_distance,
damage_max)` builder method for anvil placement code to call, matching vanilla's public
`setHurtsEntities`.

### Files to modify

- `pumpkin/src/entity/falling.rs` — the bulk of the work described above.
- `pumpkin/src/block/blocks/falling.rs` (or wherever `on_land_state` lives) — extend to
  cover `Fallable.onBrokenAfterFall`/anvil damage-stage logic, or add a parallel hook.
- Anvil block file (grep `AnvilBlock`/`anvil.rs` under `pumpkin/src/block/blocks/`) — call
  `set_hurts_entities` when spawning a `FallingEntity` for an anvil, matching vanilla's
  `FallingBlockEntity.fall` callers.
- Wherever generic fall-damage dispatch lives in `Entity`/movement code — needs a
  targeted read before implementation; not located in this pass.

### Needs verification

- Exact `ConcretePowderBlock`/`Fallable.onLand` semantics for non-concrete blocks (most
  blocks have no special `onLand` — verify against `Fallable.java` which blocks
  implement it; likely only concrete powder + red sand/anvil variants).
- Whether Pumpkin's existing fall-distance/`causeFallDamage` dispatch is per-entity
  virtual-call based (matching vanilla) or a flat player-only implementation — this
  changes how invasive wiring `FallingEntity::damage` correctly will be.

---

## 3. FireworkRocketEntity: acceleration branch, sound/particles, raycast, onHitBlock

### Vanilla source

`net/minecraft/world/entity/projectile/FireworkRocketEntity.java`:

- **Acceleration branch** (lines 150-153): only applied when NOT attached to an entity
  (elytra-boost case handled separately at 115-147) and NOT `isShotAtAngle()`:
  `horizontalAcceleration = horizontalCollision ? 1.0 : 1.15`, multiply x/z by it, add
  `0.04` to y. `isShotAtAngle()` (line 262-264) reads a synced `DATA_SHOT_AT_ANGLE`
  boolean, set true only by the dispenser-fired constructor (line 82-85, 87-92) — a
  dispenser-launched rocket at a non-default angle does NOT get the 1.15 boost, only
  hand-thrown or entity-attached ones do.
- **Launch sound + particles** (lines 168-184): `life == 0` triggers
  `FIREWORK_ROCKET_LAUNCH` sound once; every tick while alive on the client side, spawn a
  `FIREWORK` particle at current position with slight randomized velocity
  (`random.nextGaussian()*0.05`, `-deltaMovement.y*0.5`, `random.nextGaussian()*0.05`).
  Currently the Rust particle spawn is entirely absent, and there's no explicit launch
  sound — need to check if a sound is emitted anywhere else in the item-use path
  (`item/items/firework_rocket.rs`, not yet read) before assuming it's 100% missing;
  regardless, the entity-side `life==0` sound emission described here is what's missing
  from `firework_rocket.rs` (entity file).
- **Two-step raycast for explosion damage** (lines 240-247): for each candidate living
  entity within 5 blocks, vanilla tests line-of-sight from the rocket position to BOTH
  `target.getX(), target.getY(0), target.getZ()` (feet) and `target.getY(0.5)` (mid-body,
  `testStep=0,1` — the `getY(fraction)` call interpolates between feet and eye height by
  `fraction`, so `testStep*0.5` gives 0.0 then 0.5, not feet/eye exactly but feet/mid).
  Rust's `firework_rocket.rs::explode_and_remove` does a single raycast to
  `target_entity.get_eye_pos()`. This under-counts hits: a target whose feet/mid line is
  blocked but eye line is clear (or vice versa) is scored differently than vanilla.
- **onHitBlock** (lines 207-215): calls `state.entityInside(...)` (block-specific
  "entity walked/collided into me" hook — relevant e.g. for slime blocks, dirt paths from
  impact) before the explosion check; Rust's `on_hit` in `firework_rocket.rs` goes
  straight to `has_explosion(&self.item_stack)` with no `entityInside` equivalent call.
- **isAttackable() == false** (line 306-308): rockets can't be attacked/targeted as
  entities to hit; not modeled in Rust (`EntityBase` doesn't currently expose an
  `is_attackable` hook on this type at all — check whether the trait has a default and
  whether firework needs an explicit override, or whether this is enforced elsewhere).

### Current Rust state

`pumpkin/src/entity/projectile/firework_rocket.rs`. The `tick()` `EntityBase` impl
(lines 208-253) always does `velocity.x *= 1.15; velocity.z *= 1.15; velocity.y += 0.04;`
in the `else` (non-elytra-attached) branch, with no `horizontal_collision` or
`isShotAtAngle` check at all — meaning dispenser-fired-at-angle rockets currently get the
same over-acceleration as hand-thrown ones, and horizontal-collision suppression never
applies. No sound emission, no particle spawn. `on_hit` (line 263) calls
`explode_and_remove` directly for entity hits and gates block hits on `has_explosion`,
matching vanilla's *explosion* logic, but skips `entityInside`. `explode_and_remove`'s
raycast (line 144-155) uses a single `world.raycast(rocket_pos, target_entity.get_eye_pos(), ...)`.

### Target design

1. Add a `shot_at_angle: bool` field (or reuse an existing entity metadata slot if one
   exists for `DATA_SHOT_AT_ANGLE`) plumbed from whichever construction path represents
   dispenser/`shootAtAngle` firing (today `FireworkRocketEntity::new_shot`/`new_shot_with_item`
   don't distinguish this — a dispenser-fired-at-non-default-angle constructor needs to
   set it; check `item/items/firework_rocket.rs` and the wind-charge-style dispenser
   wiring design in section 4 for where this constructor would be called from once
   dispenser support exists).
2. Gate the acceleration block:
   ```rust
   if !self.attached_to_entity() && !self.shot_at_angle {
       let horizontal_accel = if entity.horizontal_collision.load(Ordering::Relaxed) {
           1.0
       } else {
           1.15
       };
       velocity.x *= horizontal_accel;
       velocity.z *= horizontal_accel;
       velocity.y += 0.04;
       entity.set_velocity(velocity);
   }
   ```
   (Requires `Entity` to expose a `horizontal_collision` flag — check if the general
   movement code already tracks this; likely yes since collision detection is shared
   infrastructure.)
3. On `life == 0` (first tick), play `Sound::EntityFireworkRocketLaunch` via
   `world.play_sound` matching the other entity sound-emission call sites in this file's
   sibling projectiles (e.g. `wind_charge.rs`'s pattern, or `tnt.rs`).
4. Each tick, spawn a `Particle::Firework` via `world.spawn_particle` with the
   gaussian-randomized velocity vanilla uses (`rand::random::<f64>()` gaussian helper —
   confirm the Rust random utilities expose a gaussian; if not, `next_triangular`-style
   substitution needs wiki/vanilla-source confirmation that MC actually uses a true
   gaussian here, not triangular, before implementing — `RandomSource.nextGaussian()` is
   a real gaussian in vanilla, distinct from `triangle()`).
5. Two-step raycast: replace the single `get_eye_pos()` raycast with a loop over
   `[0.0, 0.5]` fractional heights computing `target.get_y_at_fraction(step)`-equivalent
   (needs an `Entity::get_pos_at_fraction`/`lerp_eye` helper — check if one exists;
   otherwise compute `feet.y + (eye.y - feet.y) * step` inline), breaking on first
   unobstructed test, matching vanilla's `for testStep in 0..2`.
6. `on_hit` for block case: call the landed block's `entity_inside`-equivalent hook
   (`BlockBehaviour`'s entity-collision callback, if one exists in the `BlockBehaviour`
   trait — grep for `entity_inside`/`on_entity_collision` in `pumpkin/src/block/mod.rs`)
   before the explosion gate.
7. Add `is_attackable() -> false` to the `EntityBase` impl if the trait defines that hook
   (grep `fn is_attackable` on `EntityBase`; if absent, this is a no-op today and should
   be filed as a smaller separate trait-level gap, not blocking this fix).

### Files to modify

- `pumpkin/src/entity/projectile/firework_rocket.rs` — all six items above.
- Possibly `pumpkin/src/entity/mod.rs` (`Entity` struct) if `horizontal_collision` isn't
  already tracked — verify before assuming new state is needed.
- `pumpkin/src/item/items/firework_rocket.rs` (not read in this pass) — check for
  interaction with the `shot_at_angle` flag if item-triggered launches need to set it.

### Needs verification

- Whether `RandomSource.nextGaussian()` vs. Pumpkin's RNG utilities need a new primitive;
  cite `RandomSource.java`'s gaussian implementation if adding one.
- Confirm `BlockBehaviour`'s entity-collision hook name/signature before wiring
  `onHitBlock`'s `entityInside` call — this is referenced elsewhere in the codebase
  (other projectiles calling into block collision effects) and should be reused, not
  reinvented.

---

## 4. WindChargeItem dispenser wiring

### Vanilla source

`net/minecraft/world/item/WindChargeItem.java`:

- `asProjectile(Level, Position, ItemStack, Direction)` (lines 56-66): builds a
  `WindCharge` at the given position with a direction vector computed via
  `random.triangle(direction.getStepX()/Y/Z, 0.11485)` per axis (dispenser-specific
  spread, distinct from the hand-thrown `use()` path's `spawnProjectileFromRotation`).
- `createDispenseConfig()` (lines 72-80): declares dispenser launch parameters —
  `positionFunction = DispenserBlock.getDispensePosition(source, 1.0, Vec3.ZERO)` (1 full
  block offset, no extra vector), `uncertainty = 6.6666665F`, `power = 1.0F`,
  `overrideDispenseEvent = 1051` (a specific level-event id the dispenser fires instead
  of the generic "dispense" event — this drives client-side sound/particle selection).
- `ProjectileItem.java` (interface, lines 12-67): defines the
  `asProjectile`/`createDispenseConfig`/`shoot` contract every dispensable projectile item
  implements; vanilla's generic `DispenserBlock` dispense logic (not shown here, in
  `DispenserBlock.java`, not read this pass) does: look up the item's `ProjectileItem`
  interface if the item implements it, call `createDispenseConfig()` for
  position/uncertainty/power, build the projectile via `asProjectile`, call `shoot()` to
  apply velocity, then fire either the config's `overrideDispenseEvent` or the default
  dispense level-event.

### Current Rust state

`pumpkin/src/block/blocks/redstone/dispenser.rs`'s `on_scheduled_tick` (lines 186-242) is
a flat `if/else if` chain dispatching by item id: arrows, boats, armor stands, TNT, spawn
eggs, else generic item drop. **Wind charges are not checked anywhere** — a wind charge
item in a dispenser falls through to the final `else` branch and is dropped as a plain
item entity, never spawning a `WindChargeEntity` projectile. `wind_charge.rs` (the entity)
exists and is fully functional as a thrown/hand-used projectile
(`pumpkin/src/entity/projectile/wind_charge.rs`) — the gap is purely in the dispenser's
item-dispatch table, there's no `ProjectileItem`-style trait in Rust at all; each
dispensable item type is hardcoded inline in `dispenser.rs` (`fire_arrow`,
`dispense_boat`, `dispense_armor_stand`, `dispense_tnt`, `dispense_spawn_egg`).

### Target design

Given the codebase's existing pattern (one hardcoded `dispense_*` async fn per item
family, not a trait-object dispatch), the pragmatic fix matching house style is to add
one more branch + one more fn, not to introduce a `ProjectileItem` trait abstraction
(that would be a larger refactor better suited to a follow-up once more projectile items
need dispensing — see the broader-gap note below).

```rust
} else if item.item.id == Item::WIND_CHARGE.id {
    Self::dispense_wind_charge(&ctx, &mut item).await;
}
```

```rust
async fn dispense_wind_charge(ctx: &DispenseContext<'_>, item: &mut ItemStack) {
    let _ = item.split(1);
    let facing = to_normal(ctx.facing);
    // Vanilla: DispenserBlock.getDispensePosition(source, 1.0, Vec3.ZERO) — a full
    // block offset along facing, no extra vector.
    let spawn_pos = ctx.position.to_centered_f64().add(&(facing * 1.0));

    let entity = Entity::new(ctx.world.clone(), spawn_pos, &EntityType::WIND_CHARGE);
    // Vanilla's asProjectile spreads direction via random.triangle(component, 0.11485)
    // per axis before constructing the WindCharge; ThrownItemEntity::set_velocity's
    // uncertainty parameter already performs an equivalent per-axis triangular spread,
    // so pass 0.11485-derived uncertainty (converted to the 0.0172275-scaled parameter
    // set_velocity expects) rather than reimplementing WindChargeItem's own spread math.
    let thrown = ThrownItemEntity { entity, owner_id: None, collides_with_projectiles: false,
        has_hit: AtomicBool::new(false), gravity: WIND_CHARGE_GRAVITY };
    let wind_charge = Arc::new(WindChargeEntity::new_normal(thrown));
    wind_charge.set_velocity(facing.x, facing.y, facing.z, /* power */ 1.0, /* uncertainty */ 6.6666665);
    ctx.world.spawn_entity(wind_charge).await;

    // Vanilla overrideDispenseEvent = 1051, a distinct level-event id from the generic
    // dispense sound/smoke (WorldEvent::SoundDispenserDispense) fired by other branches.
    ctx.world.sync_world_event(WorldEvent::SoundDispenserLaunchWindCharge /* or numeric 1051 equivalent */, *ctx.position, 0);
}
```

The exact uncertainty-parameter conversion needs verification: vanilla's `asProjectile`
spread (`0.11485` triangle deviation applied directly to a unit direction component) is
architecturally different from `ThrownItemEntity::set_velocity`'s
`uncertainty * 0.0172275` scaling applied post-normalization — these are not obviously
equivalent and should be checked against `Projectile.spawnProjectileUsingShoot`'s formula
(not read this pass) before assuming the existing `set_velocity` uncertainty path
reproduces vanilla's dispenser spread correctly. This is the single largest unknown in
this subsystem.

Also confirm what numeric level-event id 1051 maps to in Pumpkin's `WorldEvent` enum (may
already exist under a different name, e.g. a `*_LAUNCH_WIND_CHARGE` variant — grep
`WorldEvent` definitions) rather than assuming `SoundDispenserDispense` is an adequate
substitute; using the wrong event changes client-side sound/particle for this one case.

### Broader related gap (noted, not designed here)

`ProjectileItem`'s other overrides — snowballs, eggs, ender pearls, splash/lingering
potions, experience bottles, fireworks — have the same "no dispenser wiring" gap as wind
charges, each needing its own `dispense_*` branch in `dispenser.rs` following the same
pattern shown above, with per-item `createDispenseConfig()` values
(`ThrownEggItem`/`ThrownPotionItem`/etc. in vanilla, not read this pass). Recommend a
follow-up census pass reading each of those `Item.java`/`*Item.java` subclasses to
enumerate exact per-item `positionFunction`/`uncertainty`/`power`/`overrideDispenseEvent`
values before batching them into one PR — they are likely NOT all identical to wind
charge's values.

### Files to modify

- `pumpkin/src/block/blocks/redstone/dispenser.rs` — add the `WIND_CHARGE` branch and
  `dispense_wind_charge` fn.

### Needs verification

- The `0.11485` vs. `0.0172275`-scaled uncertainty formula equivalence (or lack thereof)
  — cite `Projectile.java`'s `spawnProjectileUsingShoot`/`shootFromRotation` directly.
- Pumpkin's `WorldEvent` enum's mapping for numeric level-event 1051 (wind charge
  dispense) — grep `pumpkin-data`'s `world_event`/`WorldEvent` generated table.

---

## 5. MinecartItem: game event, collision guard, item-stack component transfer

### Vanilla source

`net/minecraft/world/item/MinecartItem.java:27-68` (`useOn`):

- Builds `spawnPos` at `pos + (0.5, 0.0625 [+0.5 if sloped], 0.5)`.
- `AbstractMinecart.createMinecart(level, x, y, z, type, DISPENSER, itemStack, player)`
  (lines 123-145 of `AbstractMinecart.java`): spawns the entity, then
  `EntityType.createDefaultStackConfig(level, itemStack, player).apply(entity)` — this is
  vanilla's generic "apply item-stack data components onto a freshly spawned entity"
  path (custom name from `DataComponents.CUSTOM_NAME`, and for minecarts specifically the
  contained-block/NBT via whatever component a chest/hopper/TNT/spawner minecart stores
  its "block it's carrying" state in — likely `DataComponents.BLOCK_ENTITY_DATA` or a
  minecart-specific component; not enumerated in this file, needs a read of
  `EntityType.createDefaultStackConfig` itself, not fetched this pass).
- **useExperimentalMovement guard** (lines 53-59): if the `MINECART_IMPROVEMENTS`
  feature flag is on, before actually placing the cart in the world, scan
  `level.getEntities(null, cart.getBoundingBox())` for any existing `AbstractMinecart` and
  fail placement (`InteractionResult.FAIL`) if one is found — prevents stacking two
  minecarts on the same rail tile under the new movement model.
- `serverLevel.gameEvent(GameEvent.ENTITY_PLACE, pos, Context.of(player, blockState
  below))` (line 63) — fires after `addFreshEntity`, using the block state one below the
  clicked position as context.
- `itemStack.shrink(1)` at the end (matches Rust's existing decrement).

### Current Rust state

`pumpkin/src/item/items/minecart.rs`'s `use_on_block` (lines 55-94): checks the
`MINECRAFT_RAILS` tag, computes ascending height offset (matches vanilla's slope check),
spawns a bare `Entity` + wraps it in `MinecartEntity::new`, spawns it, decrements the
stack. **Missing**: no game-event emission, no multi-minecart collision guard (any
number of minecarts can be placed on one tile), and no item-stack-component transfer —
`item.item` (the `Item`, not the `ItemStack`) is only consulted for `item_to_entity`
mapping; any custom name or `BLOCK_ENTITY_DATA`-style contained-block/loot NBT on the
placed `ItemStack` (e.g. a named minecart, or a chest minecart pre-loaded with loot from
a structure) is silently discarded.

### Target design

1. **Game event**: after `world.spawn_entity(minecart_entity).await`, emit
   `GameEvent::EntityPlace` at `location` using the existing `game_event::emit_game_event`
   helper (already used by wind charge / firework code — reuse the same pattern seen in
   `entity/projectile/mod.rs`'s `emit_projectile_land`), with context built from the
   player and the block state at `location.down()`.
   ```rust
   let context_state = world.get_block_state(&location.down());
   crate::world::game_event::emit_game_event(
       &world, GameEvent::EntityPlace, pos,
       GameEventContext::of_entity_and_state(player.get_entity_arc(), context_state), // exact ctor TBD
   ).await;
   ```
   Confirm `GameEventContext`'s exact constructor for "entity + block state" pairs (the
   `ProjectileLand` usage only shows `of_entity`; a state-carrying variant may need
   adding, or may already exist for other block-context game events — grep
   `GameEventContext` variants before assuming one needs to be added).
2. **Collision guard**: before `world.spawn_entity`, if the equivalent of
   `useExperimentalMovement` is true (check whether Pumpkin models `MINECART_IMPROVEMENTS`
   as a feature flag at all — if it doesn't yet model per-world feature flags, this half
   of the fix may need to be scoped down to "always apply the guard" or explicitly
   deferred pending feature-flag support, and noted as such in the PR), query
   `world.get_entities_at_box(&cart_bounding_box)` for any entity whose type is one of the
   minecart variants (reuse the `is_other_minecart` id-list already written out in
   `minecart.rs`'s entity `push()` method — lines ~452-458 of that file — factor it into
   a shared helper rather than duplicating the six-way `||` chain a third time) and bail
   out of `use_on_block` without decrementing the stack if found.
3. **Item-stack component transfer**: on spawn, before `spawn_entity`, apply:
   - custom name: if `item.get_data_component::<CustomNameImpl>()` (or Pumpkin's
     equivalent accessor) is present, set it on the new entity via whatever the general
     "named entity" mechanism is (grep how spawn eggs' `apply_entity_variant` or armor
     stand naming does this, since `dispenser.rs`'s spawn-egg path already demonstrates
     applying stack data onto a freshly built entity — `apply_entity_variant` is the
     closest existing precedent to extend/mirror).
   - contained block/NBT (chest minecart's items, spawner minecart's spawner NBT, TNT
     minecart fuse state if any): needs to identify which `DataComponent` vanilla actually
     uses for minecart cargo (likely `DataComponents.CONTAINER` for chest minecarts via
     `ItemContainerContents`, and `DataComponents.BLOCK_ENTITY_DATA` for command-block/
     spawner minecarts) — this overlaps directly with the BLOCK_ENTITY_DATA transfer
     design in section 6; the two fixes should share a helper (e.g. a
     `apply_block_entity_data_component(item, &dyn BlockEntity)`-style fn) rather than
     being implemented twice.

### Files to modify

- `pumpkin/src/item/items/minecart.rs` — game event emission, collision guard, and
  component-transfer call sites.
- `pumpkin/src/entity/vehicle/minecart.rs` — factor the six-way `is_other_minecart`
  check (already present in `push()`) into a `pub fn is_minecart_entity_type(&EntityType) -> bool`
  helper reused by both files.
- Possibly `pumpkin-world`/`pumpkin-data` if `MINECART_IMPROVEMENTS` feature-flag modeling
  doesn't exist yet — flag as a prerequisite, don't invent ad hoc flag plumbing inline.

### Needs verification

- Whether Pumpkin models per-world enabled feature flags (`FeatureFlags` in vanilla) at
  all; if not, the collision guard's conditionality needs a product decision (always-on
  vs. deferred) rather than a silent behavior choice.
- `EntityType.createDefaultStackConfig`'s exact component list — not read this pass, must
  be fetched from vanilla source before implementing to avoid missing a transferred
  component (name is the obvious one; verify there isn't also a "spawn with age/variant"
  concept reused from mob spawn eggs that applies to command-block minecarts too).

---

## 6. BlockItem BLOCK_ENTITY_DATA transfer

### Vanilla source

`net/minecraft/world/item/BlockItem.java`:

- `place()` (lines 48-91): after `placeBlock` succeeds and the placed state matches, in
  order: `updateBlockStateFromTag` (blockstate-property overrides, a separate, apparently
  already-handled concern), then `updateCustomBlockEntityTag(pos, level, player, itemStack,
  placedState)` (line 78), then `updateBlockEntityComponents(level, pos, itemStack)`
  (line 79).
- `updateBlockEntityComponents` (lines 101-107): fetches the freshly-placed block entity
  and calls `entity.applyComponentsFromItemStack(itemStack)` then `entity.setChanged()`.
  This is the generic per-block-entity-type "pull my relevant data components off the
  placed ItemStack" hook (e.g. a shulker box block entity reads `DataComponents.CONTAINER`
  for its contents, a sign reads text components, etc.) — NOT specific to
  `BLOCK_ENTITY_DATA`.
  `applyComponentsFromItemStack` isn't shown in this file (it's `BlockEntity`'s own
  method); the concrete `BLOCK_ENTITY_DATA` transfer instead happens via:
- `updateCustomBlockEntityTag(Level, Player, BlockPos, ItemStack)` (static, lines
  148-171): the one that actually matters for the named gap. Reads
  `itemStack.get(DataComponents.BLOCK_ENTITY_DATA)` as a `TypedEntityData<BlockEntityType<?>>`.
  If present: looks up the placed block entity, checks its `BlockEntityType` matches the
  component's stored type (mismatch → no-op, `false`), then — unless the type is
  `onlyOpCanSetNbt()` and the placing player isn't a game-master — calls
  `customData.loadInto(blockEntity, registryAccess)`, which merges the stored NBT
  compound into the block entity (this is how written books' pages, shulker box
  contents-as-raw-NBT from `/give ... {BlockEntityTag:{...}}`, spawner custom
  configuration, etc. survive a place-from-inventory round trip).
- `BlockItem.setBlockEntityData` (lines 206-214) is the mirror write-side helper (used
  when breaking a block entity back into an item), not directly relevant to placement.

### Current Rust state

`pumpkin/src/block/registry.rs`'s `place_block` (around line 500 onward) and `on_placed`
(around line 995-1013): `on_placed` creates the block entity via
`create_block_entity(state.block_entity_type, position)` whenever the placed state
declares one and none exists yet, then adds it to the world. There is **no step
anywhere** that reads a `BLOCK_ENTITY_DATA`-equivalent component off the `ItemStack` being
placed and merges it into that freshly-created block entity. `pumpkin-data` already has
the component modeled: `pumpkin-data/src/data_component_impl/block_entity.rs`'s
`BlockEntityDataImpl { pub nbt: NbtCompound }` (registered as `DataComponent::BlockEntityData`,
id 60) — the data is decodable, it's just never consulted during placement. The
`BlockEntity` trait (`pumpkin/src/block/entities/mod.rs:70+`) has `write_nbt`/`from_nbt`
but no generic "merge extra NBT in" method — `from_nbt` is a full constructor, not an
in-place merge.

### Target design

Add a merge step to `place_block` (or `on_placed`, whichever runs after the block entity
is actually created — `on_placed` is the right spot since it's guaranteed to run after
`create_block_entity`) that mirrors `updateCustomBlockEntityTag`:

```rust
if let Some(entity_data) = item_stack.get_data_component::<BlockEntityDataImpl>()
    && let Some(block_entity) = world.get_block_entity(position)
{
    // Vanilla checks the component's stored BlockEntityType against the placed
    // entity's actual type and no-ops on mismatch; Rust's BlockEntity trait doesn't
    // expose a machine-checkable "type id" today (only `resource_location()` as a
    // string) — compare via resource_location string equality as the practical
    // substitute, or add a typed id if BlockEntity gains one for other reasons.
    if block_entity.resource_location() == entity_data.stored_type_name() {
        // Needs a new BlockEntity trait method: merge external NBT into self,
        // analogous to vanilla's `loadInto`. `from_nbt` alone can't do this since
        // it's a constructor, not a mutator, and BlockEntity as currently defined
        // has no generic mutable-merge entry point.
        block_entity.merge_nbt(&entity_data.nbt).await;
        block_entity.mark_dirty(); // vanilla's setChanged()
    }
}
```

This requires extending the `BlockEntity` trait with a new method, e.g.:

```rust
fn merge_nbt<'a>(&'a self, extra: &'a NbtCompound) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
    Box::pin(async {}) // default no-op for block entities with no interior-mutable state
}
```

with a real implementation per concrete block entity that needs it (chest/shulker-box
contents, sign text, spawner config) — this is itself a moderate amount of per-block-type
work; scope the first PR to wiring the trait method + the placement-time call site with a
default no-op, and file per-block-entity-type merge implementations as follow-ups, OR
scope narrowly to the highest-value cases named in the prompt (shulker box / chest
CONTAINER contents, written/filled books, spawner NBT) as the initial PR if a full
`merge_nbt` per block entity is too large for one change.

Also note vanilla's `onlyOpCanSetNbt()` permission gate (spawner/command-block-adjacent
block entities require op/game-master permission to set arbitrary NBT this way) — needs
a corresponding permission check using Pumpkin's existing permission system (see
`Permissions::COMMANDS_GAMEMASTER` usage pattern elsewhere, e.g.
`BlockItem.shouldPrintOpWarning` at line 173-183) before merging for those block entity
types, or player-supplied NBT could grant unintended capabilities (e.g. writing a custom
command into a placed command block via inventory NBT).

### Files to modify

- `pumpkin/src/block/entities/mod.rs` — add `merge_nbt` (or equivalently named) default
  method to the `BlockEntity` trait.
- `pumpkin/src/block/registry.rs` — add the merge call in `on_placed` (or `place_block`,
  whichever definitively runs after block-entity creation and has both `item_stack` and
  `position` in scope — `on_placed`'s current signature takes `block`/`state_id`/
  `position`/`old_state_id`/`notify` but not the placing `ItemStack`; the call site
  likely needs to move up to wherever `place_block` calls `on_placed`, passing the
  item stack through, or the merge step needs to live directly in `place_block` after its
  own `on_placed` call instead).
- Individual block entity files (`pumpkin/src/block/entities/chest.rs`,
  `shulker_box.rs`, etc. — exact filenames not enumerated this pass) for concrete
  `merge_nbt` implementations, scoped per the note above.

### Needs verification

- Whether `ItemStack`'s Rust API already exposes a `get_data_component::<T>()` generic
  accessor matching the pattern used elsewhere in this doc (section 3's
  `FireworksImpl` usage confirms yes) — should be a direct reuse.
- Exact list of block entity types vanilla actually round-trips via
  `BLOCK_ENTITY_DATA` in practice vs. their own dedicated components (`CONTAINER` for
  simple inventories is a *separate* component from `BLOCK_ENTITY_DATA` per
  `updateBlockEntityComponents` vs. `updateCustomBlockEntityTag` being two distinct calls
  in `place()`) — the prompt's framing ("BLOCK_ENTITY_DATA component... shulker box or
  chest with CONTAINER contents") conflates both paths; confirm against
  `BlockEntity.applyComponentsFromItemStack`'s vanilla implementation (not fetched this
  pass) whether `CONTAINER` needs its own separate transfer step distinct from the
  `BLOCK_ENTITY_DATA` one designed above, since vanilla clearly calls both
  `updateCustomBlockEntityTag` (NBT-based) and `updateBlockEntityComponents`
  (component-based, includes `CONTAINER`) as two separate mechanisms.

---

## 7. Mannequin and Interaction entities (fully unimplemented)

### Vanilla source

**Mannequin** (`net/minecraft/world/entity/decoration/Mannequin.java`, extends `Avatar`,
which itself extends `LivingEntity` — not read this pass, assume player-shaped humanoid
base): synced data `DATA_PROFILE` (`ResolvableProfile`), `DATA_IMMOVABLE` (bool),
`DATA_DESCRIPTION` (optional text component), plus inherited `DATA_PLAYER_MODE_CUSTOMISATION`
(layer-visibility byte mask, set to `ALL_LAYERS` in the constructor). Key overrides:
- `getProfile()`/`setProfile()` (lines 80-87): backing store for skin/name identity.
- `isImmobile()` (line 115-118): `getImmovable() || super.isImmobile()`.
- `isEffectiveAi()` (line 120-123): `!getImmovable() && super.isEffectiveAi()` — an
  immovable mannequin has no AI at all (matches the "isEffectiveAi=false" requirement
  from the prompt, conditionally on the immovable flag rather than unconditionally).
- `addAdditionalSaveData`/`readAdditionalSaveData` (lines 126-153): persist `profile`,
  `hidden_layers` (via a bitmask↔list codec), `main_hand`, `pose` (validated against a
  restricted `VALID_POSES` set: STANDING/CROUCHING/SWIMMING/FALL_FLYING/SLEEPING only),
  `immovable`, and `description`/`hide_description` (description defaults to a
  translatable component, tracked separately from the synced-data optional so it can be
  hidden/shown without losing the underlying text).
- `aiStep()` (lines 176-180): `super.aiStep()` then `updateSwingTime()` — trivial, no
  special movement AI (a mannequin doesn't wander).
- `get(DataComponentType)`/`applyImplicitComponent(s)` (lines 155-174): exposes/accepts
  `DataComponents.PROFILE` as an item-component-style interface, presumably for
  spawn-egg-equivalent or `/summon`-with-NBT-components construction — likely a smaller
  concern for a first Rust pass; can be deferred if Pumpkin's entity component API
  doesn't have an equivalent mechanism yet.

**Interaction** (`net/minecraft/world/entity/Interaction.java`, extends `Entity` directly,
implements `Attackable, Targeting`): synced data `DATA_WIDTH_ID`/`DATA_HEIGHT_ID` (floats,
default 1.0/1.0), `DATA_RESPONSE_ID` (bool). Two plain (non-synced) fields:
`attack`/`interaction`, each an `Optional<PlayerAction(UUID, long timestamp)>` persisted
as NBT records.
- `defineSynchedData`/save-data (lines 45-69): straightforward.
- `onSyncedDataUpdated` (lines 71-77): width/height changes trigger `refreshDimensions()`
  — the bounding box is dynamically resizable per-entity (not a fixed `EntityType`
  dimension), which is why `getDimensions()`/`makeBoundingBox()` (lines 166-178) compute
  from the synced floats rather than the entity type's static dimensions. This matches
  the prompt's `makeBoundingBox` note directly.
- `canBeHitByProjectile() == false`, `isPickable() == true`,
  `getPistonPushReaction() == IGNORE`, `isIgnoringBlockTriggers() == true` (lines 79-97):
  a cluster of "I am a pure hitbox, not physically real" overrides.
- `skipAttackInteraction(Entity source)` (lines 99-111): the entity's *hurt* path is
  hijacked — instead of taking damage, a player attacking it records a `PlayerAction`
  (attacker UUID + game time) and, unless `getResponse()` is true, the attack is
  swallowed (`hurtServer` always returns `false`, line 113-116) — this entity never
  actually loses health; it exists purely to relay attack/interact events (e.g. to a
  datapack-driven scoreboard/function trigger) while presenting a normal hit-test target.
- `interact(Player, InteractionHand, Vec3)` (lines 118-126): symmetric to attack — records
  an `interaction` PlayerAction on right-click, returns `CONSUME`.
- `tick()` is empty (line 129-130) — genuinely a no-op every tick, matching the "invisible
  hit-detection entity" framing exactly.
- `getLastAttacker()`/`getTarget()` (lines 132-140): resolve the stored UUID back to a
  live `Player` via `level().getPlayerByUUID(...)`, returning `null` if that player isn't
  currently loaded — this is how `Attackable`/`Targeting` interfaces (used by
  advancement/function triggers elsewhere) query the interaction entity's state.

### Current Rust state

Neither entity exists at all — `pumpkin/src/entity/type.rs`'s `from_type()` match falls
through to the generic `_ => { ... }` default arm (grep confirmed no `MANNEQUIN` or
`INTERACTION` arms in the 200+ entity match list). No files under
`pumpkin/src/entity/decoration/` or elsewhere reference either type.

### Target design

**Interaction** (simpler, do first — no AI, no rendering-adjacent state, existing
`EntityBase` trait already supports every hook needed):

```rust
// pumpkin/src/entity/decoration/interaction.rs
pub struct InteractionEntity {
    entity: Entity,
    width: AtomicCell<f32>,   // default 1.0
    height: AtomicCell<f32>,  // default 1.0
    response: AtomicBool,     // default false
    attack: Mutex<Option<PlayerAction>>,
    interaction: Mutex<Option<PlayerAction>>,
}

struct PlayerAction { player: Uuid, timestamp_ticks: i64 }
```

- `EntityBase::tick`: no-op (matches vanilla's empty override) — override to a bare
  `Box::pin(async {})`, explicitly bypassing the generic `Entity::tick` physics/collision
  processing every other entity type gets, since vanilla's `Interaction.tick()` overrides
  `Entity.tick()` entirely rather than calling `super.tick()`.
- `can_hit()`: `true` (isPickable), but also needs whatever hook models
  `canBeHitByProjectile() == false` if `EntityBase` exposes one (check trait; if absent,
  file as a smaller separate gap rather than blocking this entity's core behavior).
- Hurt/attack interception: this is the trickiest mapping. Pumpkin's damage path is
  `EntityBase::damage`/`damage_with_context` (seen throughout this doc, e.g.
  `EnderDragonPart::damage`, `MinecartEntity::damage_with_context`). Vanilla splits
  "skip attack interaction" (records the attack, returns whether to *suppress* the normal
  hurt call) from `hurtServer` (unconditionally returns false). The Rust equivalent:
  override `damage`/`damage_with_context` to always return `false` (no health loss, no
  knockback — matches `hurtServer`), and separately hook wherever melee-attack dispatch
  happens (grep how player left-click-attack currently calls into
  `damage`/`damage_with_context` — likely `Player`'s attack-swing handling) to record a
  `PlayerAction` on this entity type specifically before the damage call, mirroring
  `skipAttackInteraction`'s side effect. This may require either (a) a new
  `EntityBase::skip_attack_interaction` hook with a default no-op that only
  `InteractionEntity` overrides, called from the shared attack-dispatch site, or (b) a
  downcast (`cast_any`) check in the attack-dispatch code specifically for
  `InteractionEntity` if adding a new trait method for one entity type feels
  disproportionate — prefer (a) for consistency with how `on_hit`/`on_damage` etc. are
  already modeled as trait hooks elsewhere in this codebase.
- `interact()`: Pumpkin's `EntityBase` trait already has an `interact` hook (seen in
  `MinecartEntity::interact`, section 5's read) — override it to record the
  `PlayerAction` into `self.interaction` and return an equivalent of `CONSUME` per that
  trait's return type.
- Dynamic bounding box: override whatever hook computes bounding box from entity
  dimensions (grep `get_dimensions`/`make_bounding_box`-equivalent on `EntityBase`/
  `Entity`) to read `width`/`height` fields instead of the static `EntityType` dimensions,
  and mirror vanilla's `onSyncedDataUpdated` refresh — this needs Pumpkin's metadata-sync
  callback (`init_data_tracker`/metadata update path, seen throughout this doc) to call a
  `refresh_dimensions()`-equivalent whenever width/height metadata is set post-spawn (via
  a command/function setting these, which is the actual vanilla use case — datapacks
  configure interaction entity hitboxes via `/data merge`).
- NBT: straightforward `write_nbt`/`read_nbt_non_mut` for `width`/`height`/`attack`/
  `interaction`/`response`, following the same `Mutex`-guarded-field pattern as section 1.

**Mannequin** is a much larger lift because it depends on whatever "humanoid/player-model
entity" base Pumpkin has (or doesn't have) — vanilla's `Avatar` base class (parent of both
`Mannequin` and presumably `Player`) isn't modeled in this pass. Recommend treating
Mannequin as a follow-up once Interaction establishes the pattern for a from-scratch
decoration-ish entity, and specifically flag to whoever picks this up:
- Check whether Pumpkin's `Player`/`LivingEntity` hierarchy has any shared
  "renders as a player skin, has arms/layers, has a game profile" abstraction that could
  become `Avatar`'s Rust analog, or whether Mannequin needs its own bespoke
  `ResolvableProfile`-holding struct with duplicated logic from `Player`.
- `pose` validation against the restricted `VALID_POSES` set is a good example of a
  vanilla behavior easy to silently drop if not explicitly ported.
- The `hidden_layers` bitmask↔list codec and `DATA_PLAYER_MODE_CUSTOMISATION` byte likely
  already have a Rust equivalent since players themselves expose the "which skin layers
  are visible" mechanic to clients — check `Player`'s existing metadata handling before
  assuming this needs new infrastructure.

### Files to modify

- `pumpkin/src/entity/decoration/interaction.rs` — new file, full `InteractionEntity`.
- `pumpkin/src/entity/decoration/mod.rs` — add `pub mod interaction;`.
- `pumpkin/src/entity/type.rs` — add `EntityType::INTERACTION` arm to `from_type`.
- `pumpkin/src/entity/mod.rs` — possibly add a `skip_attack_interaction` default-no-op
  hook to `EntityBase` if going with option (a) above, plus locate and modify the shared
  melee-attack dispatch call site to invoke it.
- Mannequin: deferred, no files identified as a safe starting point without first reading
  `Avatar`'s vanilla source and Pumpkin's `Player`/humanoid entity structure in depth.

### Needs verification

- Whether `EntityBase` already has hooks for `canBeHitByProjectile`, dimension
  recomputation on metadata change, and a "generic attack dispatch" call site — all three
  need an Explore pass into `pumpkin/src/entity/mod.rs`'s full `EntityBase` trait
  definition (only a partial section, around `NBTStorage`, was read this pass) and
  wherever player attack-swings resolve into `damage_with_context` calls, before
  finalizing the Interaction implementation plan above.
- `Avatar.java`'s full definition, and Pumpkin's closest existing analog, before scoping
  Mannequin at all.

---

## 8. WitherSkull projectile (entirely unimplemented)

### Vanilla source

`net/minecraft/world/entity/projectile/hurtingprojectile/WitherSkull.java`, extends
`AbstractHurtingProjectile` (a `Fireball`-family base with inherent inertia/acceleration
physics distinct from the arrow-family `Projectile` base — Pumpkin's `fireball.rs` and
`small_fireball.rs`, both built on the shared `ThrownItemEntity` helper with `gravity:
0.0`, are the closest existing Rust analog per the file layout, NOT the arrow/thrown-item
family used by snowballs/eggs/tridents).

- Synced data `DATA_DANGEROUS` (bool, default false) — set true for the "blue" wither
  skull the Wither boss fires periodically (as opposed to the default black skull),
  granting extra explosion-resistance-piercing power.
- `getInertia()` (lines 41-44): `isDangerous() ? 0.73F : super.getInertia()` — dangerous
  skulls decelerate less (fly straighter/faster) than the default `AbstractHurtingProjectile`
  inertia.
- `isOnFire() == false` (always, line 46-49) and `shouldBurn() == false` (line 115-118)
  — despite being a "hurting projectile" in the fireball family (which normally sets
  entities on fire / renders with fire), wither skulls explicitly opt out of both the
  visual fire flag and the burn-on-hit behavior; this directly satisfies the prompt's
  "isOnFire/shouldBurn" callout.
- `getBlockExplosionResistance` override (lines 51-56): when dangerous AND the target
  block passes `WitherBoss.canDestroy(block)` (a block-tag-based "can the wither break
  this" check, likely `BlockTags.WITHER_IMMUNE` inverse or similar — needs its own
  wiki/source check against `WitherBoss.canDestroy`, not read this pass), clamps
  explosion resistance to at most 0.8 — i.e., dangerous skulls can blast through
  slightly-tougher blocks than normal explosions would.
- `onHitEntity` (lines 58-91): the core combat logic —
  - if the skull has a living owner: damage source is `damageSources().witherSkull(this,
    livingOwner)`, damage amount **8.0**; on successful hit, if the target survived,
    apply post-attack enchantment effects; if the target died, **heal the owner for 5.0**
    (a wither-specific lifesteal-on-kill mechanic — easy to miss).
  - if no living owner (e.g. skull fired by a dispenser/command, not a Wither): damage
    source `damageSources().magic()`, damage amount **5.0** (lower, and typed as magic
    damage rather than wither-skull damage — different damage-type semantics matter for
    resistance/immunity checks downstream).
  - **Wither status effect application** (lines 78-89): if the hit dealt damage and the
    target is a `LivingEntity`, apply `MobEffects.WITHER` amplifier 1 for a duration that
    scales with world difficulty: **0 seconds on PEACEFUL/EASY** (no effect at all — the
    `witherSeconds` variable stays 0 and the `if (witherSeconds > 0)` guard skips
    entirely), **10 seconds on NORMAL**, **40 seconds on HARD** (both stored as
    `20 * witherSeconds` ticks). This difficulty-scaling behavior is exactly the
    "dangerous-flag damage bonus" language from the prompt is adjacent to, but note it's
    actually **difficulty-scaling**, not dangerous-flag-scaling — the `isDangerous` flag
    affects inertia and block-explosion-resistance, NOT the wither-effect duration, which
    is a distinct axis. Get this distinction right when implementing; don't conflate the
    two.
- `onHit` (lines 93-100): on any hit (block or entity, via the shared `HitResult`
  dispatch), server-side triggers `level().explode(this, x, y, z, 1.0F, false,
  Level.ExplosionInteraction.MOB)` — power 1.0, does not set fire (`false`), interaction
  type `MOB` (as opposed to `TNT`/`BLOCK` — affects which blocks the explosion can
  destroy, tie this into whatever explosion-interaction-type modeling Pumpkin's
  `world.explode*` family already has, e.g. `explode_without_blocks` seen in
  `wind_charge.rs` takes only a power argument — check if an "explosion interaction type"
  parameter needs adding to Pumpkin's explosion API, or if a `Mob`-interaction-type
  explosion already has a dedicated helper method) — then `discard()`.
- Save data (lines 120-130): just the `dangerous` boolean on top of whatever
  `AbstractHurtingProjectile`/`Projectile` base persists (owner, life, etc. — already
  handled generically by however Pumpkin's other fireball-family entities round-trip
  NBT, if at all — check `fireball.rs`'s `impl NBTStorage for FireballEntity {}`, which
  is ALSO an empty stub; if fireballs don't persist their state either, this is a
  pre-existing, separate gap this design doesn't need to fix, just shouldn't regress
  further).

### Current Rust state

No file exists. `EntityType::WITHER_SKULL` doesn't appear in `entity/type.rs`'s
`from_type` match (confirmed by the earlier grep — only `WITHER` the boss mob and the
`WitherSkullBlock`/`WitherSkeletonSkullBlock` block logic exist, per the prompt's
framing). `pumpkin/src/entity/boss/wither.rs` was checked for any embedded skull-firing
logic (e.g. inline projectile spawning inside the boss's attack AI) — none found; the
Wither boss currently has no ranged attack implemented at all, consistent with the
projectile itself not existing.

### Target design

Model directly on `pumpkin/src/entity/projectile/fireball.rs` (`FireballEntity`), the
closest existing sibling in the same `AbstractHurtingProjectile`-family — both are
"thrown, no gravity, hits either entity-or-block, explodes on impact" projectiles built
on `ThrownItemEntity`:

```rust
// pumpkin/src/entity/projectile/wither_skull.rs
pub struct WitherSkullEntity {
    thrown: ThrownItemEntity,
    dangerous: AtomicBool,
}

const GRAVITY: f64 = 0.0;

impl WitherSkullEntity {
    pub fn new_shot(entity: Entity, shooter: &Entity, dangerous: bool) -> Self {
        Self {
            thrown: ThrownItemEntity::new(entity, shooter, GRAVITY),
            dangerous: AtomicBool::new(dangerous),
        }
    }
}

impl NBTStorage for WitherSkullEntity {
    // dangerous bool only, matching vanilla's addAdditionalSaveData/readAdditionalSaveData
    // (lines 120-130) — mirror the Mutex/AtomicCell NBT pattern used in section 1.
}

impl EntityBase for WitherSkullEntity {
    fn tick(...) { self.thrown.process_tick(caller, server).await }

    fn on_hit(&self, hit: ProjectileHit) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            if let ProjectileHit::Entity { ref entity, .. } = hit {
                let dangerous = self.dangerous.load(Ordering::Relaxed);
                let world = self.get_entity().world.load();
                let owner = self.thrown.owner_id.and_then(|id| world.get_entity_by_id(id));

                let (damage_type, amount) = match &owner {
                    Some(o) if o.get_living_entity().is_some() =>
                        (DamageType::WITHER_SKULL, 8.0),
                    _ => (DamageType::MAGIC, 5.0),
                };
                let was_hurt = entity.damage(self, amount, damage_type).await;

                if was_hurt {
                    if entity.get_entity().is_alive() {
                        // post-attack enchantment effects hook, if one exists generically
                        // for other melee/projectile hits — reuse, don't duplicate.
                    } else if let Some(owner) = &owner
                        && let Some(living_owner) = owner.get_living_entity() {
                        living_owner.heal(5.0);
                    }

                    // Difficulty-scaled wither effect — NOT gated on `dangerous`.
                    let wither_seconds = match world.get_difficulty() {
                        Difficulty::Normal => 10,
                        Difficulty::Hard => 40,
                        _ => 0,
                    };
                    if wither_seconds > 0 && let Some(living) = entity.get_living_entity() {
                        living.add_effect(StatusEffect::WITHER, 20 * wither_seconds, 1, ...).await;
                    }
                }
            }

            // Vanilla explode(power=1.0, fire=false, interaction=MOB) then discard —
            // check whether Pumpkin's explosion API has an interaction-type parameter;
            // if not, this may currently be indistinguishable from a generic explosion
            // and needs that API extended, or explicitly noted as an approximation.
            let world = self.get_entity().world.load();
            world.explode_without_blocks(hit.hit_pos(), 1.0).await; // or the MOB-interaction equivalent
            self.get_entity().remove().await;
        })
    }

    // get_entity, get_living_entity, as_nbt_storage, cast_any — boilerplate per pattern.
}
```

Also needs, as prerequisite/companion work (both out of this file's named scope but
required for the projectile to ever be fired in practice):
- `pumpkin/src/entity/type.rs`: add `EntityType::WITHER_SKULL` arm to `from_type`.
- `pumpkin/src/entity/projectile/mod.rs`: add `pub mod wither_skull;` and add
  `EntityType::WITHER_SKULL` to `is_projectile()`'s match.
- `pumpkin/src/entity/boss/wither.rs`: the Wither boss's ranged attack AI itself (when to
  fire, target selection, the "dangerous" flag trigger condition, e.g. vanilla's phase-2
  triple-headed dangerous-skull volley) is a **separate, larger AI-behavior gap** not
  named in this doc's scope — flag explicitly that implementing the projectile entity
  alone does not give the Wither boss a ranged attack; that requires reading
  `WitherBoss.java`'s attack-goal/AI code (not fetched this pass) as its own follow-up.

### Files to modify

- `pumpkin/src/entity/projectile/wither_skull.rs` — new file, `WitherSkullEntity`.
- `pumpkin/src/entity/projectile/mod.rs` — module registration, `is_projectile` entry.
- `pumpkin/src/entity/type.rs` — `from_type` arm.
- (Follow-up, not this fix) `pumpkin/src/entity/boss/wither.rs` — actual firing AI.

### Needs verification

- `WitherBoss.canDestroy(BlockState)`'s exact block-tag/hardness check, for the
  `getBlockExplosionResistance` dangerous-skull piercing behavior — cite
  `WitherBoss.java` directly (not read this pass) rather than guessing a tag name.
- Whether Pumpkin's explosion API (`world.explode*` family) has, or needs, an
  interaction-type parameter (`MOB` vs `TNT` vs `BLOCK`) to correctly gate which blocks a
  wither skull explosion can destroy — check `world.rs`'s explosion implementation before
  assuming `explode_without_blocks(pos, power)` is a sufficient substitute.
- Confirm Pumpkin's `Difficulty` enum and per-world difficulty accessor exist and are
  named as assumed above (`world.get_difficulty()`) — used elsewhere in the codebase for
  other difficulty-scaled mechanics (e.g. zombie reinforcements, hunger) so should already
  exist; verify the exact accessor path.
- Confirm a generic "post-attack enchantment effects" hook exists in Pumpkin (vanilla's
  `EnchantmentHelper.doPostAttackEffects`) that this can reuse — referenced by
  `EnderDragon.java`'s `hurt`/`knockBack` methods too (section 1's read), so likely
  already has, or needs, one canonical Rust equivalent shared across all these call
  sites rather than being reinvented per projectile.

---

## Cross-cutting notes

- **Shared helper opportunities**: sections 5 and 6 both need to consult
  `BLOCK_ENTITY_DATA`/component data during entity/block-entity creation — implement one
  shared helper rather than two copies. Sections 1, 7, and 8's `Mutex`/`AtomicCell`-backed
  `NBTStorage` implementations should follow one consistent async-lock-then-serialize
  idiom; if a lightweight macro or helper trait for "N named fields behind async locks,
  round-tripped to N named NBT keys" doesn't already exist, this batch of fixes is a
  natural place to introduce one, given how repetitive the pattern already is across
  `ender_dragon.rs`, `wither.rs`, `tnt.rs`.
- **`FireballEntity`'s own `NBTStorage` is also an empty stub** (`fireball.rs` line 47) —
  not in this doc's named scope, but worth flagging to whoever picks up section 8, since
  the Wither skull design deliberately does NOT inherit that gap (it adds its own
  `dangerous`-flag persistence) — don't let the two get conflated into "fireballs already
  handle this, so skulls will too" during implementation.
- **Testing**: per repo convention (see recent commit
  "test(entity): cover the parched arrow patch surviving the item stack copy"), each fix
  in this batch should land with a targeted unit test where the logic is pure/testable
  (e.g. section 3's damage-falloff-style pure functions, section 2's mayReplace/wouldSurvive
  boolean logic extracted to a testable free function) rather than requiring live-server
  verification, consistent with this repo's stated verification-discipline rules.
