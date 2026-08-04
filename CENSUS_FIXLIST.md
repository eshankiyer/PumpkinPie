# Full census fix list (2026-08-03)

**STATUS: Category D complete.** All 6 fix batches landed and pushed to master
(final commit b06a552, 468 tests passing). One item skipped as genuinely out of
scope (button per-BlockSetType press delay — no BlockSetType data exists in
pumpkin-data at all; needs a data-generator addition first, filed below). One
new bug found during batch 6 and NOT yet fixed (furnace/smoker/blast_furnace XP
drop on break is dead code — see below). Category C design docs (12) are done,
in designs/. Phase 1 prerequisite implementation is in progress — see bottom of
this file for status once posted.


Source: 27-chunk full census of all 544 "covered" conformance units (chunks 0-26,
all Explore agents, complete). This file is the durable record of the triage —
context will get compacted, this won't.

## Category D: dispatch-worthy bugs (small, local, verifiable)

Group by file locality into worktree batches. Each fix = its own commit. Full
build gate (`cargo fmt --all && RUSTFLAGS="-D warnings" cargo clippy -p pumpkin
--all-targets`) per CLAUDE.md before every commit. Numeric/pure-function fixes
need a unit test pinning the vanilla-correct value.

### Batch 1: redstone blocks
- `WeightedPressurePlateBlock::calculate_redstone_output` (pumpkin/src/block/blocks/redstone/pressure_plate/weighted.rs)
  — integer division before f32 cast truncates partial loads to 0; vanilla does
  float division then `Mth.ceil(percent*15.0)`.
- `RepeaterBlock` normal_use (pumpkin/src/block/blocks/redstone/repeater.rs) —
  no `mayBuild`/build-permission gate before cycling delay.
- `CopperBulbBlock` (pumpkin/src/block/blocks/redstone/copper_bulb.rs) — no
  `getAnalogOutputSignal`/comparator output (should be 15 when LIT else 0).
- `TargetBlock` (pumpkin/src/block/blocks/redstone/target_block.rs) — no
  `on_place` override to reset stale OUTPUT_POWER to 0 (vanilla TargetBlock.java:107-113).
- `ButtonBlock` (pumpkin/src/block/blocks/redstone/buttons.rs) — hardcoded
  20/30 tick press delay instead of per-BlockSetType configured value.
- `CalibratedSculkSensorBlock` (pumpkin/src/block/blocks/redstone/sculk_sensor.rs)
  — emits power in all directions instead of excluding own FACING; uses 30
  active-ticks instead of vanilla's 10 for the calibrated variant.
- `EndPortalFrameBlock` (pumpkin/src/block/blocks/end_portal_frame.rs) — no
  `get_comparator_output`/`hasAnalogOutputSignal` (should be 15 when HAS_EYE).
- `LightningRodBlock::trigger` (pumpkin/src/block/blocks/redstone/lightning_rod.rs)
  — function exists but is never called from anywhere; lightning never powers rods.
- `SculkSensorBlockEntity::last_vibration_frequency` (pumpkin/src/block/entities/sculk_sensor.rs)
  — only read/written via NBT, never updated when a vibration is actually
  received; dead value feeding comparator output.
- `BellBlockEntity::ring_bell`/`activate` (pumpkin/src/block/entities/bell.rs)
  — volume/pitch swapped (plays 1.0/2.0, vanilla is 2.0/1.0).
- `BellBlockEntity::raiders_hear_bell` — hardcoded `false` TODO stub; raid-glow
  and villager bell-heard memory never fire.

### Batch 2: spawner + portal + mob-category numeric fixes
- `MobSpawnerBlockEntity::tick` (pumpkin/src/block/entities/mob_spawner.rs) —
  spawn offset uses `(rand()+rand())*range` (always positive) instead of
  vanilla's `(rand()-rand())*range` (symmetric); also no `isNearPlayer`/
  `requiredPlayerRange` gate at all; also NBT round-trip broken (writes nothing
  back for MinSpawnDelay/MaxSpawnDelay/SpawnCount/MaxNearbyEntities/
  RequiredPlayerRange/SpawnRange, and reads with wrong NBT type so they silently
  reset to defaults).
- `PortalProcessor::process_portal_teleportation` (pumpkin/src/world/portal/mod.rs)
  — pre-increments `portal_time` then compares, vanilla post-increments
  (`this.portalTime++ >= transitionTime`); teleports one tick early every time.
- `MobCategory::WATER_CREATURE` (pumpkin-codegen/src/entity_type.rs) —
  `is_persistent: true`, vanilla is `false`; water creatures never subject to
  normal despawn rules.

### Batch 3: goal-system fixes
- `Goal::should_continue` trait default (pumpkin/src/entity/ai/goal/mod.rs) —
  hardcoded `false`; vanilla's `canContinueToUse()` defaults to `canUse()`.
  Latent bug for any goal that doesn't explicitly override it.
- `GoalSelector::remove_goal` (pumpkin/src/entity/ai/goal/goal_selector.rs) —
  uses `swap_remove` with a uniform `*slot -= 1` fixup on `goals_by_control`;
  wrong index when ≥2 goals of the same generic type are removed in one call.
- `DolphinJumpGoal::tick` (pumpkin/src/entity/ai/goal/dolphin_jump.rs) — plays
  `EntityDolphinJump` every tick while in water; vanilla plays it once,
  edge-triggered on the false→true transition of a `breached` flag.

### Batch 4: plant blocks
- `SaplingBlock::random_tick` (pumpkin/src/block/blocks/plant/sapling.rs) —
  calls `generate()` unconditionally every random tick; vanilla requires light
  level above >= 9 AND a 1-in-7 chance.
- `SeaPickleBlock::use_with_item` bonemeal spread (pumpkin/src/block/blocks/plant/sea_pickles.rs)
  — `for added_z in 0..1` (always 1) instead of vanilla's diamond-shaped
  1→2→3→2→1 z-span across the 5 x-iterations.
- `LilyPadBlock::on_entity_collision` (pumpkin/src/block/blocks/plant/lily_pad.rs)
  — checks `entity_type.resource_name.ends_with("_boat")`, missing
  `bamboo_raft`/`bamboo_chest_raft` which don't match that suffix.
- `SpongeBlock::absorb_water`/`on_neighbor_update` (pumpkin/src/block/blocks/sponge.rs)
  — checks `block.id == Block::WATER.id` instead of the water fluid tag; misses
  waterlogged blocks, kelp, and seagrass, which vanilla also absorbs/removes.
- `FarmlandBlock::can_place_at` (pumpkin/src/block/blocks/farmland.rs) — checks
  `tag::Block::C_FENCE_GATES` instead of vanilla's `MAINTAINS_FARMLAND` tag
  (which also includes crops, torchflower, pitcher_crop, wheat, moving_piston).

### Batch 5: projectiles
- `SmallFireball::on_hit` (pumpkin/src/entity/projectile/small_fireball.rs) —
  block-hit branch places fire unconditionally, no `MOB_GRIEFING` gamerule
  check and no `isEmptyBlock` check; also doesn't restore `remainingFireTicks`
  when entity-hit damage fails, and never discards itself after any hit
  (vanilla always calls `discard()` in `onHit`).
- `EndCrystal::damage_with_context` (pumpkin/src/entity/decoration/end_crystal.rs)
  — no `source.getEntity() instanceof EnderDragon` guard; the Ender Dragon's
  own attacks can destroy its healing crystals, which vanilla explicitly
  prevents.

### Batch 6: container/NBT persistence fixes
- `BrewingStandBlock` (pumpkin/src/block/blocks/brewing_stand.rs) — no
  `broken` implementation at all (unlike furnace/smoker/blast_furnace/chests);
  contents are lost instead of dropped when the block is broken.
- `CampfireBlockEntity` (pumpkin/src/block/entities/campfire.rs +
  pumpkin/src/block/blocks/campfire.rs) — no `cooldownTick` (progress should
  decay by 2/tick when unlit mid-cook); no drop-contents-on-break in the block's
  `broken`/removal path, so items inside a broken campfire are lost.
- `StonecutterBlock` (pumpkin/src/block/blocks/stonecutter.rs) — never
  overrides `on_place`, so placed stonecutters always face north regardless of
  the placing player's orientation.
- `SignBlockEntity`/`SignBlock::normal_use` (pumpkin/src/block/blocks/signs.rs)
  — runs click-commands on both front and back text unconditionally; vanilla
  only runs commands for whichever side the player is actually facing.

## Category C: feature work, NOT this batch — file as PARITY.md rows

Llama caravans, Turtle egg-laying, Dolphin treasure-seeking/moistness,
WanderingTrader + MerchantOffer trade economy (demand/price escalation),
Fox stalking/pouncing/item-stealing, Piglin bartering/brain AI, Ravager
roar/stun state machine, Vex no-physics flight AI, Strider lava-riding/saddle
steering, Illusioner spellcasting, ScaffoldingBlock (entirely unimplemented),
StructureBlock (save/load/place structures, entirely unimplemented),
BrushableBlock + BrushItem (entire archaeology brushing mechanic), SculkBlock
spread-charge stack (~1400 lines in vanilla; a partial port would be worse
than the current stub per direct agent finding), LoomBlock (entirely
unimplemented, no menu), GrindstoneBlock menu (repair/disenchant UI, entirely
unimplemented), RespawnAnchorBlock (entirely unimplemented, only a stale
unmerged branch exists), Villager golem-summoning-when-panicked, Wolf
owner-defense/anger-target goals + persistent anger + armor mechanics, Cat
lying/relax/collar-dye/goal-set, Vindicator Johnny mechanic + raid door
breaking, Pillager banner inventory + raid buffs, ZombifiedPiglin group-anger
propagation, Camel dashing/rider-jump, Sheep shearing, Rabbit jump movement +
killer-bunny variant, Squid/GlowSquid/Salmon/TropicalFish/Cod per-species data
(variant/sound/bucket), Mule/Donkey/Horse chest-inventory+taming framework,
ZombieHorse full behavior, TraderLlama defend-wandering-trader goal, Creaking
heart-binding lifecycle, Sniffer digging state machine + egg-drop breeding,
ShulkerBox open/close collision-pushing state machine, Beehive full bee
lifecycle (release/anger/campfire-smoke), ChestBlock cat-sitting-blocks-open,
ConduitBlockEntity (entirely unimplemented server tick), BundleItem
stacked-item click-to-insert/remove, ShieldItem dye-name override, CompassItem
lodestone lock (entirely unimplemented), SpyglassItem (entirely unimplemented),
ShearsItem (entirely unimplemented — no Rust file at all), CrossbowItem
firework-rocket ammo support, MerchantOffer/VillagerTrade demand-pricing,
BlockItem BLOCK_ENTITY_DATA transfer on placement, EntityEquipment per-tick
`inventoryTick`, Mannequin (entirely unimplemented), Interaction entity
(entirely unimplemented), WitherSkull projectile (entirely unimplemented —
distinct from the WitherSkeletonSkullBlock block logic which does exist),
Witch raid-buffs/magic-damage-reduction/self-potion-drinking, RangedBowAttackGoal
/RangedCrossbowAttackGoal full charge-state-machine (vs current flat-cooldown
approximation), MeleeAttackGoal path-existence check (needs real pathfinding
integration), TemptGoal anti-flee jitter check + per-mob TEMPT_RANGE attribute,
FollowOwnerGoal leash/passenger gate + water-malus temp-zero, AvoidEntityGoal
family already matches — no action, Piglin full brain AI (bartering/pickup/
dance/conversion), PiglinBrute golden-axe equipment + group-anger, Hoglin
zombification/pacify/knockback-attack, EnderDragon NBTStorage persistence
(state lost on save/reload), FallingBlockEntity mayReplace/wouldSurvive check
+ despawn timeout + damage-on-landing, FireworkRocketEntity sound/particle +
isShotAtAngle branch, WindChargeItem dispenser wiring + eye-height spawn fix,
MinecartItem ENTITY_PLACE event + multi-minecart-same-tile guard, BundleItem
click-to-insert, DyeColor mixing (`getMixedColor`) for sheep breeding,
CowVariant biome-based spawn selection, structure-placement rotate/mirror for
block-state properties (RedstoneWire, StairBlock, LeafLitterBlock, FenceBlock,
BigDripleafStemBlock, MangrovePropaguleBlock, FlowerbedBlock — this is ONE
architectural gap: no generic rotate/mirror transform hook exists in
BlockBehaviour at all; note `Mirror::rotation()`/`OctahedralGroup` also
missing) — file as a single row, not per-block.

## Newly discovered during fix batch 6 (not yet fixed)

- `furnace.rs:132`, `smoker.rs:131`, `blast_furnace.rs:137` each implement `broken()` to
  extract and drop accumulated XP via `get_block_entity()`, but by the time
  `block_registry.broken()` runs, `world.break_block()` has already removed the block
  entity (`world/mod.rs:4799`, strictly before the `broken()` call sites in
  `net/java/play.rs`). So `get_block_entity()` returns `None` and the XP-orb drop is
  dead code for all three cooking blocks. Needs its own branch/PR — likely requires
  reordering the break sequence (capture block-entity data before removal) rather than
  a local fix to any one of the three files.

## Two housekeeping items (verified/resolved separately, not fix-agent work)

- `block-entity-stubs` worktree has an uncommitted, unverified PotentSulfur
  state-machine implementation sitting as dirty working-tree changes — must be
  gated (build+commit) or discarded before more agents touch overlapping files.
- Sweep for stray branches after every fix-agent batch:
  `git ls-remote forgejo | grep -v 'refs/heads/master'` — the bee-behavior
  agent previously pushed to a namesake branch instead of master.
