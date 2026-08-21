// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Weak};

use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::potion::Effect;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        axolotl_play_dead::AxolotlPlayDeadGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal,
        non_tame_random_target::NonTameRandomTargetGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    player::Player,
};
use crate::world::World;

/// Vanilla `data/minecraft/tags/entity_type/axolotl_hunt_targets.json` +
/// `axolotl_always_hostiles.json`, the type list consulted by `AxolotlAttackablesSensor`.
const AXOLOTL_TARGET_TYPES: &[&EntityType] = &[
    &EntityType::DROWNED,
    &EntityType::GUARDIAN,
    &EntityType::ELDER_GUARDIAN,
    &EntityType::TROPICAL_FISH,
    &EntityType::PUFFERFISH,
    &EntityType::SALMON,
    &EntityType::COD,
    &EntityType::SQUID,
    &EntityType::GLOW_SQUID,
    &EntityType::TADPOLE,
];

/// Vanilla `AxolotlAttackablesSensor.isMatchingEntity`: `mob.isInWater()`.
///
/// The sensor's `HAS_HUNTING_COOLDOWN` gate (a 2400-tick cooldown on re-targeting hunt-tag prey
/// after leaving the FIGHT activity) is not ported -- axolotls here are always willing to retarget
/// prey, so they hunt slightly more eagerly than vanilla after a fight ends.
async fn axolotl_attackable(
    target: crate::entity::ai::target_predicate::TargetData,
    _world: Arc<World>,
) -> bool {
    target.touching_water
}

/// `Axolotl.Variant` (`Axolotl.java:624-629`).
///
/// The id is what `DATA_VARIANT` and the `Variant` NBT tag both carry; vanilla's `common` flag
/// marks the four naturally spawning colours, leaving blue as the breeding-only rare
/// (`getSpawnVariant`, `Axolotl.java:671-674`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AxolotlVariant {
    Lucy = 0,
    Wild = 1,
    Gold = 2,
    Cyan = 3,
    Blue = 4,
}

/// `Axolotl.Variant.getSpawnVariant(random, true)`: a uniform pick over the common colours.
const COMMON_VARIANTS: [AxolotlVariant; 4] = [
    AxolotlVariant::Lucy,
    AxolotlVariant::Wild,
    AxolotlVariant::Gold,
    AxolotlVariant::Cyan,
];

impl AxolotlVariant {
    /// `Axolotl.Variant.DEFAULT` (`Axolotl.java:631`).
    pub const DEFAULT: Self = Self::Lucy;

    /// `Axolotl.Variant.byId`, whose `ByIdMap.OutOfBoundsStrategy.ZERO` maps anything unknown
    /// back to `LUCY`.
    #[must_use]
    pub const fn by_id(id: i32) -> Self {
        match id {
            1 => Self::Wild,
            2 => Self::Gold,
            3 => Self::Cyan,
            4 => Self::Blue,
            _ => Self::Lucy,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }
}

/// Represents an Axolotl, a passive aquatic mob that can play dead to regenerate health.
///
/// Wiki: <https://minecraft.wiki/w/Axolotl>
///
/// Colour variants (`Axolotl.DATA_VARIANT`, `Axolotl.java:83`; `getVariant`/`setVariant`,
/// `Axolotl.java:281-285`) are carried here as a plain atomic, synced through `DATA_VARIANT` and
/// round-tripped through the `Variant` NBT tag (`Axolotl.java:139-150`).
///
/// Two halves of vanilla's variant handling are NOT ported, both for want of a hook rather than
/// by choice:
/// - `finalizeSpawn`'s `AxolotlGroupData` (`Axolotl.java:160-183`) picks two common colours per
///   spawn group and gives every member of that group one of the two. Pumpkin has no
///   `finalizeSpawn`/spawn-group-data hook, so each axolotl rolls its own common colour in
///   `new()` -- naturally spawned groups here are more varied than vanilla's.
/// - `getBreedOffspring`'s inheritance and the 1-in-1200 rare-blue roll
///   (`Axolotl.java:342-357`, `useRareVariant`, `Axolotl.java:309-311`). `AxolotlEntity`
///   implements neither `Animal` nor `AgeableMob`, so it cannot breed at all yet; blue axolotls
///   are therefore unobtainable, since vanilla never spawns them naturally. Wiring this is a
///   follow-up on the missing `Animal` impl, not on the variant field.
pub struct AxolotlEntity {
    pub mob_entity: MobEntity,
    /// `Axolotl.DATA_VARIANT` (`Axolotl.java:83`), stored as the variant's id.
    variant: AtomicI32,
}

impl AxolotlEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        // See the struct doc: stands in for `finalizeSpawn`. An axolotl loaded from disk
        // overwrites this in `read_nbt_non_mut`, so the roll is harmless for loaded ones.
        let variant = COMMON_VARIANTS[rand::rng().random_range(0..COMMON_VARIANTS.len())];
        let axolotl = Self {
            mob_entity,
            variant: AtomicI32::new(variant.id()),
        };
        let mob_arc = Arc::new(axolotl);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(AxolotlPlayDeadGoal::new()));
            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            // Vanilla `MeleeAttack.create(20)`: 20-tick attack cooldown, matched by
            // `MeleeAttackGoal`'s fixed `attack_interval_ticks`.
            goal_selector.add_goal(2, Box::new(MeleeAttackGoal::new(1.0, true)));
            goal_selector.add_goal(3, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                4,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(5, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc.mob_entity.target_selector.lock().unwrap();
            target_selector.add_goal(
                1,
                NonTameRandomTargetGoal::new(
                    &mob_arc.mob_entity,
                    AXOLOTL_TARGET_TYPES,
                    false,
                    Some(axolotl_attackable),
                ),
            );
        };

        mob_arc
    }

    /// Vanilla `Axolotl.applySupportingEffects`: grants the player Regeneration (topping the
    /// duration up to 2400 ticks) and clears Mining Fatigue.
    async fn apply_supporting_effects(player: &Player) {
        let living = &player.living_entity;
        let existing = living.get_effect(&StatusEffect::REGENERATION).await;
        // Vanilla: `regenEffect == null || regenEffect.endsWithin(2399)`.
        let should_apply = existing
            .as_ref()
            .is_none_or(|effect| effect.duration <= 2399);
        if should_apply {
            let previous_duration = existing.map_or(0, |effect| effect.duration);
            // Vanilla: `Math.min(2400, 100 + previousDuration)`.
            let regen_duration = (100 + previous_duration).min(2400);
            living
                .add_effect(Effect {
                    effect_type: &StatusEffect::REGENERATION,
                    duration: regen_duration,
                    amplifier: 0,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                })
                .await;
        }
        living.remove_effect(&StatusEffect::MINING_FATIGUE).await;
    }

    /// `Axolotl.getVariant` (`Axolotl.java:281-283`).
    #[must_use]
    pub fn variant(&self) -> AxolotlVariant {
        AxolotlVariant::by_id(self.variant.load(Relaxed))
    }

    /// `Axolotl.setVariant` (`Axolotl.java:285-287`).
    pub fn set_variant(&self, variant: AxolotlVariant) {
        self.variant.store(variant.id(), Relaxed);
        self.get_entity().send_meta_data(
            &[Metadata::new(tracked_data::axolotl::VARIANT, variant.id())],
            None,
        );
    }
}

impl NBTStorage for AxolotlEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            // `Axolotl.addAdditionalSaveData` (`Axolotl.java:139-143`). `FromBucket` is not
            // written: nothing here sets it, since axolotl bucketing lives in `item/`.
            nbt.put_int("Variant", self.variant.load(Relaxed));
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            // `Axolotl.readAdditionalSaveData` (`Axolotl.java:146-150`): defaults to LUCY.
            let variant = nbt
                .get_int("Variant")
                .map_or(AxolotlVariant::DEFAULT, AxolotlVariant::by_id);
            self.variant.store(variant.id(), Relaxed);
        })
    }
}

impl Mob for AxolotlEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.get_entity().send_meta_data(
                &[Metadata::new(
                    tracked_data::axolotl::VARIANT,
                    self.variant.load(Relaxed),
                )],
                None,
            );
        })
    }

    /// Vanilla `Axolotl.onStopAttacking`: when a hit kills the target and the target's last
    /// damage source was a player within 20 blocks of this axolotl, that player gets a combat
    /// support buff. Simplification: vanilla checks `body.getBoundingBox().inflate(20.0)`
    /// (an AABB); this uses a plain 20-block spherical distance check instead.
    fn on_successful_attack<'a>(&'a self, target: &'a dyn EntityBase) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let Some(target_living) = target.get_living_entity() else {
                return;
            };
            if target_living.entity.is_alive() {
                return;
            }

            let attacker_id = target_living.last_attacker_id.load(Relaxed);
            let world = self.mob_entity.living_entity.entity.world.load();
            let Some(attacker) = world.get_entity_by_id(attacker_id) else {
                return;
            };
            let Some(player) = attacker.get_player() else {
                return;
            };

            let axolotl_pos = self.mob_entity.living_entity.entity.pos.load();
            let player_pos = player.get_entity().pos.load();
            if axolotl_pos.squared_distance_to_vec(&player_pos) > 20.0 * 20.0 {
                return;
            }

            Self::apply_supporting_effects(player).await;
        })
    }
}
