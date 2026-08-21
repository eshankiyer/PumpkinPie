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
# class -> extra Rust files
# ---------------------------------------------------------------------------

# A vanilla class may be implemented across SEVERAL Rust files, and several vanilla
# classes may share ONE Rust file. Name matching alone cannot see either case, so these
# are hand-verified additions layered ON TOP of whatever the automatic mapping finds.
# Keys are exact class names or fnmatch patterns. Paths that do not exist in the repo
# being measured are silently dropped, which is what keeps the table portable to Steel.
# Every entry must name the read that justifies it.
CLASS_FILE_HINTS = {
    # vanilla Attribute (world/entity/ai/attributes) - the only `pub struct Attribute` in
    # the tree is a bedrock packet payload; the real analogue is the attribute registry.
    "Attribute": ["crates/pumpkin/src/entity/attributes.rs"],
    "AttributeInstance": ["crates/pumpkin/src/entity/attributes.rs"],
    # vanilla LevelChunk (world/level/chunk) - matched bedrock/client/level_chunk.rs, a
    # packet. ChunkData is the chunk itself.
    "LevelChunk": ["crates/pumpkin-world/src/chunk/mod.rs"],
    "ChunkAccess": ["crates/pumpkin-world/src/chunk/mod.rs"],
    # vanilla Item is a behaviour base class; Pumpkin splits it into the generated data
    # table (which the automatic mapping already finds), the ItemBehaviour trait, and
    # ItemStack, which carries hurtAndBreak/getDestroySpeed/isCorrectToolForDrops.
    # Verified 2026-08-21: item_stack/mod.rs:325 damage_item, :614 get_speed,
    # :644 is_correct_for_drops.
    "Item": [
        "crates/pumpkin/src/item/mod.rs",
        "crates/pumpkin-data/src/item_stack/mod.rs",
    ],
    # MaceItem's smash attack is not in the item file at all: entity/combat.rs:82-231
    # carries canSmashAttack (inlined into AttackType::new:82), mace_smash_damage_bonus:168
    # and mace_smash_knockback:242. Verified 2026-08-21 by reading both files.
    "MaceItem": ["crates/pumpkin/src/entity/combat.rs"],
    # copper_weathering.rs declares no type at all (it is tables plus free functions), so
    # no struct or filename rule can reach it. All ten WeatheringCopper*Block classes and
    # the WeatheringCopper interface share it. Read 2026-08-21.
    "WeatheringCopper*": ["crates/pumpkin/src/block/blocks/copper_weathering.rs"],
    "*SignBlock": ["crates/pumpkin/src/block/blocks/signs.rs"],
}

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
    # --- inventory Slot, all read off pumpkin-inventory/src/slot.rs 2026-08-21 against
    # world/inventory/Slot.java. Yarn naming throughout; 9 of Slot's 13 leads were renames.
    "mayPlace": {"can_insert"},                     # slot.rs:97
    "tryRemove": {"try_take_stack_range"},          # slot.rs:208
    "hasItem": {"has_stack"},                       # slot.rs:115
    "setChanged": {"mark_dirty"},                   # slot.rs:154
    "safeInsert": {"insert_stack_count"},           # slot.rs:277
    "setByPlayer": {"set_stack_prev"},              # slot.rs:134
    "safeClone": {"get_cloned_stack"},              # slot.rs:109
    "getContainerSlot": {"get_index"},              # slot.rs:51
    "onQuickCraft": {"on_quick_move_crafted"},      # slot.rs:66, same (stack, stack) shape
    "safeTake": {"safe_take"},                      # slot.rs:246
    # --- ItemStack, read off pumpkin-data/src/item_stack/mod.rs 2026-08-21
    "hurtAndBreak": {"damage_item"},                # item_stack/mod.rs:325
    "getDestroySpeed": {"get_speed"},               # item_stack/mod.rs:614
    "isCorrectToolForDrops": {"is_correct_for_drops"},  # item_stack/mod.rs:644
}

# Renames that are true only for ONE vanilla class. Kept separate from METHOD_ALIASES so a
# narrow read cannot silently credit an unrelated class: `getAttackDamageBonus` is a base
# Item method, and only the mace's version is implemented in entity/combat.rs.
CLASS_METHOD_ALIASES = {
    "MaceItem": {
        # entity/combat.rs, read 2026-08-21
        "canSmashAttack": {"new"},                  # inlined into AttackType::new:82
        "getAttackDamageBonus": {"mace_smash_damage_bonus"},   # combat.rs:168
        "postHurtEnemy": {"mace_smash_knockback"},  # combat.rs:242
    },
    "PotionContents": {
        # item/potion.rs:140 apply_effects_to is PotionContents.applyToLivingEntity
        "applyToLivingEntity": {"apply_effects_to"},
    },
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
# Added to that pending list by the 2026-08-21 seed-2108 sample, same reason - they were
# FOUND by that sample, so adding them would invalidate the 5/15 figure it produced:
#   use                -> normal_use   (Item.use; item/items/bow.rs:26)
#   doHurtTarget       -> try_attack   (entity/mob/mod.rs:735)
# And from the task brief rather than the sample, so equally unverified here and equally
# pending: Slot.isFake / Slot.isHighlightable as CLIENT_ONLY_METHODS. Slot's other 2
# remaining leads, checkTakeAchievements and onSwapCraft, were read this session and have
# no Rust analogue at all - they are real gaps, not table entries.
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


# Hand-verified WRONG maps: the Rust file declares the same name but implements something
# else entirely, so every method of the vanilla class becomes a phantom lead. Keyed by
# exact path, so the entry is a no-op in any repo that does not have that file.
#
#   BoundingBox: vanilla's is world/level/levelgen/structure/BoundingBox, an INTEGER box
#   used to lay out structure pieces. pumpkin-util's boundingbox.rs is the float entity AABB
#   (vanilla AABB). pumpkin-world has no structure box at all - `rg BoundingBox` over
#   crates/pumpkin-world/src returns nothing (checked 2026-08-21) - so the class is genuinely
#   uncovered and its 22 "absences" were an artefact of the name collision.
CLASS_FILE_BLOCKLIST = {
    "BoundingBox": ("crates/pumpkin-util/src/math/boundingbox.rs",),
}
