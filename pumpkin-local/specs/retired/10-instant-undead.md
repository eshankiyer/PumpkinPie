WORKTREE: /home/eshanki/pumpkin-wt/instantundead
FILES: pumpkin/src/entity/living.rs:463-495, pumpkin/src/entity/player.rs:950-980
CRATES: pumpkin
FUNC: pub async fn add_effect(&self, effect: Effect) {
---
Instant Health and Instant Damage ignore whether the target is undead.

`LivingEntity::add_effect` in living.rs heals on Instant Health and damages on
Instant Damage, unconditionally:

    if effect.effect_type == &StatusEffect::INSTANT_HEALTH {
        let heal_amount = 4.0 * (1 << effect.amplifier) as f32;
        self.heal(heal_amount);
    } else if effect.effect_type == &StatusEffect::INSTANT_DAMAGE {

Vanilla inverts both for undead mobs: Instant Health damages the undead, and
Instant Damage heals them. This is what makes a splash Potion of Healing a weapon
against zombies and skeletons.

There is already an undead classification in this repo. player.rs computes one
for the Smite enchantment, around line 958:

    let is_undead = target_type == EntityType::ZOMBIE.id

Reuse that. Lift it into a shared helper the two call sites can both use, and
call it from the instant-effect branch in living.rs to swap heal and damage.

Do not invent a second, differently-populated undead list. Two lists that
disagree is worse than one incomplete list, because the Smite damage bonus and
the potion inversion would then apply to different sets of mobs and nobody would
notice until a bug report.

Do not change the damage amounts. `4 << amplifier` and `6 << amplifier` are
correct; only which of heal or damage runs changes.

Keep the change inside these two files. The effect tick loop and the potion item
do not need to know about this.

Clippy denies all, pedantic, nursery and cargo. Any identifier named in a doc
comment needs backticks. No emoji, no em-dashes.
