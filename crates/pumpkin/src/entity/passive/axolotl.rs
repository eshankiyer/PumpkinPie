// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Weak};

use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::boundingbox::EntityDimensions;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        axolotl_play_dead::AxolotlPlayDeadGoal, breed::BreedGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, non_tame_random_target::NonTameRandomTargetGoal,
        swim::SwimGoal, tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::{Animal, fill_water_bucket_result},
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AxolotlVariant {
    Lucy = 0,
    Wild = 1,
    Gold = 2,
    Cyan = 3,
    Blue = 4,
}

/// `Axolotl.Variant.getSpawnVariant(random, true)`: a uniform pick over the common colours.
pub(crate) const COMMON_VARIANTS: [AxolotlVariant; 4] = [
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

    /// `Axolotl.Variant.getCommonSpawnVariant` (`Axolotl.java:664-666`).
    pub(crate) fn random_common() -> Self {
        COMMON_VARIANTS[rand::rng().random_range(0..COMMON_VARIANTS.len())]
    }

    /// The `Axolotl.Variant` serialized names (`lucy`, `wild`, `gold`, `cyan`, `blue`), with an
    /// optional `minecraft:` prefix as spawn-egg components carry it.
    fn from_name(name: &str) -> Option<Self> {
        match name.strip_prefix("minecraft:").unwrap_or(name) {
            "lucy" => Some(Self::Lucy),
            "wild" => Some(Self::Wild),
            "gold" => Some(Self::Gold),
            "cyan" => Some(Self::Cyan),
            "blue" => Some(Self::Blue),
            _ => None,
        }
    }
}

/// `Axolotl.isFood` is `#minecraft:axolotl_food` (`Axolotl.java:360-362`), whose only member is
/// the tropical fish bucket; `FollowTemptation` uses the same items.
const TEMPT_ITEMS: &[&Item] = &[&Item::TROPICAL_FISH_BUCKET];

/// `Axolotl.useRareVariant` (`Axolotl.java:309-311`): one bred baby in 1200 is blue.
const RARE_VARIANT_CHANCE: u32 = 1200;

/// `Axolotl.AxolotlGroupData.getVariant` plus the baby half of `Axolotl.finalizeSpawn`
/// (`Axolotl.java:160-183, 585-596`) for one natural-spawn group member. The first member of a
/// group creates the pair of common colours; `group_size` is the number of members already
/// finalized, so the third and later ones are babies. Returns `(variant, is_baby)`.
fn natural_group_spawn(
    group: &mut Option<[AxolotlVariant; 2]>,
    group_size: i32,
    pick: usize,
) -> (AxolotlVariant, bool) {
    let (pair, is_baby) = if let Some(pair) = group {
        (*pair, group_size >= 2)
    } else {
        let pair = [
            AxolotlVariant::random_common(),
            AxolotlVariant::random_common(),
        ];
        *group = Some(pair);
        (pair, false)
    };
    (pair[pick], is_baby)
}

/// Represents an Axolotl, a passive aquatic mob that can play dead to regenerate health.
///
/// Wiki: <https://minecraft.wiki/w/Axolotl>
///
/// Colour variants (`Axolotl.DATA_VARIANT`, `Axolotl.java:83`; `getVariant`/`setVariant`,
/// `Axolotl.java:281-285`) are carried here as a plain atomic, synced through `DATA_VARIANT` and
/// round-tripped through the `Variant` NBT tag (`Axolotl.java:139-150`).
///
/// Variant selection:
/// - Natural spawns go through [`AxolotlEntity::finalize_natural_spawn`], the port of
///   `finalizeSpawn`'s `AxolotlGroupData` (`Axolotl.java:160-183`): each spawn group shares two
///   common colours, and its third and later members are babies. Every other spawn path keeps the
///   uniform common-colour roll made in `new()`, which is what fresh group data gives vanilla.
/// - Breeding (`getBreedOffspring`, `Axolotl.java:342-357`) inherits a parent's colour, or is
///   blue with a 1-in-1200 chance (`useRareVariant`, `Axolotl.java:309-311`).
///
/// Feeding plays `entity.axolotl.idle_water` through `animal_interact`, like every other
/// `Animal` here; vanilla's axolotl inherits the empty `Animal.playEatingSound`
/// (`Animal.java:165-166`) and is silent, so that sound is a known divergence.
pub struct AxolotlEntity {
    pub mob_entity: MobEntity,
    /// `Axolotl.DATA_VARIANT` (`Axolotl.java:83`), stored as the variant's id.
    variant: AtomicI32,
    pub ageable_data: AgeableData,
}

impl AxolotlEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        // See the struct doc: stands in for `finalizeSpawn`. An axolotl loaded from disk
        // overwrites this in `read_nbt_non_mut`, so the roll is harmless for loaded ones.
        let variant = AxolotlVariant::random_common();
        let axolotl = Self {
            mob_entity,
            variant: AtomicI32::new(variant.id()),
            ageable_data: AgeableData::default(),
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
            // `AxolotlAi.initIdleActivity` (`AxolotlAi.java:91-104`): `AnimalMakeLove(0.2F)` at 1
            // outranks the `FollowTemptation`/`BabyFollowAdult` pair at 2 (in-water speeds 0.5
            // and 0.6). Both sit below the attack, since FIGHT replaces IDLE while there is a
            // target.
            goal_selector.add_goal(3, BreedGoal::new(0.2));
            goal_selector.add_goal(4, Box::new(TemptGoal::new(0.5, TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(0.6)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));

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

    /// `Axolotl.finalizeSpawn` (`Axolotl.java:160-183`) for a natural spawn, with `group` the
    /// spawn group's `AxolotlGroupData` colours (reset per group by the caller) and `group_size`
    /// the members of this group finalized so far. Runs before the entity is spawned, so
    /// `mob_init_data_tracker` syncs the result.
    pub(crate) fn finalize_natural_spawn(
        &self,
        group: &mut Option<[AxolotlVariant; 2]>,
        group_size: i32,
    ) {
        let (variant, is_baby) =
            natural_group_spawn(group, group_size, rand::rng().random_range(0..2));
        self.variant.store(variant.id(), Relaxed);
        if is_baby {
            self.set_baby(true);
        }
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
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
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
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
        })
    }
}

impl AgeableMob for AxolotlEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// `Axolotl.BABY_DIMENSIONS` (`Axolotl.java:113-115`).
    fn baby_dimensions(&self) -> Option<EntityDimensions> {
        Some(EntityDimensions::new(0.375, 0.21, 0.09375))
    }
}

impl Animal for AxolotlEntity {
    fn as_ageable_mob(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    /// `Axolotl.isFood` (`Axolotl.java:360-362`).
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_AXOLOTL_FOOD)
    }

    /// `Axolotl.usePlayerItem` (`Axolotl.java:545-551`): the tropical fish bucket leaves a water
    /// bucket behind.
    fn animal_use_player_item<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if item_stack.item == &Item::TROPICAL_FISH_BUCKET {
                fill_water_bucket_result(player, item_stack).await;
            } else {
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            }
        })
    }
}

impl Mob for AxolotlEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Sends the variant, plus the baby flag the `Mob` default would have sent.
    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            entity.send_meta_data(
                &[Metadata::new(
                    tracked_data::axolotl::VARIANT,
                    self.variant.load(Relaxed),
                )],
                None,
            );
            if entity.age.load(Relaxed) < 0 {
                entity.send_meta_data(
                    &[Metadata::new(tracked_data::ageable_mob::DATA_BABY_ID, true)],
                    None,
                );
            }
        })
    }

    /// `Axolotl.applyImplicitComponent` for `AXOLOTL_VARIANT` (`Axolotl.java:300-307`).
    fn mob_set_variant_name(&self, name: &str) {
        if let Some(variant) = AxolotlVariant::from_name(name) {
            self.variant.store(variant.id(), Relaxed);
        }
    }

    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.ageable_ai_step();
        })
    }

    /// `Axolotl.mobInteract` (`Axolotl.java:429-431`). The bucket pickup that vanilla tries first
    /// runs from the water bucket's own entity use when this returns false, and a water bucket is
    /// never axolotl food, so the order is the same.
    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        self.animal_interact(player, item_stack, Sound::EntityAxolotlIdleWater)
    }

    /// `Axolotl.getBreedOffspring` (`Axolotl.java:342-357`). `BreedGoal` marks the baby
    /// persistent and sets its baby age; breeding never reaches `finalizeSpawn`.
    fn create_offspring<'a>(
        &'a self,
        mate: &'a dyn EntityBase,
        world: &'a Arc<World>,
    ) -> EntityBaseFuture<'a, Option<Arc<dyn EntityBase>>> {
        Box::pin(async move {
            let entity = self.get_entity();
            let baby = crate::entity::r#type::from_type(
                entity.entity_type,
                entity.pos.load(),
                world,
                uuid::Uuid::new_v4(),
            );
            let mut rng = rand::rng();
            let variant = if rng.random_range(0..RARE_VARIANT_CHANCE) == 0 {
                // `getRareSpawnVariant`: blue is the only non-common colour.
                AxolotlVariant::Blue
            } else if rng.random_bool(0.5) {
                self.variant()
            } else {
                mate.cast_any()
                    .downcast_ref::<Self>()
                    .map_or_else(|| self.variant(), Self::variant)
            };
            if let Some(child) = baby.cast_any().downcast_ref::<Self>() {
                child.variant.store(variant.id(), Relaxed);
            }
            Some(baby)
        })
    }

    /// `Axolotl.getAmbientSound` (`Axolotl.java:512-515`) with the `playAmbientSound` gate
    /// (`Axolotl.java:152-157`): silent while playing dead. `AxolotlPlayDeadGoal` marks the
    /// play-dead window with `not_targetable_as_enemy`, the mirror of `canBeSeenAsEnemy`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        let living = &self.mob_entity.living_entity;
        if living.not_targetable_as_enemy.load(Relaxed) {
            return None;
        }
        Some(if living.entity.touching_water.load(Relaxed) {
            Sound::EntityAxolotlIdleWater
        } else {
            Sound::EntityAxolotlIdleAir
        })
    }

    /// `Axolotl.isPushedByFluid` (`Axolotl.java:318-321`).
    fn mob_is_pushed_by_fluids(&self) -> bool {
        false
    }

    /// `Axolotl.travelInWater` (`Axolotl.java:537-542`): `moveRelative(getSpeed(), input)`, move,
    /// then a flat 0.9 drag, replacing the generic water friction, gravity and jump-out logic.
    /// Outside water the generic travel path applies.
    fn custom_travel<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let living = &self.mob_entity.living_entity;
            let entity = &living.entity;
            if !entity.touching_water.load(Relaxed) {
                return false;
            }
            entity.update_velocity_from_input(living.movement_input.load(), living.speed.load());
            entity.move_entity(caller, entity.velocity.load()).await;
            entity.velocity.store(entity.velocity.load() * 0.9);
            true
        })
    }

    /// Vanilla `Axolotl.onStopAttacking`: when a hit kills the target and the target's last
    /// damage source was a player within 20 blocks of this axolotl, that player gets a combat
    /// support buff. Simplification: vanilla checks `body.getBoundingBox().inflate(20.0)`
    /// (an AABB); this uses a plain 20-block spherical distance check instead.
    fn on_successful_attack<'a>(&'a self, target: &'a dyn EntityBase) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            // `Mob.doHurtTarget` -> `Axolotl.playAttackSound` (`Axolotl.java:397-400`).
            let entity = self.get_entity();
            entity.world.load().play_sound_fine(
                Sound::EntityAxolotlAttack,
                self.get_sound_source(),
                &entity.pos.load(),
                1.0,
                1.0,
            );

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

#[cfg(test)]
mod tests {
    use super::{AxolotlVariant, COMMON_VARIANTS, TEMPT_ITEMS, natural_group_spawn};
    use pumpkin_data::tag::{self, Taggable};

    #[test]
    fn tempt_items_match_axolotl_food_tag() {
        // Vanilla tempts with `Axolotl.isFood`, i.e. `#minecraft:axolotl_food`.
        for item in TEMPT_ITEMS {
            assert!(
                item.has_tag(&tag::Item::MINECRAFT_AXOLOTL_FOOD),
                "{} is not in #minecraft:axolotl_food",
                item.registry_key
            );
        }
        assert_eq!(TEMPT_ITEMS.len(), tag::Item::MINECRAFT_AXOLOTL_FOOD.0.len());
    }

    #[test]
    fn natural_group_shares_two_colours_and_babies_from_third() {
        let mut group = None;
        let (first, first_baby) = natural_group_spawn(&mut group, 0, 0);
        let pair = group.expect("first member creates the group data");
        assert_eq!(first, pair[0]);
        assert!(!first_baby);
        assert!(pair.iter().all(|v| COMMON_VARIANTS.contains(v)));

        let (second, second_baby) = natural_group_spawn(&mut group, 1, 1);
        assert_eq!(second, pair[1]);
        assert!(!second_baby);
        assert_eq!(group, Some(pair));

        let (_, third_baby) = natural_group_spawn(&mut group, 2, 0);
        assert!(third_baby);
    }

    #[test]
    fn variant_names_include_blue_and_prefix() {
        assert_eq!(
            AxolotlVariant::from_name("minecraft:blue"),
            Some(AxolotlVariant::Blue)
        );
        assert_eq!(AxolotlVariant::from_name("cyan"), Some(AxolotlVariant::Cyan));
        assert_eq!(AxolotlVariant::from_name("pink"), None);
    }
}
