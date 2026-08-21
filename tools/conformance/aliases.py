"""Hand-verified rename tables for the conformance instrument.

Every entry here was confirmed by reading BOTH sides - the 26.2 decompile and the Rust -
during the 2026-08-06 and 2026-08-20 audit passes. They are a fixed lookup, never fuzzy
matching, so they cannot reintroduce the false-positive problem a similarity score has.

They exist because renames are the dominant false-positive source: a hand-verified
18-lead sample of the previous instrument's output found 10 of 18 flagged "gaps" were
Yarn renames or methods inlined into a caller, and 4 more were client-only. Without these
tables the number this instrument prints is roughly three times too pessimistic.

Provenance: salvaged verbatim from the previous conformance run
(enumerate.py / map_coverage.py / method_gaps.py), which is not checked in. Do not add an
entry that has not been justified by an actual read of both sides.
"""

# ---------------------------------------------------------------------------
# class-level renames
# ---------------------------------------------------------------------------

# vanilla class -> Pumpkin struct/enum/trait name.
KNOWN_ALIASES = {
    # hunger/food is modeled as "hunger" throughout Pumpkin
    "FoodData": "HungerManager",
    "FoodConstants": "HungerManager",
    # AI goal renames, all Yarn-derived, confirmed 2026-08-06. 118 goal files exist and
    # several vanilla names simply do not appear verbatim.
    "FloatGoal": "SwimGoal",
    "RandomStrollGoal": "WanderAroundGoal",
    "WaterAvoidingRandomStrollGoal": "WanderAroundGoal",
    "LookAtPlayerGoal": "LookAtEntityGoal",
    "RandomLookAroundGoal": "LookAroundGoal",
    "PanicGoal": "EscapeDangerGoal",
    "NearestAttackableTargetGoal": "ActiveTargetGoal",
    "HurtByTargetGoal": "RevengeGoal",
    "EatBlockGoal": "EatGrassGoal",
    "MeleeAttackGoal": "MeleeAttackGoal",
    "RangedBowAttackGoal": "RangedBowAttackGoal",
}

# vanilla command class -> Pumpkin command file stem. The command WORD changed, not just
# its casing, so command_flatten() cannot recover these.
KNOWN_COMMAND_ALIASES = {
    "BanPlayerCommands": "ban",
    "ListPlayersCommand": "list",
    "ClearInventoryCommands": "clear",
    "SetSpawnCommand": "spawnpoint",
    "SetPlayerIdleTimeoutCommand": "setidletimeout",
    # VersionCommand is folded into Pumpkin's own /pumpkin command (NAMES = pumpkin, version)
    "VersionCommand": "pumpkin",
    # EmoteCommands registers "/me"; flattening gives "emote", which would otherwise land
    # on an unrelated bedrock protocol file named emote.rs.
    "EmoteCommands": "me",
}

# whole naming families: vanilla's `*Menu` GUI classes are Pumpkin's `*ScreenHandler`
# (Yarn-derived), e.g. AnvilMenu -> pumpkin-inventory/src/anvil/anvil_screen_handler.rs.
SUFFIX_REWRITES = [
    ("Menu", "ScreenHandler"),
]

# Java doc-marker files, not conformance units.
NOISE_CLASSES = {"package-info"}

# ---------------------------------------------------------------------------
# method-level renames
# ---------------------------------------------------------------------------

# vanilla method name -> the Pumpkin fn name(s) that implement it. Snake-casing alone
# cannot recover these because Pumpkin took its naming from Yarn, not Mojang.
METHOD_ALIASES = {
    # BlockBehaviour (SlabBlock.java:116 vs crates/pumpkin/src/block/mod.rs:170)
    "updateShape": {"get_state_for_neighbor_update"},
    "neighborChanged": {"on_neighbor_update"},
    "canSurvive": {"can_update_at", "can_place_at"},
    # BlockEntity NBT hooks
    "saveAdditional": {"write_nbt"},
    "loadAdditional": {"from_nbt"},
    # redstone accessors (DaylightDetectorBlock.java:96 vs redstone/daylight_detector.rs)
    "ownSignal": {"get_weak_redstone_power"},
    "isSignalSource": {"emits_redstone_power"},
    # AI goal lifecycle (Goal.java:14 vs entity/ai/goal/mod.rs:134)
    "canContinueToUse": {"should_continue"},
    "canUse": {"can_start"},
    "setPlacedBy": {"player_placed"},
    # Entity NBT hooks
    "addAdditionalSaveData": {"write_nbt"},
    "readAdditionalSaveData": {"read_nbt_non_mut", "read_nbt"},
    # NeutralMob anger API -> PersistentAnger; Pumpkin stores remaining ticks rather than
    # an absolute end tick, so the vanilla get/set pair collapses.
    "startPersistentAngerTimer": {"start_timer"},
    "setPersistentAngerTarget": {"set_angry_at"},
    "getPersistentAngerTarget": {"is_angry", "angry_at"},
    "isAngryAt": {"is_angry_at"},
    # attributes are generated from real 26.2 data, not hand-written per mob
    "createAttributes": {"attributes", "default_attributes"},
    # LivingEntity. aiStep is NOT baseTick - an automated pass once conflated them.
    "aiStep": {"tick_movement"},
    "getJumpPower": {"get_jump_velocity"},
    "checkTotemDeathProtection": {"try_use_death_protector"},
    "doHurtEquipment": {"damage_armor_items"},
    # Mob / PathfinderMob / AgeableMob
    "getBreedOffspring": {"create_offspring"},
    "getWalkTargetValue": {"get_pathfinding_favor"},
    "getHeadRotSpeed": {"get_max_look_yaw_change"},
    "getMaxHeadXRot": {"get_max_look_pitch_change"},
    "getMaxHeadYRot": {"get_max_head_rotation"},
    "wantsToPickUp": {"wants_to_pick_up_item"},
    # Leashable
    "shouldStayCloseToLeashHolder": {"should_follow_leash"},
    "closeRangeLeashBehaviour": {"on_short_leash_tick"},
    "whenLeashedTo": {"before_leash_tick"},
    "followLeashSpeed": {"get_follow_leash_speed"},
    # Goal trait
    "isInterruptable": {"can_stop"},
    "requiresUpdateEveryTick": {"should_run_every_tick"},
    "adjustedTickDelay": {"get_tick_count"},
    "getFlags": {"controls"},
    "setFlags": {"controls"},
    # ItemEntity and ChestBlock ports
    "hasPickUpDelay": {"has_pickup_delay"},
    "areMergable": {"are_mergeable_stacks"},
    "playerTouch": {"on_player_collision"},
    "hurtServer": {"damage_with_context", "damage"},
    "defineSynchedData": {"init_data_tracker", "mob_init_data_tracker"},
    "getDefaultGravity": {"get_gravity"},
    "isChestBlockedAt": {"is_chest_blocked"},
    "isBlockedChestByBlock": {"has_block_on_top"},
    # is_solid_block IS the redstone-conductor predicate (pumpkin-data/src/block_state.rs)
    "isRedstoneConductor": {"is_solid_block"},
    "distanceToSqr": {"distance_squared"},
    "getDimensions": {"get_entity_dimensions"},
    "isNoGravity": {"has_no_gravity"},
    "setNoGravity": {"set_has_no_gravity"},
    "isUnderWater": {"is_eye_in_water"},
    "setPosRaw": {"set_pos"},
    "canAttack": {"can_attack"},
    "causeFallDamage": {"handle_fall_damage"},
    # StonecutterMenu
    "clickMenuButton": {"on_button_click", "select_recipe"},
    "isValidRecipeIndex": {"select_recipe"},
    "setupResultSlot": {"update_output"},
    "setupRecipeList": {"update_output"},
    "onTake": {"on_take_item"},
    # FishingHook / FishingRodItem
    "retrieve": {"reel_in"},
    # AbstractArrow piercing
    "onHitEntity": {"on_hit"},
    "canHitEntity": {"should_skip_collision"},
    # --- verified 2026-08-21 by reading both sides, from this run's 18-lead sample ---
    # Player.isStayingOnGroundSurface:300 is a one-line alias for isShiftKeyDown.
    "isStayingOnGroundSurface": {"is_sneaking"},
    # FireBlock.canBurn:285 is "ignite odds > 0"; block/blocks/fire/fire.rs:37 is_flammable
    # is the same predicate over the generated flammable table.
    "canBurn": {"is_flammable"},
    # LevelChunk.markUnsaved -> chunk/format/mod.rs:67 mark_dirty
    "markUnsaved": {"mark_dirty"},
    # ChunkGenerator.getSpawnHeight:431 -> world/spawn_finder.rs:198 initial_spawn_height,
    # which cites it by name and handles the flat-generator override too.
    "getSpawnHeight": {"initial_spawn_height"},
    # FoliagePlacer.foliageHeight -> tree/foliage/*.rs get_random_height
    "foliageHeight": {"get_random_height"},
    # EntitySelectorParser.shouldInvertValue:327 -> entity_selector/parser.rs:399
    "shouldInvertValue": {"consume_inverted_start"},
    # ItemStack.shrink -> item_stack/mod.rs:540 decrement
    "shrink": {"decrement"},
    # ClientboundTrackedWaypointPacket.addWaypointPosition:34 -> waypoint.rs:89 add_position
    "addWaypointPosition": {"add_position"},
    "updateWaypointPosition": {"update_position"},
    "removeWaypoint": {"remove"},
    # --- block trait renames, read off crates/pumpkin/src/block/mod.rs 2026-08-21.
    # These are the biggest single false-positive families in the lead list
    # (useWithoutItem 50, useItemOn 22, useOn 22, affectNeighborsAfterRemoval 25,
    # entityInside 21). NOTE the trait carries default bodies, so a block that does NOT
    # override one still gets "elsewhere in workspace" credit - which is exactly why the
    # strict, per-file number is reported alongside the workspace-wide one.
    "useWithoutItem": {"normal_use"},          # block/mod.rs:86
    "useItemOn": {"use_with_item"},            # block/mod.rs:90
    "useOn": {"use_with_item", "use_on_block"},
    "entityInside": {"on_entity_collision"},   # block/mod.rs:97
    "affectNeighborsAfterRemoval": {"on_state_replaced"},  # block/mod.rs:207
    "getAnalogOutputSignal": {"get_comparator_output"},    # block/mod.rs:237
}

# Vanilla methods that exist only for the client/renderer, so a dedicated server can
# never call them. Each verified by reading vanilla: the body is isClientSide-gated, or
# the only callers are client render classes.
CLIENT_ONLY_METHODS = {
    # LivingEntity.java:3719 empty no-op; only Parrot overrides it, for the client dance
    "setRecordPlayingNearby",
    # Entity.java:1925 - reached only via the !(level instanceof ServerLevel) branch
    "hurtClient",
    # BaseSpawner.onEventTriggered:249 - body runs only under level.isClientSide()
    "triggerEvent",
}

# Vanilla methods that exist to feed Mojang's codec/registry machinery, which this
# codebase replaces with generated data in pumpkin-data rather than with functions. Each
# justified by a read; excluded from scoring and reported separately, the same way
# client-only methods are. Deliberately short - a broad rule like "every get*" would hide
# real gaps.
# PENDING - found by the 2026-08-21 15-lead verification but deliberately NOT added yet,
# because adding them would invalidate the 4/15 precision figure the docstring quotes.
# Add them together and re-sample:
#   ignoreExplosion    -> should_damage_entity  (inverted, on the explosion side)
#   getMinX/getMinZ/getMaxX/getMaxZ -> world/border.rs bounds()
#   doHurtTarget       -> try_attack            (entity/mob/mod.rs:735)
#   createMenu         -> the *ScreenHandlerFactory impls
DATA_MODELED_METHODS = {
    # DarkOakTrunkPlacer.type():25 just returns TrunkPlacerType.DARK_OAK_TRUNK_PLACER; the
    # dispatch it feeds is a serde-tagged enum here. 177 leads, all of this shape.
    "type",
    # BlockBehaviour.createBlockStateDefinition builds the state definition Mojang derives
    # at runtime; pumpkin-data/src/generated/block.rs ships it as data. 120 leads.
    "createBlockStateDefinition",
    # StringRepresentable.getSerializedName - the enum's wire/NBT name. Pumpkin round-trips
    # these with from_str/to_string on the generated enums (e.g. JigsawJointType::from_str,
    # block/entities/jigsaw_block.rs:258). 43 leads.
    "getSerializedName",
    # Codec/MapCodec accessors, same machinery.
    "codec",
}

# Method-name prefixes/exact names that are Java plumbing rather than behaviour. Kept
# empty deliberately: every exclusion must be justified by a read, and broad prefix rules
# (`get*`) hide real gaps. Add here only with a comment naming the evidence.
STRUCTURAL_EXCLUSIONS: set[str] = set()
