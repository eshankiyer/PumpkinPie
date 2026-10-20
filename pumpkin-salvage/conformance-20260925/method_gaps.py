#!/usr/bin/env python3
"""
Per-class, method-level gap list: for each COVERED vanilla class, which of its enumerated
methods have no plausibly-named counterpart in the Rust file that declares the matched
type. Mechanical name matching only -- no AI, no behavioural comparison.

This turns "aim for 100% parity" into a ranked worklist. Run after enumerate.py and
map_coverage.py.

WHAT THIS IS NOT
----------------
A missing method here is a LEAD, not a proven gap. Known false-positive sources:

  * Rust puts the impl in a different file than the type declaration (trait impls,
    split modules). Only the declaring file is scanned.
  * Pumpkin names the method differently (vanilla `getShape` -> `get_outline_shape`).
  * The behaviour exists but is inlined into a caller rather than being its own fn.
  * Vanilla's method is a codec/serialization hook Pumpkin models declaratively.

A method listed as PRESENT is a stronger signal than one listed as MISSING, because a
name collision is less likely than a name divergence. Treat the output as "read these
classes first", and confirm against the decompile before claiming a gap exists.

MEASURED PRECISION (2026-08-06)
-------------------------------
An 18-lead random sample (seed 4903) was verified by reading both the vanilla decompile
and the Rust for every lead:

    REAL_GAP        3   genuinely unimplemented
    FALSE_POSITIVE 10   implemented under a Yarn rename, or inlined into a caller
    CLIENT_ONLY     4   render/client-side; unreachable on a dedicated server
    DATA_MODELED    1   expressed as generated data rather than a fn

So roughly ONE IN SIX flagged methods is a real gap. Do not read the absent-count as a
backlog size: scale it by ~0.17 for a rough expected yield. Every rename and client-only
method that sample exposed is now encoded in METHOD_ALIASES / CLIENT_ONLY_METHODS below,
so re-running the sample should measure higher precision than 17% -- re-measure rather
than assuming this figure still holds after those rules were added.

The three confirmed gaps from that sample, as worked examples of what a real one looks
like:
  * FlowerBlock.makeEffectList  -- suspicious stew effects absent entirely; the item
    component id exists but the flower->effect table does not.
  * EnderDragon.hurtServer      -- EnderDragonEntity::hurt exists but has zero callers;
    part damage reduction and phase.onHurt are missing, so the consumer side is fed by
    dead code.
  * CopperGolem.thunderHit      -- no lightning-strike-on-entity path exists at all;
    the spawned bolt has no tick behavior for any entity.
"""
import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
HERE = Path(__file__).resolve().parent
CRATES = sorted(
    f"crates/{p.name}" for p in (REPO_ROOT / "crates").iterdir() if p.is_dir()
)

# Vanilla accessor prefixes Pumpkin routinely drops: getShape -> shape, isAir -> is_air
# (is_ is kept, it survives snake_casing as a real prefix).
GETTER_RE = re.compile(r"^get([A-Z]\w*)$")

# Verified vanilla-method -> Pumpkin-fn renames. Each was confirmed by reading BOTH sides;
# these are the false positives pure snake_casing cannot avoid, because Pumpkin took its
# naming from Yarn rather than Mojang. Keep every entry justified by a real read.
#
# Without this table the tool reports "absent workspace-wide" for behaviour that is fully
# implemented -- the exact failure mode that makes a raw gap count untrustworthy.
METHOD_ALIASES = {
    # vanilla BlockBehaviour.updateShape -> Pumpkin BlockBehaviour::get_state_for_neighbor_update
    # (verified: SlabBlock.java:116 vs crates/pumpkin/src/block/mod.rs:170)
    "updateShape": {"get_state_for_neighbor_update"},
    # vanilla neighborChanged -> Pumpkin on_neighbor_update (crates/pumpkin/src/block/mod.rs:161)
    "neighborChanged": {"on_neighbor_update"},
    # vanilla canSurvive -> Pumpkin can_update_at / can_place_at family
    "canSurvive": {"can_update_at", "can_place_at"},
    # --- verified 2026-08-06 by reading both sides on a random 18-lead sample ---
    # BlockEntity NBT hooks -> Pumpkin's BlockEntity trait
    # (PotentSulfurBlockEntity.java:142 vs block/entities/potent_sulfur.rs:59/:48)
    "saveAdditional": {"write_nbt"},
    "loadAdditional": {"from_nbt"},
    # redstone accessors (DaylightDetectorBlock.java:96 vs redstone/daylight_detector.rs:53/:64)
    "ownSignal": {"get_weak_redstone_power"},
    "isSignalSource": {"emits_redstone_power"},
    # AI goal lifecycle (Goal.java:14 vs entity/ai/goal/mod.rs:134)
    "canContinueToUse": {"should_continue"},
    "canUse": {"can_start"},
    # block placement callback (Block.setPlacedBy vs block/mod.rs:134 player_placed)
    "setPlacedBy": {"player_placed"},
    # --- verified 2026-08-06 during the ZombifiedPiglin port ---
    # Entity NBT hooks; the sibling spelling of save/loadAdditional above.
    "addAdditionalSaveData": {"write_nbt"},
    "readAdditionalSaveData": {"read_nbt_non_mut", "read_nbt"},
    # NeutralMob anger API -> PersistentAnger (entity/ai/persistent_anger.rs:52,78,86).
    # Pumpkin stores remaining ticks rather than an absolute end tick, so the vanilla
    # get/set end-time pair collapses into these.
    "startPersistentAngerTimer": {"start_timer"},
    "setPersistentAngerTarget": {"set_angry_at"},
    "getPersistentAngerTarget": {"is_angry", "angry_at"},
    "isAngryAt": {"is_angry_at"},
    # Attributes are generated from real 26.2 data rather than hand-written per mob, so
    # every mob's createAttributes looks absent and never is.
    "createAttributes": {"attributes", "default_attributes"},
    # --- 18 renames from the 2026-08-06 entity/AI alias audit, each confirmed by
    # reading both sides. Numeric defaults were matched where vanilla has one. ---
    # LivingEntity. aiStep is NOT baseTick -- an automated pass conflated them.
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
    # --- from the 2026-08-06 ItemEntity and ChestBlock ports ---
    "hasPickUpDelay": {"has_pickup_delay"},
    "areMergable": {"are_mergeable_stacks"},
    "playerTouch": {"on_player_collision"},
    "hurtServer": {"damage_with_context", "damage"},
    "defineSynchedData": {"init_data_tracker", "mob_init_data_tracker"},
    "getDefaultGravity": {"get_gravity"},
    "isChestBlockedAt": {"is_chest_blocked"},
    "isBlockedChestByBlock": {"has_block_on_top"},
    # Pumpkin's is_solid_block IS the redstone-conductor predicate, just renamed
    # (pumpkin-data/src/block_state.rs:114-117).
    "isRedstoneConductor": {"is_solid_block"},
    # --- 8 survivors of a 55-proposal local-model batch, each re-read on both sides ---
    # The other 47 were rejected: getter/setter confusion, predicate inversions, and
    # names matched onto unrelated types. See the note below on why that path was
    # abandoned.
    "distanceToSqr": {"distance_squared"},
    "getDimensions": {"get_entity_dimensions"},
    "isNoGravity": {"has_no_gravity"},
    "setNoGravity": {"set_has_no_gravity"},
    "isUnderWater": {"is_eye_in_water"},
    "setPosRaw": {"set_pos"},
    "canAttack": {"can_attack"},
    "causeFallDamage": {"handle_fall_damage"},
    # --- verified 2026-08-06 while disproving stale backlog leads ---
    # StonecutterMenu -- the whole button-click chain already exists.
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
    # --- harvested 2026-08-31 from round 14's WAVE_REPORT classifications, each
    # re-verified this session by grepping the target Rust fn actually exists.
    # A few round-14 RENAMED claims were dropped from this batch (e.g. calculateState ->
    # update) because the Rust target name is too generic on its own to trust as a
    # global alias -- it would credit unrelated vanilla methods sharing that name.
    "affectNeighborsAfterRemoval": {"on_state_replaced"},
    "defaultAcceptableLandingSpot": {"is_acceptable_landing_position"},
    "fallOn": {"on_landed_upon"},
    "getEntityInsideCollisionShape": {"get_inside_collision_shape"},
    "getJumpCandidate": {"take_jump_candidate"},
    "mayPlace": {"can_insert"},
    "modifySenderForExecution": {"function_source_for_execution"},
    "tryRemove": {"try_take_stack_range"},
    "useItemOn": {"use_with_item"},
    "useWithoutItem": {"normal_use"},
    # --- harvested 2026-08-31 from round 15's Item-class WAVE_REPORT, each re-verified
    # this session by grepping the target Rust fn actually exists. "use" -> "prepare_hand_item_for_use"
    # was dropped from this batch: too generic a vanilla name to trust as a global alias.
    "canDestroyBlock": {"can_mine"},
    "getDefaultMaxStackSize": {"get_max_stack_size"},
    "getDestroySpeed": {"get_speed"},
    "getPlayerPOVHitResult": {"get_start_and_end_pos"},
    "interactLivingEntity": {"use_on_entity"},
    "isCorrectToolForDrops": {"is_correct_for_drops"},
    "releaseUsing": {"on_stopped_using"},
    "useOn": {"use_on_block"},
}

# MEASURED, 2026-08-06: mining these aliases with a local 7B model does not pay off.
# A 400-lead batch produced 55 proposals of which a verifier accepted 8 (15%), costing
# ~111k verification tokens. A single agent auditing the entity/AI traits directly
# produced 18 verified renames for ~109k tokens. Same cost, more than twice the yield.
#
# The reason is that the decision is semantic, not mechanical: judging whether `move` and
# `move_to` mean the same thing requires reading both implementations. The cheap gates in
# llm_alias.py (name exists, name lives in the right module subtree) catch hallucination
# but cannot catch wrongness, so a human-equivalent reviewer is needed either way and the
# cheap model only adds a filtering step. Prefer direct agent audits.

# Vanilla methods that exist only for the client/renderer. A dedicated server can never
# call them, so flagging them as parity gaps is noise. Each verified by reading the
# vanilla source: either the body is isClientSide-gated, or the only callers are client
# render classes.
CLIENT_ONLY_METHODS = {
    # LivingEntity.java:3719 empty no-op; only Parrot overrides it, feeding the
    # client-side dance animation. No caller anywhere in the server tree.
    "setRecordPlayingNearby",
    # Entity.java:1925 -- reached only via the `!(level instanceof ServerLevel)` branch;
    # a dedicated server always takes hurtServer.
    "hurtClient",
    # BaseSpawner.onEventTriggered:249 -- body runs only under level.isClientSide().
    "triggerEvent",
    # --- harvested 2026-08-31 from round 14's WAVE_REPORT classifications. `display`
    # and `getProvider` were dropped from this batch: both names are too generic to
    # trust globally client-only, since an unrelated server-relevant method elsewhere
    # could share the name.
    "animateTick",
    "copyWithStyle",
    "getDetailedMessage",
    "getShadeBrightness",
    "getUseAnimation",
    "isCapableOfSpawning",
    "isSkippable",
    "optionalIngredientToDisplay",
    "showJumpParticles",
    "showSlideParticles",
    # --- harvested 2026-08-31 from round 15's Item-class WAVE_REPORT. `registries` was
    # dropped from this batch: plausible name for a legitimate server-side registries
    # accessor elsewhere, too risky to trust as globally client-only.
    "getBarColor",
    "getBarWidth",
    "getTooltipImage",
    "isBarVisible",
    "isFoil",
    "isPeaceful",
    "mapData",
}


def camel_to_snake(name: str) -> str:
    s = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", name)
    s = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", s)
    return s.lower()


def method_aliases(java_name: str) -> set[str]:
    """Plausible Rust spellings of one Java method name."""
    out = {camel_to_snake(java_name)}
    m = GETTER_RE.match(java_name)
    if m:
        # getShape -> shape, and get_shape
        out.add(camel_to_snake(m.group(1)))
    out |= METHOD_ALIASES.get(java_name, set())
    return out


def _decl_score(name: str, path: str) -> tuple:
    """Rank candidate files declaring `name`. Higher is better.

    rg returns matches in nondeterministic order, so taking the first hit picked the
    wrong file whenever a name is declared twice -- vanilla `Player` resolved to
    pumpkin-protocol's PlayerInfoUpdate packet struct instead of the actual entity,
    making all 201 of its methods look absent. Score explicitly instead.
    """
    p = Path(path)
    stem_match = p.stem == camel_to_snake(name)
    # Behaviour lives in the pumpkin crate; pumpkin-data holds generated tables and
    # pumpkin-protocol holds wire structs that merely share a name.
    in_gameplay = path.startswith("crates/pumpkin/")
    generated = "/generated/" in path
    return (stem_match, in_gameplay, not generated, -len(path), path)


def build_decl_index() -> dict[str, str]:
    """Rust type name -> the best file declaring it."""
    out = subprocess.run(
        ["rg", "--no-heading", "--with-filename", "-g", "*.rs", "-o",
         r"^\s*pub (?:struct|enum|trait) (\w+)", "-r", "$1"] + CRATES,
        cwd=REPO_ROOT, capture_output=True, text=True, check=False,
    )
    cands: dict[str, list[str]] = defaultdict(list)
    for line in out.stdout.splitlines():
        path, _, name = line.partition(":")
        cands[name].append(path)
    return {
        name: max(paths, key=lambda p: _decl_score(name, p))
        for name, paths in cands.items()
    }


def all_fn_names() -> set[str]:
    """Every `fn` name anywhere in the workspace.

    A vanilla method whose Rust spelling appears nowhere in this set is the strongest
    mechanical gap signal available without reading behaviour: not in the declaring
    file, not on some other type, not in a trait impl, not inlined under its own name.
    """
    out = subprocess.run(
        ["rg", "--no-filename", "-g", "*.rs", "-o", r"\bfn\s+(\w+)", "-r", "$1"] + CRATES,
        cwd=REPO_ROOT, capture_output=True, text=True, check=False,
    )
    return set(out.stdout.split())


def fns_in_file(path: Path) -> set[str]:
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return set()
    return set(re.findall(r"\bfn\s+(\w+)", text))


def main() -> int:
    catalog_path = HERE / "catalog.json"
    if not catalog_path.exists():
        print("catalog.json missing -- run enumerate.py then map_coverage.py", file=sys.stderr)
        return 1
    catalog = json.loads(catalog_path.read_text())

    decls = build_decl_index()
    all_fns = all_fn_names()
    print(f"indexed {len(decls)} rust type declarations, {len(all_fns)} distinct fn names")

    rows = []
    unresolved = 0
    for unit in catalog["units"]:
        if unit["coverage"] == "not_covered":
            continue
        matched = unit.get("pumpkin_matched_name")
        ref = unit.get("pumpkin_ref") or (decls.get(matched) if matched else None)
        if not ref:
            unresolved += 1
            continue
        fns = fns_in_file(REPO_ROOT / ref)
        if not fns:
            unresolved += 1
            continue

        in_file, elsewhere, absent, client_only = [], [], [], []
        for m in unit["methods"]:
            if m in CLIENT_ONLY_METHODS:
                client_only.append(m)
                continue
            aliases = method_aliases(m)
            if aliases & fns:
                in_file.append(m)
            elif aliases & all_fns:
                elsewhere.append(m)
            else:
                absent.append(m)

        rows.append({
            "subsystem": unit["subsystem"],
            "vanilla_class": unit["vanilla_class"],
            "vanilla_path": unit["vanilla_path"],
            "rust_file": ref,
            "total_methods": len(unit["methods"]),
            "in_declaring_file": len(in_file),
            "elsewhere_in_workspace": len(elsewhere),
            "client_only_skipped": len(client_only),
            "absent_workspace_wide": absent,
        })

    # Rank by workspace-wide absences: those are the leads worth reading first.
    rows.sort(key=lambda r: (-len(r["absent_workspace_wide"]), r["vanilla_class"]))

    total_m = sum(r["total_methods"] for r in rows)
    total_present = sum(r["in_declaring_file"] + r["elsewhere_in_workspace"] for r in rows)
    total_absent = sum(len(r["absent_workspace_wide"]) for r in rows)
    by_sub: dict[str, list[int]] = defaultdict(lambda: [0, 0])
    for r in rows:
        by_sub[r["subsystem"]][0] += r["in_declaring_file"] + r["elsewhere_in_workspace"]
        by_sub[r["subsystem"]][1] += r["total_methods"]

    report = {
        "milestone": catalog["milestone"],
        "generated_by": "conformance/method_gaps.py",
        "method": (
            "mechanical name match of vanilla methods against `fn` names in the single Rust "
            "file declaring the matched type. Leads, not proven gaps -- see module docstring "
            "for false-positive sources."
        ),
        "classes_analysed": len(rows),
        "classes_unresolved": unresolved,
        "methods_total": total_m,
        "methods_name_matched": total_present,
        "methods_absent_workspace_wide": total_absent,
        "name_match_pct": round(100 * total_present / total_m, 2) if total_m else 0.0,
        "by_subsystem": {
            k: {"matched": v[0], "total": v[1],
                "pct": round(100 * v[0] / v[1], 2) if v[1] else 0.0}
            for k, v in sorted(by_sub.items())
        },
        "classes": rows,
    }
    out_path = HERE / "method_gaps.json"
    out_path.write_text(json.dumps(report, indent=1))

    print(f"analysed {len(rows)} covered classes ({unresolved} unresolved to a Rust file)")
    print(f"method name-match (file or workspace): {total_present}/{total_m} = "
          f"{report['name_match_pct']}%")
    print(f"absent workspace-wide (the actionable leads): {total_absent}")
    for k, v in report["by_subsystem"].items():
        print(f"  {k}: {v['matched']}/{v['total']} = {v['pct']}%")
    print("\ntop 15 classes by workspace-wide absent methods:")
    for r in rows[:15]:
        print(f"  {r['vanilla_class']:<32} absent {len(r['absent_workspace_wide']):<4}"
              f"of {r['total_methods']:<4} {r['rust_file']}")
    print(f"\n-> {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
