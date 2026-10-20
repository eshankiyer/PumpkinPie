BLOCKED 2026-07-28: the premise is false. This spec tells the model the damage
type "should be reachable from the arguments this function already receives."
It is not: `damage_armor_items(&self, caller: &dyn EntityBase, damage_amount:
f32)` carries no DamageType and the trait exposes no way to get one. The model
said exactly this in its CANNOT and was right. The real fix threads DamageType
through from the caller, which widens the signature: a small human/Claude PR,
not a single-function edit. Do not requeue until the spec targets that wider
change or the pipeline grows a mode for it.

WORKTREE: /home/eshanki/pumpkin-wt/anvilhelm
FILES: pumpkin/src/entity/living.rs:1720-1760
CRATES: pumpkin
FUNC: async fn damage_armor_items(&self, caller: &dyn EntityBase, damage_amount: f32) {
---
# Bug: falling anvil damage wears out every armour piece, not just the helmet

You are shown `damage_armor_items`. It applies `floor(damage / 4)` durability
damage to every slot in `armor_slots`, with no check on the damage type. There
are TODO comments directly above the loop saying the falling anvil and
stalactite cases should only damage the helmet.

VERIFIED VANILLA: only the helmet protects against, and is damaged by, falling
anvil and falling stalactite damage.
Source: https://minecraft.wiki/w/Anvil (Helmet section)

# Change

Gate the loop so that when the incoming damage type is the falling-anvil or
falling-stalactite type, only the helmet slot takes durability damage. Every
other damage type keeps the current all-slots behaviour.

The damage type should be reachable from the arguments this function already
receives. Look at what is passed in and at how other functions in this file
identify a damage type; there is a `DamageType` enum in `pumpkin_data`.

Replace the TODO comments with a comment naming the rule:
    // Vanilla only lets the helmet absorb, and take durability from, falling
    // anvil and stalactite damage.

# Do not

- Do not change the `floor(damage / 4)` amount.
- Do not change the unenchanted/enchanted handling or anything about
  protection calculations.
- Do not touch any other function.
- IMPORTANT: if the damage type is NOT available in this function's arguments,
  do not thread a new parameter through callers. Stop and report that instead.
  Widening the signature is a design decision I want to make, not you.
