WORKTREE: /home/eshanki/pumpkin-wt/knockback
FILES: pumpkin/src/entity/combat.rs:25-62
CRATES: pumpkin
FUNC: pub async fn new(player: &Player, attack_cooldown_progress: f32) -> Self {
ASSERT_PRESENT: BLINDNESS
ASSERT_ABSENT: if is_strong && !on_ground && fall_distance > 0.0 {
---
# Bug: critical hits are granted in situations vanilla forbids

You are shown `AttackType::new`. The critical branch tests only:

    is_strong && !on_ground && fall_distance > 0.0

VERIFIED VANILLA additionally forbids a critical hit when the attacker is:
- in water / swimming
- on a ladder or vine (climbing)
- under the Blindness effect
- riding as a passenger

Sources: https://minecraft.wiki/w/Damage and https://minecraft.wiki/w/Blindness

# Change

Extend the critical-hit condition with those four exclusions, using accessors
that already exist on `Player` or its `LivingEntity`. Before writing anything,
find the real accessor names in the codebase; likely candidates are something
like `is_touching_water`, `is_climbing`, `has_effect(StatusEffect::BLINDNESS)`
and a vehicle or passenger check. Use what actually exists.

Add a comment naming the rule:
    // Vanilla also forbids crits while swimming, climbing, blinded, or riding.

# Do not

- Do not change the existing `is_strong`, `!on_ground` or `fall_distance` parts.
- Do not touch the sweeping-attack or knockback branches in the same function.
- IMPORTANT: if any of the four states has no accessor in this codebase, apply
  only the ones that do, and REPORT clearly which you had to skip and why. Do
  not invent a state tracker or add a field to Player. Partial and honest beats
  complete and fabricated.
