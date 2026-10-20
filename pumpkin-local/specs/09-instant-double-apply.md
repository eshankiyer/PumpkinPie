WORKTREE: /home/eshanki/pumpkin-wt/instantdouble
FILES: pumpkin/src/item/potion.rs:115-186, pumpkin/src/entity/living.rs:463-490
CRATES: pumpkin
FUNC: pub async fn apply_effects_to(
ASSERT_ABSENT: let amount = (4 * ((amplifier as i32) + 1)) as f32 * instant_scale;
ASSERT_ABSENT: let amount = (6 * ((amplifier as i32) + 1)) as f32 * instant_scale;
---
Instant Health and Instant Damage are applied twice, on two different formulas.

`apply_effects` in potion.rs detects an instant effect and heals or damages
directly:

    let amount = (4 * ((amplifier as i32) + 1)) as f32 * instant_scale;
    target.heal(amount);

then falls through and calls `target.add_effect(eff)` with the same instant
effect type, and `LivingEntity::add_effect` in living.rs heals or damages a
second time:

    if effect.effect_type == &StatusEffect::INSTANT_HEALTH {
        let heal_amount = 4.0 * (1 << effect.amplifier) as f32;
        self.heal(heal_amount);

So drinking a Potion of Healing II heals `4 * 3 = 12` from the first path and
`4 * 4 = 16` from the second, for 28 total.

Vanilla applies the effect exactly once, and the amplifier is a power of two:
`4 << amplifier` for Instant Health and `6 << amplifier` for Instant Damage.
The living.rs path already implements that correctly. The potion.rs path does
not, and it is also the duplicate.

Remove the direct heal and damage calls from potion.rs, so instant effects take
the single `add_effect` path like every other effect. Keep the `is_instant`
branch itself, because it still has a job: it sets `duration: 1` and it applies
the area-effect-cloud potency halving. That halving must survive, so scale the
value passed onward rather than deleting `instant_scale` outright; if the only
way to preserve it is to leave the scale computation in place and unused, do not
do that, because clippy denies unused variables. Thread it or drop it cleanly.

Do not change the formulas in living.rs. They match vanilla and are not the bug.

Do not "fix" this by deleting the living.rs branch instead. That branch is
reached by every other caller of `add_effect`, including tipped arrows and
lingering clouds, and deleting it would silently disable instant effects for all
of them.

Clippy denies all, pedantic, nursery and cargo. No emoji, no em-dashes.
