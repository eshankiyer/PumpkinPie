// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use pumpkin_data::BlockDirection;
use pumpkin_data::item::Item;
use pumpkin_data::particle::Particle;
use pumpkin_data::potion::Effect;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_inventory::build_equipment_slots;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::InventoryPlayer;
use pumpkin_protocol::bedrock::client::take_item_actor::CTakeItemActor;
use pumpkin_protocol::bedrock::server::actor_event::{ActorEventType, SActorEvent};
use pumpkin_protocol::codec::var_ulong::VarULong;
use pumpkin_util::GameMode;
use pumpkin_util::Hand;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::atomic::{
    AtomicBool, AtomicU8, AtomicU32, AtomicU64,
    Ordering::{Relaxed, SeqCst},
};
use std::{collections::HashMap, sync::atomic::AtomicI32};
use tracing::warn;

use super::experience_orb::ExperienceOrbEntity;
use super::{Entity, EntityBase, NBTStorage, NBTStorageInit};
use crate::block::OnLandedUponArgs;
use crate::entity::attributes::AttributeInstance;
use crate::entity::attributes::Modifier;
use crate::entity::attributes::ModifierOperation;
use crate::entity::combat::{breach_armor_fraction, knockback_after_resistance};
use crate::entity::mob::Mob;
use crate::entity::mob::equipment::DEFAULT_EQUIPMENT_DROP_CHANCE;
use crate::entity::mob::slime::SlimeEntity;
use crate::entity::mob::sulfur_cube::SulfurCubeEntity;
use crate::entity::passive::happy_ghast::HappyGhastEntity;
use crate::entity::player::Player;
use crate::entity::player::statistics::{CustomStatistic, StatisticCategory};
use crate::entity::{EntityBaseFuture, NbtFuture};
use crate::server::Server;
use crate::world::World;
use crate::world::loot::{LootContextParameters, LootTableExt};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DeathMessageType;
use pumpkin_data::data_component_impl::Operation;
use pumpkin_data::data_component_impl::food::{
    ConsumableImpl, ConsumeAnimation, ConsumeEffect, UseEffectsImpl, UseRemainderImpl,
};
use pumpkin_data::data_component_impl::{
    AttackRangeImpl, AttributeModifiersImpl, BlocksAttacksImpl, DamageResistantImpl,
    DamageResistantType, DeathProtectionImpl, EnchantmentsImpl, EquipmentSlot, EquipmentType,
    EquippableImpl, FoodImpl, GliderImpl, OminousBottleAmplifierImpl, WeaponImpl,
};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::{EntityPose, EntityStatus, EntityType, MobCategory};
use pumpkin_data::item_stack::{DamageResult, ItemStack};
use pumpkin_data::sound::SoundCategory;
use pumpkin_data::{Block, Enchantment, translation};
use pumpkin_data::{damage::DamageType, sound::Sound};
use pumpkin_inventory::entity_equipment::EntityEquipment;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackTemplateSerializer;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::{
    CEntityStatus, CHurtAnimation, CSetPlayerInventory, CTakeItemEntity, CUpdateMobEffect,
};
use pumpkin_protocol::{
    codec::item_stack_seralizer::ItemStackSerializer,
    java::client::play::{CDamageEvent, CSetEquipment, Metadata, MetadataSerializer},
    ser::{NetworkWriteExt, WritingError},
};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;
use pumpkin_util::text::TextComponent;
use rand::{RngExt, SeedableRng, rngs::StdRng};
use std::sync::RwLock;
use tokio::sync::Mutex;
use uuid::Uuid;

fn knockback_strength_with_resistance(strength: f64, resistance: f64) -> f64 {
    strength * (1.0 - resistance.clamp(0.0, 1.0))
}

/// `LivingEntity.handleOnClimbable` keeps a sneaking player from sliding down a ladder, except
/// while inside scaffolding (`LivingEntity.java:2694-2700`).
const fn suppress_climb_descent(is_player: bool, sneaking: bool, in_scaffolding: bool) -> bool {
    is_player && sneaking && !in_scaffolding
}

/// Selects the category used by the shared hurt-sound path. Vanilla's mob override delegates
/// to `makeSound`, which uses the mob sound source (`Mob.java:295-299`; `LivingEntity.java:1427-1434`).
const fn hurt_sound_category(
    is_player: bool,
    mob_sound_source: Option<SoundCategory>,
) -> SoundCategory {
    if is_player {
        SoundCategory::Players
    } else {
        match mob_sound_source {
            Some(category) => category,
            None => SoundCategory::Neutral,
        }
    }
}

/// Vanilla turns a non-finite damage value into the largest finite value before it reaches
/// cooldown, armor, or health calculations (`LivingEntity.hurtServer`). This matters at the
/// plugin boundary too: a malformed replacement amount must not poison persistent combat state.
const fn normalize_non_finite_damage(amount: f32) -> f32 {
    if amount.is_finite() { amount } else { f32::MAX }
}

/// Vanilla records combat damage statistics in tenths of a point using `Math.round`, not a
/// truncating conversion. This applies equally to absorbed and health damage.
fn damage_stat_amount(damage: f32) -> i32 {
    (damage * 10.0).round() as i32
}

/// Vanilla `LivingEntity.doesEmitEquipEvent` defaults true (`LivingEntity.java:685-686`);
/// `Player` restricts it to humanoid armor (`Player.java:1664-1666`).
fn does_emit_equip_event(is_player: bool, slot: &EquipmentSlot) -> bool {
    !is_player || slot.slot_type() == EquipmentType::HumanoidArmor
}

/// Vanilla `Player.causeFallDamage` awards this statistic before delegating to the
/// general fall-damage path. The threshold and rounding intentionally match
/// `Math.round(fallDistance * 100.0)`.
fn fall_one_cm_stat_amount(fall_distance: f32) -> Option<i32> {
    (fall_distance >= 2.0).then_some((fall_distance * 100.0).round() as i32)
}

/// Vanilla `LivingEntity.causeFallDamage` (`LivingEntity.java:1788-1797`) limits fall distance
/// to the vertical distance below the most recent impulse impact position.
fn impulse_limited_fall_distance(fall_distance: f32, entity_y: f64, impact_y: f64) -> f32 {
    fall_distance.min((impact_y - entity_y) as f32)
}

/// Vanilla `LivingEntity.canBeSeenAsEnemy` (`LivingEntity.java:952-958`) combines the base
/// invulnerability/visibility checks with the entity-specific targetability override.
const fn can_be_seen_as_enemy_state(
    invulnerable: bool,
    can_be_seen_by_anyone: bool,
    not_targetable_as_enemy: bool,
) -> bool {
    !invulnerable && can_be_seen_by_anyone && !not_targetable_as_enemy
}

/// Vanilla `Entity.isInLiquid` (`Entity.java:1604-1605`) supplies the fluid-state gate used by
/// `LivingEntity.shouldTravelInFluid` (`LivingEntity.java:2421-2437`); the Strider override from
/// `Strider.canStandOnFluid` (`Strider.java:180-182`) makes lava standable while water is not.
const fn should_travel_in_fluid(
    entity_type: &'static EntityType,
    in_liquid: bool,
    touching_water: bool,
    touching_lava: bool,
) -> bool {
    in_liquid && (touching_water || (touching_lava && entity_type.id != EntityType::STRIDER.id))
}

/// Vanilla `LivingEntity.hasLandedInLiquid` (`LivingEntity.java:404-406`).
const fn has_landed_in_liquid_state(
    velocity_y: f64,
    touching_water: bool,
    touching_lava: bool,
) -> bool {
    velocity_y < 1.0E-5 && (touching_water || touching_lava)
}

/// Applies vanilla's mounted-entity hitbox floor before melee range checks.
/// `LivingEntity.getHitbox` raises the minimum Y to the passenger's riding position
/// (`LivingEntity.java:1692-1700`).
const fn hitbox_with_riding_floor(bounding_box: BoundingBox, riding_y: Option<f64>) -> BoundingBox {
    let Some(riding_y) = riding_y else {
        return bounding_box;
    };

    BoundingBox::new(
        Vector3::new(
            bounding_box.min.x,
            bounding_box.min.y.max(riding_y),
            bounding_box.min.z,
        ),
        bounding_box.max,
    )
}

/// Vanilla `LivingEntity.isDeadOrDying` (`LivingEntity.java:1171-1173`).
const fn dead_or_dying_state(health: f32, dead: bool) -> bool {
    health <= 0.0 || dead
}

fn accumulated_fall_distance_after_impulse(velocity_y: f64, fall_distance: f32) -> f32 {
    if velocity_y > -0.5 && fall_distance > 1.0 {
        1.0
    } else {
        fall_distance
    }
}

/// Builds the default attack range used when an item has no `attack_range` component.
/// `LivingEntity.getAttackRangeWith` uses the entity-interaction attribute for that default
/// (`LivingEntity.java:2230-2233`; `AttackRange.java:55-59`).
const fn default_attack_range(interaction_range: f64) -> AttackRangeImpl {
    let interaction_range = interaction_range as f32;
    AttackRangeImpl {
        min_reach: 0.0,
        max_reach: interaction_range,
        min_creative_reach: 0.0,
        max_creative_reach: interaction_range,
        hitbox_margin: 0.0,
        mob_factor: 1.0,
    }
}

fn armor_resists_damage(stack: &ItemStack, damage_type: &DamageType) -> bool {
    let Some(resistant) = stack.get_data_component::<DamageResistantImpl>() else {
        return false;
    };

    match resistant.res_type {
        DamageResistantType::Fire => damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_FIRE),
        DamageResistantType::Explosion => {
            damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION)
        }
        DamageResistantType::Fall => damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_FALL),
        DamageResistantType::Freezing => {
            damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_FREEZING)
        }
        DamageResistantType::Lightning => {
            damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_LIGHTNING)
        }
        DamageResistantType::Drowning => {
            damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_DROWNING)
        }
        DamageResistantType::Projectile => {
            damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_PROJECTILE)
        }
        DamageResistantType::PlayerAttack => {
            damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_PLAYER_ATTACK)
        }
        DamageResistantType::MaceSmash => {
            damage_type.has_tag(&tag::DamageType::MINECRAFT_MACE_SMASH)
        }
        _ => false,
    }
}

/// Vanilla `LivingEntity.getActiveItem` (`LivingEntity.java:2235-2241`) selects the stored use
/// stack while using an item and otherwise selects the main-hand stack.
fn active_item_for_state(
    using_item: bool,
    item_in_use: Option<&ItemStack>,
    main_hand: &ItemStack,
) -> ItemStack {
    if using_item {
        item_in_use
            .cloned()
            .unwrap_or_else(|| ItemStack::EMPTY.clone())
    } else {
        main_hand.clone()
    }
}

const MAX_AIR_SUPPLY: i32 = 300;

/// A raider's membership in an active `Raid`.
///
/// Mirrors `Raider.wave`/`raid`/`isPatrolLeader` (`Raider.java`). Kept on `LivingEntity` rather
/// than `MobEntity` so the wave-clear/death hook in `on_death` below can read it directly.
#[derive(Clone, Copy)]
pub struct RaidMembership {
    pub raid_id: i32,
    pub wave: i32,
    pub is_patrol_leader: bool,
}

/// Represents a living entity within the game world.
///
/// This struct encapsulates the core properties and behaviors of living entities, including players, mobs, and other creatures.
pub struct LivingEntity {
    /// The underlying entity object, providing basic entity information and functionality.
    pub entity: Entity,
    /// Tracks the remaining time until the entity can regenerate health.
    pub hurt_cooldown: AtomicI32,
    /// Vanilla `LivingEntity.skipDropExperience` (`LivingEntity.java:278`), set through
    /// `skipDropExperience()` (`:1680`) and read by `shouldDropExperience()` in the death
    /// path (`:1527`).
    ///
    /// A sculk catalyst that absorbs a nearby death claims the experience as charge, so the
    /// orb must not also drop (`SculkCatalystBlockEntity.java:80`).
    pub skip_drop_experience: AtomicBool,
    /// Vanilla `Mob.lootTableSeed` is stored here so every concrete mob can use its existing
    /// `LivingEntity` NBT delegation (`Mob.java:383-385, 406-407`).
    pub loot_table_seed: AtomicCell<i64>,
    /// Vanilla `LivingEntity.lastHurtByPlayerMemoryTime`: ticks left in which a death still
    /// counts as a player kill for loot and experience. Set to 100 by any player-sourced damage.
    pub last_hurt_by_player_time: AtomicI32,
    /// Stores the amount of damage the entity last received.
    pub last_damage_taken: AtomicCell<f32>,
    /// Packed `(game_time * 2) + panic_causing` state for the most recent accepted damage.
    ///
    /// The current health level of the entity.
    pub health: AtomicCell<f32>,
    /// The remaining air supply used by vanilla `LivingEntity.baseTick`.
    pub air_supply: AtomicI32,
    /// Vanilla `LivingEntity.getArrowCount`/`setArrowCount` tracked state
    /// (`LivingEntity.java:1994-2000`).
    pub arrow_count: AtomicI32,
    /// Vanilla `LivingEntity.removeArrowTime` (`LivingEntity.java:228`), used by the
    /// server-side arrow-count decay in `LivingEntity.tick` (`LivingEntity.java:2754-2767`).
    remove_arrow_time: AtomicI32,
    pub stinger_count: AtomicI32,
    /// Entity-local random stream used by vanilla's air depletion roll.
    air_random: std::sync::Mutex<StdRng>,
    /// Whether the initial air value has been published to clients.
    air_metadata_initialized: AtomicBool,
    /// The current absorption (yellow hearts) on the entity.
    pub absorption: AtomicCell<f32>,
    pub item_use_time: AtomicI32,
    pub item_in_use: Mutex<Option<ItemStack>>,
    pub active_hand: Mutex<Option<Hand>>,
    /// Vanilla `LivingEntity.autoSpinAttackTicks`.
    pub auto_spin_attack_ticks: AtomicI32,
    pub auto_spin_attack_state: Mutex<()>,
    /// Vanilla `LivingEntity.autoSpinAttackDmg` and `autoSpinAttackItemStack`.
    pub auto_spin_attack_damage: AtomicCell<f32>,
    pub auto_spin_attack_item_stack: Mutex<Option<ItemStack>>,
    pub death_time: AtomicU8,
    /// Vanilla `Entity.moveDist` (Entity.java:238): the scaled distance walked, accumulated by
    /// `Entity.applyMovementEmissionAndPlaySound` (Entity.java:867-901).
    move_dist: AtomicCell<f32>,
    /// Vanilla `Entity.nextStep` (Entity.java:241): the `moveDist` value at which the next step
    /// or swim sound fires, advanced by `Entity.nextStep` (Entity.java:1259-1261).
    next_step: AtomicCell<f32>,
    /// Indicates whether the entity is dead. (`on_death` called)
    pub dead: AtomicBool,
    /// The distance the entity has been falling.
    pub fall_distance: AtomicCell<f32>,
    /// Vanilla `LivingEntity.currentImpulseContextResetGraceTime` and
    /// `currentImpulseImpactPos` (`LivingEntity.java:211, 284`).
    post_impulse_context_reset_grace_time: AtomicI32,
    current_impulse_impact_pos: AtomicCell<Option<Vector3<f64>>>,
    pub active_effects: Mutex<HashMap<&'static StatusEffect, Effect>>,
    /// Vanilla `MobEffectInstance.hiddenEffect`: instances of an active effect that were taken
    /// over by a stronger or longer one and are restored when it runs out. Nearest first.
    pub hidden_effects: Mutex<HashMap<&'static StatusEffect, Vec<Effect>>>,
    pub entity_equipment: Arc<Mutex<EntityEquipment>>,
    pub equipment_drop_chances: Arc<Mutex<HashMap<EquipmentSlot, f32>>>,
    pub movement_input: AtomicCell<Vector3<f64>>,
    /// `LivingEntity.speed` in vanilla: the per-tick movement factor consumed by
    /// `travel`/`getFrictionInfluencedSpeed`. For players this is the raw
    /// `MOVEMENT_SPEED` attribute (`Player.aiStep`), for mobs it is
    /// `speedModifier * MOVEMENT_SPEED` as written by `MoveControl` via `Mob.setSpeed`.
    pub speed: AtomicCell<f64>,
    pub equipment_slots: Arc<HashMap<usize, EquipmentSlot>>,

    pub jumping: AtomicBool,

    pub jumping_cooldown: AtomicU8,

    /// Vanilla `LivingEntity.fallFlyTicks`: consecutive ticks spent fall flying, driving
    /// the glide game event and glider durability schedule in `updateFallFlying`.
    pub fall_fly_ticks: AtomicU32,
    /// Vanilla `LivingEntity.discardFriction` (`LivingEntity.java:225`), set for long-jump
    /// arcs and consumed by `travel_in_air`.
    discard_friction: AtomicBool,

    pub climbing: AtomicBool,

    /// The position where the entity was last climbing, used for death messages
    pub climbing_pos: AtomicCell<Option<BlockPos>>,

    /// The entity ID of the entity that last attacked this living entity.
    pub last_attacker_id: AtomicI32,
    /// The tick at which this entity was last attacked (entity age).
    pub last_attacked_time: AtomicI32,
    /// Packed tick and panic-causing flag for vanilla's last damage source.
    pub last_damage_state: AtomicCell<(u64, i64, bool)>,
    last_damage_sequence: AtomicU64,

    /// The entity ID of the entity this living entity last attacked.
    pub last_attacking_id: AtomicI32,
    /// The tick at which this entity last attacked something (entity age).
    pub last_attack_time: AtomicI32,

    /// Vanilla `LivingEntity.canBeSeenAsEnemy()` override hook (e.g. `Axolotl.canBeSeenAsEnemy`
    /// while playing dead): when `true`, target-selection AI (`TargetPredicate`) treats this
    /// entity as unattackable, independent of `can_take_damage` -- players and existing attackers
    /// can still damage it normally, it just won't be picked as a *new* AI target.
    pub not_targetable_as_enemy: AtomicBool,

    water_movement_speed_multiplier: f32,
    livings_flags: AtomicU8,

    /// The attributes of the entity
    pub attributes: RwLock<HashMap<u8, AttributeInstance>>,

    /// Snapshot of the items this entity had in every equipment slot at the end of the last
    /// tick, i.e. vanilla `LivingEntity.lastEquipmentItems` (`LivingEntity.java:2948`). Used
    /// solely to detect equipment *changes*, which is when enchantment attribute modifiers get
    /// re-evaluated.
    last_equipment_items: Mutex<HashMap<EquipmentSlot, ItemStack>>,
    /// Whether Soul Speed's `minecraft:location_changed` attribute modifiers are currently
    /// applied. Vanilla keeps this as the enchantment's "active location-based effect" set and
    /// re-tests it through `enchantment_active_check` (`soul_speed.json`).
    soul_speed_active: AtomicBool,
    /// The block position the location-changed enchantment effects last ran for.
    ///
    /// Vanilla fires them from `LivingEntity.onChangedBlock` (`LivingEntity.java:547-549`),
    /// which runs when the entity's block position changes rather than every tick.
    last_effect_block_pos: AtomicCell<Option<BlockPos>>,

    /// `Raider.raid`/`Raider.wave`/`Raider.isPatrolLeader` (`Raider.java`).
    pub raid_membership: AtomicCell<Option<RaidMembership>>,
    /// `Raider.canJoinRaid` (`Raider.java`).
    pub can_join_raid: AtomicBool,
    /// `Raider.ticksOutsideRaid` (`Raider.java`).
    pub ticks_outside_raid: AtomicI32,
}

struct EffectParticle {
    particle_id: VarInt,
    color: i32,
}

struct EffectParticles(Vec<EffectParticle>);

impl MetadataSerializer for EffectParticles {
    fn write_metadata(
        &self,
        writer: &mut impl std::io::Write,
        _version: &pumpkin_util::version::JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let count = i32::try_from(self.0.len())
            .map_err(|_| WritingError::Message("Too many effect particles".into()))?;
        writer.write_var_int(&VarInt(count))?;
        for particle in &self.0 {
            writer.write_var_int(&particle.particle_id)?;
            writer.write_i32(particle.color)?;
        }
        Ok(())
    }
}

impl EffectParticle {
    const fn from_effect(effect: &Effect) -> Self {
        Self {
            particle_id: VarInt(Particle::EntityEffect as i32),
            color: (((if effect.ambient { 38 } else { 255 }) as u32) << 24
                | effect.effect_type.color as u32) as i32,
        }
    }
}

impl LivingEntity {
    /// Vanilla `LivingEntity.getRelativePortalPosition` delegates to the entity calculation and
    /// then clears its forward offset (`LivingEntity.java:3385-3387`).
    pub(crate) const fn reset_forward_direction_of_relative_portal_position(
        relative_position: Vector3<f64>,
    ) -> Vector3<f64> {
        Vector3::new(relative_position.x, relative_position.y, 0.0)
    }

    /// Returns the hitbox used by vanilla melee range checks, including the riding-position
    /// floor for a passenger (`LivingEntity.java:1692-1700`; `Mob.java:1359-1360`).
    pub(crate) async fn get_hitbox(&self) -> BoundingBox {
        let bounding_box = self.entity.bounding_box.load();
        let vehicle = self.entity.vehicle.lock().await.clone();
        let Some(vehicle) = vehicle else {
            return bounding_box;
        };

        let vehicle_entity = vehicle.get_entity();
        let riding_position = vehicle_entity.pos.load().y + vehicle.get_passengers_riding_offset()
            - self
                .get_vehicle_attachment_point(vehicle_entity)
                .map_or(0.0, |offset| offset.y);
        hitbox_with_riding_floor(bounding_box, Some(riding_position))
    }

    pub fn knockback_with_resistance(&self, strength: f64, x: f64, z: f64) {
        let resistance = self.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE);
        self.entity.knockback(
            knockback_strength_with_resistance(strength, resistance),
            x,
            z,
        );
    }

    /// Vanilla `LivingEntity.isUsingItem` (`LivingEntity.java:3417-3419`).
    #[must_use]
    pub fn is_using_item(&self) -> bool {
        self.livings_flags.load(Relaxed) & Self::USING_ITEM_FLAG != 0
    }

    /// Vanilla `LivingEntity.hasLandedInLiquid` (`LivingEntity.java:404-406`).
    #[must_use]
    pub fn has_landed_in_liquid(&self) -> bool {
        let velocity = self.entity.velocity.load();
        has_landed_in_liquid_state(
            velocity.y,
            self.entity.touching_water.load(SeqCst),
            self.entity.touching_lava.load(SeqCst),
        )
    }

    /// Vanilla `LivingEntity.isDeadOrDying` (`LivingEntity.java:1171-1173`).
    #[must_use]
    pub fn is_dead_or_dying(&self) -> bool {
        dead_or_dying_state(self.health.load(), self.dead.load(Relaxed))
    }

    /// Vanilla `LivingEntity.isIgnoringFallDamageFromCurrentImpulse` (`LivingEntity.java:1826-1828`).
    #[must_use]
    pub fn is_ignoring_fall_damage_from_current_impulse(&self) -> bool {
        self.current_impulse_impact_pos.load().is_some()
    }

    /// Vanilla `LivingEntity.isInPostImpulseGraceTime` (`LivingEntity.java:1836-1838`).
    #[must_use]
    pub fn is_in_post_impulse_grace_time(&self) -> bool {
        self.post_impulse_context_reset_grace_time.load(Relaxed) > 0
    }

    const USING_ITEM_FLAG: u8 = 1;
    const OFF_HAND_ACTIVE_FLAG: u8 = 2;
    const USING_RIPTIDE_FLAG: u8 = 4;

    const PREVENT_AREA_FALL_DAMAGE_BLOCKS: [&'static Block; 4] = [
        &Block::COBWEB,
        &Block::LADDER,
        &Block::POWDER_SNOW,
        &Block::SLIME_BLOCK,
    ];

    fn hurt_sound_for_entity(entity_type: &'static EntityType) -> Sound {
        entity_type.hurt_sound.unwrap_or(Sound::EntityGenericHurt)
    }

    #[inline]
    pub fn is_undead(&self) -> bool {
        self.entity
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_INVERTED_HEALING_AND_HARM)
    }

    pub(crate) const fn instant_effect_is_damage(
        effect_type: &'static StatusEffect,
        inverted: bool,
    ) -> bool {
        (effect_type.id == StatusEffect::INSTANT_HEALTH.id && inverted)
            || (effect_type.id == StatusEffect::INSTANT_DAMAGE.id && !inverted)
    }

    pub fn new(entity: Entity) -> Self {
        let water_movement_speed_multiplier = if entity.entity_type == &EntityType::POLAR_BEAR {
            0.98
        } else if entity.entity_type == &EntityType::SKELETON_HORSE {
            0.96
        } else {
            0.8
        };
        let mut max_health: f32 = 20.0; // Overridden by attribute base below
        let mut base_movement_speed = Attributes::MOVEMENT_SPEED.default_value;
        let max_air_supply = if entity.entity_type == &EntityType::AXOLOTL {
            6000
        } else if entity.entity_type == &EntityType::DOLPHIN {
            4800
        } else {
            MAX_AIR_SUPPLY
        };
        Self {
            // Populate local attribute instances from the default registry and get initial vars
            attributes: {
                let mut m = std::collections::HashMap::new();

                for (attr, base) in entity.entity_type.attributes {
                    if attr.id == Attributes::MAX_HEALTH.id {
                        max_health = *base as f32;
                    }
                    if attr.id == Attributes::MOVEMENT_SPEED.id {
                        base_movement_speed = *base;
                    }
                    m.insert(
                        attr.id,
                        AttributeInstance::new(*base, attr.min_value, attr.max_value),
                    );
                }
                std::sync::RwLock::new(m)
            },
            health: AtomicCell::new(max_health), // Initial health value from attributes
            air_supply: AtomicI32::new(max_air_supply),
            arrow_count: AtomicI32::new(0),
            remove_arrow_time: AtomicI32::new(0),
            stinger_count: AtomicI32::new(0),
            air_random: std::sync::Mutex::new(StdRng::seed_from_u64(rand::rng().random())),
            air_metadata_initialized: AtomicBool::new(false),
            entity,
            hurt_cooldown: AtomicI32::new(0),
            skip_drop_experience: AtomicBool::new(false),
            loot_table_seed: AtomicCell::new(0),
            last_hurt_by_player_time: AtomicI32::new(0),
            last_damage_taken: AtomicCell::new(0.0),
            absorption: AtomicCell::new(0.0),
            fall_distance: AtomicCell::new(0.0),
            post_impulse_context_reset_grace_time: AtomicI32::new(0),
            current_impulse_impact_pos: AtomicCell::new(None),
            death_time: AtomicU8::new(0),
            move_dist: AtomicCell::new(0.0),
            next_step: AtomicCell::new(1.0),
            dead: AtomicBool::new(false),
            item_use_time: AtomicI32::new(0),
            item_in_use: Mutex::new(None),
            active_hand: Mutex::new(None),
            auto_spin_attack_ticks: AtomicI32::new(0),
            auto_spin_attack_state: Mutex::new(()),
            auto_spin_attack_damage: AtomicCell::new(0.0),
            auto_spin_attack_item_stack: Mutex::new(None),
            livings_flags: AtomicU8::new(0),
            active_effects: Mutex::new(HashMap::new()),
            hidden_effects: Mutex::new(HashMap::new()),
            entity_equipment: Arc::new(Mutex::new(EntityEquipment::new())),
            equipment_drop_chances: Arc::new(Mutex::new(HashMap::new())),
            equipment_slots: Arc::new(build_equipment_slots()),
            jumping: AtomicBool::new(false),
            jumping_cooldown: AtomicU8::new(0),
            fall_fly_ticks: AtomicU32::new(0),
            discard_friction: AtomicBool::new(false),
            climbing: AtomicBool::new(false),
            climbing_pos: AtomicCell::new(None),
            last_attacker_id: AtomicI32::new(0),
            last_attacked_time: AtomicI32::new(0),
            last_damage_state: AtomicCell::new((0, 0, false)),
            last_damage_sequence: AtomicU64::new(0),
            last_attacking_id: AtomicI32::new(0),
            last_attack_time: AtomicI32::new(0),
            not_targetable_as_enemy: AtomicBool::new(false),
            movement_input: AtomicCell::new(Vector3::default()),
            speed: AtomicCell::new(base_movement_speed),
            water_movement_speed_multiplier,
            last_equipment_items: Mutex::new(HashMap::new()),
            soul_speed_active: AtomicBool::new(false),
            last_effect_block_pos: AtomicCell::new(None),
            raid_membership: AtomicCell::new(None),
            can_join_raid: AtomicBool::new(false),
            ticks_outside_raid: AtomicI32::new(0),
        }
    }

    /// The slots vanilla's `collectEquipmentChanges` walks (`EquipmentSlot.VALUES`,
    /// `LivingEntity.java:2951`).
    const fn attribute_equipment_slots() -> [EquipmentSlot; 8] {
        [
            EquipmentSlot::MAIN_HAND,
            EquipmentSlot::OFF_HAND,
            EquipmentSlot::FEET,
            EquipmentSlot::LEGS,
            EquipmentSlot::CHEST,
            EquipmentSlot::HEAD,
            EquipmentSlot::BODY,
            EquipmentSlot::SADDLE,
        ]
    }

    /// `LivingEntity.getItemBySlot` for every slot at once. The main hand is special-cased
    /// because a player's held item lives in `PlayerInventory.main_inventory[selected]`, not in
    /// `entity_equipment` -- which also means scrolling the hotbar counts as an equipment
    /// change, exactly as in vanilla.
    ///
    /// The main hand is resolved *before* the equipment lock is taken: for a non-player
    /// `held_item` reads `entity_equipment` itself, so doing it under the lock would deadlock.
    async fn items_by_equipment_slot(
        &self,
        caller: &dyn EntityBase,
    ) -> Vec<(EquipmentSlot, ItemStack)> {
        let main_hand = self.held_item(caller).await;
        let equipment = self.entity_equipment.lock().await;
        Self::attribute_equipment_slots()
            .into_iter()
            .map(|slot| {
                let stack = if matches!(slot, EquipmentSlot::MainHand(_)) {
                    main_hand.clone()
                } else {
                    equipment.get(&slot)
                };
                (slot, stack)
            })
            .collect()
    }

    /// Applies `modifiers` to this entity's attributes, recording every attribute touched so
    /// one `CUpdateAttributes` can be sent for the batch.
    fn apply_attribute_modifiers(
        &self,
        modifiers: Vec<(&'static Attributes, Modifier)>,
        remove: bool,
        touched: &mut Vec<Attributes>,
    ) {
        for (attribute, modifier) in modifiers {
            self.update_attribute(attribute, |instance| {
                if remove {
                    instance.remove_modifier(&modifier.id);
                } else {
                    instance.add_or_replace_modifier(modifier.clone());
                }
            });
            if !touched.iter().any(|a| a.id == attribute.id) {
                touched.push(attribute.clone());
            }
        }
    }

    /// Vanilla `LivingEntity.detectEquipmentUpdates`/`collectEquipmentChanges`
    /// (`LivingEntity.java:2938`/`:2948`), narrowed to the part Pumpkin needs: when the stack
    /// in a slot changes, drop the outgoing stack's enchantment attribute modifiers
    /// (`stopLocationBasedEffects`, `LivingEntity.java:3850`) and install the incoming
    /// stack's (`LivingEntity.java:2972`).
    ///
    /// Recomputing only on change -- rather than every tick -- is what vanilla does and what
    /// keeps `add_or_replace_modifier` from thrashing the attribute cache.
    pub async fn tick_equipment_attributes(&self, caller: &dyn EntityBase) {
        let current_items = self.items_by_equipment_slot(caller).await;
        let mut changes: Vec<(EquipmentSlot, ItemStack, ItemStack, bool)> = Vec::new();
        {
            let mut last = self.last_equipment_items.lock().await;
            for (slot, current) in current_items {
                let first_observation = !last.contains_key(&slot);
                let previous = last
                    .get(&slot)
                    .cloned()
                    .unwrap_or_else(|| ItemStack::EMPTY.clone());
                if !previous.are_equal(&current) {
                    last.insert(slot.clone(), current.clone());
                    changes.push((slot, previous, current, first_observation));
                }
            }
        }

        if changes.is_empty() {
            return;
        }

        let mut touched: Vec<Attributes> = Vec::new();
        for (slot, previous, current, first_observation) in changes {
            // Vanilla `LivingEntity.onEquipItem` is reached by equipment changes after the
            // initial entity tick (`LivingEntity.java:689-713`; `:2938-2982`).
            self.on_equip_item(caller, &slot, &previous, &current, first_observation)
                .await;
            if !previous.is_empty() {
                let stale = crate::enchantment::attribute_modifiers_for_slot(&previous, &slot);
                self.apply_attribute_modifiers(stale, true, &mut touched);
            }
            if !current.is_empty() {
                let fresh = crate::enchantment::attribute_modifiers_for_slot(&current, &slot);
                self.apply_attribute_modifiers(fresh, false, &mut touched);
            }
        }

        if !touched.is_empty() {
            crate::entity::attributes::send_attribute_updates_for_living(self, touched).await;
        }
    }

    /// Applies the server-visible part of vanilla `LivingEntity.onEquipItem`: an equippable item
    /// plays its declared sound, and every changed slot emits EQUIP or UNEQUIP
    /// (`LivingEntity.java:689-713`).
    async fn on_equip_item(
        &self,
        caller: &dyn EntityBase,
        slot: &EquipmentSlot,
        old_stack: &ItemStack,
        stack: &ItemStack,
        first_observation: bool,
    ) {
        if first_observation || caller.is_spectator() || old_stack.are_equal(stack) {
            return;
        }

        let world = self.entity.world.load();
        if !self.entity.is_silent()
            && let Some(equippable) = stack.get_data_component::<EquippableImpl>()
            && equippable.slot == slot
        {
            let category = if caller.get_player().is_some() {
                SoundCategory::Players
            } else {
                SoundCategory::Neutral
            };
            world.play_sound_event(&equippable.equip_sound, category, &self.entity.pos.load());
        }

        // `LivingEntity.doesEmitEquipEvent` (`LivingEntity.java:685-686`) is overridden by
        // `Player` to allow only humanoid armor (`Player.java:1664-1666`).
        if does_emit_equip_event(caller.get_player().is_some(), slot) {
            let event = if stack.get_data_component::<EquippableImpl>().is_some() {
                pumpkin_data::game_event::GameEvent::Equip
            } else {
                pumpkin_data::game_event::GameEvent::Unequip
            };
            let context = world
                .get_entity_by_uuid(self.entity.entity_uuid)
                .map_or_else(
                    crate::world::game_event::GameEventContext::none,
                    crate::world::game_event::GameEventContext::of_entity,
                );
            crate::world::game_event::emit_game_event(
                &world,
                event,
                self.entity.pos.load(),
                context,
            )
            .await;
        }
    }

    /// Vanilla `LivingEntity.onEquippedItemBroken` (`LivingEntity.java:3845-3857`) sends the
    /// slot break event and immediately removes the broken stack's attribute and location-based
    /// effects before the equipment slot is cleared.
    pub async fn on_equipped_item_broken(&self, broken_item: &ItemStack, slot: &EquipmentSlot) {
        let world = self.entity.world.load();
        world.send_entity_status(&self.entity, super::equipment_break_status(slot), None);

        let mut modifiers = crate::enchantment::attribute_modifiers_for_slot(broken_item, slot);
        if *slot == EquipmentSlot::FEET && self.soul_speed_active.swap(false, Relaxed) {
            modifiers.extend(
                crate::enchantment::location_based_attribute_modifiers_for_slot(broken_item, slot),
            );
        }

        let mut touched = Vec::new();
        self.apply_attribute_modifiers(modifiers, true, &mut touched);
        if !touched.is_empty() {
            crate::entity::attributes::send_attribute_updates_for_living(self, touched).await;
        }
    }

    /// Forgets the equipment snapshot so the next tick re-derives every enchantment attribute
    /// modifier from scratch. Needed wherever the attribute map itself is rebuilt, otherwise an
    /// unchanged snapshot would suppress the re-application.
    pub async fn clear_equipment_attribute_snapshot(&self) {
        self.last_equipment_items.lock().await.clear();
        self.soul_speed_active.store(false, Relaxed);
    }

    /// `EnchantmentHelper.runLocationChangedEffects` as driven by
    /// `LivingEntity.onChangedBlock` (`LivingEntity.java:547-549`): the equipment's
    /// `location_changed` effects run when the entity's block position changes, not every
    /// tick.
    ///
    /// Frost Walker freezes water under the wearer, and Soul Speed rolls its boot damage,
    /// through this path. Both evaluators existed with no caller until this hook.
    async fn tick_location_changed_effects(&self, caller: &dyn EntityBase) {
        let pos = self.entity.block_pos.load();
        if self.last_effect_block_pos.swap(Some(pos)) == Some(pos) {
            return;
        }

        let boots = self.entity_equipment.lock().await.get(&EquipmentSlot::FEET);

        let frost_level = boots.get_enchantment_level(&Enchantment::FROST_WALKER);
        if frost_level > 0 {
            crate::enchantment::apply_frost_walker(
                &self.entity.world.load(),
                self.entity.pos.load(),
                frost_level,
                None,
            )
            .await;
        }

        let soul_level = boots.get_enchantment_level(&Enchantment::SOUL_SPEED);
        if soul_level > 0
            && let Some((chance, amount)) =
                crate::enchantment::location_based_item_damage(&Enchantment::SOUL_SPEED, soul_level)
            && self
                .entity
                .get_block_with_y_offset(0.500_001)
                .1
                .has_tag(&tag::Block::MINECRAFT_SOUL_SPEED_BLOCKS)
            && rand::random::<f32>() < chance
            && let Some(player) = caller.get_player()
        {
            player
                .damage_item_in_slot(&EquipmentSlot::FEET, amount)
                .await;
        }
    }

    /// Soul Speed's `minecraft:location_changed` effects (`soul_speed.json`).
    /// `LivingEntity.removeFrost`/`tryAddFrost` (`LivingEntity.java:523-545`): while freezing
    /// in powder snow, movement speed is slowed by `-0.05 * percentFrozen` via a transient
    /// `MOVEMENT_SPEED` modifier, removed as soon as `frozen_ticks` returns to 0.
    /// `Entity::tick_frozen` (called just before this from `Entity`, which owns
    /// `frozen_ticks`/`is_in_powder_snow`) already ports the tick-counter and freeze-damage
    /// half; this ports the remaining speed-modifier half. Vanilla removes the modifier before
    /// `tryAddFrost`, which only adds it when `getBlockStateOnLegacy()` is non-air
    /// (`LivingEntity.java:523-545`).
    const POWDER_SNOW_SPEED_MODIFIER_ID: &'static str = "minecraft:powder_snow";

    async fn tick_frost(&self) {
        let frozen_ticks = self.entity.frozen_ticks.load(Relaxed);
        if frozen_ticks <= 0 || self.entity.get_block_state_on_legacy().is_air() {
            let had_modifier = {
                let map = self
                    .attributes
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                map.get(&Attributes::MOVEMENT_SPEED.id)
                    .is_some_and(|instance| {
                        instance
                            .modifiers
                            .iter()
                            .any(|m| m.id == Self::POWDER_SNOW_SPEED_MODIFIER_ID)
                    })
            };
            if had_modifier {
                self.update_attribute(&Attributes::MOVEMENT_SPEED, |instance| {
                    instance.remove_modifier(Self::POWDER_SNOW_SPEED_MODIFIER_ID);
                });
                crate::entity::attributes::send_attribute_updates_for_living(
                    self,
                    vec![Attributes::MOVEMENT_SPEED],
                )
                .await;
            }
            return;
        }

        // `LivingEntity.tryAddFrost` uses `Entity.getPercentFrozen`, including its clamp
        // (`LivingEntity.java:523-545`; `Entity.java:2815-2818`).
        let percent_frozen = f64::from(self.entity.get_percent_frozen());
        self.update_attribute(&Attributes::MOVEMENT_SPEED, |instance| {
            instance.add_or_replace_modifier(Modifier {
                id: Self::POWDER_SNOW_SPEED_MODIFIER_ID.to_string(),
                amount: -0.05 * percent_frozen,
                operation: ModifierOperation::Add,
            });
        });
        crate::entity::attributes::send_attribute_updates_for_living(
            self,
            vec![Attributes::MOVEMENT_SPEED],
        )
        .await;
    }

    /// Unlike every other enchantment attribute this one is *not* equip-time: vanilla only
    /// holds the `movement_speed`/`movement_efficiency` modifiers while the entity is walking
    /// on `#minecraft:soul_speed_blocks`, and keeps them for the airborne part of a stride
    /// once already active (the `enchantment_active_check` branch of the requirement).
    async fn tick_soul_speed(&self, caller: &dyn EntityBase) {
        let boots = self.entity_equipment.lock().await.get(&EquipmentSlot::FEET);
        let level = boots.get_enchantment_level(&Enchantment::SOUL_SPEED);
        let active = self.soul_speed_active.load(Relaxed);

        let should_be_active = if level <= 0 {
            false
        } else {
            let flying = match caller.get_player() {
                Some(player) => player.abilities.lock().await.flying,
                None => false,
            };
            if flying || self.entity.has_vehicle().await {
                false
            } else {
                let on_soul_block = self
                    .entity
                    .get_block_with_y_offset(0.500_001)
                    .1
                    .has_tag(&tag::Block::MINECRAFT_SOUL_SPEED_BLOCKS);
                if active {
                    on_soul_block || !self.entity.on_ground.load(Relaxed)
                } else {
                    on_soul_block
                }
            }
        };

        if should_be_active == active {
            return;
        }

        let modifiers = crate::enchantment::location_based_attribute_modifiers_for_slot(
            &boots,
            &EquipmentSlot::FEET,
        );
        if modifiers.is_empty() && should_be_active {
            return;
        }

        let mut touched: Vec<Attributes> = Vec::new();
        if should_be_active {
            self.apply_attribute_modifiers(modifiers, false, &mut touched);
        } else {
            // The boots may already be gone, so rebuild the ids from the enchantment itself
            // rather than from whatever is in the slot now.
            for attribute in [
                &Attributes::MOVEMENT_SPEED,
                &Attributes::MOVEMENT_EFFICIENCY,
            ] {
                let id = crate::enchantment::modifier_id_for_slot(
                    &Enchantment::SOUL_SPEED,
                    &EquipmentSlot::FEET,
                );
                self.update_attribute(attribute, |instance| instance.remove_modifier(&id));
                touched.push(attribute.clone());
            }
        }
        self.soul_speed_active.store(should_be_active, Relaxed);

        if !touched.is_empty() {
            crate::entity::attributes::send_attribute_updates_for_living(self, touched).await;
        }
    }

    pub fn send_equipment_changes(&self, equipment: &[(EquipmentSlot, ItemStack)]) {
        if equipment.is_empty() {
            return;
        }
        let equipment_java: Vec<(i8, ItemStackSerializer)> = equipment
            .iter()
            .map(|(slot, stack)| {
                (
                    slot.discriminant(),
                    ItemStackSerializer::from(stack.clone()),
                )
            })
            .collect();
        let je_packet = CSetEquipment::new(self.entity_id().into(), equipment_java);

        let mut sent_editioned = false;
        for (slot, stack) in equipment {
            if *slot == EquipmentSlot::MAIN_HAND || *slot == EquipmentSlot::OFF_HAND {
                let window_id = if *slot == EquipmentSlot::OFF_HAND {
                    120
                } else {
                    0
                };
                let be_packet = pumpkin_protocol::bedrock::client::CMobEquipment::new(
                    self.entity_id() as u64,
                    pumpkin_protocol::bedrock::network_item::NetworkItemStackDescriptor::from(
                        stack,
                    ),
                    0,
                    0,
                    window_id,
                );
                self.entity
                    .world
                    .load()
                    .broadcast_packet_except_editioned_sync(
                        &[self.entity.entity_uuid],
                        &je_packet,
                        &be_packet,
                    );
                sent_editioned = true;
            }
        }

        if !sent_editioned {
            self.entity
                .world
                .load()
                .broadcast_packet_except(&[self.entity.entity_uuid], &je_packet);
        }
    }

    /// Vanilla `LivingEntity.spawnItemParticles` (`LivingEntity.java:3546-3563`): sends the
    /// broken stack as item particles around the entity's eye position before the equipment is
    /// cleared. The item template is serialized per Java protocol version, as required by the
    /// particle packet's item-payload codec.
    pub fn spawn_item_particles(&self, item_stack: &ItemStack, count: usize) {
        if item_stack.is_empty() {
            return;
        }

        let item = ItemStackTemplateSerializer::from(item_stack.clone());
        let position = self.entity.pos.load();
        let eye_y = self.entity.get_eye_y();
        let pitch = f64::from(self.entity.pitch.load()).to_radians();
        let yaw = f64::from(self.entity.yaw.load()).to_radians();
        let players = self.entity.world.load().players.load();

        for player in players.iter() {
            let Ok(data) = ({
                let mut data = Vec::new();
                item.write_with_version(&mut data, &player.client.java_version())
                    .map(|()| data)
            }) else {
                continue;
            };

            for _ in 0..count {
                let mut velocity = Vector3::new(
                    (f64::from(rand::random::<f32>()) - 0.5) * 0.1,
                    f64::from(rand::random::<f32>()) * 0.1 + 0.1,
                    0.0,
                );
                velocity = rotate_particle_vector(velocity, pitch, yaw);

                let mut offset = Vector3::new(
                    (f64::from(rand::random::<f32>()) - 0.5) * 0.3,
                    -f64::from(rand::random::<f32>()) * 0.6 - 0.3,
                    0.6,
                );
                offset = rotate_particle_vector(offset, pitch, yaw);
                let particle_position = Vector3::new(
                    position.x + offset.x,
                    eye_y + offset.y,
                    position.z + offset.z,
                );
                let particle_velocity = Vector3::new(
                    velocity.x as f32,
                    (velocity.y + 0.05) as f32,
                    velocity.z as f32,
                );
                player.spawn_particle_with_data(
                    particle_position,
                    particle_velocity,
                    0.0,
                    1,
                    Particle::Item,
                    &data,
                );
            }
        }
    }

    /// Picks up an Item entity or XP Orb
    pub fn pickup(&self, item: &Entity, stack_amount: u32) {
        let mut pickup_event =
            crate::plugin::api::events::entity::entity_pickup_item::EntityPickupItemEvent::new(
                self.entity.entity_id,
                item.entity_type.id.to_string(),
                stack_amount as u8,
            );
        if let Some(server) = self.entity.world.load().server.upgrade() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    server.plugin_manager.fire(&server, &mut pickup_event).await;
                });
            });
            if pickup_event.cancelled {
                return;
            }
        }

        let chunk_pos = self.entity.chunk_pos.load();
        self.entity.world.load().broadcast_to_chunk_editioned_sync(
            chunk_pos,
            &CTakeItemEntity::new(
                item.entity_id.into(),
                self.entity.entity_id.into(),
                VarInt(stack_amount as i32),
            ),
            &CTakeItemActor::new(
                VarULong(item.entity_id as u64),
                VarULong(self.entity.entity_id as u64),
            ),
        );
    }

    /// Sends the Hand animation to all others, used when Eating for example
    pub async fn set_active_hand(&self, hand: Hand, stack: ItemStack, duration: i32) {
        let emits_use_vibration = stack.get_data_component::<UseEffectsImpl>().is_some();
        self.item_use_time.store(duration, Ordering::Relaxed);
        *self.item_in_use.lock().await = Some(stack);
        *self.active_hand.lock().await = Some(hand);
        self.set_living_flag(Self::USING_ITEM_FLAG, true);
        self.set_living_flag(Self::OFF_HAND_ACTIVE_FLAG, hand == Hand::Left);

        // Vanilla `LivingEntity.startUsingItem` calls `ItemStack.causeUseVibration`
        // (`LivingEntity.java:3497-3505`; `ItemStack.java:749-754`).
        if emits_use_vibration {
            let world = self.entity.world.load();
            let context = world
                .get_entity_by_uuid(self.entity.entity_uuid)
                .map_or_else(
                    crate::world::game_event::GameEventContext::none,
                    crate::world::game_event::GameEventContext::of_entity,
                );
            crate::world::game_event::emit_game_event(
                &world,
                pumpkin_data::game_event::GameEvent::ItemInteractStart,
                self.entity.pos.load(),
                context,
            )
            .await;
        }
    }

    fn set_living_flag(&self, flag: u8, value: bool) {
        let index = flag;
        let mut b = self.livings_flags.load(Ordering::Relaxed);
        if value {
            b |= index;
        } else {
            b &= !index;
        }
        self.livings_flags.store(b, Ordering::Relaxed);

        let bedrock_meta = (flag == Self::USING_ITEM_FLAG).then(|| {
            // The Bedrock FLAGS field is a full bitfield: a SetActorData that carries it
            // replaces the client's entire flag set. Building this from an empty metadata
            // sent FLAGS with only USING_ITEM, which cleared HAS_GRAVITY (and HAS_COLLISION,
            // CLIMB, BREATHING) that were set on spawn, so the moment a player used an item
            // the client stopped applying gravity and floated. Keep the entity's accumulated
            // flags up to date and send the whole field so gravity is preserved.
            let index =
                pumpkin_protocol::bedrock::client::set_actor_data::entity_data_flag::USING_ITEM;
            let mask = 1i64 << index;
            if value {
                self.entity.bedrock_flags.fetch_or(mask, Ordering::Relaxed);
            } else {
                self.entity
                    .bedrock_flags
                    .fetch_and(!mask, Ordering::Relaxed);
            }

            let mut meta = pumpkin_protocol::bedrock::client::set_actor_data::EntityMetadata::new();
            meta.set(
                pumpkin_protocol::bedrock::client::set_actor_data::entity_data_key::FLAGS,
                pumpkin_protocol::bedrock::client::set_actor_data::MetadataValue::Long(
                    self.entity.bedrock_flags.load(Ordering::Relaxed),
                ),
            );
            meta
        });

        self.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::living_entity::DATA_LIVING_ENTITY_FLAGS,
                b,
            )],
            bedrock_meta.as_ref(),
        );
    }

    pub async fn clear_active_hand(&self) {
        let emits_use_vibration = self
            .item_in_use
            .lock()
            .await
            .as_ref()
            .is_some_and(|stack| stack.get_data_component::<UseEffectsImpl>().is_some());
        // Vanilla `LivingEntity.stopUsingItem` calls `ItemStack.causeUseVibration` for an
        // active item (`LivingEntity.java:3614-3621`; `ItemStack.java:749-754`).
        if emits_use_vibration {
            let world = self.entity.world.load();
            let context = world
                .get_entity_by_uuid(self.entity.entity_uuid)
                .map_or_else(
                    crate::world::game_event::GameEventContext::none,
                    crate::world::game_event::GameEventContext::of_entity,
                );
            crate::world::game_event::emit_game_event(
                &world,
                pumpkin_data::game_event::GameEvent::ItemInteractFinish,
                self.entity.pos.load(),
                context,
            )
            .await;
        }
        *self.item_in_use.lock().await = None;
        *self.active_hand.lock().await = None;
        self.item_use_time.store(0, Ordering::Relaxed);

        self.set_living_flag(Self::USING_ITEM_FLAG, false);
    }

    /// Starts vanilla's temporary auto-spin attack state.
    pub async fn start_auto_spin_attack(
        &self,
        activation_ticks: i32,
        damage: f32,
        item_stack: ItemStack,
    ) {
        let _state = self.auto_spin_attack_state.lock().await;
        self.auto_spin_attack_ticks
            .store(activation_ticks, Ordering::Relaxed);
        self.auto_spin_attack_damage.store(damage);
        *self.auto_spin_attack_item_stack.lock().await = Some(item_stack);
        self.set_living_flag(Self::USING_RIPTIDE_FLAG, true);
    }

    pub fn is_auto_spin_attack(&self) -> bool {
        self.livings_flags.load(Ordering::Relaxed) & Self::USING_RIPTIDE_FLAG != 0
    }

    pub async fn auto_spin_attack_item(&self) -> ItemStack {
        self.auto_spin_attack_item_stack
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| ItemStack::EMPTY.clone())
    }

    async fn tick_auto_spin_attack(
        &self,
        caller: &Arc<dyn EntityBase>,
        previous_bounding_box: BoundingBox,
    ) {
        let _state = self.auto_spin_attack_state.lock().await;
        let ticks = self.auto_spin_attack_ticks.load(Ordering::Relaxed);
        if ticks <= 0 {
            return;
        }

        let remaining = ticks - 1;
        self.auto_spin_attack_ticks
            .store(remaining, Ordering::Relaxed);

        let current_bounding_box = self.entity.bounding_box.load();
        let search_box = BoundingBox::new(
            Vector3::new(
                previous_bounding_box.min.x.min(current_bounding_box.min.x),
                previous_bounding_box.min.y.min(current_bounding_box.min.y),
                previous_bounding_box.min.z.min(current_bounding_box.min.z),
            ),
            Vector3::new(
                previous_bounding_box.max.x.max(current_bounding_box.max.x),
                previous_bounding_box.max.y.max(current_bounding_box.max.y),
                previous_bounding_box.max.z.max(current_bounding_box.max.z),
            ),
        );
        let world = self.entity.world.load();
        let candidates: Vec<_> = world
            .get_all_at_box(&search_box)
            .into_iter()
            .filter(|candidate| candidate.get_entity().entity_id != self.entity.entity_id)
            .collect();
        if let Some(player) = caller.get_player() {
            for candidate in &candidates {
                let candidate_entity = candidate.get_entity();
                if candidate_entity.entity_id == self.entity.entity_id
                    || candidate.get_living_entity().is_none()
                {
                    continue;
                }

                player.attack(candidate.clone()).await;
                self.auto_spin_attack_ticks.store(0, Ordering::Relaxed);
                self.entity
                    .set_velocity(self.entity.velocity.load().multiply(-0.2, -0.2, -0.2));
                break;
            }
        }

        if candidates.is_empty() && self.entity.horizontal_collision.load(SeqCst) {
            self.auto_spin_attack_ticks.store(0, Ordering::Relaxed);
        }

        if self.auto_spin_attack_ticks.load(Ordering::Relaxed) <= 0 {
            self.auto_spin_attack_damage.store(0.0);
            *self.auto_spin_attack_item_stack.lock().await = None;
            self.set_living_flag(Self::USING_RIPTIDE_FLAG, false);
        }
    }

    /// Vanilla: `Raider::hasActiveRaid` (`getCurrentRaid() != null && raid.isActive()`).
    ///
    /// Approximation: reads the cached `RaidMembership` captured at spawn time rather than
    /// re-querying `RaidManager`; membership is cleared on death (see `on_death` below), so
    /// `is_some()` is a reasonable proxy for a live raider's `hasActiveRaid` for the raid-gated
    /// goals that consume this (Vindicator's door goals, Witch's heal-raiders goal).
    #[must_use]
    pub fn has_active_raid(&self) -> bool {
        self.raid_membership.load().is_some()
    }

    /// Vanilla: `Raider::getWave` (via `getCurrentRaid()`/`Raid.getGroupsSpawned() + 1`).
    #[must_use]
    pub fn raid_wave(&self) -> Option<i32> {
        self.raid_membership.load().map(|m| m.wave)
    }

    pub async fn is_blocking(&self) -> bool {
        // `LivingEntity.isUsingItem` gates `getItemBlockingWith` before the shield component is
        // inspected (`LivingEntity.java:3417-3429`).
        if !self.is_using_item() {
            return false;
        }
        let item_in_use = self.item_in_use.lock().await;
        if let Some(item) = item_in_use.as_ref()
            && item.get_data_component::<BlocksAttacksImpl>().is_some()
        {
            let use_time = self.item_use_time.load(Ordering::Relaxed);
            let required_time = if let Some(dyn_self) = self
                .entity
                .world
                .load()
                .get_entity_by_id(self.entity.entity_id)
                && let Some(player) = dyn_self
                    .cast_any()
                    .downcast_ref::<crate::entity::player::Player>()
                && matches!(
                    player.client.as_ref(),
                    crate::net::ClientPlatform::Bedrock(_)
                ) {
                0
            } else {
                5
            };
            return item.get_max_use_time() - use_time >= required_time;
        }
        false
    }

    pub fn heal(&self, additional_health: f32) {
        assert!(additional_health > 0.0);
        let mut event =
            crate::plugin::api::events::entity::entity_regain_health::EntityRegainHealthEvent::new(
                self.entity.entity_id,
                additional_health,
            );
        if let Some(server) = self.entity.world.load().server.upgrade() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    server.plugin_manager.fire(&server, &mut event).await;
                });
            });
            if event.cancelled {
                return;
            }
        }
        self.set_health(self.health.load() + additional_health);
    }

    pub fn set_health(&self, health: f32) {
        // Clamp to [0, max_health]
        let max_health = self.get_max_health();
        let clamped = health.max(0.0).min(max_health);
        self.health.store(clamped);
        // tell everyone entities health changed
        self.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::living_entity::DATA_HEALTH_ID,
                clamped,
            )],
            None,
        );
    }

    /// Returns the current maximum health for this entity
    pub fn get_max_health(&self) -> f32 {
        self.get_attribute_value(&Attributes::MAX_HEALTH) as f32
    }

    /// Vanilla `LivingEntity.getMaxAbsorption` reads the current maximum absorption attribute
    /// (`LivingEntity.java:1990-1992`).
    #[must_use]
    pub fn get_max_absorption(&self) -> f32 {
        self.get_attribute_value(&Attributes::MAX_ABSORPTION) as f32
    }

    /// Sets the maximum health for this entity
    pub async fn set_max_health(&self, max_health: f32) {
        // Update base attribute
        self.set_attribute_base(&Attributes::MAX_HEALTH, max_health as f64);

        // Broadcast the attribute change
        crate::entity::attributes::send_attribute_updates_for_living(
            self,
            vec![Attributes::MAX_HEALTH],
        )
        .await;

        // Clamp current health to new max if needed and send metadata update
        let current_health = self.health.load();
        if current_health > max_health {
            self.set_health(max_health);
        }
    }

    /// Returns the current absorption amount for this entity (yellow hearts)
    pub fn get_absorption(&self) -> f32 {
        self.absorption.load()
    }

    /// Sets the current absorption amount for this entity (yellow hearts)
    pub async fn set_absorption(&self, new_abs: f32) {
        // `LivingEntity.setAbsorptionAmount` (`LivingEntity.java:3397-3400`) clamps both
        // sides, including writes above the current max-absorption attribute.
        let max_absorption = self.get_max_absorption();
        let new_abs = new_abs.clamp(0.0, max_absorption);

        // Set local state
        self.absorption.store(new_abs);

        // Broadcast attribute update for max_absorption so clients receive
        // the updated absorption value via the attribute packet.
        crate::entity::attributes::send_attribute_updates_for_living(
            self,
            vec![Attributes::MAX_ABSORPTION],
        )
        .await;

        // Send absorption metadata for players (visual yellow hearts)
        if self.entity.entity_type == &EntityType::PLAYER {
            self.entity.send_meta_data(
                &[Metadata::new(
                    tracked_data::player::DATA_PLAYER_ABSORPTION_ID,
                    new_abs,
                )],
                None,
            );
        }
    }

    /// Applies the `post_piercing_attack` enchantment effects after a melee attack.
    ///
    /// Vanilla `LivingEntity.postPiercingAttack` (`LivingEntity.java:1707-1711`) delegates
    /// this to `EnchantmentHelper`; `Player.attack` and `Mob.doHurtTarget` invoke it after the
    /// attack attempt (`Player.java:1004`, `Mob.java:1404`).
    pub async fn post_piercing_attack(&self, caller: &dyn EntityBase) {
        if self.entity.vehicle.lock().await.is_some()
            || self.entity.is_fall_flying()
            || self.entity.touching_water.load(Relaxed)
        {
            return;
        }

        if let Some(player) = caller.get_player()
            && player.gamemode.load() != GameMode::Creative
            && player.get_food_level() < 7
        {
            return;
        }

        let item = self.held_item(caller).await;
        let Some(enchantments) = item.get_data_component::<EnchantmentsImpl>() else {
            return;
        };
        let Some((magnitude, exhaustion, item_damage)) =
            enchantments
                .enchantment
                .iter()
                .find_map(|(enchantment, level)| {
                    crate::enchantment::post_piercing_lunge(enchantment, *level)
                })
        else {
            return;
        };

        let look = Vector3::from_yaw_pitch(self.entity.yaw.load(), self.entity.pitch.load());
        self.entity
            .add_velocity(look.multiply(f64::from(magnitude), 0.0, f64::from(magnitude)));
        // `ApplyEntityImpulse.apply` (`ApplyEntityImpulse.java:24-32`) grants living entities
        // ten ticks before an impulse context may be reset.
        self.apply_post_impulse_grace_time(10);

        if let Some(player) = caller.get_player() {
            player.add_exhaustion(exhaustion).await;
            player.damage_held_item(item_damage).await;
        } else if let Some(mob) = caller.get_mob() {
            mob.get_mob_entity()
                .damage_main_hand_weapon_after_hit()
                .await;
        }

        let sound = match rand::random_range(0..3) {
            0 => Sound::ItemSpearLunge1,
            1 => Sound::ItemSpearLunge2,
            _ => Sound::ItemSpearLunge3,
        };
        let category = if caller.get_player().is_some() {
            SoundCategory::Players
        } else {
            SoundCategory::Neutral
        };
        self.entity
            .world
            .load()
            .play_sound(sound, category, &self.entity.pos.load());
    }

    /// Convenience helper to mutate an attribute instance. Automatically inserts
    /// a new instance populated from the registry base if needed.
    pub fn update_attribute<F: FnOnce(&mut AttributeInstance)>(
        &self,
        attribute: &Attributes,
        f: F,
    ) {
        let mut map = self
            .attributes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let inst = map.entry(attribute.id).or_insert_with(|| {
            let base = self
                .entity
                .entity_type
                .attributes
                .iter()
                .find(|a| a.0.id == attribute.id)
                .map_or_else(
                    || {
                        tracing::warn!(
                            "Entity type {:?} has no base value for attribute {:?}; falling back to default {}",
                            self.entity.entity_type,
                            attribute.id,
                            attribute.default_value,
                        );
                        attribute.default_value
                    },
                    |a| a.1,
                );
            AttributeInstance::new(base, attribute.min_value, attribute.max_value)
        });

        f(inst);
        // Vanilla `AttributeInstance.setDirty` (`AttributeInstance.java:112-115`) is the
        // invalidation boundary for all mutations performed by this helper.
        inst.set_dirty();
    }

    /// Returns the computed value for `attribute` using the local instance, falling back
    /// to `attribute.default_value` if no local instance exists.
    pub fn get_attribute_value(&self, attribute: &Attributes) -> f64 {
        let map = self
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.get(&attribute.id)
            .map_or(attribute.default_value, AttributeInstance::value)
    }

    /// Vanilla `LivingEntity.getAttackRangeWith` returns the item's component or the
    /// entity-interaction default (`LivingEntity.java:2230-2233`; `AttackRange.java:55-59`).
    /// The returned generated component is consumed by the live player attack-range and
    /// kinetic-weapon sweep paths.
    pub(crate) fn get_attack_range_with(&self, weapon_item: &ItemStack) -> AttackRangeImpl {
        weapon_item
            .get_data_component::<AttackRangeImpl>()
            .cloned()
            .unwrap_or_else(|| {
                default_attack_range(
                    self.get_attribute_value(&Attributes::ENTITY_INTERACTION_RANGE),
                )
            })
    }

    /// Vanilla `LivingEntity.getArmorValue` (`LivingEntity.java:1877-1879`).
    #[must_use]
    pub fn get_armor_value(&self) -> i32 {
        armor_value_from_attribute(self.get_attribute_value(&Attributes::ARMOR))
    }

    /// Vanilla `LivingEntity.getArrowCount`/`setArrowCount` (`LivingEntity.java:1994-2000`).
    #[must_use]
    pub fn get_arrow_count(&self) -> i32 {
        self.arrow_count.load(Relaxed)
    }

    /// Increments the tracked arrow count after a successful non-piercing arrow hit.
    pub fn add_arrow(&self) {
        let count = self.arrow_count.fetch_add(1, Relaxed) + 1;
        self.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::living_entity::DATA_ARROW_COUNT_ID,
                count,
            )],
            None,
        );
    }

    /// `Mob.setSpeed`: stores the movement factor and mirrors it into the forward
    /// movement input (`setZza`).
    pub fn set_speed(&self, speed: f64) {
        self.speed.store(speed);
        let mut input = self.movement_input.load();
        input.z = speed;
        self.movement_input.store(input);
    }

    /// `speedModifier * MOVEMENT_SPEED`, the value `MoveControl` passes to `Mob.setSpeed`.
    #[must_use]
    pub fn speed_for_modifier(&self, speed_modifier: f64) -> f64 {
        speed_modifier * self.get_attribute_value(&Attributes::MOVEMENT_SPEED)
    }

    /// Returns the base attribute value for `attribute` for this entity's type.
    pub fn get_attribute_base(&self, attribute: &Attributes) -> f64 {
        // Check the local base value first (could be modified)
        let map = self
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(instance) = map.get(&attribute.id) {
            return instance.base_value;
        }

        // Fall back to registry base value if no local instance exists
        self.entity
            .entity_type
            .attributes
            .iter()
            .find(|a| a.0.id == attribute.id)
            .map_or(attribute.default_value, |a| a.1)
    }

    /// Update or insert the base value for an attribute on this entity.
    /// If the attribute doesn't exist locally yet, it will be inserted.
    pub fn set_attribute_base(&self, attribute: &Attributes, new_base: f64) {
        let mut map = self
            .attributes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(inst) = map.get_mut(&attribute.id) {
            // Vanilla `AttributeInstance.setBaseValue` (`AttributeInstance.java:45-49`) marks
            // the cached effective value dirty only when the base actually changes.
            inst.set_base_value(new_base);
        } else {
            let ai = AttributeInstance::new(new_base, attribute.min_value, attribute.max_value);
            ai.dirty.store(true, Ordering::Relaxed);
            map.insert(attribute.id, ai);
        }
    }

    /// `Mth.ceil(numberOfTicks * getAttributeValue(Attributes.BURNING_TIME))`
    /// (`LivingEntity.java:3990`).
    #[must_use]
    pub fn scale_ignite_ticks(&self, ticks: u32) -> u32 {
        let scaled = f64::from(ticks) * self.get_attribute_value(&Attributes::BURNING_TIME);
        if scaled <= 0.0 {
            return 0;
        }
        scaled.ceil() as u32
    }

    pub async fn reset_effects_and_attributes(&self) {
        self.clear_equipment_attribute_snapshot().await;
        // Clear active effects and reset modified attributes
        let effects_to_remove: Vec<_> = {
            let lock = self.active_effects.lock().await;
            lock.keys().copied().collect()
        };

        for effect_type in effects_to_remove {
            self.remove_effect(effect_type).await;
        }
    }

    pub const fn entity_id(&self) -> i32 {
        self.entity.entity_id
    }

    /// Vanilla `LivingEntity.getFallFlyingTicks` exposes the consecutive glide counter
    /// (`LivingEntity.java:3661-3663`).
    #[must_use]
    pub fn get_fall_flying_ticks(&self) -> u32 {
        self.fall_fly_ticks.load(Relaxed)
    }

    /// Vanilla `LivingEntity.getLastHurtByMobTimestamp` exposes the tick at which an attacker
    /// last damaged this entity (`LivingEntity.java:629-631`).
    #[must_use]
    pub fn get_last_hurt_by_mob_timestamp(&self) -> i32 {
        self.last_attacked_time.load(Relaxed)
    }

    /// Vanilla `LivingEntity.getLastHurtMobTimestamp` exposes the tick at which this entity
    /// last damaged another living entity (`LivingEntity.java:655-657`).
    #[must_use]
    pub fn get_last_hurt_mob_timestamp(&self) -> i32 {
        self.last_attack_time.load(Relaxed)
    }

    /// Vanilla `LivingEntity.getLastHurtByPlayerMemoryTime` exposes the remaining player-kill
    /// credit timer (`LivingEntity.java:4011-4013`).
    #[must_use]
    pub fn get_last_hurt_by_player_memory_time(&self) -> i32 {
        self.last_hurt_by_player_time.load(Relaxed)
    }

    /// Vanilla `LivingEntity.isAffectedByPotions`, which `ArmorStand` overrides to false: splash
    /// and lingering potions pass an armour stand by rather than dosing it.
    #[must_use]
    pub fn is_affected_by_potions(&self) -> bool {
        self.entity.entity_type != &EntityType::ARMOR_STAND
    }

    /// `LivingEntity.canBeAffected` plus the species overrides that sit in front of it:
    /// `Parched` (immune to the Weakness its own arrows apply), `Spider` and `AbstractNautilus`
    /// (poison) and `WitherBoss` / `WitherSkeleton` (wither).
    #[must_use]
    pub fn can_be_affected(&self, effect_type: &StatusEffect) -> bool {
        effect_applies_to(self.entity.entity_type, effect_type)
    }

    /// Vanilla `LivingEntity.addEffect`: returns whether the active instance changed, which is
    /// what decides whether a caller counts the application as a success.
    pub async fn add_effect(&self, mut effect: Effect) -> bool {
        if !self.can_be_affected(effect.effect_type) {
            return false;
        }

        let mut effect_event =
            crate::plugin::api::events::entity::entity_potion_effect::EntityPotionEffectEvent::new(
                self.entity.entity_id,
                effect.effect_type.translation_key.to_string(),
                effect.duration,
                effect.amplifier,
            );
        if let Some(server) = self.entity.world.load().server.upgrade() {
            server.plugin_manager.fire(&server, &mut effect_event).await;
        }
        if effect_event.cancelled {
            return false;
        }

        let applied_amplifier = effect.amplifier;
        let inverted = self.is_undead();
        let is_instant = effect.effect_type.id == StatusEffect::INSTANT_HEALTH.id
            || effect.effect_type.id == StatusEffect::INSTANT_DAMAGE.id;
        if !Self::instant_effect_is_damage(effect.effect_type, inverted) && is_instant {
            let heal_amount = instant_effect_amount(4, effect.amplifier).max(0.0);
            self.heal(heal_amount);
            return true;
        } else if is_instant {
            let damage_amount = instant_effect_amount(6, effect.amplifier);
            let dyn_self = self
                .entity
                .world
                .load()
                .get_entity_by_id(self.entity.entity_id);
            if let Some(dyn_self) = dyn_self {
                dyn_self
                    .damage(&*dyn_self, damage_amount, DamageType::MAGIC)
                    .await;
            }
        } else {
            let did_apply = {
                let mut active_effects = self.active_effects.lock().await;
                let mut hidden_effects = self.hidden_effects.lock().await;
                if let Some(current) = active_effects.get(effect.effect_type) {
                    let mut chain = Vec::with_capacity(2);
                    chain.push(current.clone());
                    if let Some(hidden) = hidden_effects.get(effect.effect_type) {
                        chain.extend_from_slice(hidden);
                    }
                    let changed = update_effect_chain(&mut chain, 0, &effect);
                    effect = chain.remove(0);
                    active_effects.insert(effect.effect_type, effect.clone());
                    if chain.is_empty() {
                        hidden_effects.remove(effect.effect_type);
                    } else {
                        hidden_effects.insert(effect.effect_type, chain);
                    }
                    changed
                } else {
                    active_effects.insert(effect.effect_type, effect.clone());
                    true
                }
            };

            // Vanilla runs `MobEffectInstance.onEffectStarted` on every application, including
            // one that loses the merge, so absorption is granted before that check. It dispatches
            // to the added effect's own `MobEffect`, and only `AbsorptionMobEffect` overrides it,
            // so no other effect may touch the absorption amount.
            if effect.effect_type == &StatusEffect::ABSORPTION {
                self.start_absorption(applied_amplifier).await;
            }

            if !did_apply {
                return false;
            }

            self.apply_effect_attribute_modifiers(&effect).await;

            // Apply invisible effect
            if effect.effect_type == &StatusEffect::INVISIBILITY {
                self.entity.set_invisible(true).await;
            }

            // Apply glowing effect
            if effect.effect_type == &StatusEffect::GLOWING {
                self.entity.set_glowing(true).await;
            }
        }

        self.sync_effect_to_clients(&effect);
        self.update_effect_visibility().await;

        true
    }

    /// Applies an active effect's attribute modifiers and sends the resulting attribute snapshot
    /// to tracking clients. `add_effect` and `force_add_effect` share this so replacement cannot
    /// leave the old amplifier installed on the server.
    async fn apply_effect_attribute_modifiers(&self, effect: &Effect) {
        if effect.effect_type.attribute_modifiers.is_empty() {
            return;
        }

        let mut touched_attrs: Vec<Attributes> = Vec::new();
        for effect_modifier in effect.effect_type.attribute_modifiers {
            let attribute = effect_modifier.attribute;
            let operation = match effect_modifier.operation {
                Operation::AddValue => ModifierOperation::Add,
                Operation::AddMultipliedBase => ModifierOperation::MultiplyBase,
                Operation::AddMultipliedTotal => ModifierOperation::MultiplyTotal,
            };
            let modifier = Modifier {
                id: effect_modifier.id.to_string(),
                amount: effect_modifier.base_value * (f64::from(effect.amplifier) + 1.0),
                operation,
            };
            self.update_attribute(attribute, |instance| {
                instance.add_or_replace_modifier(modifier.clone());
            });
            if !touched_attrs
                .iter()
                .any(|current| current.id == attribute.id)
            {
                touched_attrs.push(attribute.clone());
            }
        }

        crate::entity::attributes::send_attribute_updates_for_living(self, touched_attrs).await;
    }

    /// Removes one active effect's attribute modifiers and synchronizes every attribute it
    /// touched. This mirrors `LivingEntity.onEffectsRemoved` and is intentionally paired with
    /// `apply_effect_attribute_modifiers` above.
    async fn remove_effect_attribute_modifiers(&self, effect: &Effect) {
        if effect.effect_type.attribute_modifiers.is_empty() {
            return;
        }

        let mut touched_attrs: Vec<Attributes> = Vec::new();
        for modifier in effect.effect_type.attribute_modifiers {
            let id = modifier.id.to_string();
            self.update_attribute(modifier.attribute, |instance| {
                instance.remove_modifier(&id);
            });
            if !touched_attrs
                .iter()
                .any(|attribute| attribute.id == modifier.attribute.id)
            {
                touched_attrs.push(modifier.attribute.clone());
            }
        }

        crate::entity::attributes::send_attribute_updates_for_living(self, touched_attrs).await;
    }

    /// Sends the `ClientboundUpdateMobEffectPacket` equivalent after an effect's active
    /// instance changes. Keeping this separate from insertion lets `force_add_effect` follow
    /// vanilla's replacement path without accidentally merging a hidden-effect chain.
    fn sync_effect_to_clients(&self, effect: &Effect) {
        let mut flag: i8 = 0;
        if effect.ambient {
            flag |= 1;
        }
        if effect.show_particles {
            flag |= 2;
        }
        if effect.show_icon {
            flag |= 4;
        }
        if effect.blend {
            flag |= 8;
        }

        let je_packet = CUpdateMobEffect::new(
            self.entity.entity_id.into(),
            VarInt(i32::from(effect.effect_type.id)),
            effect.amplifier.into(),
            effect.duration.into(),
            flag,
        );

        let be_packet = pumpkin_protocol::bedrock::client::CMobEffect::new(
            VarULong(self.entity.entity_id as u64),
            pumpkin_protocol::bedrock::client::CMobEffect::EVENT_ADD,
            VarInt(effect.effect_type.to_bedrock_id()),
            VarInt(i32::from(effect.amplifier)),
            effect.show_particles,
            VarInt(effect.duration),
            VarULong(0),
            effect.ambient,
        );

        let chunk_pos = self.entity.chunk_pos.load();
        self.entity
            .world
            .load()
            .broadcast_to_chunk_editioned_sync(chunk_pos, &je_packet, &be_packet);
    }

    /// `LivingEntity.updateEffectVisibility`, applied immediately on the server because Pumpkin
    /// does not have vanilla's deferred `effectsDirty` metadata pass. Glowing deliberately stays
    /// under `Entity::set_glowing`: teams and other entity state can also make an entity glow.
    pub async fn update_effect_visibility(&self) {
        let has_invisibility = self
            .active_effects
            .lock()
            .await
            .contains_key(&&StatusEffect::INVISIBILITY);
        self.entity
            .set_invisible(has_invisibility || self.entity.persistent_invisible.load(Relaxed))
            .await;
        self.sync_effect_particles().await;
    }

    async fn sync_effect_particles(&self) {
        let effects = self.active_effects.lock().await;
        let has_effects = !effects.is_empty();
        let particles = EffectParticles(
            effects
                .values()
                .filter(|effect| effect.show_particles)
                .map(EffectParticle::from_effect)
                .collect(),
        );
        let ambient = effects
            .values()
            .filter(|effect| effect.show_particles)
            .all(|effect| effect.ambient);
        drop(effects);

        self.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::living_entity::EFFECT_PARTICLES,
                particles,
            )],
            None,
        );
        if has_effects {
            self.entity.send_meta_data(
                &[Metadata::new(
                    tracked_data::living_entity::EFFECT_AMBIENCE_ID,
                    ambient,
                )],
                None,
            );
        }
    }

    pub async fn remove_all_effects(&self) -> bool {
        let effect_list: Vec<&'static StatusEffect> =
            self.active_effects.lock().await.keys().copied().collect();

        let mut succeeded = false;
        for effect_type in effect_list {
            succeeded |= self.remove_effect(effect_type).await;
        }
        succeeded
    }

    /// Snapshot counterpart to vanilla `LivingEntity.getActiveEffectsMap`. Returning a clone
    /// keeps the asynchronous Rust state private while allowing gameplay systems to reason
    /// about one coherent active-effect view.
    pub async fn get_active_effects_map(&self) -> HashMap<&'static StatusEffect, Effect> {
        self.active_effects.lock().await.clone()
    }

    /// Vanilla `LivingEntity.forceAddEffect`: replace the active instance directly instead of
    /// merging it into the old instance's hidden-effect chain. This is used by state-restoring
    /// and authoritative gameplay paths where the supplied instance must win exactly.
    pub async fn force_add_effect(&self, effect: Effect) -> bool {
        if !self.can_be_affected(effect.effect_type) {
            return false;
        }

        let previous = {
            let mut active_effects = self.active_effects.lock().await;
            let mut hidden_effects = self.hidden_effects.lock().await;
            hidden_effects.remove(effect.effect_type);
            active_effects.insert(effect.effect_type, effect.clone())
        };

        if let Some(previous) = previous {
            self.remove_effect_attribute_modifiers(&previous).await;
        }
        self.apply_effect_attribute_modifiers(&effect).await;

        if effect.effect_type == &StatusEffect::INVISIBILITY {
            self.entity.set_invisible(true).await;
        }
        if effect.effect_type == &StatusEffect::GLOWING {
            self.entity.set_glowing(true).await;
        }

        self.sync_effect_to_clients(&effect);
        self.update_effect_visibility().await;
        true
    }

    /// Vanilla `LivingEntity.removeEffectNoUpdate`. A hidden chain belongs to the same logical
    /// `MobEffectInstance`, so discard it with the active instance while deliberately leaving
    /// attributes, metadata, and packets untouched for the caller to reconcile.
    pub async fn remove_effect_no_update(
        &self,
        effect_type: &'static StatusEffect,
    ) -> Option<Effect> {
        self.hidden_effects.lock().await.remove(&effect_type);
        self.active_effects.lock().await.remove(&effect_type)
    }

    pub async fn remove_effect(&self, effect_type: &'static StatusEffect) -> bool {
        let Some(effect) = self.remove_effect_no_update(effect_type).await else {
            return false;
        };

        // Broadcast effect removal
        self.entity
            .world
            .load()
            .send_remove_mob_effect(&self.entity, effect_type);

        self.remove_effect_attribute_modifiers(&effect).await;

        // Vanilla has no absorption reset on removal: dropping the effect drops its
        // MAX_ABSORPTION modifier, and `LivingEntity.onAttributeUpdated` clamps the current
        // amount down to whatever maximum is left instead of zeroing it outright.
        if effect_type == &StatusEffect::ABSORPTION {
            let max_absorption = self.get_max_absorption();
            let clamped = self.absorption.load().min(max_absorption);
            self.set_absorption(clamped).await;
        }

        // If health boost effect removed, clamp current health to new max and notify clients
        if effect_type == &StatusEffect::HEALTH_BOOST {
            let new_max = self.get_max_health();
            if self.health.load() > new_max {
                // Update local health and send both health and absorption metadata together
                self.set_health(new_max.max(0.0));
            }
        }

        // If invisible effect removed, disable invisibility
        if effect_type == &StatusEffect::INVISIBILITY {
            self.entity.set_invisible(false).await;
        }

        // If glowing effect removed, disable glowing
        if effect_type == &StatusEffect::GLOWING {
            self.entity.set_glowing(false).await;
        }

        self.update_effect_visibility().await;
        true
    }

    pub async fn has_effect(&self, effect: &'static StatusEffect) -> bool {
        let effects = self.active_effects.lock().await;
        effects.contains_key(&effect)
    }

    pub async fn get_effect(&self, effect: &'static StatusEffect) -> Option<Effect> {
        let effects = self.active_effects.lock().await;
        effects.get(&effect).cloned()
    }

    pub fn is_in_fall_damage_resetting(&self) -> (bool, &Block) {
        let block_pos = self.entity.block_pos.load();
        let block = self.entity.world.load().get_block(&block_pos);
        (
            block.has_tag(&tag::Block::MINECRAFT_FALL_DAMAGE_RESETTING),
            block,
        )
    }

    // Check if the entity is in water
    pub fn is_in_water(&self) -> bool {
        let block_pos = self.entity.block_pos.load();
        self.entity.world.load().get_block(&block_pos) == &Block::WATER
    }

    // Check if the entity is in powder snow
    pub fn is_in_powder_snow(&self) -> bool {
        let block_pos = self.entity.block_pos.load();
        self.entity.world.load().get_block(&block_pos) == &Block::POWDER_SNOW
    }

    pub fn should_prevent_fall_damage(&self) -> bool {
        let (prevents, block) = self.is_in_fall_damage_resetting();

        if block == &Block::SCAFFOLDING && !self.entity.is_sneaking() {
            return false;
        }

        if block == &Block::WATER {
            return true;
        }

        if self.entity.entity_type == &EntityType::PLAYER {
            if block == &Block::END_GATEWAY || block == &Block::END_PORTAL {
                return true;
            }

            if block == &Block::NETHER_PORTAL {
                let world = self.entity.world.load();
                let level_info = world.level_info.load();

                return level_info.game_rules.players_nether_portal_default_delay == 0;
            }
        }

        prevents
    }

    pub fn should_prevent_fall_damage_in_area(&self) -> bool {
        let world = self.entity.world.load();
        let block_pos = self.entity.block_pos.load().down();
        let entity_pos = self.entity.pos.load();

        let min = BlockPos(Vector3::new(
            block_pos.0.x - 1,
            block_pos.0.y,
            block_pos.0.z - 1,
        ));
        let max = BlockPos(Vector3::new(
            block_pos.0.x + 1,
            block_pos.0.y,
            block_pos.0.z + 1,
        ));
        let pos_iter = BlockPos::iterate(min, max);

        // FIXME: it seems the java server checks all blocks around with a raycast and check if miss or hit,
        // then added to a collision checker to handle in the tick handler
        for pos in pos_iter {
            let block = world.get_block(&pos);

            if Self::PREVENT_AREA_FALL_DAMAGE_BLOCKS.contains(&block) {
                let block_center = Vector3::new(
                    f64::from(pos.0.x) + 0.5,
                    f64::from(pos.0.y) + 0.5,
                    f64::from(pos.0.z) + 0.5,
                );
                let distance = entity_pos.squared_distance_to_vec(&block_center);

                // Fetch safe fall distance from attribute
                let safe_distance = self.get_attribute_value(&Attributes::SAFE_FALL_DISTANCE);
                return distance.sqrt() <= safe_distance * safe_distance;
            }
        }

        false
    }

    pub fn is_immune_to_fall_damage(&self) -> bool {
        self.entity
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_FALL_DAMAGE_IMMUNE)
    }

    async fn get_effective_gravity(&self, caller: &Arc<dyn EntityBase>) -> f64 {
        let final_gravity = caller.get_gravity();

        if self.entity.velocity.load().y <= 0.0
            && self.has_effect(&StatusEffect::SLOW_FALLING).await
        {
            final_gravity.min(0.01)
        } else {
            final_gravity
        }
    }

    pub async fn swing_hand(&self) {
        let world = self.entity.world.load();
        let entity_id = self.entity_id();

        let je_packet = pumpkin_protocol::java::client::play::CEntityAnimation::new(
            entity_id.into(),
            pumpkin_protocol::java::client::play::Animation::SwingMainArm,
        );
        let be_packet = pumpkin_protocol::bedrock::server::animate::SAnimate {
            action: pumpkin_protocol::bedrock::server::animate::AnimateAction::SwingArm,
            runtime_entity_id: pumpkin_protocol::codec::var_ulong::VarULong(entity_id as u64),
            data: 0.0,
            swing_source: None,
        };

        world.broadcast_editioned(&je_packet, &be_packet).await;
    }

    /// Vanilla `LivingEntity.setDiscardFriction` (`LivingEntity.java:681-683`) is toggled by
    /// the live Breeze and goat long-jump callers so `travelInAir` can preserve their launch
    /// velocity for the arc.
    pub fn set_discard_friction(&self, discard: bool) {
        self.discard_friction.store(discard, Relaxed);
    }

    /// Vanilla `LivingEntity.shouldDiscardFriction` (`LivingEntity.java:677-679`) gates the
    /// friction branch in `travelInAir` (`LivingEntity.java:2477-2485`).
    #[must_use]
    pub fn should_discard_friction(&self) -> bool {
        self.discard_friction.load(Relaxed)
    }

    /// Vanilla `LivingEntity.tick` computes a movement-facing body target after `aiStep`
    /// (`LivingEntity.java:2797-2816`) and then applies `tickHeadTurn`
    /// (`LivingEntity.java:3018-3027`). The mob tick already publishes `body_yaw`, so keep that
    /// existing packet path supplied for entities whose movement controller did not set it.
    fn tick_head_turn(&self, max_head_rotation: f32) {
        let position = self.entity.pos.load();
        let previous_position = self.entity.last_pos.load();
        let dx = position.x - previous_position.x;
        let dz = position.z - previous_position.z;
        let target_body_yaw = if dx.mul_add(dx, dz * dz) > 0.0025000002 {
            let walk_direction = dz.atan2(dx).to_degrees() as f32 - 90.0;
            let facing_difference = wrap_degrees(self.entity.yaw.load() - walk_direction).abs();
            if (95.0..265.0).contains(&facing_difference) {
                walk_direction - 180.0
            } else {
                walk_direction
            }
        } else {
            self.entity.body_yaw.load()
        };

        let body_yaw = head_turn_body_yaw(
            self.entity.body_yaw.load(),
            target_body_yaw,
            self.entity.yaw.load(),
            max_head_rotation,
        );
        self.entity.body_yaw.store(body_yaw);
    }

    #[allow(clippy::too_many_lines)]
    async fn tick_movement<'a>(&'a self, server: &'a Server, caller: &'a Arc<dyn EntityBase>) {
        // `LivingEntity.aiStep` does not call travel when `Mob.isEffectiveAi()` is false.
        // Keep the rest of this method running so block collisions and frozen-state updates
        // still happen for NoAI entities.
        let no_ai = self.entity.no_ai.load(Relaxed);

        if self.jumping_cooldown.load(Relaxed) != 0 {
            self.jumping_cooldown.fetch_sub(1, Relaxed);
        }

        let should_swim_in_fluids = if let Some(player) = caller.get_player() {
            !player.is_flying().await
        } else {
            true
        };

        self.entity.check_zero_velo();

        // Player.aiStep: players refresh `speed` from the attribute every tick. Mobs get
        // theirs from MoveControl/navigation as `speedModifier * MOVEMENT_SPEED`.
        if caller.get_player().is_some() {
            self.speed
                .store(self.get_attribute_value(&Attributes::MOVEMENT_SPEED));
        }

        let mut movement_input = self.movement_input.load();

        movement_input.x *= 0.98;

        movement_input.z *= 0.98;

        self.movement_input.store(movement_input);

        // Vanilla runs Mob.serverAiStep from LivingEntity.aiStep after applyInput has damped
        // the current movement input, but before jump handling and travel.
        // `LivingEntity.isDeadOrDying` is the health/death predicate used by `aiStep`
        // (`LivingEntity.java:1171-1173`).
        let is_alive = !self.is_dead_or_dying();
        if is_alive
            && !no_ai
            && let Some(mob) = caller.get_mob()
            && mob.get_entity().entity_id == self.entity.entity_id
        {
            crate::entity::mob::tick_mob_ai(mob, caller).await;
        }

        // `LivingEntity.aiStep` clears input and jumping for dead/dying entities through
        // `isImmobile`, but still lets the later travel phase apply existing knockback.
        if !is_alive {
            self.jumping.store(false, SeqCst);
            self.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
            if let Some(mob) = caller.get_mob()
                && mob.get_entity().entity_id == self.entity.entity_id
            {
                mob.get_mob_entity().jump_requested.store(false, Relaxed);
            }
        }

        if self.jumping.load(SeqCst) && should_swim_in_fluids {
            let in_lava = self.entity.touching_lava.load(SeqCst);

            let in_water = self.entity.touching_water.load(SeqCst);

            let fluid_height = if in_lava {
                self.entity.lava_height.load()
            } else {
                self.entity.water_height.load()
            };

            let swim_height = self.get_swim_height();

            let on_ground = self.entity.on_ground.load(SeqCst);

            if (in_water || in_lava) && (!on_ground || fluid_height > swim_height) {
                // Swim upward

                let mut velo = self.entity.velocity.load();

                // `Mob.jumpInLiquid` (`Mob.java:1409-1414`) uses LivingEntity's ordinary
                // 0.04 impulse only when the mob's navigation can float; otherwise it uses
                // 0.3. `MagmaCube.jumpInLiquid` (`MagmaCube.java:102-109`) then replaces the
                // lava value with its size-scaled impulse.
                let can_float = Self::mob_can_float(caller.as_ref());
                if in_lava && self.entity.entity_type == &EntityType::MAGMA_CUBE {
                    velo.y = 0.22 + f64::from(self.entity.data.load(Relaxed)) * 0.05;
                } else {
                    velo.y += if can_float { 0.04 } else { 0.3 };
                }

                self.entity.velocity.store(velo);
            } else if (on_ground || in_water && fluid_height <= swim_height)
                && self.jumping_cooldown.load(SeqCst) == 0
            {
                self.jump().await;

                self.jumping_cooldown.store(10, SeqCst);
            }
        } else {
            self.jumping_cooldown.store(0, SeqCst);
        }

        if self.has_effect(&StatusEffect::SLOW_FALLING).await
            || self.has_effect(&StatusEffect::LEVITATION).await
        {
            self.fall_distance.store(0.0);
        }

        // `LivingEntity.aiStep`: count consecutive glide ticks, then re-validate glider
        // eligibility and run the durability/glide-event schedule right before travel.
        // A failed check clears the glide flag, so the dispatch below falls back to the
        // normal air/fluid travel exactly like vanilla's `travel` re-check.
        self.tick_glide_state(caller).await;

        let old_y = self.entity.pos.load().y;
        let effective_ai = caller.is_effective_ai();

        let custom_travel = if no_ai || !effective_ai {
            false
        } else if let Some(mob) = caller.get_mob()
            && mob.get_entity().entity_id == self.entity.entity_id
        {
            mob.custom_travel(caller).await
        } else {
            false
        };

        if !no_ai && effective_ai && !custom_travel {
            let touching_water = self.entity.touching_water.load(SeqCst);

            // `LivingEntity.shouldTravelInFluid` and `Strider.canStandOnFluid`
            // (`LivingEntity.java:2421-2437`, `Strider.java:180-182`) leave striders in water
            // movement while allowing them to stand on lava.

            if should_travel_in_fluid(
                self.entity.entity_type,
                self.entity.is_in_liquid(),
                touching_water,
                self.entity.touching_lava.load(SeqCst),
            ) && should_swim_in_fluids
            {
                self.travel_in_fluid(caller, touching_water).await;
            } else if self.entity.is_fall_flying() {
                self.travel_fall_flying(caller).await;
            } else {
                self.travel_in_air(caller).await;
            }
        }

        // Vanilla `Entity.move` calls `doCheckFallDamage`, which dispatches to
        // `LivingEntity.checkFallDamage` after collision resolution (`Entity.java:1543-1550`;
        // `LivingEntity.java:363-390`). The existing `fall` method is that dispatch's live
        // equivalent; invoke it for every movement path, including custom travel.
        self.fall(
            caller.clone(),
            self.entity.pos.load().y - old_y,
            self.entity.on_ground.load(SeqCst),
            false,
        )
        .await;

        // TODO: Apply Soul Speed boot durability when tick_block_underneath is implemented.
        //self.entity.tick_block_underneath(&caller);

        let suffocating = self.entity.tick_block_collisions(caller, server).await;

        if suffocating {
            self.damage(&**caller, 1.0, DamageType::IN_WALL).await;
        }

        // `LivingEntity.aiStep` (`LivingEntity.java:3163-3166`): a water-sensitive mob takes one
        // point of drown damage for every tick it spends in water or rain.
        if caller.is_sensitive_to_water() && self.entity.is_in_water_or_rain().await {
            self.damage(caller.as_ref(), 1.0, DamageType::DROWN).await;
        }
    }

    async fn travel_in_air<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) {
        // applyMovementInput

        // LivingEntity.getFrictionInfluencedSpeed uses `getSpeed()`, not the raw attribute.
        let effective_speed = self.speed.load();

        let air_drag = modified_friction(
            0.91,
            self.get_attribute_value(&Attributes::AIR_DRAG_MODIFIER),
        );

        let (speed, friction) = if self.entity.on_ground.load(SeqCst) {
            // getVelocityAffectingPos

            let slipperiness = f64::from(
                self.entity
                    .get_block_with_y_offset(0.500_001)
                    .1
                    .slipperiness,
            );

            (
                friction_influenced_speed(effective_speed, slipperiness),
                slipperiness * air_drag,
            )
        } else {
            let speed = if let Some(player) = caller.get_player() {
                player.get_off_ground_speed().await
            } else {
                // TODO: If the passenger is a player, ogs = movement_speed * 0.1

                0.02
            };

            (speed, air_drag)
        };

        self.entity
            .update_velocity_from_input(self.movement_input.load(), speed);

        // `LivingEntity.travel` consults `onClimbable` before applying the ladder speed and after
        // movement (`LivingEntity.java:2525-2572`); Player overrides that result while flying
        // (`Player.java:2023-2026`).
        self.apply_climbing_speed(self.on_climbable_for(caller.as_ref()).await);

        self.make_move(caller).await;

        let mut velo = self.entity.velocity.load();
        let climbing = self.on_climbable_for(caller.as_ref()).await;

        let can_powder_snow_climb = if self.entity.was_in_powder_snow.load(Relaxed) {
            crate::block::blocks::powder_snow::can_entity_walk_on_powder_snow(caller.as_ref()).await
        } else {
            false
        };

        if (self.entity.horizontal_collision.load(SeqCst) || self.jumping.load(SeqCst))
            && (climbing || can_powder_snow_climb)
        {
            velo.y = 0.2;
        }

        let levitation = self.get_effect(&StatusEffect::LEVITATION).await;

        if let Some(lev) = levitation {
            velo.y += 0.05f64.mul_add(f64::from(lev.amplifier + 1), -velo.y) * 0.2;
        } else {
            velo.y -= self.get_effective_gravity(caller).await;

            // TODO: If world is not loaded: replace effective gravity with:

            // if below world's bottom y then -0.1, else 0.0
        }

        // If entity has no drag: store velo and return

        // Vanilla `LivingEntity.travelInAir` skips all horizontal and vertical drag while
        // `discardFriction` is set (`LivingEntity.java:2477-2488`).
        let vertical_friction = caller.get_y_velocity_drag().unwrap_or_else(|| {
            if caller.is_flutterer() {
                friction
            } else {
                modified_friction(
                    0.98,
                    self.get_attribute_value(&Attributes::AIR_DRAG_MODIFIER),
                )
            }
        });
        velo = apply_air_friction(
            velo,
            friction,
            vertical_friction,
            self.should_discard_friction(),
        );

        self.entity.velocity.store(velo);
    }

    /// Reads the active mob navigator for `Mob.jumpInLiquid`; players use the ordinary
    /// `LivingEntity` impulse because they have no mob navigation.
    fn mob_can_float(caller: &dyn EntityBase) -> bool {
        caller
            .get_mob()
            .is_none_or(|mob| mob.get_mob_entity().navigator.lock().unwrap().can_float())
    }

    /// Vanilla `LivingEntity.travelFallFlying`: update velocity from the look vector before
    /// moving through the normal collision solver.
    async fn travel_fall_flying<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) {
        if self.on_climbable_for(caller.as_ref()).await {
            self.travel_in_air(caller).await;
            // Vanilla calls `stopFallFlying` here, which forces a flags resync.
            self.entity.stop_fall_flying();
            return;
        }

        let velocity = self.entity.velocity.load();
        let horizontal_speed = velocity.x.hypot(velocity.z);
        let look = caller.get_looking_vector();
        let pitch = f64::from(self.entity.pitch.load()).to_radians();
        let gravity = self.get_effective_gravity(caller).await;
        self.entity
            .velocity
            .store(fall_flying_velocity(velocity, look, pitch, gravity));
        self.make_move(caller).await;

        if self.entity.horizontal_collision.load(SeqCst) {
            let new_horizontal_speed = self
                .entity
                .velocity
                .load()
                .x
                .hypot(self.entity.velocity.load().z);
            if let Some(damage) =
                fall_flying_collision_damage(horizontal_speed, new_horizontal_speed)
            {
                self.damage(caller.as_ref(), damage, DamageType::FLY_INTO_WALL)
                    .await;
            }
        }
    }

    /// Vanilla `LivingEntity.aiStep` glide bookkeeping (`LivingEntity.java:2854-2858` and
    /// `LivingEntity.java:3116-3118`): count consecutive glide ticks, then re-validate
    /// eligibility and run the durability/glide-event schedule before travel. Both steps
    /// run even when travel itself is skipped for `NoAI` or ridden entities.
    async fn tick_glide_state<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) {
        if self.entity.is_fall_flying() {
            self.fall_fly_ticks.fetch_add(1, Relaxed);
            self.update_fall_flying(caller).await;
        } else {
            self.fall_fly_ticks.store(0, Relaxed);
        }
    }

    /// Vanilla `LivingEntity.updateFallFlying` (`LivingEntity.java:3182-3202`): while
    /// gliding, clamp accumulated fall distance on slow ticks, stop when no valid glider
    /// is equipped, and at every tenth consecutive glide tick broadcast the glide game
    /// event, damaging a random equipped glider on alternating one-second intervals.
    async fn update_fall_flying<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) {
        self.check_fall_distance_accumulation();

        if !self.can_glide(caller).await {
            // `this.setSharedFlag(7, false)`: end the glide without the forced-resync
            // toggle of `stopFallFlying`, exactly like vanilla's ineligibility branch.
            self.entity.set_fall_flying(false).await;
            return;
        }

        // The counter was already advanced for this tick by `tick_movement`; vanilla adds
        // one to it here (`LivingEntity.java:3190`).
        let check_fall_fly_ticks = self.get_fall_flying_ticks().wrapping_add(1);
        let (glide_event_tick, damage_glider_tick) = fall_flying_schedule(check_fall_fly_ticks);

        if damage_glider_tick {
            self.damage_random_glider(caller).await;
        }

        if glide_event_tick {
            // `this.gameEvent(GameEvent.ELYTRA_GLIDE)` with the default source entity.
            let world = self.entity.world.load();
            crate::world::game_event::emit_game_event(
                &world,
                pumpkin_data::game_event::GameEvent::ElytraGlide,
                self.entity.pos.load(),
                crate::world::game_event::GameEventContext::of_entity(caller.clone()),
            )
            .await;
        }
    }

    /// Vanilla `Entity.checkFallDistanceAccumulation` (`Entity.java:2890-2894`) keeps a fall
    /// from continuing to grow after the entity receives a non-downward impulse.
    pub fn check_fall_distance_accumulation(&self) {
        let velocity = self.entity.velocity.load();
        let fall_distance = self.fall_distance.load();
        self.fall_distance
            .store(accumulated_fall_distance_after_impulse(
                velocity.y,
                fall_distance,
            ));
    }

    /// Vanilla `LivingEntity.canGlide` (`LivingEntity.java:3204-3216`): airborne, not
    /// ridden, not levitating, and wearing an item that passes `canGlideUsing` in its slot.
    pub async fn can_glide(&self, caller: &Arc<dyn EntityBase>) -> bool {
        if self.entity.on_ground.load(SeqCst)
            || self.entity.has_vehicle().await
            || self.has_effect(&StatusEffect::LEVITATION).await
        {
            return false;
        }

        // `Player.canGlide` additionally rejects ability flight. Creative players may
        // still use a glider after turning ability flight off, just as in vanilla.
        if let Some(player) = caller.get_player()
            && player.is_flying().await
        {
            return false;
        }

        // Resolve the main hand before taking the equipment lock; see
        // `items_by_equipment_slot` for why players store their held item elsewhere.
        let main_hand = self.held_item(caller.as_ref()).await;
        !self.gliding_equipment_slots(&main_hand).await.is_empty()
    }

    /// Every equipment slot currently holding a stack that passes vanilla
    /// `canGlideUsing`, iterated in `EquipmentSlot.VALUES` order.
    async fn gliding_equipment_slots(&self, main_hand: &ItemStack) -> Vec<EquipmentSlot> {
        let equipment = self.entity_equipment.lock().await;
        Self::attribute_equipment_slots()
            .into_iter()
            .filter(|slot| {
                if matches!(slot, EquipmentSlot::MainHand(_)) {
                    can_glide_using(main_hand, slot)
                } else {
                    // A missing entry behaves like vanilla's empty getItemBySlot stacks.
                    equipment
                        .equipment
                        .get(slot)
                        .is_some_and(|stack| can_glide_using(stack, slot))
                }
            })
            .collect()
    }

    /// Vanilla `updateFallFlying`'s glider damage step: pick uniformly among slots passing
    /// `canGlideUsing` and apply `ItemStack.hurtAndBreak(1, owner, slot)`.
    async fn damage_random_glider<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) {
        let main_hand = self.held_item(caller.as_ref()).await;
        let candidates = self.gliding_equipment_slots(&main_hand).await;
        // `Util.getRandom(slotsWithGliders, this.random)`: an empty list picks nothing.
        let Some(slot) = (!candidates.is_empty())
            .then(|| rand::random_range(0..candidates.len()))
            .map(|index| candidates[index].clone())
        else {
            return;
        };

        // Player main-hand items live in the hotbar rather than `entity_equipment`.
        // The player helper mutates the authoritative inventory slot, emits break events,
        // and synchronizes both the owner and observers for every eligible equipment slot.
        if let Some(player) = caller.get_player() {
            player.damage_item_in_slot(&slot, 1).await;
            return;
        }

        let damaged = {
            let mut equipment = self.entity_equipment.lock().await;
            let mut stack = equipment.get(&slot);
            let broken_item = stack.clone();
            let result = stack.damage_item(1);
            (result != DamageResult::Untouched).then(|| {
                equipment.put(&slot, stack.clone());
                (result, stack, broken_item)
            })
        };

        if let Some((result, stack, broken_item)) = damaged {
            if result == DamageResult::Broken {
                // Vanilla `updateFallFlying` delegates the break to `onEquippedItemBroken`
                // (`LivingEntity.java:3845-3848`), which broadcasts the break status and removes
                // the item's attribute modifiers; the client then plays `breakItem`'s sound and
                // particles (`LivingEntity.java:1439-1448`) in response to that broadcast.
                self.on_equipped_item_broken(&broken_item, &slot).await;
                self.spawn_item_particles(&broken_item, 5);
            }
            self.send_equipment_changes(&[(slot, stack)]);
        }
    }

    async fn travel_in_fluid<'a>(&'a self, caller: &'a Arc<dyn EntityBase>, water: bool) {
        let movement_input = self.movement_input.load();

        let old_y = self.entity.pos.load().y;
        let falling = self.entity.velocity.load().y <= 0.0;
        let gravity = self.get_effective_gravity(caller).await;
        // LivingEntity.travelInFluid also blends toward `getSpeed()`, not the raw attribute.
        let effective_speed = self.speed.load();

        if water {
            let mut friction = if self.entity.sprinting.load(Relaxed) {
                0.9
            } else {
                f64::from(self.water_movement_speed_multiplier)
            };

            let mut speed = 0.02;

            // Apply water movement efficiency attribute
            let mut water_movement_efficiency =
                self.get_attribute_value(&Attributes::WATER_MOVEMENT_EFFICIENCY);

            if water_movement_efficiency > 0.0 {
                if !self.entity.on_ground.load(SeqCst) {
                    water_movement_efficiency *= 0.5;
                }

                friction += (0.546_000_06 - friction) * water_movement_efficiency;
                speed += (effective_speed - speed) * water_movement_efficiency;
            }

            if self.has_effect(&StatusEffect::DOLPHINS_GRACE).await {
                friction = 0.96;
            }

            self.entity
                .update_velocity_from_input(movement_input, speed);

            self.make_move(caller).await;

            let mut velo = self.entity.velocity.load();
            if self.entity.horizontal_collision.load(SeqCst)
                && self.on_climbable_for(caller.as_ref()).await
            {
                velo.y = 0.2;
            }

            velo = velo.multiply(friction, 0.8, friction);

            self.apply_fluid_moving_speed(&mut velo.y, gravity, falling);
            self.entity.velocity.store(velo);
        } else {
            self.entity.update_velocity_from_input(movement_input, 0.02);

            self.make_move(caller).await;

            let mut velo = self.entity.velocity.load();

            if self.entity.lava_height.load() <= self.get_swim_height() {
                velo.x *= 0.5;
                velo.z *= 0.5;
                velo.y *= 0.8;

                self.apply_fluid_moving_speed(&mut velo.y, gravity, falling);
            } else {
                velo = velo * 0.5;
            }

            if gravity != 0.0 {
                velo.y -= gravity / 4.0; // Negative gravity = buoyancy
            }

            self.entity.velocity.store(velo);
        }

        let mut velo = self.entity.velocity.load();

        // Vanilla `LivingEntity.jumpOutOfFluid`: nudges the entity upward when it's swum
        // into a wall, but only if the space it would be nudged into is actually free
        // (`Entity.isFree` = no block collision *and* no liquid there, Entity.java:668-670).
        // This previously probed a box shifted by the raw velocity and checked only for
        // the absence of liquid, so pushing against solid ground or a wall while
        // horizontally colliding (e.g. a Drowned pathing along an uneven seafloor) would
        // repeatedly apply the 0.3 upward boost even though the probed space was inside a
        // solid block, launching the entity above the water surface.
        //
        // Vanilla probes `(movement.x, movement.y + 0.6 - getY() + oldY, movement.z)`: a
        // roughly fixed ~0.6-block step-up check, adjusted for how much of the intended Y
        // movement collision actually consumed this tick. `getY() - oldY` is the actual
        // net Y movement already applied by `make_move` above; the `+ 0.6` alone is used
        // when movement wasn't vertically obstructed.
        let actual_dy = self.entity.pos.load().y - old_y;
        if self.entity.horizontal_collision.load(SeqCst)
            && self
                .entity
                .is_free(velo.x, velo.y + 0.6 - actual_dy, velo.z)
        {
            velo.y = 0.3;

            self.entity.velocity.store(velo);
        }
    }

    fn apply_fluid_moving_speed(&self, dy: &mut f64, gravity: f64, falling: bool) {
        if gravity != 0.0 && !self.entity.sprinting.load(Relaxed) {
            if falling && (*dy - 0.005).abs() >= 0.003 && (*dy - gravity / 16.0).abs() < 0.003 {
                *dy = -0.003;
            } else {
                *dy -= gravity / 16.0;
            }
        }
    }

    async fn make_move<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) {
        self.entity
            .move_entity(caller, self.entity.velocity.load())
            .await;

        self.check_climbing();
    }

    fn check_climbing(&self) {
        // HappyGhast.onClimbable (`HappyGhast.java:171-173`) is always false. Do not let the
        // generic climbable-block check turn a happy ghast into a ladder-climbing mob.
        // AbstractHorse.onClimbable (`AbstractHorse.java:950-952`) is likewise always false.
        if self.entity.entity_type == &EntityType::HAPPY_GHAST
            || matches!(
                self.entity.entity_type.id,
                id if id == EntityType::HORSE.id
                    || id == EntityType::DONKEY.id
                    || id == EntityType::MULE.id
                    || id == EntityType::SKELETON_HORSE.id
                    || id == EntityType::ZOMBIE_HORSE.id
            )
        {
            self.climbing.store(false, Relaxed);
            self.climbing_pos.store(None);
            return;
        }

        if let Some(climbing) = spider_climbing_state(
            self.entity.entity_type,
            self.entity.horizontal_collision.load(SeqCst),
        ) {
            let was_climbing = self.climbing.swap(climbing, Relaxed);

            if climbing != was_climbing {
                self.entity.send_meta_data(
                    &[Metadata::new(
                        tracked_data::spider::DATA_FLAGS_ID,
                        u8::from(climbing),
                    )],
                    None,
                );
            }
            return;
        }

        let pos = self.entity.block_pos.load();
        let world = self.entity.world.load();
        if world
            .get_block(&pos)
            .has_tag(&tag::Block::MINECRAFT_CLIMBABLE)
        {
            self.climbing.store(true, Relaxed);
            self.climbing_pos.store(Some(pos));
            return;
        }

        self.climbing.store(false, Relaxed);

        if self.entity.on_ground.load(SeqCst) {
            self.climbing_pos.store(None);
        }
    }

    /// Applies the ladder movement branch selected by `LivingEntity.onClimbable`
    /// (`LivingEntity.java:2525-2541`).
    fn apply_climbing_speed(&self, climbing: bool) {
        if climbing {
            self.fall_distance.store(0.0);

            let mut velo = self.entity.velocity.load();

            let pos = 0.15;

            let neg = -0.15;

            if velo.x < neg {
                velo.x = neg;
            } else if velo.x > pos {
                velo.x = pos;
            }

            if velo.z < neg {
                velo.z = neg;
            } else if velo.z > pos {
                velo.z = pos;
            }

            velo.y = velo.y.max(neg);

            // `LivingEntity.handleOnClimbable` checks `getInBlockState` before suppressing
            // downward motion (`LivingEntity.java:2694-2700`).
            if velo.y < 0.0
                && suppress_climb_descent(
                    self.entity.entity_type == &EntityType::PLAYER,
                    self.entity.sneaking.load(Relaxed),
                    Block::from_state_id(self.entity.get_in_block_state().id)
                        == &Block::SCAFFOLDING,
                )
            {
                velo.y = 0.0;
            }

            self.entity.velocity.store(velo);
        }
    }

    async fn on_climbable_for(&self, caller: &dyn EntityBase) -> bool {
        // `Player.onClimbable` delegates to the base result only when ability flight is off
        // (`Player.java:2023-2026`; `LivingEntity.java:1721-1737`).
        if let Some(player) = caller.get_player() {
            player.on_climbable().await
        } else {
            self.climbing.load(Relaxed)
        }
    }

    pub fn get_swim_height(&self) -> f64 {
        // `SulfurCube.getFluidJumpThreshold` (`SulfurCube.java:196-198`) uses 20% of
        // the current bounding-box height. `LivingEntity.tickMovement` consumes this
        // value when deciding between a ground jump and `jumpInLiquid`; keeping the
        // species-specific value here makes that existing movement path reachable.
        if self.entity.entity_type == &EntityType::SULFUR_CUBE {
            return f64::from(self.entity.entity_dimension.load().height) * 0.2;
        }

        let eye_height = self.entity.get_eye_height();

        if self.entity.entity_type == &EntityType::BREEZE {
            eye_height
        } else if eye_height < 0.4 {
            0.0
        } else {
            0.4
        }
    }

    async fn jump(&self) {
        let jump = self.get_jump_velocity(1.0).await;

        // `MagmaCube.jumpFromGround` (`net/minecraft/world/entity/monster/cubemob/MagmaCube.java:94-99`)
        // uses the cube size as an additional jump boost and does not apply the generic
        // sprinting impulse. Unlike the base implementation, the override has no small-power
        // early return.
        if self.entity.entity_type == &EntityType::MAGMA_CUBE {
            let mut velo = self.entity.velocity.load();
            velo.y = jump + f64::from(self.entity.data.load(Relaxed)) * 0.1;
            self.entity.velocity.store(velo);
            self.entity.velocity_dirty.store(true, SeqCst);
            return;
        }

        if jump <= 1.0e-5 {
            return;
        }

        let mut velo = self.entity.velocity.load();

        velo.y = jump.max(velo.y);

        if self.entity.sprinting.load(Relaxed) {
            let yaw = f64::from(self.entity.yaw.load()).to_radians();

            velo.x -= yaw.sin() * 0.2;
            velo.z += yaw.cos() * 0.2;
        }

        self.entity.velocity.store(velo);

        self.entity.velocity_dirty.store(true, SeqCst);
    }

    /// Vanilla `LivingEntity.applyPostImpulseGraceTime` (`LivingEntity.java:1813-1824`)
    /// retains an impulse context for at least the requested number of ticks. This is used by
    /// lunge's `ApplyEntityImpulse` effect and by the mace impact path below.
    pub fn apply_post_impulse_grace_time(&self, ticks: i32) {
        self.post_impulse_context_reset_grace_time
            .fetch_max(ticks, Relaxed);
    }

    /// Vanilla `LivingEntity.setIgnoreFallDamageFromCurrentImpulse` (`LivingEntity.java:1813-1819`)
    /// records the impact position used to limit the next fall's damage calculation.
    pub fn set_ignore_fall_damage_from_current_impulse(
        &self,
        ignore_fall_damage: bool,
        new_impulse_impact_pos: Vector3<f64>,
    ) {
        if ignore_fall_damage {
            self.apply_post_impulse_grace_time(40);
            self.current_impulse_impact_pos
                .store(Some(new_impulse_impact_pos));
        } else {
            self.post_impulse_context_reset_grace_time.store(0, Relaxed);
        }
    }

    /// Vanilla `ServerGamePacketListenerImpl.handleMovePlayer` clears a completed impulse
    /// context only when its grace timer has expired (`ServerGamePacketListenerImpl.java:1178-1184`).
    pub fn try_reset_current_impulse_context(&self) {
        if self.post_impulse_context_reset_grace_time.load(Relaxed) == 0 {
            self.reset_current_impulse_context();
        }
    }

    fn reset_current_impulse_context(&self) {
        self.post_impulse_context_reset_grace_time.store(0, Relaxed);
        self.current_impulse_impact_pos.store(None);
    }

    async fn get_jump_velocity(&self, mut strength: f64) -> f64 {
        strength *= self.get_attribute_value(&Attributes::JUMP_STRENGTH);
        strength *= f64::from(self.entity.get_jump_velocity_multiplier());
        if let Some(effect) = self.get_effect(&StatusEffect::JUMP_BOOST).await {
            strength += 0.1 * f64::from(effect.amplifier + 1);
        }
        strength
    }

    pub async fn fall(
        &self,
        caller: Arc<dyn EntityBase>,
        height_difference: f64,
        ground: bool,
        dont_damage: bool,
    ) {
        // HappyGhast.checkFallDamage (`HappyGhast.java:167-168`) is an empty override: neither
        // fall distance nor landing damage is accumulated for this flying mob.
        if caller.get_entity().entity_type == &EntityType::HAPPY_GHAST {
            self.fall_distance.store(0.0);
            return;
        }

        // A passenger is snapped back onto its vehicle every tick by `Entity.positionRider`, so
        // it never builds up a fall of its own; the vehicle hands it the fall through
        // `Entity.propagateFallToPassengers` (`Entity.java:1583-1589`) instead. Without this
        // guard a rider would be hurt twice for the same drop.
        if self.entity.has_vehicle().await {
            self.fall_distance.store(0.0);
            return;
        }

        if ground {
            let fall_distance = self.fall_distance.swap(0.0);
            let fall_distance = self
                .is_ignoring_fall_damage_from_current_impulse()
                .then(|| self.current_impulse_impact_pos.load())
                .flatten()
                .map_or(fall_distance, |impact_pos| {
                    let effective = impulse_limited_fall_distance(
                        fall_distance,
                        self.entity.pos.load().y,
                        impact_pos.y,
                    );
                    if effective <= 0.0 {
                        self.reset_current_impulse_context();
                    } else {
                        self.try_reset_current_impulse_context();
                    }
                    effective
                });
            if fall_distance <= 0.0
                || dont_damage
                || self.should_prevent_fall_damage()
                || self.should_prevent_fall_damage_in_area()
                || self.is_immune_to_fall_damage()
            {
                return;
            }
            let world = self.entity.world.load();
            let landed_pos = self.entity.get_pos_with_y_offset(0.2).0;
            let block = world.get_block(&landed_pos);
            let pumpkin_block = world.block_registry.get_pumpkin_block(block.id);
            if let Some(pumpkin_block) = pumpkin_block {
                pumpkin_block
                    .on_landed_upon(OnLandedUponArgs {
                        world: &world,
                        position: &landed_pos,
                        fall_distance,
                        entity: caller.as_ref(),
                    })
                    .await;
            } else {
                self.handle_fall_damage(&*caller, fall_distance, 1.0).await;
            }

            // `Entity.checkFallDamage` emits HIT_GROUND after the landing hook with the
            // supporting block in its context (`Entity.java:1550-1567`).
            crate::world::game_event::emit_game_event(
                &world,
                pumpkin_data::game_event::GameEvent::HitGround,
                self.entity.pos.load(),
                crate::world::game_event::GameEventContext::of_entity_with_block_state(
                    caller.clone(),
                    world.get_block_state(&landed_pos).id,
                ),
            )
            .await;
        } else if height_difference < 0.0 {
            let new_fall_distance = if !self.should_prevent_fall_damage()
                && !self.should_prevent_fall_damage_in_area()
            {
                let distance = self.fall_distance.load();
                distance - (height_difference as f32)
            } else {
                0f32
            };
            self.fall_distance.store(new_fall_distance);
        }
    }

    pub async fn handle_fall_damage(
        &self,
        caller: &dyn EntityBase,
        fall_distance: f32,
        damage_per_distance: f32,
    ) {
        let may_fly = if let Some(player) = caller.get_player() {
            player.abilities.lock().await.allow_flying
        } else {
            false
        };
        if may_fly {
            return;
        }

        // `Player.causeFallDamage` awards FALL_ONE_CM before its `super` call,
        // so an otherwise immune player still receives the distance statistic.
        if let Some(player) = caller.get_player()
            && let Some(amount) = fall_one_cm_stat_amount(fall_distance)
        {
            player
                .increment_custom_stat(CustomStatistic::FallOneCm, amount)
                .await;
        }

        if self.is_immune_to_fall_damage() {
            return;
        }

        // `LivingEntity.causeFallDamage` reaches `Entity.causeFallDamage`
        // (`Entity.java:1574-1581`) through `super` before applying its own damage, and that is
        // what hurts whoever is riding the falling entity.
        self.propagate_fall_to_passengers(fall_distance, damage_per_distance)
            .await;

        let damage = caller.calculate_fall_damage(f64::from(fall_distance), damage_per_distance);
        if damage > 0 {
            #[allow(clippy::cast_precision_loss)]
            let check_damage = self.damage(caller, damage as f32, DamageType::FALL).await; // Fall
            if check_damage {
                self.entity
                    .play_sound(caller.get_fall_sound(fall_distance as i32));
            }
        }
    }

    /// Vanilla `Entity.propagateFallToPassengers` (`Entity.java:1583-1589`): a falling vehicle
    /// passes the same fall to everyone riding it, so a player who rides a horse off a cliff is
    /// hurt alongside the horse.
    ///
    /// Boxed because the recursion (vehicle -> passenger -> its own passengers) would otherwise
    /// give the future an infinite type.
    fn propagate_fall_to_passengers<'a>(
        &'a self,
        fall_distance: f32,
        damage_per_distance: f32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let passengers = self.entity.passengers.lock().await.clone();
            for passenger in passengers {
                if let Some(living) = passenger.get_living_entity() {
                    living
                        .handle_fall_damage(passenger.as_ref(), fall_distance, damage_per_distance)
                        .await;
                }
            }
        })
    }

    /// Vanilla `LivingEntity.calculateFallDamage` (`LivingEntity.java:1845-1852`), including the
    /// `FALL_DAMAGE_MULTIPLIER` attribute that horses, camels and llamas set to 0.5.
    ///
    /// Split out so a subclass override (`Goat.calculateFallDamage`,
    /// `Frog.calculateFallDamage`) can reach the `LivingEntity`-level result without recursing
    /// back into itself.
    pub fn default_calculate_fall_damage(&self, fall_distance: f64, damage_modifier: f32) -> i32 {
        if self.is_immune_to_fall_damage() {
            return 0;
        }
        let fall_power =
            fall_distance + 1.0E-6 - self.get_attribute_value(&Attributes::SAFE_FALL_DISTANCE);
        let damage = fall_power
            * f64::from(damage_modifier)
            * self.get_attribute_value(&Attributes::FALL_DAMAGE_MULTIPLIER);
        #[allow(clippy::cast_possible_truncation)]
        {
            damage.floor() as i32
        }
    }

    pub async fn get_death_message(
        dyn_self: &dyn EntityBase,
        damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> TextComponent {
        match damage_type.death_message_type {
            DeathMessageType::Default => {
                if let Some(cause) = cause
                    && source.is_some()
                {
                    TextComponent::translate_cross(
                        format!("death.attack.{}.player", damage_type.message_id),
                        format!("death.attack.{}.player", damage_type.message_id),
                        [
                            dyn_self.get_display_name().await,
                            cause.get_display_name().await,
                        ],
                    )
                } else {
                    TextComponent::translate_cross(
                        format!("death.attack.{}", damage_type.message_id),
                        format!("death.attack.{}", damage_type.message_id),
                        [dyn_self.get_display_name().await],
                    )
                }
            }
            DeathMessageType::FallVariants => {
                //TODO
                TextComponent::translate_cross(
                    translation::java::DEATH_FELL_ACCIDENT_GENERIC,
                    translation::bedrock::DEATH_FELL_ACCIDENT_GENERIC,
                    [dyn_self.get_display_name().await],
                )
            }
            DeathMessageType::IntentionalGameDesign => TextComponent::text("[")
                .add_child(TextComponent::translate_cross(
                    format!("death.attack.{}.message", damage_type.message_id),
                    format!("death.attack.{}.message", damage_type.message_id),
                    [dyn_self.get_display_name().await],
                ))
                .add_child(TextComponent::text("]")),
        }
    }

    /// Vanilla `Villager.releaseAllPois` (called from `Villager.die`):
    /// release any claimed POI ticket (bed, job site) on death so it isn't
    /// held forever by an entity that no longer exists. Uses the generic
    /// `EntityBase::get_home_pos`/`get_job_site_pos` hooks (default `None`)
    /// rather than downcasting to `VillagerEntity`, so any future
    /// POI-claiming mob gets this for free.
    async fn release_claimed_pois(world: &crate::world::World, dyn_self: &dyn EntityBase) {
        if let Some(home) = dyn_self.get_home_pos() {
            world.release_poi(home).await;
        }
        if let Some(job_site) = dyn_self.get_job_site_pos() {
            world.release_poi(job_site).await;
        }
    }

    #[allow(clippy::too_many_lines)]
    pub async fn on_death(
        &self,
        damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) {
        let world = self.entity.world.load();
        if self
            .dead
            .compare_exchange(false, true, Relaxed, Relaxed)
            .is_ok()
        {
            self.movement_input.store(Vector3::default());
            self.jumping.store(false, Relaxed);

            let Some(dyn_self) = world.get_entity_by_id(self.entity.entity_id) else {
                // Damage can finish after the entity has been removed by another task.
                // Vanilla's entity remains addressable during die(); do not bring down
                // the server when Pumpkin observes the equivalent race.
                return;
            };

            Self::release_claimed_pois(&world, &*dyn_self).await;

            self.apply_killed_effects(&world).await;

            // Statistics updates
            self.update_death_stats(&*dyn_self, cause).await;

            // Raider.die (Raider.java): report the removal to the owning raid, hand out
            // Hero of the Village credit, and clear the wave-leader slot if this was the banner
            // carrier.
            if let Some(membership) = self.raid_membership.swap(None) {
                let mut raids = world.raids.lock().await;
                if let Some(raid) = raids.raid_mut(membership.raid_id) {
                    if membership.is_patrol_leader {
                        raid.remove_leader(membership.wave);
                    }
                    if let Some(killer) = cause
                        && killer.get_entity().entity_type.id == EntityType::PLAYER.id
                    {
                        raid.add_hero_of_the_village(killer.get_entity().entity_uuid);
                    }
                    raid.remove_from_raid(membership.wave, self.entity.entity_uuid, 0.0, false);
                }
            }

            // Plays the death sound
            world.send_entity_status(
                &self.entity,
                EntityStatus::Death,
                Some(ActorEventType::Death),
            );
            let looting_level;
            let tool = if let Some(cause_ent) = cause {
                if let Some(player) = cause_ent
                    .cast_any()
                    .downcast_ref::<crate::entity::player::Player>()
                {
                    let hand_stack = player
                        .inventory()
                        .get_stack_in_hand(pumpkin_util::Hand::Right)
                        .await;
                    looting_level = hand_stack
                        .get_enchantment_level(&Enchantment::LOOTING)
                        .max(0) as u32;
                    (!hand_stack.is_empty()).then(|| hand_stack.clone())
                } else {
                    looting_level = 0;
                    None
                }
            } else {
                looting_level = 0;
                None
            };

            let is_raining = world.is_raining().await;
            let is_thundering = world.is_thundering().await;

            let params = LootContextParameters {
                // `dropAllDeathLoot`: the player-kill branch is driven by the memory window, so
                // a mob that a player tagged and that then died to fall damage, fire or drowning
                // still drops and still awards experience.
                killed_by_player: Some(self.get_last_hurt_by_player_memory_time() > 0),
                this_entity: Some(self.entity.entity_type),
                killer_entity: cause.map(|c| c.get_entity().entity_type),
                direct_killer_entity: source.map(|s| s.get_entity().entity_type),
                position: Some(self.entity.pos.load()),
                world_time: world.level_info.load().day_time as u64,
                damage_type: Some(damage_type),
                tool,
                is_raining: Some(is_raining),
                is_thundering: Some(is_thundering),
                is_on_fire: Some(
                    self.entity
                        .fire_ticks
                        .load(std::sync::atomic::Ordering::Relaxed)
                        > 0,
                ),
                ..Default::default()
            };

            if let Some(mob) = dyn_self.get_mob() {
                mob.on_mob_death(cause).await;
            }

            // LivingEntity.die, line 1472: this.gameEvent(GameEvent.ENTITY_DIE), fired
            // right before dropAllDeathLoot. Entity::gameEvent(event) (Entity.java:1431)
            // defaults the source entity to `this`.
            crate::world::game_event::emit_game_event(
                &world,
                pumpkin_data::game_event::GameEvent::EntityDie,
                self.entity.pos.load(),
                crate::world::game_event::GameEventContext::of_entity(dyn_self.clone()),
            )
            .await;

            // `LivingEntity.dropAllDeathLoot` only reaches the loot table when `shouldDropLoot`
            // holds: the mob_drops game rule, and for everything but a monster, not being a
            // baby (`LivingEntity.shouldDropLoot` / `Monster.shouldDropLoot`).
            let is_baby = self.entity.age.load(Relaxed) < 0;
            let is_monster = self.entity.entity_type.category == &MobCategory::MONSTER;
            let should_drop_loot =
                world.level_info.load().game_rules.mob_drops && (is_monster || !is_baby);
            if should_drop_loot {
                self.drop_loot(params.clone(), dyn_self.as_ref()).await;
            }

            // `LivingEntity.die` (`world/entity/LivingEntity.java:1474`) calls
            // `createWitherRose(killer)` directly after `dropAllDeathLoot`, with `killer`
            // being `getKillCredit()` (:1438) - `cause` here.
            self.create_wither_rose(cause).await;

            // Award experience
            let always_drops_experience = dyn_self.get_player().is_some();
            if !self.skip_drop_experience.load(Ordering::Relaxed)
                && (always_drops_experience
                    || (params.killed_by_player.unwrap_or(false)
                        && world.level_info.load().game_rules.mob_drops))
            {
                let amount = dyn_self.get_experience_reward(cause);
                if amount > 0 {
                    ExperienceOrbEntity::spawn(&world, self.entity.pos.load(), amount).await;
                }
            }
            self.entity.pose.store(EntityPose::Dying);

            // `LivingEntity.dropAllDeathLoot` (`LivingEntity.java:1508-1516`) always calls
            // `this.dropEquipment(level)`, but the base implementation
            // (`LivingEntity.java:1519-1520`) is empty and `Mob` never overrides it: a mob's
            // armour/hand items are dropped by its `dropCustomDeathLoot` override
            // (`Mob.java:892-910`, chance-per-slot with looting and damage randomization),
            // which is what `Self::drop_equipment` below models. `Player.dropEquipment`
            // (`Player.java:551-557`) is the *only* override of the real method: it drops every
            // equipped item unconditionally (no chance roll, no damage randomization) via
            // `inventory.dropAll()` (`Inventory.java:538-541`, `EntityEquipment.dropAll`,
            // `EntityEquipment.java:62-68`), gated only by the `keepInventory` gamerule. That
            // path is implemented in `Player::handle_killed`, so the mob-style chance roll must
            // not also run for a player here - it would drop armour far less often than vanilla,
            // with spurious damage randomization, and ignore `keepInventory` entirely.
            if dyn_self.get_player().is_none() {
                self.drop_equipment(
                    looting_level,
                    params.killed_by_player.unwrap_or(false),
                    should_drop_loot,
                )
                .await;
            }

            // Broadcast death message if it's a player and the gamerule is enabled
            self.broadcast_death_message(&*dyn_self, damage_type, source, cause)
                .await;

            self.reset_effects_and_attributes().await;
        }
    }

    async fn drop_equipment(
        &self,
        looting_level: u32,
        killed_by_player: bool,
        should_drop_loot: bool,
    ) {
        let world = self.entity.world.load();
        let block_pos = self.entity.block_pos.load();

        let drop_chances = self.equipment_drop_chances.lock().await;

        let slots_to_drop: Vec<EquipmentSlot> = {
            let mut slots: Vec<_> = self.equipment_slots.values().cloned().collect();
            slots.push(EquipmentSlot::MAIN_HAND);
            slots
        };

        for slot in &slots_to_drop {
            let mut chance = drop_chances
                .get(slot)
                .copied()
                .unwrap_or(DEFAULT_EQUIPMENT_DROP_CHANCE);
            // Vanilla `Mob.dropCustomDeathLoot` only drops a slot for a player kill or a
            // preserved chance above 1.0, and skips `PREVENT_EQUIPMENT_DROP`
            // (`Mob.java:895-907`).
            let preserve = chance > 1.0;
            if !should_drop_loot || chance == 0.0 || (!killed_by_player && !preserve) {
                continue;
            }
            let item = self
                .entity_equipment
                .lock()
                .await
                .equipment
                .get(slot)
                .cloned()
                .unwrap_or_else(|| ItemStack::EMPTY.clone());
            if item.is_empty() || Self::item_prevents_equipment_drop(&item) {
                continue;
            }
            // Vanilla approximation: EnchantmentHelper.processEquipmentDropChance
            // adds lootingLevel * 0.01 to the per-slot equipment drop chance.
            chance += looting_level as f32 * 0.01;
            chance = chance.min(1.0);
            if rand::random::<f32>() >= chance {
                continue;
            }
            let mut item = self
                .entity_equipment
                .lock()
                .await
                .equipment
                .remove(slot)
                .unwrap_or_else(|| ItemStack::EMPTY.clone());
            if item.is_empty() {
                continue;
            }
            // Vanilla approximation: Mob.dropCustomDeathLoot applies random
            // damage to dropped equipment using two chained random calls:
            // setDamageValue(maxDamage - random.nextInt(1 + random.nextInt(max(maxDamage - 3, 1))))
            if !preserve && let Some(max_damage) = item.get_max_damage() {
                let mut rng = rand::rng();
                let inner = rng.random_range(0..(max_damage - 3).max(1));
                let outer = rng.random_range(0..=inner);
                item.set_damage((max_damage - outer).max(0));
            }
            world.drop_stack(&block_pos, item).await;
        }
    }

    /// `EnchantmentHelper.has(itemStack, PREVENT_EQUIPMENT_DROP)` for the mob death path
    /// (`Mob.java:904-906`). The workspace's enchantment effect table is the existing source
    /// for the Vanishing Curse effect.
    fn item_prevents_equipment_drop(stack: &ItemStack) -> bool {
        stack
            .get_data_component::<EnchantmentsImpl>()
            .is_some_and(|enchantments| {
                enchantments.enchantment.iter().any(|(enchantment, level)| {
                    *level > 0
                        && crate::enchantment::effects_for(enchantment)
                            .iter()
                            .any(|effect| {
                                matches!(
                                    effect,
                                    crate::enchantment::EnchantmentEffect::PreventEquipmentDrop
                                )
                            })
                })
            })
    }

    async fn broadcast_death_message(
        &self,
        dyn_self: &dyn EntityBase,
        damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) {
        let world = self.entity.world.load();
        let show_death_messages = { world.level_info.load().game_rules.show_death_messages };
        // `TamableAnimal.die` (`net/minecraft/world/entity/TamableAnimal.java:223-230`)
        // sends the combat death message to a server-player owner before delegating to
        // `LivingEntity.die`. The generic player broadcast below does not cover pets.
        if show_death_messages
            && let Some(owner_uuid) = dyn_self
                .get_mob()
                .and_then(crate::entity::mob::Mob::get_owner_uuid)
            && let Some(owner) = world.get_player_by_uuid(owner_uuid)
        {
            let death_message = Self::get_death_message(dyn_self, damage_type, source, cause).await;
            owner.send_system_message(&death_message).await;
        }
        if self.entity.entity_type == &EntityType::PLAYER && show_death_messages {
            //TODO: KillCredit
            let death_message = Self::get_death_message(dyn_self, damage_type, source, cause).await;
            if let Some(server) = world.server.upgrade() {
                for player in server.get_all_players() {
                    player.send_system_message(&death_message).await;
                }
            }
        }
    }

    #[expect(clippy::too_many_lines)]
    async fn update_death_stats(&self, dyn_self: &dyn EntityBase, cause: Option<&dyn EntityBase>) {
        if let Some(victim_player) = dyn_self.get_player() {
            victim_player
                .increment_stat(StatisticCategory::Custom, CustomStatistic::Deaths as i32, 1)
                .await;
            victim_player
                .set_stat(
                    StatisticCategory::Custom,
                    CustomStatistic::TimeSinceDeath as i32,
                    0,
                )
                .await;
            if let Some(killer_entity) = cause.map(EntityBase::get_entity) {
                victim_player
                    .increment_stat(
                        StatisticCategory::KilledBy,
                        killer_entity.entity_type.id as i32,
                        1,
                    )
                    .await;
            }
        }

        if let Some(killer_player) = cause.and_then(|c| c.get_player()) {
            let victim_is_player = dyn_self.get_player().is_some();
            if victim_is_player {
                killer_player
                    .increment_stat(
                        StatisticCategory::Custom,
                        CustomStatistic::PlayerKills as i32,
                        1,
                    )
                    .await;
            } else {
                killer_player
                    .increment_stat(
                        StatisticCategory::Custom,
                        CustomStatistic::MobKills as i32,
                        1,
                    )
                    .await;

                let resource_name = self.entity.entity_type.resource_name;
                let criterion_key = format!("minecraft:{resource_name}");
                killer_player
                    .trigger_advancement(
                        crate::entity::player::advancement::trigger::AdvancementTrigger::PlayerKilledEntity {
                            entity_type_resource: criterion_key,
                        }
                    )
                    .await;

                if resource_name == "skeleton" {
                    let distance_sq = killer_player
                        .position()
                        .squared_distance_to_vec(&self.entity.pos.load());
                    if distance_sq >= 2500.0 {
                        killer_player.trigger_advancement(crate::entity::player::advancement::trigger::AdvancementTrigger::SniperDuel).await;
                    }
                }

                if resource_name == "phantom" {
                    killer_player.trigger_advancement(crate::entity::player::advancement::trigger::AdvancementTrigger::TwoBirdsOneArrow).await;
                }

                let held_item = killer_player.inventory().held_item().await;
                let is_crossbow = held_item.item.registry_key == "crossbow";
                if is_crossbow {
                    killer_player.trigger_advancement(crate::entity::player::advancement::trigger::AdvancementTrigger::Arbalistic).await;
                }
            }
            killer_player
                .increment_stat(
                    StatisticCategory::Killed,
                    self.entity.entity_type.id as i32,
                    1,
                )
                .await;

            // `Entity.awardKillScore` triggers the player-killed-entity criterion, while the
            // `ServerPlayer` override updates total/player kill objectives
            // (`Entity.java:2021-2024`; `ServerPlayer.java:955-968`). The statistic updates above
            // cover the persisted counters; update the live scoreboard criteria and the player
            // victim criterion here for the existing death path.
            if killer_player.entity_id() != self.entity.entity_id {
                let victim_resource =
                    format!("minecraft:{}", self.entity.entity_type.resource_name);
                if victim_is_player {
                    killer_player
                        .trigger_advancement(
                            crate::entity::player::advancement::trigger::AdvancementTrigger::PlayerKilledEntity {
                                entity_type_resource: victim_resource,
                            },
                        )
                        .await;
                }

                let world = self.entity.world.load();
                let killer_name = killer_player.gameprofile.name.clone();
                let mut scoreboard = world.scoreboard.lock().await;
                let victim_name = crate::world::scoreboard::entity_scoreboard_name(dyn_self);
                let killer_team_color = scoreboard
                    .get_team_for_scoreboard_name(&killer_name)
                    .map(|team| crate::world::scoreboard::named_color_to_str(team.color));
                let victim_team_color = scoreboard
                    .get_team_for_scoreboard_name(&victim_name)
                    .map(|team| crate::world::scoreboard::named_color_to_str(team.color));
                let mut criteria = vec![(killer_name.clone(), "totalKillCount".to_string())];
                if victim_is_player {
                    criteria.push((killer_name.clone(), "playerKillCount".to_string()));
                }
                if let Some(color) = victim_team_color {
                    criteria.push((killer_name.clone(), format!("teamkill.{color}")));
                }
                if let Some(color) = killer_team_color {
                    criteria.push((victim_name, format!("killedByTeam.{color}")));
                }

                for (holder, criterion) in criteria {
                    let updates: Vec<(String, i32)> = scoreboard
                        .get_objectives()
                        .values()
                        .filter(|objective| objective.criterion == criterion)
                        .map(|objective| {
                            let value = scoreboard
                                .get_score_value(&holder, &objective.name)
                                .unwrap_or(0)
                                .saturating_add(1);
                            (objective.name.clone(), value)
                        })
                        .collect();
                    for (objective, value) in updates {
                        scoreboard
                            .set_score_value(world.as_ref(), holder.clone(), objective, value)
                            .await;
                    }
                }
            }
        }
    }

    /// Vanilla `LivingEntity.createWitherRose`
    /// (`world/entity/LivingEntity.java:1488-1506`), called from `die` at :1474 immediately
    /// after `dropAllDeathLoot`.
    ///
    /// When the kill credit belongs to a wither, a wither rose is planted on the victim's
    /// block position if `mobGriefing` is on and the block is air and the rose can survive
    /// there. If either check fails - or `mobGriefing` is off - the rose drops as an item
    /// instead, so the player gets one either way.
    ///
    /// Deviation: vanilla spawns the `ItemEntity` at the victim's exact `getX/Y/Z`; this uses
    /// `World::drop_stack`, which is block-position based, matching every other drop path here.
    ///
    /// KNOWN LIMITATION - only the wither's *melee* kills reach this today. Vanilla passes
    /// `getKillCredit()`, which resolves a projectile to its shooter; Pumpkin has no such
    /// resolution, and `cause` is whatever the damage call site supplies. `Mob::attack`
    /// (`entity/mob/mod.rs:784`) supplies the attacking mob, so melee works. Wither skulls do
    /// not: `entity/projectile/wither_skull.rs:130` uses the bare `EntityBase::damage`, whose
    /// default (`entity/mod.rs:352-363`) passes `cause: None`, so a skull kill plants no rose.
    /// Closing that needs the shooter plumbed through as `cause` in `entity/projectile/`
    /// (arrows already do this at `arrow.rs:617`, but pass the *arrow* rather than the
    /// shooter, so owner resolution is still missing there too).
    async fn create_wither_rose(&self, killer: Option<&dyn EntityBase>) {
        let is_wither =
            killer.is_some_and(|k| k.get_entity().entity_type.id == EntityType::WITHER.id);
        if !is_wither {
            return;
        }

        let world = self.entity.world.load();
        let pos = self.entity.block_pos.load();
        let mut planted = false;

        if world.level_info.load().game_rules.mob_griefing {
            let state = Block::WITHER_ROSE.default_state;
            if world.get_block_state(&pos).is_air()
                && world.block_registry.can_place_at(
                    None,
                    None,
                    &**world,
                    None,
                    &Block::WITHER_ROSE,
                    state,
                    &pos,
                    None,
                    None,
                )
            {
                world
                    .set_block_state(&pos, state.id, BlockFlags::NOTIFY_ALL)
                    .await;
                planted = true;
            }
        }

        if !planted {
            world
                .drop_stack(&pos, ItemStack::new(1, &Item::WITHER_ROSE))
                .await;
        }
    }

    async fn drop_loot(&self, mut params: LootContextParameters, caller: &dyn EntityBase) {
        if let Some(loot_table) = &self.get_entity().entity_type.loot_table {
            // `LivingEntity.dropFromLootTable` passes `getLootTableSeed()` to the table
            // (`LivingEntity.java:1547-1551, 1577-1578`); carry the same optional seed through
            // the existing generated entity-table path.
            params.loot_table_seed = caller
                .get_mob()
                .map(Mob::get_loot_table_seed)
                .filter(|seed| *seed != 0);
            // `LootContext.Builder.create` resolves a table random sequence from the server
            // world seed (`LootContext.java:138-142`; `MinecraftServer.java:1766-1767`).
            params.world_seed = self.entity.world.load().level.seed.0;
            let pos = self.entity.block_pos.load();
            for stack in loot_table.get_loot(params) {
                self.entity.world.load().drop_stack(&pos, stack).await;
            }
        }
    }

    /// Vanilla `MobEffect.onMobRemoved` with `RemovalReason.KILLED`: `OozingMobEffect` leaves
    /// slimes behind and `WindChargedMobEffect` sets off a wind burst when the carrier dies.
    async fn apply_killed_effects(&self, world: &Arc<World>) {
        let effects: Vec<&'static StatusEffect> =
            self.active_effects.lock().await.keys().copied().collect();

        for effect_type in effects {
            if effect_type == &StatusEffect::OOZING {
                self.spawn_oozing_slimes(world).await;
            } else if effect_type == &StatusEffect::WEAVING {
                self.spawn_weaving_cobwebs(world).await;
            } else if effect_type == &StatusEffect::WIND_CHARGED {
                let pos = self.entity.pos.load();
                let height = f64::from(self.entity.entity_dimension.load().height);
                let gust_strength = f64::from(3.0 + rand::random::<f32>() * 2.0);
                world
                    .explode_knockback_only(
                        Vector3::new(pos.x, pos.y + height / 2.0, pos.z),
                        gust_strength,
                        1.0,
                    )
                    .await;
            }
        }
    }

    /// Vanilla `WeavingMobEffect.onMobRemoved`: two or three cobwebs scattered over the blocks
    /// around the carrier, taking any replaceable spot that has a sturdy top face beneath it. Only
    /// players ignore the `mob_griefing` game rule.
    async fn spawn_weaving_cobwebs(&self, world: &Arc<World>) {
        const PLACEMENT_ATTEMPTS: usize = 15;
        const SCAN_RADIUS: i32 = 1;

        let is_player = self.entity.entity_type.id == EntityType::PLAYER.id;
        if !is_player && !world.level_info.load().game_rules.mob_griefing {
            return;
        }

        let cobweb_count = rand::random_range(2..=3);
        let center = self.entity.block_pos.load();
        let mut positions: Vec<BlockPos> = Vec::with_capacity(cobweb_count);

        for _ in 0..PLACEMENT_ATTEMPTS {
            let position = BlockPos::new(
                center.0.x + rand::random_range(-SCAN_RADIUS..=SCAN_RADIUS),
                center.0.y + rand::random_range(-SCAN_RADIUS..=SCAN_RADIUS),
                center.0.z + rand::random_range(-SCAN_RADIUS..=SCAN_RADIUS),
            );
            if positions.contains(&position) {
                continue;
            }

            let below = position.down();
            if world.get_block_state_async(&position).await.replaceable()
                && world
                    .get_block_state_async(&below)
                    .await
                    .is_side_solid(BlockDirection::Up)
            {
                positions.push(position);
                if positions.len() >= cobweb_count {
                    break;
                }
            }
        }

        for position in positions {
            world
                .set_block_state(
                    &position,
                    Block::COBWEB.default_state.id,
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
            world.sync_world_event(WorldEvent::AnimationSpawnCobweb, position, 0);
        }
    }

    /// Vanilla `OozingMobEffect.onMobRemoved`: two size-2 slimes at the carrier's feet, capped by
    /// the room the `maxEntityCramming` game rule leaves once the slimes already within two blocks
    /// are counted.
    async fn spawn_oozing_slimes(&self, world: &Arc<World>) {
        const SLIME_SIZE: i32 = 2;
        const REQUESTED_SLIMES: i64 = 2;
        const RADIUS_TO_CHECK_SLIMES: f64 = 2.0;

        let max_entity_cramming = world.level_info.load().game_rules.max_entity_cramming;
        let nearby_slimes = if max_entity_cramming < 1 {
            0
        } else {
            let mut slimes = Vec::new();
            let own_id = self.entity.entity_id;
            world.extend_entities_in_box_where(
                &mut slimes,
                usize::try_from(max_entity_cramming).unwrap_or(usize::MAX),
                self.entity
                    .bounding_box
                    .load()
                    .expand_all(RADIUS_TO_CHECK_SLIMES),
                |entity| {
                    entity.get_entity().entity_type.id == EntityType::SLIME.id
                        && entity.get_entity().entity_id != own_id
                },
            );
            slimes.len()
        };

        let to_spawn = oozing_slimes_to_spawn(max_entity_cramming, nearby_slimes, REQUESTED_SLIMES);
        let pos = self.entity.pos.load();

        for _ in 0..to_spawn {
            let entity = Entity::new(
                world.clone(),
                Vector3::new(pos.x, pos.y + 0.5, pos.z),
                &EntityType::SLIME,
            );
            let slime = SlimeEntity::new(entity);
            slime.set_size(SLIME_SIZE, true);
            slime.get_entity().yaw.store(rand::random_range(0.0..360.0));
            world.spawn_entity(slime).await;
        }
    }

    /// Vanilla `AbsorptionMobEffect.onEffectStarted`: absorption is raised to `4 * (1 + amplifier)`
    /// rather than accumulated, so drinking a second absorption potion does not stack hearts, and
    /// `LivingEntity.setAbsorptionAmount` clamps the result to the max absorption attribute.
    async fn start_absorption(&self, amplifier: u8) {
        let max_absorption = self.get_max_absorption();
        let granted =
            absorption_after_application(self.absorption.load(), amplifier, max_absorption);
        self.set_absorption(granted).await;
    }

    /// Re-applies an effect's `MobEffect.AttributeTemplate` modifiers (`base * (amplifier + 1)`,
    /// `MobEffect.java:200-204`) into the local attribute map. Used when effects come back from
    /// disk, where vanilla instead restores the saved permanent modifiers.
    fn restore_effect_attribute_modifiers(&self, effect: &Effect) {
        for m in effect.effect_type.attribute_modifiers {
            let operation = match m.operation {
                Operation::AddValue => ModifierOperation::Add,
                Operation::AddMultipliedBase => ModifierOperation::MultiplyBase,
                Operation::AddMultipliedTotal => ModifierOperation::MultiplyTotal,
            };
            let modifier = Modifier {
                id: m.id.to_string(),
                amount: m.base_value * (f64::from(effect.amplifier) + 1.0),
                operation,
            };
            self.update_attribute(m.attribute, |inst| {
                inst.add_or_replace_modifier(modifier.clone());
            });
        }
    }

    /// Vanilla `MobEffectInstance.downgradeToHiddenEffect`: when the active instance runs out the
    /// nearest hidden one takes its place. If that one has run out as well the whole chain goes
    /// with it, matching vanilla's `hasRemainingDuration` check right after the downgrade.
    async fn expire_effect(&self, effect_type: &'static StatusEffect) {
        let promoted = {
            let mut hidden_effects = self.hidden_effects.lock().await;
            hidden_effects
                .remove(&effect_type)
                .filter(|chain| !chain.is_empty())
                .map(|mut chain| (chain.remove(0), chain))
        };

        self.remove_effect(effect_type).await;

        if let Some((next, rest)) = promoted
            && next.duration != 0
        {
            self.add_effect(next).await;
            if !rest.is_empty() {
                self.hidden_effects.lock().await.insert(effect_type, rest);
            }
        }
    }

    async fn tick_effects(&self) {
        let mut effects_to_remove = Vec::new();
        let mut effects_to_apply = Vec::new();

        {
            let mut effects = self.active_effects.lock().await;
            let mut hidden_effects = self.hidden_effects.lock().await;
            let entity_age = self.entity.age.load(Relaxed);
            for effect in effects.values_mut() {
                if effect.duration == 0 {
                    effects_to_remove.push(effect.effect_type);
                    continue;
                }

                let tick_duration = if effect.duration == -1 {
                    entity_age
                } else {
                    effect.duration
                };

                if Self::should_apply_effect_tick(effect, tick_duration) {
                    effects_to_apply.push((effect.effect_type, effect.amplifier));
                }

                if effect.duration != -1 {
                    effect.duration -= 1;
                }

                // Vanilla `MobEffectInstance.tickDownDuration` ticks the whole hidden chain, so a
                // covered instance keeps running out while it is not the active one.
                if let Some(chain) = hidden_effects.get_mut(effect.effect_type) {
                    for hidden_effect in chain.iter_mut() {
                        if hidden_effect.duration > 0 {
                            hidden_effect.duration -= 1;
                        }
                    }
                }
            }
        }

        // Call the central removal function for each expired effect
        // This will now trigger your logs and absorption resets!
        for effect_type in effects_to_remove {
            self.expire_effect(effect_type).await;
        }

        for (effect_type, amplifier) in effects_to_apply {
            self.apply_effect_tick(effect_type, amplifier).await;
        }
    }

    /// Determines if an effect should apply its tick effect this frame
    /// Based on vanilla Minecraft's effect tick frequencies
    ///
    /// TODO: villager, beacon, and other effects.
    fn should_apply_effect_tick(effect: &pumpkin_data::potion::Effect, duration: i32) -> bool {
        let effect_type = effect.effect_type;

        if effect_type == &StatusEffect::REGENERATION {
            effect_ticks_this_tick(50, effect.amplifier, duration)
        } else if effect_type == &StatusEffect::POISON {
            effect_ticks_this_tick(25, effect.amplifier, duration)
        } else if effect_type == &StatusEffect::WITHER {
            effect_ticks_this_tick(40, effect.amplifier, duration)
        } else if effect_type == &StatusEffect::HUNGER {
            // `HungerMobEffect.shouldApplyEffectTickThisTick`: every tick.
            true
        } else if effect_type == &StatusEffect::ABSORPTION {
            // `AbsorptionMobEffect.shouldApplyEffectTickThisTick`: every tick.
            true
        } else if effect_type == &StatusEffect::SATURATION {
            // Saturation every tick
            true
        } else if effect_type == &StatusEffect::BAD_OMEN {
            // BadOmenMobEffect.shouldApplyEffectTickThisTick: every tick.
            true
        } else if effect_type == &StatusEffect::RAID_OMEN {
            // RaidOmenMobEffect.shouldApplyEffectTickThisTick: only on the last tick before
            // natural expiry.
            duration == 1
        } else {
            // Other effects that don't tick
            false
        }
    }

    /// Applies the actual effect to the entity
    /// This is called by `tick_effects` when an effect should trigger this tick
    async fn apply_effect_tick(&self, effect_type: &'static StatusEffect, amplifier: u8) {
        if effect_type == &StatusEffect::REGENERATION {
            let current_health = self.health.load();
            let max_health = self.get_max_health();
            if current_health < max_health && current_health > 0.0 {
                self.heal(1.0);
            }
        } else if effect_type == &StatusEffect::POISON {
            let current_health = self.health.load();
            if current_health > 1.0
                && let Some(dyn_self) = self
                    .entity
                    .world
                    .load()
                    .get_entity_by_id(self.entity.entity_id)
            {
                // `PoisonMobEffect.applyEffectTick` deals a flat point of magic damage once the
                // carrier is above one health, so it can take them below a single heart even
                // though it never lands the killing blow itself.
                dyn_self.damage(&*dyn_self, 1.0, DamageType::MAGIC).await;
            }
        } else if effect_type == &StatusEffect::ABSORPTION {
            // `AbsorptionMobEffect.applyEffectTick` returns false once the hearts are gone, which
            // ends the effect instead of leaving it running with nothing to absorb.
            if self.absorption.load() <= 0.0 {
                self.remove_effect(effect_type).await;
            }
        } else if effect_type == &StatusEffect::WITHER {
            let damage_amount = 1.0;
            let dyn_self = self
                .entity
                .world
                .load()
                .get_entity_by_id(self.entity.entity_id);
            if let Some(dyn_self) = dyn_self {
                dyn_self
                    .damage(&*dyn_self, damage_amount, DamageType::WITHER)
                    .await;
            }
        } else if effect_type == &StatusEffect::HUNGER {
            let world = self.entity.world.load();
            if let Some(entity) = world.get_entity_by_id(self.entity.entity_id)
                && let Some(player) = entity.get_player()
            {
                // `HungerMobEffect.applyEffectTick`: a twentieth of this lands every tick.
                let exhaustion = 0.005 * (f32::from(amplifier) + 1.0);
                player.hunger_manager.add_exhaustion(exhaustion);
            }
            drop(world);
        } else if effect_type == &StatusEffect::SATURATION {
            let world = self.entity.world.load();
            if let Some(entity) = world.get_entity_by_id(self.entity.entity_id)
                && let Some(player) = entity.get_player()
            {
                // Add hunger and saturation
                let hunger = amplifier + 1;
                player.hunger_manager.add_hunger(hunger);
                player.hunger_manager.add_saturation(hunger as f32 * 2.0);
            }
        } else if effect_type == &StatusEffect::BAD_OMEN {
            // BadOmenMobEffect.applyEffectTick: on entering a real village (with room left
            // to raise the raid's omen level), convert BAD_OMEN into RAID_OMEN and remember
            // where, so RaidOmenMobEffect can trigger the raid when RAID_OMEN expires.
            let world = self.entity.world.load();
            if let Some(entity) = world.get_entity_by_id(self.entity.entity_id)
                && let Some(player) = entity.get_player()
                && player.gamemode.load() != pumpkin_util::GameMode::Spectator
                && world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful
            {
                let pos = BlockPos::floored_v(self.entity.pos.load());
                if world.is_close_to_village(pos, 1).await {
                    let raids = world.raids.lock().await;
                    let raid_ok = raids.raid_omen_level_near(pos).is_none_or(|level| {
                        level < crate::world::raid::DEFAULT_MAX_RAID_OMEN_LEVEL
                    });
                    drop(raids);
                    if raid_ok {
                        self.remove_effect(&StatusEffect::BAD_OMEN).await;
                        self.add_effect(Effect {
                            effect_type: &StatusEffect::RAID_OMEN,
                            duration: 600,
                            amplifier,
                            ambient: false,
                            show_particles: true,
                            show_icon: true,
                            blend: false,
                        })
                        .await;
                        player.raid_omen_position.store(Some(pos));
                    }
                }
            }
        } else if effect_type == &StatusEffect::RAID_OMEN {
            // RaidOmenMobEffect.applyEffectTick: on expiry, trigger/extend the raid at the
            // stored position, then let the effect's own duration reach 0 as normal.
            let world = self.entity.world.load();
            if let Some(entity) = world.get_entity_by_id(self.entity.entity_id)
                && let Some(player) = entity.get_player()
                && let Some(pos) = player.raid_omen_position.swap(None)
            {
                let mut raids = world.raids.lock().await;
                raids.create_or_extend_raid(&world, player, pos).await;
            }
        }
    }

    /// Tries to use a totem of undying from the entity's hands. If successful, applies the totem effects and returns true.
    async fn try_use_death_protector(&self, caller: &dyn EntityBase) -> bool {
        for hand in Hand::all() {
            let mut stack = self.get_stack_in_hand(caller, hand).await;

            // Clear the stack and use the totem of undying
            if stack.get_data_component::<DeathProtectionImpl>().is_some() {
                let mut resurrect_event =
                    crate::plugin::api::events::entity::entity_resurrect::EntityResurrectEvent::new(
                        self.entity.entity_id,
                    );
                if let Some(server) = self.entity.world.load().server.upgrade() {
                    server
                        .plugin_manager
                        .fire(&server, &mut resurrect_event)
                        .await;
                }
                if resurrect_event.cancelled {
                    return false;
                }

                stack.clear();
                let slot = match hand {
                    Hand::Right => EquipmentSlot::MAIN_HAND,
                    Hand::Left => EquipmentSlot::OFF_HAND,
                };
                if let Some(player) = caller.get_player() {
                    player
                        .inventory()
                        .entity_equipment
                        .lock()
                        .await
                        .equipment
                        .insert(slot, stack);
                } else {
                    self.entity_equipment
                        .lock()
                        .await
                        .equipment
                        .insert(slot, stack);
                }
                self.set_health(1.0);
                self.entity.world.load().send_entity_status(
                    &self.entity,
                    EntityStatus::ProtectedFromDeath,
                    Some(ActorEventType::TalismanActivate),
                );

                self.remove_all_effects().await;

                // Set Absorption, Regeneration, and Fire Resistance effects
                self.add_effect(Effect {
                    effect_type: &StatusEffect::ABSORPTION,
                    duration: 100,
                    amplifier: 1,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                })
                .await;
                self.add_effect(Effect {
                    effect_type: &StatusEffect::REGENERATION,
                    duration: 900,
                    amplifier: 1,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                })
                .await;
                self.add_effect(Effect {
                    effect_type: &StatusEffect::FIRE_RESISTANCE,
                    duration: 800,
                    amplifier: 0,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                })
                .await;

                return true;
            }
        }

        false
    }

    async fn damage_armor_items(
        &self,
        caller: &dyn EntityBase,
        damage_amount: f32,
        damage_type: &DamageType,
        helmet_only: bool,
    ) {
        // Vanilla's base `hurtArmor` and `hurtHelmet` are empty
        // (`LivingEntity.java:1881-1885`). The live overrides are Player's humanoid armor
        // hooks (`Player.java:738-745`), Wolf's body armor hook (`Wolf.java:443-444`), and
        // Horse's body armor hook (`Horse.java:233-234`). Keep durability on those same
        // concrete entities; generic living mobs must not lose armor merely because they have
        // armor attributes or equipment.
        let entity_type = self.entity.entity_type;
        let supported = entity_type == &EntityType::PLAYER
            || entity_type == &EntityType::WOLF
            || entity_type == &EntityType::HORSE;
        if !supported {
            return;
        }

        // Formula: armor loses floor(incoming_damage / 4) durability, minimum 1.
        let armor_damage = (damage_amount / 4.0).floor().max(1.0) as i32;
        let mut equipment_updates = Vec::new();

        // TODO: Implement DAMAGE_RESISTANT component checks (e.g. netherite vs fire).

        let armor_slots: Vec<(usize, ItemStack, EquipmentSlot)> = {
            let equipment_lock = self.entity_equipment.lock().await;
            self.equipment_slots
                .iter()
                .filter(|(_, slot)| hurt_armor_slot(entity_type, slot, helmet_only))
                .filter_map(|(index, slot)| {
                    equipment_lock
                        .equipment
                        .get(slot)
                        .cloned()
                        .map(|stack| (*index, stack, slot.clone()))
                })
                .collect()
        };

        for (slot_index, mut stack, slot) in armor_slots {
            if stack.is_empty() || armor_resists_damage(&stack, damage_type) {
                continue;
            }

            let takes_damage = stack
                .get_data_component::<EquippableImpl>()
                .is_none_or(|equippable| equippable.damage_on_hurt);

            if takes_damage {
                let broken_item = stack.clone();
                let slot_result = stack.damage_item(armor_damage);
                if slot_result != pumpkin_data::item_stack::DamageResult::Untouched {
                    if slot_result == pumpkin_data::item_stack::DamageResult::Broken {
                        // Vanilla armor damage calls `onEquippedItemBroken` while the broken item
                        // is still available (`LivingEntity.java:3845-3848`), which broadcasts
                        // the break status and removes attribute modifiers; the client then plays
                        // `breakItem`'s particles (`LivingEntity.java:1439-1448`) in response.
                        self.on_equipped_item_broken(&broken_item, &slot).await;
                        self.spawn_item_particles(&broken_item, 5);
                    }
                    equipment_updates.push((slot.clone(), stack.clone()));
                    if let Some(player) = caller.get_player() {
                        player
                            .enqueue_slot_set_packet(&CSetPlayerInventory::new(
                                (slot_index as i32).into(),
                                &ItemStackSerializer::from(stack),
                            ))
                            .await;
                    }
                }
            }
        }

        if !equipment_updates.is_empty() {
            self.send_equipment_changes(&equipment_updates);
        }
    }

    pub async fn held_item(&self, caller: &dyn EntityBase) -> ItemStack {
        if let Some(player) = caller.get_player() {
            return player.inventory.held_item().await;
        }
        let equipment = self.entity_equipment.lock().await;
        equipment
            .equipment
            .get(&EquipmentSlot::MAIN_HAND)
            .cloned()
            .unwrap_or_else(|| ItemStack::EMPTY.clone())
    }

    /// Vanilla `LivingEntity.getActiveItem` (`LivingEntity.java:2235-2241`) returns the item
    /// being used, falling back to the main-hand item when no use is active.
    pub async fn active_item(&self, caller: &dyn EntityBase) -> ItemStack {
        if caller.is_spectator() {
            return ItemStack::EMPTY.clone();
        }

        let main_hand = self.held_item(caller).await;
        let item_in_use = self.item_in_use.lock().await;
        active_item_for_state(self.is_using_item(), item_in_use.as_ref(), &main_hand)
    }

    pub async fn get_stack_in_hand(&self, caller: &dyn EntityBase, hand: Hand) -> ItemStack {
        match hand {
            Hand::Left => self.off_hand_item(caller).await,
            Hand::Right => self.held_item(caller).await,
        }
    }

    /// getOffHandStack in source
    pub async fn off_hand_item(&self, caller: &dyn EntityBase) -> ItemStack {
        if let Some(player) = caller.get_player() {
            return player.inventory.off_hand_item().await;
        }
        let Some(slot) = self.equipment_slots.get(&PlayerInventory::OFF_HAND_SLOT) else {
            return ItemStack::EMPTY.clone();
        };
        let equipment = self.entity_equipment.lock().await;
        equipment
            .equipment
            .get(slot)
            .cloned()
            .unwrap_or_else(|| ItemStack::EMPTY.clone())
    }

    /// Vanilla `LivingEntity.isHolding` (`LivingEntity.java:2243-2249`) checks both hand slots.
    #[must_use]
    pub async fn is_holding(&self, caller: &dyn EntityBase, item: &Item) -> bool {
        self.held_item(caller).await.item.id == item.id
            || self.off_hand_item(caller).await.item.id == item.id
    }

    pub fn can_take_damage(&self) -> bool {
        !self.entity.invulnerable.load(Ordering::Relaxed) && self.is_part_of_game()
    }

    pub fn is_part_of_game(&self) -> bool {
        !self.is_spectator() && self.entity.is_alive()
    }

    /// Vanilla `LivingEntity.canEquipWithDispenser` (`LivingEntity.java:3860-3874`) accepts a
    /// dispensable equippable item only when its slot is empty and its entity restriction allows
    /// this entity.
    pub async fn can_equip_with_dispenser(&self, item_stack: &ItemStack) -> bool {
        if !self.entity.is_alive() || self.is_spectator() {
            return false;
        }

        let Some(equippable) = item_stack.get_data_component::<EquippableImpl>() else {
            return false;
        };
        if !equippable.dispensable {
            return false;
        }

        let allowed = equippable
            .allowed_entities
            .as_ref()
            .is_none_or(|allowed| match allowed {
                pumpkin_data::data_component_impl::IDSet::IDs(ids) => ids
                    .iter()
                    .any(|entity_type| entity_type.id == self.entity.entity_type.id),
                pumpkin_data::data_component_impl::IDSet::Tag(tag) => {
                    self.entity.entity_type.is_tagged_with(tag).unwrap_or(false)
                }
            });
        if !allowed {
            return false;
        }

        let equipment = self.entity_equipment.lock().await;
        equipment.get(equippable.slot).is_empty()
    }

    /// Vanilla `LivingEntity.getEquipmentSlotForItem` (`LivingEntity.java:3880-3883`) returns the
    /// equippable component's declared slot for dispenser equipment.
    pub fn equipment_slot_for_item(&self, item_stack: &ItemStack) -> EquipmentSlot {
        item_stack
            .get_data_component::<EquippableImpl>()
            .map_or(EquipmentSlot::MAIN_HAND, |equippable| {
                equippable.slot.clone()
            })
    }

    /// Vanilla `LivingEntity.attackable` (`LivingEntity.java:3715-3717`), overridden `false`
    /// only by `ArmorStand.attackable` (`ArmorStand.java:621-624`). Distinct from
    /// `Entity.isAttackable`/`is_attackable` (whether the entity can be hit at all) -- this
    /// gates whether AI target-selection predicates such as `WitherBoss`'s and the Johnny
    /// Vindicator's may ever pick this entity as a target.
    pub fn is_valid_ai_target(&self) -> bool {
        self.entity.entity_type != &EntityType::ARMOR_STAND
    }

    /// Vanilla `LivingEntity.canBeSeenAsEnemy` (`LivingEntity.java:952-958`): combat targeting
    /// must honor both the entity's invulnerability and its living-entity visibility hook.
    #[must_use]
    pub fn can_be_seen_as_enemy(&self) -> bool {
        can_be_seen_as_enemy_state(
            self.entity.invulnerable.load(Relaxed),
            self.can_be_seen_by_anyone(),
            self.not_targetable_as_enemy.load(Relaxed),
        )
    }

    pub async fn reset_state(&self) {
        self.entity.reset_state().await;

        // Restore to maximum health for this entity type
        let max_health = self.get_max_health();
        self.set_health(max_health);
        // Clear any absorption
        self.absorption.store(0.0);
        // Send health metadata
        self.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::living_entity::DATA_HEALTH_ID,
                max_health,
            )],
            None,
        );

        self.reset_effects_and_attributes().await;

        // Give a short grace period of invulnerability after respawn
        self.hurt_cooldown.store(20, Relaxed);
        self.last_damage_taken.store(0f32);

        self.entity.portal_cooldown.store(0, Relaxed);
        *self.entity.portal_manager.lock().await = None;

        // Clear fall/fire state
        self.fall_distance.store(0f32);
        self.death_time.store(0, Relaxed);
        self.entity.extinguish();
        self.entity.fire_ticks.store(0, Relaxed);

        // Clear velocity and movement input to remove persisted momentum
        self.entity.velocity.store(Vector3::default());
        self.entity.velocity_dirty.store(true, SeqCst);
        self.movement_input.store(Vector3::default());
        self.jumping.store(false, Relaxed);

        // If this LivingEntity corresponds to a Player, reset their hunger manager
        let world = self.entity.world.load();
        if let Some(player) = world.get_player_by_id(self.entity.entity_id) {
            player.hunger_manager.restart();
        }

        self.dead.store(false, Relaxed);
    }

    /// Try to spawn silverfish when this entity is infested and hurt.
    async fn try_spawn_infested_silverfish(&self) {
        if !self.has_effect(&StatusEffect::INFESTED).await {
            return;
        }

        // Wither, ender dragon and silverfish are immune
        if self.entity.entity_type == &EntityType::WITHER
            || self.entity.entity_type == &EntityType::ENDER_DRAGON
            || self.entity.entity_type == &EntityType::SILVERFISH
        {
            return;
        }

        let world = self.entity.world.load();

        // 10% chance
        if rand::rng().random::<f32>() <= 0.1 {
            let count = rand::rng().random_range(1..3);
            for _ in 0..count {
                // Spawn at center of entity
                let bbox = self.entity.bounding_box.load();
                let center = Vector3::new(
                    f64::midpoint(bbox.min.x, bbox.max.x),
                    f64::midpoint(bbox.min.y, bbox.max.y),
                    f64::midpoint(bbox.min.z, bbox.max.z),
                );

                // Random direction
                let yaw_rad = self.entity.yaw.load().to_radians() as f64;
                let random_angle = rand::rng().random::<f64>() * std::f64::consts::PI
                    - std::f64::consts::FRAC_PI_2;
                let angle = yaw_rad + random_angle;
                let speed = 0.3f64;
                let dx = -angle.sin() * speed;
                let dz = angle.cos() * speed;
                let dy = 0.1f64;

                // Spawn
                let silver = crate::entity::r#type::from_type(
                    &EntityType::SILVERFISH,
                    center,
                    &world,
                    Uuid::new_v4(),
                );

                silver.get_entity().set_pos(center);
                silver.get_entity().velocity.store(Vector3::new(dx, dy, dz));

                world.spawn_entity(silver).await;

                // Play sound
                world.play_sound(Sound::EntitySilverfishHurt, SoundCategory::Players, &center);
            }
        }
    }

    pub fn is_player(&self) -> bool {
        let world = self.entity.world.load();
        world.get_player_by_id(self.entity.entity_id).is_some()
    }

    pub fn get_movement(&self) -> Vector3<f64> {
        self.entity.movement.load()
    }

    pub(crate) fn is_eye_in_water(&self, world: &World) -> bool {
        self.entity
            .is_eye_in_fluid(world, &tag::Fluid::MINECRAFT_WATER)
    }

    pub(crate) fn max_air_supply(&self) -> i32 {
        if self.entity.entity_type == &EntityType::AXOLOTL {
            6000
        } else if self.entity.entity_type == &EntityType::DOLPHIN {
            4800
        } else {
            MAX_AIR_SUPPLY
        }
    }

    pub(crate) fn decrease_air_supply(&self, current_supply: i32) -> i32 {
        let oxygen_bonus = self.get_attribute_value(&Attributes::OXYGEN_BONUS);
        let mut random = self.air_random.lock().unwrap();
        if oxygen_bonus > 0.0 && random.random::<f64>() >= 1.0 / (oxygen_bonus + 1.0) {
            current_supply
        } else {
            current_supply - 1
        }
    }

    pub(crate) fn send_air_supply(&self) {
        let air = self.air_supply.load(Relaxed);
        let mut bedrock_meta =
            pumpkin_protocol::bedrock::client::set_actor_data::EntityMetadata::new();
        bedrock_meta.set(
            pumpkin_protocol::bedrock::client::set_actor_data::entity_data_key::AIR_SUPPLY,
            pumpkin_protocol::bedrock::client::set_actor_data::MetadataValue::Short(
                air.clamp(0, i32::from(i16::MAX)) as i16,
            ),
        );
        self.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::entity::DATA_AIR_SUPPLY_ID,
                VarInt(air),
            )],
            Some(&bedrock_meta),
        );
    }

    const fn is_water_animal(&self) -> bool {
        let id = self.entity.entity_type.id;
        id == EntityType::AXOLOTL.id
            || id == EntityType::COD.id
            || id == EntityType::GLOW_SQUID.id
            || id == EntityType::NAUTILUS.id
            || id == EntityType::PUFFERFISH.id
            || id == EntityType::SALMON.id
            || id == EntityType::SQUID.id
            || id == EntityType::TADPOLE.id
            || id == EntityType::TROPICAL_FISH.id
    }

    async fn tick_water_animal_air_supply(
        &self,
        caller: &Arc<dyn EntityBase>,
        pre_tick_air_supply: i32,
    ) {
        let world = self.entity.world.load();
        let in_water_or_rain = if self.entity.entity_type == &EntityType::AXOLOTL {
            let block_pos = self.entity.block_pos.load();
            let max_y = self.entity.bounding_box.load().max.y;
            let rain_pos =
                BlockPos::floored(f64::from(block_pos.0.x), max_y, f64::from(block_pos.0.z));
            self.entity.touching_water.load(SeqCst)
                || world.is_raining_at(&block_pos).await
                || world.is_raining_at(&rain_pos).await
        } else {
            self.entity.touching_water.load(SeqCst)
        };
        if self.entity.is_removed() {
            return;
        }
        let max_air = self.max_air_supply();

        if self.is_dead_or_dying() {
            if self.air_supply.swap(max_air, Relaxed) != max_air {
                self.send_air_supply();
            }
            return;
        }

        if in_water_or_rain {
            if self.air_supply.swap(max_air, Relaxed) != max_air {
                self.send_air_supply();
            }
            return;
        }

        let air = pre_tick_air_supply - 1;
        self.air_supply.store(air, Relaxed);
        self.send_air_supply();
        if air <= -20 {
            self.air_supply.store(0, Relaxed);
            self.send_air_supply();
            let damage_type = if self.entity.entity_type == &EntityType::NAUTILUS
                || self.entity.entity_type == &EntityType::AXOLOTL
            {
                DamageType::DRY_OUT
            } else {
                DamageType::DROWN
            };
            self.damage(caller.as_ref(), 2.0, damage_type).await;
        }
    }

    async fn dismount_underwater_vehicle(&self, underwater: bool) {
        if !underwater {
            return;
        }

        let vehicle = self.entity.vehicle.lock().await.clone();
        if let Some(vehicle) = vehicle
            && vehicle
                .get_entity()
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_DISMOUNTS_UNDERWATER)
            && !self.entity.is_removed()
        {
            vehicle
                .get_entity()
                .remove_passenger(self.entity.entity_id)
                .await;
        }
    }

    async fn tick_generic_air_supply(
        &self,
        caller: &Arc<dyn EntityBase>,
        world: &World,
        eye_in_water: bool,
        in_bubble_column: bool,
    ) {
        if self.is_dead_or_dying() {
            return;
        }

        if eye_in_water && !in_bubble_column {
            let can_breathe_underwater = self.can_breathe_underwater(caller).await;
            let has_water_breathing = self.has_effect(&StatusEffect::WATER_BREATHING).await;
            let has_conduit_power = self.has_effect(&StatusEffect::CONDUIT_POWER).await;
            let has_breath_of_the_nautilus =
                self.has_effect(&StatusEffect::BREATH_OF_THE_NAUTILUS).await;
            if self.is_dead_or_dying() || self.entity.is_removed() {
                return;
            }
            let water_breathing =
                has_water_breathing || has_conduit_power || has_breath_of_the_nautilus;
            let refill_air =
                !has_breath_of_the_nautilus || has_water_breathing || has_conduit_power;
            let max_air = self.max_air_supply();

            if !can_breathe_underwater && !water_breathing {
                let previous_air = self.air_supply.load(Relaxed);
                let air = self.decrease_air_supply(previous_air);
                if air != previous_air {
                    self.air_supply.store(air, Relaxed);
                    self.send_air_supply();
                }

                if air <= -20 {
                    self.air_supply.store(0, Relaxed);
                    self.send_air_supply();
                    world.send_entity_status(&self.entity, EntityStatus::DrownParticles, None);
                    self.damage(caller.as_ref(), 2.0, DamageType::DROWN).await;
                }
            } else if refill_air && self.air_supply.load(Relaxed) < max_air {
                if self.entity.entity_type == &EntityType::DOLPHIN {
                    self.air_supply.store(max_air, Relaxed);
                } else {
                    self.air_supply
                        .fetch_update(Relaxed, Relaxed, |air| Some((air + 4).min(max_air)))
                        .ok();
                }
                self.send_air_supply();
            }
        } else if self.air_supply.load(Relaxed) < self.max_air_supply() {
            let max_air = self.max_air_supply();
            if self.entity.entity_type == &EntityType::DOLPHIN {
                self.air_supply.store(max_air, Relaxed);
            } else {
                self.air_supply
                    .fetch_update(Relaxed, Relaxed, |air| Some((air + 4).min(max_air)))
                    .ok();
            }
            self.send_air_supply();
        }

        self.dismount_underwater_vehicle(eye_in_water && !in_bubble_column)
            .await;
    }

    async fn tick_air_supply(&self, caller: &Arc<dyn EntityBase>, was_alive_before_air: bool) {
        if self.entity.is_removed() {
            return;
        }
        if self.entity.entity_type != &EntityType::PLAYER
            && !self.air_metadata_initialized.swap(true, Relaxed)
        {
            self.send_air_supply();
        }
        let world = self.entity.world.load();
        let pos = self.entity.pos.load();
        let eye_block = BlockPos::floored(pos.x, self.entity.get_eye_y(), pos.z);
        let eye_in_water = self.is_eye_in_water(&world);
        let in_bubble_column = world.get_block(&eye_block) == &Block::BUBBLE_COLUMN;

        // Players keep their ability/game-rule-aware BreathManager, but still use the
        // LivingEntity underwater vehicle rule.
        if self.entity.entity_type == &EntityType::PLAYER {
            if !was_alive_before_air {
                return;
            }
            self.dismount_underwater_vehicle(eye_in_water && !in_bubble_column)
                .await;
            return;
        }

        let custom_water_air = self.is_water_animal()
            && (!self.entity.no_ai.load(Relaxed)
                || (self.entity.entity_type != &EntityType::AXOLOTL
                    && self.entity.entity_type != &EntityType::NAUTILUS));
        if custom_water_air {
            let pre_tick_air_supply = self.air_supply.load(Relaxed);
            // WaterAnimal/Axolotl/Nautilus invoke their override after LivingEntity.baseTick.
            // Run the superclass first so its generic air update and underwater dismount occur
            // before the subclass-specific reset/dry-out logic.
            self.tick_generic_air_supply(caller, &world, eye_in_water, in_bubble_column)
                .await;
            self.tick_water_animal_air_supply(caller, pre_tick_air_supply)
                .await;
            return;
        }

        self.tick_generic_air_supply(caller, &world, eye_in_water, in_bubble_column)
            .await;
    }

    async fn can_breathe_underwater(&self, caller: &Arc<dyn EntityBase>) -> bool {
        if self
            .entity
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_CAN_BREATHE_UNDER_WATER)
        {
            return true;
        }

        if let Some(sulfur_cube) = caller.cast_any().downcast_ref::<SulfurCubeEntity>() {
            return sulfur_cube.can_breathe_underwater().await;
        }

        if let Some(happy_ghast) = caller.cast_any().downcast_ref::<HappyGhastEntity>() {
            return happy_ghast.can_breathe_underwater();
        }

        false
    }

    /// `Entity.getMovementEmission` (Entity.java:1533-1535) is `ALL` by default, but these
    /// living types downgrade it to `EVENTS` or `NONE` and so make no movement sound at all:
    /// Bat.java:183-186, Squid.java:96-99 (inherited by GlowSquid.java:26), Breeze.java:282-285,
    /// Endermite.java:55-58, Guardian.java:169-172 (inherited by ElderGuardian.java:19),
    /// Shulker.java:105-108, Silverfish.java:56-59.
    fn movement_emits_sounds(entity_type: &'static EntityType) -> bool {
        entity_type != &EntityType::BAT
            && entity_type != &EntityType::SQUID
            && entity_type != &EntityType::GLOW_SQUID
            && entity_type != &EntityType::BREEZE
            && entity_type != &EntityType::ENDERMITE
            && entity_type != &EntityType::GUARDIAN
            && entity_type != &EntityType::ELDER_GUARDIAN
            && entity_type != &EntityType::SHULKER
            && entity_type != &EntityType::SILVERFISH
    }

    /// Vanilla `MovementEmission.emitsEvents` remains enabled for living entities except
    /// shulkers, whose override returns `NONE` (`Entity.java:4113-4137`; `Shulker.java:104-108`).
    fn movement_emits_events(entity_type: &'static EntityType) -> bool {
        entity_type != &EntityType::SHULKER
    }

    /// `Entity.getSwimSound` (Entity.java:1263-1265) returns `entity.generic.swim` for every
    /// entity; the two nautilus species override it.
    fn swim_sound(caller: &Arc<dyn EntityBase>) -> Sound {
        if let Some(skeleton_horse) = caller
            .cast_any()
            .downcast_ref::<crate::entity::passive::skeleton_horse::SkeletonHorseEntity>(
        ) {
            return skeleton_horse.get_swim_sound();
        }
        if let Some(nautilus) = caller
            .cast_any()
            .downcast_ref::<crate::entity::passive::nautilus::NautilusEntity>()
        {
            return nautilus.get_swim_sound();
        }
        if let Some(zombie_nautilus) = caller
            .cast_any()
            .downcast_ref::<crate::entity::passive::zombie_nautilus::ZombieNautilusEntity>(
        ) {
            return zombie_nautilus.get_swim_sound();
        }
        Sound::EntityGenericSwim
    }

    /// `Entity.applyMovementEmissionAndPlaySound` (Entity.java:867-901): the horizontal distance
    /// moved is accumulated into `moveDist`, and each time it passes `nextStep` the entity emits
    /// either a step sound or, when it is in water and produced no step side effects, the swim
    /// sound (Entity.java:889-893). `nextStep` still advances in the other case
    /// (`Entity.nextStep`, Entity.java:1259-1261). Volume comes from
    /// `Entity.waterSwimSound` (Entity.java:1428-1437) and the pitch
    /// spread from `Entity.playSwimSound` (Entity.java:1475-1477).
    ///
    /// Players are skipped: their client simulates its own movement and plays this sound
    /// locally, which vanilla accounts for by excluding the player from its own
    /// `Player.playSound` broadcast, a distinction `World::play_sound` here does not draw.
    async fn tick_swim_sound(&self, caller: &Arc<dyn EntityBase>) {
        // `Entity.move` invokes movement emission only when `getMovementEmission` emits anything
        // (`Entity.java:785-794`). Player's override suppresses this while flying or sneaking on
        // ground (`Player.java:1642-1644`); other living entities use the ordinary path.
        if let Some(player) = caller.get_player()
            && !player.get_movement_emission().await
        {
            return;
        }
        let moved = self.entity.pos.load() - self.entity.last_pos.load();
        let horizontal = moved.x.hypot(moved.z) as f32 * 0.6;
        let move_dist = self.move_dist.load() + horizontal;
        self.move_dist.store(move_dist);
        if move_dist <= self.next_step.load() {
            return;
        }
        self.next_step.store(move_dist.floor() + 1.0);

        // `Entity.applyMovementEmissionAndPlaySound` emits STEP from the supporting block, or
        // SWIM when no step side effect was produced (`Entity.java:867-901`).
        if Self::movement_emits_events(self.entity.entity_type) {
            let (_, supporting_block, supporting_state) =
                self.entity.get_block_with_y_offset(0.00001);
            let on_ground = self.entity.on_ground.load(Relaxed);
            let can_step = !supporting_state.is_air()
                && (on_ground
                    || supporting_block.has_tag(&tag::Block::MINECRAFT_CLIMBABLE)
                    || (self.entity.sneaking.load(Relaxed) && moved.y == 0.0))
                && !self.entity.swimming.load(Relaxed);
            let world = self.entity.world.load_full();
            if can_step {
                crate::world::game_event::emit_game_event(
                    &world,
                    pumpkin_data::game_event::GameEvent::Step,
                    self.entity.pos.load(),
                    crate::world::game_event::GameEventContext::of_entity_with_block_state(
                        caller.clone(),
                        supporting_state.id,
                    ),
                )
                .await;
            } else if self.entity.touching_water.load(Relaxed) {
                crate::world::game_event::emit_game_event(
                    &world,
                    pumpkin_data::game_event::GameEvent::Swim,
                    self.entity.pos.load(),
                    crate::world::game_event::GameEventContext::of_entity(caller.clone()),
                )
                .await;
            }
        }

        if self.entity.is_silent() || !Self::movement_emits_sounds(self.entity.entity_type) {
            return;
        }
        // Vanilla's player movement sound is client-local; keep the existing server-side sound
        // suppression after allowing the server vibration event above.
        if caller.get_player().is_some() {
            return;
        }
        let on_ground = self.entity.on_ground.load(Relaxed);
        let skeleton_horse_in_water = self.entity.entity_type == &EntityType::SKELETON_HORSE;
        if self.entity.touching_water.load(Relaxed) && (!on_ground || skeleton_horse_in_water) {
            let velocity = self.entity.velocity.load();
            let water_volume = ((velocity.x * velocity.x)
                .mul_add(
                    0.2,
                    velocity
                        .y
                        .mul_add(velocity.y, velocity.z * velocity.z * 0.2),
                )
                .sqrt() as f32
                * 0.35)
                .min(1.0);
            let volume = if skeleton_horse_in_water {
                skeleton_swim_sound_volume(on_ground, water_volume)
            } else {
                water_volume
            };
            let mut rng = rand::rng();
            let pitch = (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.4, 1.0);
            self.entity.world.load().play_sound_fine(
                Self::swim_sound(caller),
                SoundCategory::Neutral,
                &self.entity.pos.load(),
                volume,
                pitch,
            );
        } else if self.entity.on_ground.load(Relaxed)
            && let Some(sound) = caller.get_mob().and_then(Mob::get_step_sound)
        {
            // `vibrationAndSoundEffectsFromBlock` only calls `walkingStepSound` ->
            // `playStepSound` when `onGround() || climbable || (crouching && no vertical
            // movement) || onRails()` (Entity.java:993-994); this checks only the common
            // `onGround()` case, so a skeleton mid-air (falling, not swimming) no longer
            // plays its step sound, but climbing/crouching/on-rails still won't.
            self.entity.world.load().play_sound_fine(
                sound,
                SoundCategory::Neutral,
                &self.entity.pos.load(),
                0.15,
                1.0,
            );
        }
    }

    fn hurt_sound(&self) -> Sound {
        if self.entity.entity_type == &EntityType::SLIME {
            SlimeEntity::hurt_sound_for_size(self.entity.data.load(Relaxed))
        } else if self.entity.entity_type == &EntityType::MAGMA_CUBE {
            SlimeEntity::magma_cube_hurt_sound_for_size(self.entity.data.load(Relaxed))
        } else if self.entity.entity_type == &EntityType::SULFUR_CUBE {
            SulfurCubeEntity::hurt_sound_for_size(self.entity.data.load(Relaxed))
        } else if self.entity.entity_type == &EntityType::ZOMBIE_NAUTILUS {
            // `ZombieNautilus.getHurtSound` (ZombieNautilus.java:91-94) picks by
            // `isUnderWater` (Entity.java:1608-1610). The generated
            // `ZOMBIE_NAUTILUS.hurt_sound` is None, so without this branch the mob falls
            // through to the generic hurt sound.
            crate::entity::passive::zombie_nautilus::hurt_sound_for(
                self.entity.was_eye_in_water.load(Relaxed)
                    && self.entity.touching_water.load(Relaxed),
            )
        } else {
            Self::hurt_sound_for_entity(self.entity.entity_type)
        }
    }
}

/// Ticks equipment stacks, matching `EntityEquipment.tick` (`EntityEquipment.java:48-55`).
/// Player main-hand stacks are handled by `Inventory.tick`; the other equipment slots remain
/// owned by this entity's equipment store.
async fn tick_equipment_items(living: &LivingEntity, owner: &dyn EntityBase, server: &Server) {
    let player = owner.get_player();
    for (slot, mut stack) in living.items_by_equipment_slot(owner).await {
        if stack.is_empty() || (player.is_some() && slot == EquipmentSlot::MAIN_HAND) {
            continue;
        }

        let before = stack.clone();
        server
            .item_registry
            .inventory_tick(&mut stack, owner, server)
            .await;
        if stack.are_equal(&before) {
            continue;
        }

        living
            .entity_equipment
            .lock()
            .await
            .put(&slot, stack.clone());
        living.send_equipment_changes(&[(slot.clone(), stack.clone())]);
        if let Some(player) = player {
            let slot_index = player
                .inventory()
                .equipment_slots
                .iter()
                .find_map(|(index, mapped)| (mapped == &slot).then_some(*index));
            if let Some(slot_index) = slot_index {
                player
                    .enqueue_slot_set_packet(&CSetPlayerInventory::new(
                        (slot_index as i32).into(),
                        &ItemStackSerializer::from(stack),
                    ))
                    .await;
            }
        }
    }
}

// Vanilla persists only an attribute instance's *permanent* modifiers
// (`AttributeInstance.java:186-188`, which packs `permanentModifiers`). Everything applied
// through `addTransientModifier` is rebuilt from its source after load instead: equipment and
// enchantments (`LivingEntity.java:2972-2976`), the witch's drinking slowdown
// (`Witch.java:170`), the enderman and zombified piglin attack speed boosts
// (`EnderMan.java:135`, `ZombifiedPiglin.java:102`) and the killer bunny's damage bonus
// (`Rabbit.java:376`). Status effect modifiers are deliberately *not* in that set: vanilla adds
// them permanently (`MobEffect.java:172`), so they belong in the saved list. Explicitly marked
// permanent modifiers are tracked by `AttributeInstance`; legacy IDs retain the same fallback
// classification for existing callers.

/// `AttributeModifier.Operation.getSerializedName` (`AttributeModifier.java:38-40`).
const fn modifier_operation_name(operation: ModifierOperation) -> &'static str {
    match operation {
        ModifierOperation::Add => "add_value",
        ModifierOperation::MultiplyBase => "add_multiplied_base",
        ModifierOperation::MultiplyTotal => "add_multiplied_total",
    }
}

fn modifier_operation_from_name(name: &str) -> Option<ModifierOperation> {
    match name {
        "add_value" => Some(ModifierOperation::Add),
        "add_multiplied_base" => Some(ModifierOperation::MultiplyBase),
        "add_multiplied_total" => Some(ModifierOperation::MultiplyTotal),
        _ => None,
    }
}

/// `AttributeMap.pack` (`AttributeMap.java:132-140`) plus `AttributeInstance.pack`
/// (`AttributeInstance.java:186-188`), serialized through `AttributeInstance.Packed.CODEC`
/// (`AttributeInstance.java:203-210`): `{id: <attribute name>, base: double, modifiers: [...]}`.
///
/// Vanilla's map only holds instances something has touched, while this one is pre-filled with
/// every default for the entity type, so an instance still sitting at its default base with no
/// permanent modifiers is skipped: `AttributeMap.apply` leaves an unlisted attribute alone
/// (`AttributeMap.java:142-149`), which is exactly that state.
fn pack_attributes(
    attributes: &HashMap<u8, AttributeInstance>,
    defaults: &[(Attributes, f64)],
) -> Vec<NbtTag> {
    let mut ids: Vec<u8> = attributes.keys().copied().collect();
    ids.sort_unstable();

    let mut packed = Vec::new();
    for id in ids {
        let Some(instance) = attributes.get(&id) else {
            continue;
        };
        let Some(attribute) = Attributes::ALL.iter().find(|entry| entry.id == id) else {
            continue;
        };

        let modifiers: Vec<NbtTag> = instance
            .modifiers
            .iter()
            .filter(|modifier| instance.is_permanent_modifier(&modifier.id))
            .map(|modifier| {
                let mut compound = NbtCompound::new();
                compound.put_string("id", modifier.id.clone());
                compound.put_double("amount", modifier.amount);
                compound.put_string(
                    "operation",
                    modifier_operation_name(modifier.operation).to_string(),
                );
                NbtTag::Compound(compound)
            })
            .collect();

        let default_base = defaults
            .iter()
            .find(|(entry, _)| entry.id == id)
            .map_or(attribute.default_value, |(_, base)| *base);
        if modifiers.is_empty() && instance.base_value.to_bits() == default_base.to_bits() {
            continue;
        }

        let mut compound = NbtCompound::new();
        compound.put_string("id", attribute.name.to_string());
        compound.put_double("base", instance.base_value);
        if !modifiers.is_empty() {
            compound.put_list("modifiers", modifiers);
        }
        packed.push(NbtTag::Compound(compound));
    }
    packed
}

/// Reads the `attributes` tag, if present, into the entity's live attribute map.
/// `LivingEntity.readAdditionalSaveData` (`LivingEntity.java:802`) does this before health and
/// absorption, both of which clamp against attributes it may just have changed. Kept out of
/// `read_nbt_non_mut` so the lock guard's scope is obvious and cannot straddle an await.
fn load_attributes_from_nbt(
    attributes: &RwLock<HashMap<u8, AttributeInstance>>,
    nbt: &NbtCompound,
) {
    let Some(packed) = nbt.get_list("attributes") else {
        return;
    };
    let mut attributes = attributes
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    apply_packed_attributes(&mut attributes, packed);
}

/// `AttributeMap.apply` (`AttributeMap.java:142-149`) plus `AttributeInstance.apply`
/// (`AttributeInstance.java:190-200`): the saved base replaces the current one and every saved
/// modifier is put back; attributes absent from the list keep their defaults.
fn apply_packed_attributes(attributes: &mut HashMap<u8, AttributeInstance>, packed: &[NbtTag]) {
    for tag in packed {
        let NbtTag::Compound(entry) = tag else {
            continue;
        };
        let Some(name) = entry.get_string("id") else {
            continue;
        };
        let Some(attribute) = Attributes::ALL.iter().find(|candidate| {
            candidate.name == name || candidate.name.strip_prefix("minecraft:") == Some(name)
        }) else {
            warn!("Unknown attribute {name} in entity NBT");
            continue;
        };

        let instance = attributes.entry(attribute.id).or_insert_with(|| {
            AttributeInstance::new(
                attribute.default_value,
                attribute.min_value,
                attribute.max_value,
            )
        });
        instance.base_value = entry.get_double("base").unwrap_or(0.0);
        for modifier_tag in entry.get_list("modifiers").unwrap_or_default() {
            let NbtTag::Compound(modifier) = modifier_tag else {
                continue;
            };
            let (Some(id), Some(amount), Some(operation)) = (
                modifier.get_string("id"),
                modifier.get_double("amount"),
                modifier
                    .get_string("operation")
                    .and_then(modifier_operation_from_name),
            ) else {
                warn!("Malformed attribute modifier in entity NBT");
                continue;
            };
            // Vanilla `AttributeInstance.apply` (`AttributeInstance.java:190-200`) restores
            // packed modifiers into the permanent modifier set.
            instance.add_or_replace_permanent_modifier(Modifier {
                id: id.to_string(),
                amount,
                operation,
            });
        }
        instance.dirty.store(true, Relaxed);
    }
}

impl NBTStorage for LivingEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.entity.write_nbt(nbt).await;
            nbt.put("Health", NbtTag::Float(self.health.load()));
            if self.entity.entity_type != &EntityType::PLAYER {
                nbt.put_short("Air", self.air_supply.load(Relaxed) as i16);
            }
            // Avoid persisting a lethal fall distance when the entity is dead to prevent death loops
            let fall_distance = if self.dead.load(Relaxed) {
                0.0
            } else {
                self.fall_distance.load()
            };
            // Persist current absorption amount
            nbt.put("AbsorptionAmount", NbtTag::Float(self.absorption.load()));
            nbt.put("FallDistance", NbtTag::Float(fall_distance));
            // `LivingEntity.addAdditionalSaveData` (`LivingEntity.java:753`) stores the packed
            // attribute list here, right after AbsorptionAmount. An all-defaults entity packs to
            // nothing, and the tag is then left out rather than written as an empty list.
            {
                let packed = {
                    let attributes = self
                        .attributes
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    pack_attributes(&attributes, self.entity.entity_type.attributes)
                };
                if !packed.is_empty() {
                    nbt.put_list("attributes", packed);
                }
            }
            // `LivingEntity.java:748-758`.
            nbt.put_short("HurtTime", self.hurt_cooldown.load(Relaxed).max(0) as i16);
            nbt.put_short("DeathTime", i16::from(self.death_time.load(Relaxed)));
            nbt.put_bool("FallFlying", self.entity.is_fall_flying());
            // `Mob.addAdditionalSaveData` writes a non-zero DeathLootTableSeed
            // (`Mob.java:382-385`).
            let loot_table_seed = self.loot_table_seed.load();
            if loot_table_seed != 0 {
                nbt.put_long("DeathLootTableSeed", loot_table_seed);
            }
            {
                let effects = self.active_effects.lock().await;
                let hidden_effects = self.hidden_effects.lock().await;
                if !effects.is_empty() {
                    // Iterate effects and create Box<[NbtTag]>
                    let mut effects_list = Vec::with_capacity(effects.len());
                    for effect in effects.values() {
                        let mut effect_nbt = pumpkin_nbt::compound::NbtCompound::new();
                        effect.write_nbt(&mut effect_nbt).await;
                        // Vanilla nests the hidden chain inside the instance covering it, so the
                        // furthest one down the chain is written innermost.
                        if let Some(chain) = hidden_effects.get(effect.effect_type) {
                            let mut nested: Option<NbtCompound> = None;
                            for hidden_effect in chain.iter().rev() {
                                let mut hidden_nbt = NbtCompound::new();
                                hidden_effect.write_nbt(&mut hidden_nbt).await;
                                if let Some(inner) = nested.take() {
                                    hidden_nbt.put("hidden_effect", NbtTag::Compound(inner));
                                }
                                nested = Some(hidden_nbt);
                            }
                            if let Some(nested) = nested {
                                effect_nbt.put("hidden_effect", NbtTag::Compound(nested));
                            }
                        }
                        effects_list.push(NbtTag::Compound(effect_nbt));
                    }
                    nbt.put("active_effects", NbtTag::List(effects_list));
                }
            }
            let equipment = self.entity_equipment.lock().await;
            let mut hand_items = Vec::with_capacity(2);
            for slot in [EquipmentSlot::MAIN_HAND, EquipmentSlot::OFF_HAND] {
                let stack = equipment.get(&slot);
                let mut item_nbt = NbtCompound::new();
                if !stack.is_empty() {
                    stack.write_item_stack(&mut item_nbt);
                }
                hand_items.push(NbtTag::Compound(item_nbt));
            }
            nbt.put("HandItems", NbtTag::List(hand_items));

            let mut armor_items = Vec::with_capacity(4);
            for slot in [
                EquipmentSlot::FEET,
                EquipmentSlot::LEGS,
                EquipmentSlot::CHEST,
                EquipmentSlot::HEAD,
            ] {
                let stack = equipment.get(&slot);
                let mut item_nbt = NbtCompound::new();
                if !stack.is_empty() {
                    stack.write_item_stack(&mut item_nbt);
                }
                armor_items.push(NbtTag::Compound(item_nbt));
            }
            nbt.put("ArmorItems", NbtTag::List(armor_items));
            // todo more...
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.entity.read_nbt_non_mut(nbt).await;
            load_attributes_from_nbt(&self.attributes, nbt);
            self.health.store(nbt.get_float("Health").unwrap_or(0.0));
            if self.entity.entity_type != &EntityType::PLAYER
                && let Some(air) = nbt
                    .get_int("Air")
                    .or_else(|| nbt.get_short("Air").map(i32::from))
            {
                self.air_supply.store(air, Relaxed);
            }

            // Clamp any persisted absorption to the entity's configured max
            let raw_abs = nbt.get_float("AbsorptionAmount").unwrap_or(0.0);
            let max_abs = self.get_max_absorption();
            let clamped_abs = raw_abs.max(0.0).min(max_abs);
            self.absorption.store(clamped_abs);

            // Load fall distance, but if this entity is currently marked dead ensure we don't restore
            // a lethal fall distance that would immediately re-kill on spawn.
            let fd = nbt
                .get_float("FallDistance")
                .or_else(|| nbt.get_float("fall_distance"))
                .unwrap_or(0.0);
            if self.dead.load(Relaxed) {
                self.fall_distance.store(0.0);
            } else {
                self.fall_distance.store(fd);
            }
            if let Some(hurt_time) = nbt.get_short("HurtTime") {
                self.hurt_cooldown.store(i32::from(hurt_time), Relaxed);
            }
            if let Some(death_time) = nbt.get_short("DeathTime") {
                self.death_time.store(death_time as u8, Relaxed);
            }
            self.entity
                .fall_flying
                .store(nbt.get_bool("FallFlying").unwrap_or(false), Relaxed);
            // `Mob.readAdditionalSaveData` restores the optional loot seed, defaulting to zero
            // (`Mob.java:405-407`).
            self.loot_table_seed
                .store(nbt.get_long("DeathLootTableSeed").unwrap_or(0));
            let mut loaded_effects: Vec<Effect> = Vec::new();
            {
                let mut active_effects = self.active_effects.lock().await;
                let mut hidden_effects = self.hidden_effects.lock().await;
                let nbt_effects = nbt.get_list("active_effects");
                if let Some(nbt_effects) = nbt_effects {
                    for effect in nbt_effects {
                        if let NbtTag::Compound(effect_nbt) = effect {
                            if let Some(mut effect) =
                                Effect::create_from_nbt(&mut effect_nbt.clone()).await
                            {
                                effect.blend = true; // TODO: change, is taken from effect give command
                                let mut chain = Vec::new();
                                let mut nested = effect_nbt.get_compound("hidden_effect").cloned();
                                while let Some(mut hidden_nbt) = nested {
                                    nested = hidden_nbt.get_compound("hidden_effect").cloned();
                                    if let Some(mut hidden_effect) =
                                        Effect::create_from_nbt(&mut hidden_nbt).await
                                    {
                                        hidden_effect.blend = true;
                                        chain.push(hidden_effect);
                                    } else {
                                        warn!("Unable to read hidden effect from nbt");
                                        break;
                                    }
                                }
                                if !chain.is_empty() {
                                    hidden_effects.insert(effect.effect_type, chain);
                                }
                                loaded_effects.push(effect.clone());
                                active_effects.insert(effect.effect_type, effect);
                            } else {
                                warn!("Unable to read effect from nbt");
                            }
                        }
                    }
                }
            }
            // Vanilla saves the effects' attribute modifiers in the `attributes` tag, which is
            // applied right before `active_effects` (`LivingEntity.java:802`) and is now read
            // above. Rebuilding them from the loaded effects as well is idempotent (same
            // modifier ids, replaced in place) and keeps worlds saved before that tag existed
            // from losing a reloaded Speed or Strength.
            for effect in loaded_effects {
                self.restore_effect_attribute_modifiers(&effect);
                if effect.effect_type == &StatusEffect::INVISIBILITY {
                    self.entity.set_invisible(true).await;
                } else if effect.effect_type == &StatusEffect::GLOWING {
                    self.entity.set_glowing(true).await;
                }
            }

            self.load_equipment_from_nbt(nbt).await;
        })
        // todo more...
    }
}

impl LivingEntity {
    pub fn add_stinger(&self) {
        let count = self.stinger_count.fetch_add(1, Relaxed) + 1;
        self.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::living_entity::DATA_STINGER_COUNT_ID,
                count,
            )],
            None,
        );
    }

    /// Vanilla `LivingEntity.setArrowCount` (`LivingEntity.java:1994-2000`) publishes the
    /// number of ordinary arrows currently stuck in this entity.
    pub fn set_arrow_count(&self, count: i32) {
        self.arrow_count.store(count, Relaxed);
        self.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::living_entity::DATA_ARROW_COUNT_ID,
                count,
            )],
            None,
        );
    }

    fn tick_stingers(&self) {
        let count = self.stinger_count.load(Relaxed);
        if count <= 0 {
            return;
        }
        if rand::random_range(0..20) == 0 {
            let remaining = self.stinger_count.fetch_sub(1, Relaxed) - 1;
            self.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::living_entity::DATA_STINGER_COUNT_ID,
                    remaining,
                )],
                None,
            );
        }
    }

    /// Vanilla `LivingEntity.tick` arrow removal (`LivingEntity.java:2754-2767`) uses a
    /// count-dependent timer and publishes each decrement through tracked metadata.
    fn tick_arrows(&self, count: i32) {
        if count <= 0 {
            return;
        }

        if self.remove_arrow_time.load(Relaxed) <= 0 {
            self.remove_arrow_time
                .store(arrow_removal_delay(count), Relaxed);
        }

        if self.remove_arrow_time.fetch_sub(1, Relaxed) <= 1 {
            self.set_arrow_count(count - 1);
        }
    }

    /// Restores the `HandItems` / `ArmorItems` lists written by `write_nbt`. Split out of
    /// `read_nbt_non_mut` purely to keep that function within its line budget.
    async fn load_equipment_from_nbt(&self, nbt: &NbtCompound) {
        let mut equipment = self.entity_equipment.lock().await;
        for (key, slots) in [
            (
                "HandItems",
                [EquipmentSlot::MAIN_HAND, EquipmentSlot::OFF_HAND].as_slice(),
            ),
            (
                "ArmorItems",
                [
                    EquipmentSlot::FEET,
                    EquipmentSlot::LEGS,
                    EquipmentSlot::CHEST,
                    EquipmentSlot::HEAD,
                ]
                .as_slice(),
            ),
        ] {
            let Some(items) = nbt.get_list(key) else {
                continue;
            };
            for (index, slot) in slots.iter().enumerate() {
                let Some(compound) = items.get(index).and_then(NbtTag::extract_compound) else {
                    continue;
                };
                let Some(stack) = ItemStack::read_item_stack(compound) else {
                    continue;
                };
                equipment.put(slot, stack);
            }
        }
    }
}

impl EntityBase for LivingEntity {
    /// Vanilla `LivingEntity.getBlockSpeedFactor` interpolates movement efficiency between the
    /// base factor and one (`LivingEntity.java:511-512`; `Entity.java:1084-1091`).
    fn get_block_speed_factor(&self) -> f32 {
        let base = self.entity.get_velocity_multiplier();
        let efficiency = self.get_attribute_value(&Attributes::MOVEMENT_EFFICIENCY) as f32;
        block_speed_factor(base, efficiency)
    }

    /// Vanilla `LivingEntity.getEntityBounciness` reads the bounciness attribute
    /// (`LivingEntity.java:2192-2195`).
    fn get_entity_bounciness(&self) -> f64 {
        self.get_attribute_value(&Attributes::BOUNCINESS)
    }

    /// Vanilla `LivingEntity.getDismountPoses` returns standing as the default pose
    /// (`LivingEntity.java:3735-3737`; `AbstractBoat.java:653-660`).
    fn get_dismount_poses(&self) -> Vec<EntityPose> {
        vec![EntityPose::Standing]
    }

    /// `LivingEntity.igniteForTicks` (`LivingEntity.java:3989`): a living entity scales every
    /// ignite duration by its `minecraft:burning_time` attribute before handing it to
    /// `Entity.igniteForTicks`. Fire Protection is the only vanilla source of a modifier on
    /// that attribute (`fire_protection.json`, `add_multiplied_base` of -0.15 per level).
    fn set_on_fire_for_ticks(&self, ticks: u32) {
        let ticks = self.scale_ignite_ticks(ticks);
        let entity = self.get_entity();
        let mut event = crate::plugin::api::events::entity::entity_combust::EntityCombustEvent::new(
            entity.entity_id,
            ticks as f32 / 20.0,
        );
        if let Some(server) = entity.world.load().server.upgrade() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    server.plugin_manager.fire(&server, &mut event).await;
                });
            });
            if event.cancelled {
                return;
            }
        }
        if entity.fire_ticks.load(Relaxed) < ticks as i32 {
            entity.fire_ticks.store(ticks as i32, Relaxed);
        }
        entity.clear_freeze();
    }

    #[allow(clippy::too_many_lines)]
    fn damage_with_context<'a>(
        &'a self,
        caller: &'a dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&'a dyn EntityBase>,
        cause: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let mut amount = amount;

            // Check invulnerability before applying damage
            if self.entity.is_invulnerable_to(&damage_type).await {
                return false;
            }

            if self.health.load() <= 0.0 || self.dead.load(Relaxed) {
                return false; // Dying or dead
            }

            // `LivingEntity.resolvePlayerResponsibleForDamage`: any player-sourced hit starts a
            // hundred-tick window in which a death still counts as that player's kill.
            if cause
                .or(source)
                .is_some_and(|attacker| attacker.get_entity().entity_type == &EntityType::PLAYER)
            {
                self.last_hurt_by_player_time
                    .store(PLAYER_KILL_MEMORY_TICKS, Relaxed);
            }

            // `LivingEntity.hurtServer` clamps negative incoming damage to zero rather than
            // treating it as an invalid hit. Do this before plugins observe the value, matching
            // the vanilla damage domain exposed to later damage processing.
            if amount < 0.0 {
                amount = 0.0;
            }

            let mut damage_event =
                crate::plugin::api::events::entity::entity_damage::EntityDamageEvent::new(
                    self.entity.entity_id,
                    damage_type,
                    amount,
                );
            if let Some(server) = self.entity.world.load().server.upgrade() {
                server.plugin_manager.fire(&server, &mut damage_event).await;
            }
            if damage_event.cancelled {
                return false;
            }
            amount = normalize_non_finite_damage(damage_event.damage);

            // A successful hit restarts a mob's inactivity timer. Vanilla keeps this field on
            // every living entity; Pumpkin stores it on MobEntity, so player victims simply do
            // not have a corresponding timer to reset.
            if let Some(mob) = caller.get_mob() {
                mob.get_mob_entity().no_action_time.store(0, Relaxed);
            }

            // Brain `HURT_BY`. Vanilla routes this through `HurtBySensor`
            // (`ai/sensing/HurtBySensor.java:18-28`), which each tick mirrors
            // `LivingEntity.getLastDamageSource()` -- a field that self-clears 40 ticks after
            // the last hit (`LivingEntity.java:1419-1425`). Pumpkin has no `lastDamageSource`
            // field, so the write happens here instead, with a 40-tick memory expiry standing
            // in for that self-clearing.
            //
            // This is the write path the split-lock design exists for: this method is called
            // from projectiles, blocks and fluids, i.e. from outside the damaged mob's own AI
            // tick. It works only because `MemoryStore` is never taken out of its mutex.
            // `HURT_BY_ENTITY` is not written -- no behavior in the current port reads it.
            if let Some(mob) = caller.get_mob()
                && let Some(brain) = mob.get_mob_entity().brain.as_ref()
            {
                brain.set_with_expiry::<crate::entity::ai::brain::memory::HurtByMemory>(
                    damage_type,
                    40,
                );
            }

            let world = self.entity.world.load();
            // `LivingEntity.hurtServer:1185` and `Player.hurtServer:671` both key on the whole
            // `is_fire` tag, which also covers campfires and the two fireball sources.
            let is_fire_damage = damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_FIRE);

            // Like fire and drowning damage, these are gated for players only;
            // mobs still take them.
            if self.entity.entity_type == &EntityType::PLAYER {
                let game_rules = &world.level_info.load().game_rules;
                if damage_type == DamageType::FALL && !game_rules.fall_damage {
                    return false;
                }
                if damage_type == DamageType::FREEZE && !game_rules.freeze_damage {
                    return false;
                }
            }

            // Fire damage can be prevented by either game rules or fire resistance
            if is_fire_damage {
                // Check game rule for fire damage (only for players)
                if self.entity.entity_type == &EntityType::PLAYER
                    && !world.level_info.load().game_rules.fire_damage
                {
                    return false;
                }

                // Check for fire resistance effect
                if self.has_effect(&StatusEffect::FIRE_RESISTANCE).await {
                    return false;
                }
            }

            // Vanilla stops a resting victim after damage is accepted and immunity gates have
            // passed, but before blocking or hurt-cooldown rejection. Use the occupied bed,
            // rather than the respawn point, to clear the actual sleep state.
            if let Some(player) = caller.get_player() {
                player.stop_sleeping_after_damage().await;
            }

            // Check for shield blocking. Vanilla resolves blocking in `applyItemBlocking`
            // (hurtServer:1200) before the freeze multiplier (1203-1205) and the helmet
            // multiplier/hurtHelmet call (1207-1210), so this must run on the raw `amount`
            // before either multiplier is applied.
            let shield_source_position = position.or_else(|| {
                source
                    .or(cause)
                    .map(|entity| entity.get_entity().pos.load())
            });

            // These damage types bypass the hurt cooldown and death protection
            let bypasses_cooldown_protection =
                damage_type == DamageType::GENERIC_KILL || damage_type == DamageType::OUT_OF_WORLD;

            let mut damage_after_armor = amount;
            if !bypasses_armor_durability(&damage_type) {
                let mut armor = 0.0f32;
                let mut toughness = 0.0f32;
                {
                    let equipment_lock = self.entity_equipment.lock().await;
                    for slot in [
                        EquipmentSlot::HEAD,
                        EquipmentSlot::CHEST,
                        EquipmentSlot::LEGS,
                        EquipmentSlot::FEET,
                    ] {
                        if let Some(stack) = equipment_lock.equipment.get(&slot)
                            && !stack.is_empty()
                            && let Some(modifiers) =
                                stack.get_data_component::<AttributeModifiersImpl>()
                        {
                            for modifier in modifiers.attribute_modifiers.iter() {
                                if modifier.r#type == &Attributes::ARMOR {
                                    armor += modifier.amount as f32;
                                } else if modifier.r#type == &Attributes::ARMOR_TOUGHNESS {
                                    toughness += modifier.amount as f32;
                                }
                            }
                        }
                    }
                }
                let value = 2.0f32 + toughness / 4.0;
                let clamped_armor = (armor - damage_after_armor / value)
                    .max(armor / 5.0)
                    .min(20.0);
                damage_after_armor *= 1.0 - clamped_armor / 25.0;
            }

            let mut damage_after_enchantments = damage_after_armor;
            if damage_type != DamageType::OUT_OF_WORLD {
                let mut epf = 0i32;
                {
                    let equipment_lock = self.entity_equipment.lock().await;
                    for slot in [
                        EquipmentSlot::HEAD,
                        EquipmentSlot::CHEST,
                        EquipmentSlot::LEGS,
                        EquipmentSlot::FEET,
                    ] {
                        if let Some(stack) = equipment_lock.equipment.get(&slot)
                            && !stack.is_empty()
                            && let Some(enchantments) =
                                stack.get_data_component::<EnchantmentsImpl>()
                        {
                            for (enchantment, level) in enchantments.enchantment.iter() {
                                let mut factor = 0;
                                let enc = *enchantment;
                                if enc == &Enchantment::PROTECTION {
                                    if damage_type != DamageType::DROWN
                                        && damage_type != DamageType::STARVE
                                        && damage_type != DamageType::GENERIC_KILL
                                    {
                                        factor = *level;
                                    }
                                } else if enc == &Enchantment::FIRE_PROTECTION {
                                    if is_fire_damage {
                                        factor = *level * 2;
                                    }
                                } else if enc == &Enchantment::BLAST_PROTECTION {
                                    if damage_type == DamageType::EXPLOSION
                                        || damage_type == DamageType::PLAYER_EXPLOSION
                                    {
                                        factor = *level * 2;
                                    }
                                } else if enc == &Enchantment::PROJECTILE_PROTECTION {
                                    if damage_type == DamageType::ARROW
                                        || damage_type == DamageType::MOB_PROJECTILE
                                        || damage_type == DamageType::THROWN
                                    {
                                        factor = (*level) * 2;
                                    }
                                } else if enc == &Enchantment::FEATHER_FALLING
                                    && damage_type == DamageType::FALL
                                {
                                    factor = (*level) * 4;
                                }
                                epf += factor;
                            }
                        }
                    }
                }
                epf = epf.min(20);
                if epf > 0 {
                    damage_after_enchantments *= 1.0 - (epf as f32 * 0.04);
                }
            }

            // Apply Resistance effect reduction (20% per level), excluding bypasses_cooldown_protection and starvation damage
            let resistance_reduction =
                if !bypasses_cooldown_protection && damage_type != DamageType::STARVE {
                    self.get_effect(&StatusEffect::RESISTANCE)
                        .await
                        .map_or(0.0, |e| 0.2 * (e.amplifier + 1) as f32)
                } else {
                    0.0
                };

            // Total damage after reductions
            if resistance_reduction > 0.0 {
                let resisted = damage_after_enchantments * resistance_reduction;
                if let Some(player) = caller.get_player() {
                    player
                        .increment_stat(
                            StatisticCategory::Custom,
                            CustomStatistic::DamageResisted as i32,
                            damage_stat_amount(resisted),
                        )
                        .await;
                }
                if let Some(attacker_player) = cause.and_then(|c| c.get_player()) {
                    attacker_player
                        .increment_stat(
                            StatisticCategory::Custom,
                            CustomStatistic::DamageDealtResisted as i32,
                            damage_stat_amount(resisted),
                        )
                        .await;
                }
            }

            // Check for shield blocking
            if self.is_blocking().await
                && !bypasses_shield(&damage_type)
                && let Some(pos) = shield_source_position
            {
                let player_pos = self.entity.pos.load();
                let look_vec = Vector3::rotation_vector(0.0, self.entity.yaw.load() as f64);
                let mut source_to_player = (player_pos - pos).normalize();
                source_to_player.y = 0.0;

                if source_to_player.dot(&look_vec) < 0.0 {
                    world.play_sound(Sound::ItemShieldBlock, SoundCategory::Players, &player_pos);

                    // Vanilla: `LivingEntity.blockUsingShield` -> `attacker.blockedByItem(this,
                    // source, damage)`. Called on the attacker, not the defender.
                    if let Some(attacker_mob) = cause.and_then(EntityBase::get_mob) {
                        attacker_mob.blocked_by_item(caller, amount).await;
                    }

                    if let Some(player) = caller.get_player() {
                        player
                            .increment_stat(
                                StatisticCategory::Custom,
                                CustomStatistic::DamageBlockedByShield as i32,
                                (amount * 10.0) as i32,
                            )
                            .await;

                        player.trigger_advancement(crate::entity::player::advancement::trigger::AdvancementTrigger::DeflectedDamage).await;
                    }

                    // `LivingEntity.getSecondsToDisableBlocking` (`LivingEntity.java:3967-3971`)
                    // reads the active attacker's `Weapon.disableBlockingForSeconds`, and
                    // `Player.blockUsingItem` (`Player.java:716-720`) passes it to
                    // `BlocksAttacks.disable` (`BlocksAttacks.java:81-99`).
                    if let Some(attacker) = cause
                        && let Some(victim_player) = caller.get_player()
                        && let Some(attacker_living) = attacker.get_living_entity()
                    {
                        // `LivingEntity.getSecondsToDisableBlocking` compares the weapon item
                        // with `getActiveItem` (`LivingEntity.java:3967-3971`; active selection
                        // at `LivingEntity.java:2235-2241`) before applying its cooldown.
                        let weapon_item = attacker_living.held_item(attacker).await;
                        let active_item = attacker_living.active_item(attacker).await;
                        if weapon_item.are_equal(&active_item)
                            && let Some(weapon) = weapon_item.get_data_component::<WeaponImpl>()
                        {
                            let cooldown_ticks =
                                (weapon.disable_blocking_for_seconds * 20.0).round() as i32;
                            if cooldown_ticks > 0 {
                                let active_hand = *self.active_hand.lock().await;
                                if let Some(hand) = active_hand {
                                    let blocking_item =
                                        victim_player.inventory().get_stack_in_hand(hand).await;
                                    victim_player
                                        .start_cooldown(
                                            blocking_item.item.registry_key.to_string(),
                                            cooldown_ticks,
                                        )
                                        .await;
                                }
                                self.clear_active_hand().await;
                                world.broadcast_packet_all(&CEntityStatus::new(
                                    self.entity.entity_id,
                                    30,
                                ));
                            }
                        }
                    }

                    // `BlocksAttacks.hurtBlockingItem` applies only to players. Resolve the
                    // used hand through PlayerInventory so main-hand shields, owner updates,
                    // break effects, and the broken-item statistic all use the same authoritative
                    // path as other player item damage.
                    let active_hand = *self.active_hand.lock().await;
                    if let Some(player) = caller.get_player()
                        && let Some(hand) = active_hand
                    {
                        let slot = equipment_slot_for_hand(hand);
                        let blocking_item = player.inventory().get_stack_in_hand(hand).await;
                        player
                            .increment_stat(
                                StatisticCategory::Used,
                                blocking_item.item.id as i32,
                                1,
                            )
                            .await;

                        if let Some(durability_damage) = shield_block_durability_damage(amount) {
                            player.damage_item_in_slot(&slot, durability_damage).await;
                            if player.inventory().get_stack_in_hand(hand).await.is_empty() {
                                self.clear_active_hand().await;
                            }
                        }
                    }

                    return false;
                }
            }

            // Vanilla parity: entities in FREEZE_HURTS_EXTRA_TYPES take 5x freezing damage.
            if damage_type == DamageType::FREEZE
                && self
                    .entity
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_FREEZE_HURTS_EXTRA_TYPES)
            {
                amount *= 5.0;
            }

            // Vanilla calls `hurtHelmet` before the hurt-cooldown comparison
            // (`LivingEntity.java:1207-1210`). Only Player overrides this hook in the current
            // entity hierarchy (`Player.java:743-745`); route its head slot through the same
            // equipment-damage path before applying the 0.75 damage multiplier.
            if damages_helmet(&damage_type) {
                self.damage_armor_items(caller, amount, &damage_type, true)
                    .await;
                amount *= 0.75;
            }

            amount = normalize_non_finite_damage(amount);

            // These damage types bypass the hurt cooldown and death protection
            let bypasses_cooldown_protection =
                damage_type == DamageType::GENERIC_KILL || damage_type == DamageType::OUT_OF_WORLD;

            // Apply hurt cooldown logic. Vanilla compares and stores `damage` in the same
            // pre-armor/enchantment/resistance domain (LivingEntity.hurtServer:1217-1230):
            // only the freeze/helmet multipliers and shield blocking happen before this.
            let last_damage = self.last_damage_taken.load();
            let (raw_increment, play_sound) =
                if self.hurt_cooldown.load(Relaxed) > 10 && !bypasses_cooldown_protection {
                    if amount <= last_damage {
                        return false;
                    }
                    (amount - last_damage, false)
                } else {
                    self.hurt_cooldown.store(20, Relaxed);
                    (amount, true)
                };
            let damage_sequence = self.last_damage_sequence.fetch_add(1, Relaxed) + 1;
            self.last_damage_taken.store(amount);

            // Armor, enchantment protection and resistance reduce only the incremental
            // damage actually dealt this hit, not the full raw amount (actuallyHurt calls
            // getDamageAfterArmorAbsorb/getDamageAfterMagicAbsorb on `damage - lastHurt`,
            // LivingEntity.java:1222,1953-1956).
            let mut damage_after_armor = raw_increment;
            if !bypasses_armor_durability(&damage_type) {
                let mut armor = 0.0f32;
                let mut toughness = 0.0f32;
                {
                    let equipment_lock = self.entity_equipment.lock().await;
                    for slot in [
                        EquipmentSlot::HEAD,
                        EquipmentSlot::CHEST,
                        EquipmentSlot::LEGS,
                        EquipmentSlot::FEET,
                    ] {
                        let stack = equipment_lock.get(&slot);
                        if !stack.is_empty()
                            && let Some(modifiers) =
                                stack.get_data_component::<AttributeModifiersImpl>()
                        {
                            for modifier in modifiers.attribute_modifiers.iter() {
                                if modifier.r#type == &Attributes::ARMOR {
                                    armor += modifier.amount as f32;
                                } else if modifier.r#type == &Attributes::ARMOR_TOUGHNESS {
                                    toughness += modifier.amount as f32;
                                }
                            }
                        }
                    }
                }
                let value = 2.0f32 + toughness / 4.0;
                let clamped_armor = (armor - damage_after_armor / value)
                    .max(armor / 5.0)
                    .min(20.0);
                let mut armor_fraction = clamped_armor / 25.0;

                if let Some(attacker) = source
                    && let Some(player) = attacker
                        .cast_any()
                        .downcast_ref::<crate::entity::player::Player>()
                {
                    let held = player.inventory().held_item();
                    let breach_level = held.await.get_enchantment_level(&Enchantment::BREACH);
                    if breach_level > 0 {
                        armor_fraction = breach_armor_fraction(armor_fraction, breach_level);
                    }
                }

                damage_after_armor *= 1.0 - armor_fraction;
            }

            // Apply Resistance unless the damage source bypasses effects or resistance.
            // Vanilla applies resistance before enchantment protection
            // (getDamageAfterMagicAbsorb, LivingEntity.java:1913-1926 resistance,
            // 1935-1948 enchantments).
            let resistance_reduction = if !damage_type
                .has_tag(&tag::DamageType::MINECRAFT_BYPASSES_EFFECTS)
                && !damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_RESISTANCE)
            {
                self.get_effect(&StatusEffect::RESISTANCE)
                    .await
                    .map_or(0.0, |e| (0.2 * (e.amplifier + 1) as f32).min(1.0))
            } else {
                0.0
            };

            let damage_after_resistance = damage_after_armor * (1.0 - resistance_reduction);

            if resistance_reduction > 0.0 {
                let resisted = damage_after_armor * resistance_reduction;
                if let Some(player) = caller.get_player() {
                    player
                        .increment_stat(
                            StatisticCategory::Custom,
                            CustomStatistic::DamageResisted as i32,
                            (resisted * 10.0) as i32,
                        )
                        .await;
                }
                // `Player.attack` handles direct melee and mace damage from the victim's real
                // health delta, including the overkill cap. Keep this generic path for every
                // other player-caused source, but never award both paths for one attack.
                let direct_player_attack = damage_type == DamageType::PLAYER_ATTACK
                    || damage_type == DamageType::MACE_SMASH;
                if !direct_player_attack
                    && let Some(attacker_player) = cause.and_then(|c| c.get_player())
                {
                    attacker_player
                        .increment_stat(
                            StatisticCategory::Custom,
                            CustomStatistic::DamageDealtResisted as i32,
                            (resisted * 10.0) as i32,
                        )
                        .await;
                }
            }

            let mut damage_after_enchantments = damage_after_resistance;
            if !bypasses_enchantments(&damage_type) {
                let mut epf = 0i32;
                {
                    let equipment_lock = self.entity_equipment.lock().await;
                    for slot in [
                        EquipmentSlot::HEAD,
                        EquipmentSlot::CHEST,
                        EquipmentSlot::LEGS,
                        EquipmentSlot::FEET,
                    ] {
                        let stack = equipment_lock.get(&slot);
                        if !stack.is_empty()
                            && let Some(enchantments) =
                                stack.get_data_component::<EnchantmentsImpl>()
                        {
                            for (enchantment, level) in enchantments.enchantment.iter() {
                                for effect in crate::enchantment::effects_for(enchantment) {
                                    let crate::enchantment::EnchantmentEffect::DamageProtection(
                                        condition,
                                        value,
                                    ) = effect
                                    else {
                                        continue;
                                    };
                                    let applies = match condition {
                                        crate::enchantment::ProtectionCondition::Always => {
                                            damage_type != DamageType::DROWN
                                                && damage_type != DamageType::STARVE
                                                && damage_type != DamageType::GENERIC_KILL
                                        }
                                        crate::enchantment::ProtectionCondition::IsFire => {
                                            is_fire_damage
                                        }
                                        crate::enchantment::ProtectionCondition::IsExplosion => {
                                            damage_type == DamageType::EXPLOSION
                                                || damage_type == DamageType::PLAYER_EXPLOSION
                                        }
                                        crate::enchantment::ProtectionCondition::IsProjectile => {
                                            damage_type == DamageType::ARROW
                                                || damage_type == DamageType::MOB_PROJECTILE
                                                || damage_type == DamageType::THROWN
                                        }
                                        crate::enchantment::ProtectionCondition::IsFall => {
                                            damage_type == DamageType::FALL
                                        }
                                    };
                                    if applies {
                                        epf += value.calculate(*level) as i32;
                                    }
                                }
                            }
                        }
                    }
                }
                epf = epf.min(20);
                if epf > 0 {
                    damage_after_enchantments *= 1.0 - (epf as f32 * 0.04);
                }
            }

            // Finalize state: damage actually applied this hit, after armor/resistance/enchant.
            let damage_amount = damage_after_enchantments.max(0.0);

            let Some(server) = world.server.upgrade() else {
                return false;
            };

            // Vanilla stores the last damage source for every successful, non-blocked hit,
            // including environmental damage. Pack its world tick and panic-causing tag
            // together so EscapeDangerGoal reads one consistent state.
            let damage_tick = world.get_world_age().await;
            let damage_state = (
                damage_sequence,
                damage_tick,
                damage_causes_panic(damage_type),
            );
            let mut observed = self.last_damage_state.load();
            while observed.0 < damage_state.0 {
                match self
                    .last_damage_state
                    .compare_exchange(observed, damage_state)
                {
                    Ok(_) => break,
                    Err(actual) => observed = actual,
                }
            }

            let config = &server.advanced_config.pvp;

            if config.hurt_animation {
                let entity_id = self.entity.entity_id;
                let hurt_yaw = source.map_or(0.0, |source| {
                    let src = source.get_entity().pos.load();
                    let tgt = self.entity.pos.load();
                    (src.z - tgt.z).atan2(src.x - tgt.x).to_degrees() as f32
                        - self.entity.yaw.load()
                });
                let hurt_event = SActorEvent {
                    entity_runtime_id: VarULong(entity_id as u64),
                    event_type: ActorEventType::Hurt,
                    event_data: VarInt(0),
                    fire_at_position: None,
                };
                world
                    .broadcast_editioned(
                        &CHurtAnimation::new(VarInt(entity_id), hurt_yaw),
                        &hurt_event,
                    )
                    .await;
                world.broadcast_packet_all(&CEntityStatus::new(entity_id, 2));
            }

            world.broadcast_packet_all(&CDamageEvent::new(
                self.entity.entity_id.into(),
                damage_type.id.into(),
                source.map(|e| e.get_entity().entity_id.into()),
                cause.map(|e| e.get_entity().entity_id.into()),
                position,
            ));

            // Try to spawn infested silverfish
            self.try_spawn_infested_silverfish().await;

            if play_sound {
                // `LivingEntity.hurtServer` calls `Entity.markHurt` for a full-impact hit
                // (`LivingEntity.java:1244`). The flag is consumed after this entity tick so
                // the current motion is sent even when knockback did not change it.
                if !damage_type.has_tag(&tag::DamageType::MINECRAFT_NO_IMPACT) {
                    self.entity.mark_hurt();
                }

                // `Mob.playHurtSound` (Mob.java:295-299) resets the idle-sound timer, so a mob
                // that was just hit does not chirp immediately afterwards.
                if let Some(mob) = caller.get_mob() {
                    mob.get_mob_entity()
                        .ambient_sound_time
                        .store(-mob.get_ambient_sound_interval(), Relaxed);
                }
                // `LivingEntity.playHurtSound` resolves `getHurtSound` on the concrete mob
                // first so instance-dependent sounds win (e.g. the copper golem's oxidation
                // stage, `CopperGolem.java:389-391`), then falls back to the static
                // per-entity-type table.
                let hurt_sound = caller
                    .get_mob()
                    .and_then(Mob::get_hurt_sound)
                    .unwrap_or_else(|| self.hurt_sound());
                let pitch = caller.get_mob().map_or(1.0, Mob::get_sound_pitch);
                // `LivingEntity.makeSound` passes `getVoicePitch` to the sound packet
                // (`LivingEntity.java:1427-1434`).
                // `Mob.playHurtSound` delegates to `LivingEntity.playHurtSound`, whose
                // `makeSound` ultimately uses the entity's sound source (`Mob.java:295-299`;
                // `LivingEntity.java:1427-1434`). Players retain their player category; mobs
                // use the existing `Mob::get_sound_source` override.
                let sound_category = hurt_sound_category(
                    caller.get_player().is_some(),
                    caller.get_mob().map(Mob::get_sound_source),
                );
                world.play_sound_fine(
                    hurt_sound,
                    sound_category,
                    &self.entity.pos.load(),
                    1.0,
                    pitch,
                );

                // `LivingEntity.hurtServer` gates the default 0.4 knockback on the
                // `no_knockback` damage-type tag (LivingEntity.java:1247-1249) before calling
                // `dealDefaultKnockback` (LivingEntity.java:1290-1305). Without that gate a
                // creeper blast knocked the victim back twice - once from the explosion's own
                // impulse and again from this hit - and magic, wither, dragon breath and spear
                // damage knocked back at all, none of which vanilla does.
                //
                // `dealDefaultKnockback` itself branches on `source.getDirectEntity()`
                // (LivingEntity.java:1292-1299): when the direct entity is a `Projectile`,
                // knockback follows *the projectile's own flight direction*
                // (`Projectile.calculateHorizontalHurtKnockbackDirection`,
                // `Projectile.java:380-384`, the default returning `-deltaMovement.{x,z}`), not
                // the position of whoever fired it. `cause` here is the direct/physical hit
                // cause (e.g. the arrow itself - see `projectile_owner_id`'s doc comment above),
                // matching vanilla's `directEntity`. Without this branch every arrow hit fell
                // through with no `source` at all (arrow.rs passes `source: None`) and applied
                // no base knockback whatsoever - only the separate Punch-enchantment bonus
                // (`AbstractArrow.doKnockback`) still landed.
                let knockback_direction = cause
                    .filter(|c| {
                        crate::entity::projectile::is_projectile(c.get_entity().entity_type)
                    })
                    .map(|projectile| {
                        let (x, z) = projectile.calculate_horizontal_hurt_knockback_direction(self);
                        (-x, -z)
                    })
                    .or_else(|| {
                        source.map(|source| {
                            let source_pos = source.get_entity().pos.load();
                            let target_pos = self.entity.pos.load();
                            (source_pos.x - target_pos.x, source_pos.z - target_pos.z)
                        })
                    });
                if let Some((dx, dz)) = knockback_direction
                    && !damage_type.has_tag(&tag::DamageType::MINECRAFT_NO_KNOCKBACK)
                {
                    let resistance = self.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE);
                    self.entity.apply_knockback(
                        knockback_after_resistance(0.4, resistance),
                        dx,
                        dz,
                    );
                    self.entity.send_velocity();
                }
            }

            // Consume absorption first, then apply remaining damage to health
            let mut remaining = damage_amount;
            let current_abs = self.absorption.load();
            if current_abs > 0.0 {
                let absorbed = current_abs.min(remaining);
                if let Some(player) = caller.get_player() {
                    player
                        .increment_stat(
                            StatisticCategory::Custom,
                            CustomStatistic::DamageAbsorbed as i32,
                            damage_stat_amount(absorbed),
                        )
                        .await;
                }

                if let Some(attacker_player) = cause.and_then(|c| c.get_player()) {
                    attacker_player
                        .increment_stat(
                            StatisticCategory::Custom,
                            CustomStatistic::DamageDealtAbsorbed as i32,
                            damage_stat_amount(absorbed),
                        )
                        .await;
                }

                if current_abs >= remaining {
                    let new_abs = current_abs - remaining;
                    self.set_absorption(new_abs).await;
                    remaining = 0.0;
                } else {
                    remaining -= current_abs;
                    self.set_absorption(0.0).await;
                }

                // Track attacker for RevengeGoal (only after confirming damage)
                if let Some(attacker) = cause.or(source) {
                    self.last_attacker_id
                        .store(attacker.get_entity().entity_id, Relaxed);
                    self.last_attacked_time
                        .store(self.entity.age.load(Relaxed), Relaxed);
                }
            }

            // Apply remaining damage to health (clamped)
            let max_h = self.get_max_health();
            let new_health = self.health.load() - remaining;
            let clamped_health = new_health.max(0.0).min(max_h);
            if remaining > 0.0 {
                self.set_health(clamped_health);
                // `LivingEntity.actuallyHurt` emits ENTITY_DAMAGE after health changes
                // (`LivingEntity.java:1953-1970`). Resolve the victim from the active world so
                // the event context carries the same source entity for players and mobs.
                let damaged_entity = world
                    .get_player_by_uuid(self.entity.entity_uuid)
                    .map(|player| player as Arc<dyn EntityBase>)
                    .or_else(|| world.get_entity_by_uuid(self.entity.entity_uuid));
                crate::world::game_event::emit_game_event(
                    &world,
                    pumpkin_data::game_event::GameEvent::EntityDamage,
                    self.entity.pos.load(),
                    damaged_entity.map_or_else(
                        crate::world::game_event::GameEventContext::none,
                        crate::world::game_event::GameEventContext::of_entity,
                    ),
                )
                .await;
            }

            // `PanicGoal.shouldPanic` checks the most recent accepted damage source against
            // `DamageTypeTags.PANIC_CAUSES`. Publish it immediately after the health change,
            // matching `LivingEntity.hurtServer` before later statistics and thorns work.

            if remaining > 0.0 {
                // Statistics updates
                if let Some(player) = caller.get_player() {
                    player
                        .increment_stat(
                            StatisticCategory::Custom,
                            CustomStatistic::DamageTaken as i32,
                            damage_stat_amount(remaining),
                        )
                        .await;
                }

                if let Some(attacker_player) = cause.and_then(|c| c.get_player()) {
                    attacker_player
                        .increment_stat(
                            StatisticCategory::Custom,
                            CustomStatistic::DamageDealt as i32,
                            damage_stat_amount(remaining),
                        )
                        .await;
                }

                // Track attacker for RevengeGoal (only after confirming damage)
                if let Some(attacker) = cause.or(source) {
                    self.last_attacker_id
                        .store(attacker.get_entity().entity_id, Relaxed);
                    self.last_attacked_time
                        .store(self.entity.age.load(Relaxed), Relaxed);
                }

                // Thorns (`thorns.json`, `minecraft:post_attack`): each worn armor piece
                // with Thorns rolls independently, chance = 0.15 * level. On success it
                // damages the attacker (DamageType::THORNS, uniform 1.0-5.0) and wears
                // 2 durability off that armor piece.
                if let Some(attacker) = cause {
                    let mut equipment_lock = self.entity_equipment.lock().await;
                    for slot in [
                        EquipmentSlot::HEAD,
                        EquipmentSlot::CHEST,
                        EquipmentSlot::LEGS,
                        EquipmentSlot::FEET,
                    ] {
                        let mut stack = equipment_lock.get(&slot);
                        if stack.is_empty() {
                            continue;
                        }
                        let level = stack.get_enchantment_level(&Enchantment::THORNS);
                        if level <= 0 {
                            continue;
                        }
                        if rand::random::<f32>() >= 0.15 * level as f32 {
                            continue;
                        }
                        let thorns_damage = 1.0 + rand::random::<f32>() * 4.0;
                        let broken_item = stack.clone();
                        if stack.damage_item(2) == DamageResult::Broken {
                            if let Some(player) = self.get_player() {
                                player
                                    .increment_stat(
                                        StatisticCategory::Broken,
                                        stack.item.id as i32,
                                        1,
                                    )
                                    .await;
                            }
                            // Vanilla `hurtArmor` routes a broken equipped item through
                            // `onEquippedItemBroken` (`LivingEntity.java:3845-3848`), which
                            // broadcasts the break status and removes attribute modifiers; the
                            // client then plays `breakItem`'s particles
                            // (`LivingEntity.java:1439-1448`) in response.
                            self.on_equipped_item_broken(&broken_item, &slot).await;
                            self.spawn_item_particles(&broken_item, 5);
                            stack = ItemStack::EMPTY.clone();
                            let broken_stack = stack.clone();
                            equipment_lock.put(&slot, stack);
                            self.send_equipment_changes(&[(slot, broken_stack)]);
                        } else {
                            equipment_lock.put(&slot, stack);
                        }
                        // `DamageEntity.apply`: the thorns hit is dealt BY the wearer, so a
                        // kill is credited to them and the attacker can retaliate.
                        attacker
                            .damage_with_context(
                                attacker,
                                thorns_damage,
                                DamageType::THORNS,
                                Some(self.entity.pos.load()),
                                Some(caller),
                                Some(caller),
                            )
                            .await;
                    }
                }
            }

            // Check if the entity died and isn't protected by a death protection mechanic (ex. totem of undying)
            if clamped_health <= 0.0
                && (bypasses_cooldown_protection || !self.try_use_death_protector(caller).await)
            {
                let mut death_event =
                    crate::plugin::api::events::entity::entity_death::EntityDeathEvent::new(
                        self.entity.entity_id,
                        0,
                    );
                if let Some(server) = world.server.upgrade() {
                    server.plugin_manager.fire(&server, &mut death_event).await;
                }
                if let Some(player) = caller.get_player()
                    && let Some(player_arc) = world.get_player_by_uuid(player.gameprofile.id)
                {
                    let mut player_death_event =
                        crate::plugin::api::events::entity::entity_death::PlayerDeathEvent::new(
                            player_arc,
                            pumpkin_util::text::TextComponent::text("Died"),
                            0,
                        );
                    if let Some(server) = world.server.upgrade() {
                        server
                            .plugin_manager
                            .fire(&server, &mut player_death_event)
                            .await;
                    }
                }

                self.on_death(damage_type, source, cause).await;
            }

            // Armor durability wear uses the pre-armor-absorb increment, matching vanilla's
            // `hurtArmor(damageSource, damage)` call inside `getDamageAfterArmorAbsorb`
            // (LivingEntity.java:1903), entered from `actuallyHurt` with the same `dmg` that
            // `getDamageAfterAbsorb` reduces at line 1904 -- i.e. `raw_increment`, not the
            // post-reduction `damage_amount`.
            if raw_increment > 0.0 && !bypasses_armor_durability(&damage_type) {
                self.damage_armor_items(caller, raw_increment, &damage_type, false)
                    .await;
            }

            true
        })
    }

    fn tick_in_void<'a>(&'a self, dyn_self: &'a dyn EntityBase) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            dyn_self
                .damage(dyn_self, 4.0, DamageType::OUT_OF_WORLD)
                .await;
        })
    }

    fn get_gravity(&self) -> f64 {
        self.get_attribute_value(&Attributes::GRAVITY)
    }

    #[allow(clippy::too_many_lines)]
    fn tick<'a>(
        &'a self,
        caller: &'a Arc<dyn EntityBase>,
        server: &'a Server,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.entity.tick(caller, server).await;
            // Vanilla `LivingEntity.tick` decrements the post-impulse grace timer once per tick
            // (`LivingEntity.java:2864-2868`).
            let _ = self.post_impulse_context_reset_grace_time.fetch_update(
                Relaxed,
                Relaxed,
                |ticks| (ticks > 0).then_some(ticks - 1),
            );
            if let Some(mob) = caller.get_mob()
                && mob.get_entity().entity_id == self.entity.entity_id
            {
                mob.update_swimming().await;
            }
            tick_equipment_items(self, caller.as_ref(), server).await;
            self.tick_equipment_attributes(caller.as_ref()).await;
            self.tick_soul_speed(caller.as_ref()).await;
            self.tick_location_changed_effects(caller.as_ref()).await;
            let was_alive_before_air =
                !self.dead.load(Relaxed) && self.health.load() > 0.0 && !self.entity.is_removed();
            if self.entity.entity_type == &EntityType::PLAYER
                && was_alive_before_air
                && let Some(player) = caller.cast_any().downcast_ref::<Player>()
            {
                player.breath_manager.tick(player).await;
                // `Player.tick` (`Player.java:479-482`): shoulder-entity dismount checks run
                // every tick regardless of alive/air state.
                player.handle_shoulder_entities().await;
            }
            self.tick_air_supply(caller, was_alive_before_air).await;

            // Only tick movement if the entity is alive. This prevents a dead "corpse"
            // from continuing to be simulated (accumulating fall_distance/velocity).
            // We allow movement during death animation (20 ticks) so knockback is applied.
            let is_alive = !self.dead.load(Relaxed) && self.health.load() > 0.0;
            let in_death_animation =
                self.health.load() <= 0.0 && self.death_time.load(Relaxed) < 20;
            if !self.entity.is_removed()
                && (is_alive
                    || (in_death_animation && self.entity.entity_type != &EntityType::PLAYER))
            {
                let previous_bounding_box = self.entity.bounding_box.load();
                self.tick_movement(server, caller).await;
                // Vanilla-like order: freeze logic runs after movement/collisions.
                self.entity.tick_frozen(caller.as_ref()).await;
                self.tick_frost().await;
                self.tick_auto_spin_attack(caller, previous_bounding_box)
                    .await;
                self.push_entities(caller).await;
                // The shared movement emission path emits sounds and STEP/SWIM events after
                // movement crosses the next threshold (`Entity.java:867-901`).
                self.tick_swim_sound(caller).await;
            }

            // `LivingEntity.tickHeadTurn` queries the virtual head limit after movement
            // (`LivingEntity.java:3018-3025`); `Player` narrows it while blocking
            // (`Player.java:288-290`).
            let max_head_rotation = if let Some(player) = caller.get_player() {
                player.get_max_head_rotation_relative_to_body().await
            } else {
                50.0
            };
            self.tick_head_turn(max_head_rotation);

            // TODO
            let player = caller.get_player();
            let is_player = player.is_some();

            if !is_player {
                self.entity.send_pos_rot();
            }

            // Vanilla `ServerEntity` consumes `Entity.hurtMarked` after the entity tick and
            // sends motion even when the hit did not change velocity (`ServerEntity.java:225-228`).
            if self.entity.hurt_marked.swap(false, Relaxed) {
                self.entity.send_velocity();
            }

            // Fetch supporting blocks for players or other entities
            let supporting_pos = caller.get_player().map_or_else(
                || self.entity.get_supporting_block_pos(),
                super::player::Player::get_supporting_block_pos,
            );

            // Notify the block under the entity each tick if a supporting block position is found
            if let Some(supporting) = supporting_pos {
                let world = self.entity.world.load();
                let (block, state) = world.get_block_and_state(&supporting);

                world
                    .block_registry
                    .on_entity_step(
                        block,
                        &world,
                        caller.as_ref() as &dyn EntityBase,
                        &supporting,
                        state,
                        false,
                    )
                    .await;

                // Check slightly below supporting_pos for additional supporting blocks (blocks under carpets and the like)
                if !block.is_solid() {
                    let below_supporting = supporting.down();
                    let (below_block, below_state) = world.get_block_and_state(&below_supporting);

                    // If block is not air, notify it as well
                    world
                        .block_registry
                        .on_entity_step(
                            below_block,
                            &world,
                            caller.as_ref() as &dyn EntityBase,
                            &below_supporting,
                            below_state,
                            true, // below supporting block
                        )
                        .await;
                }
            }

            self.tick_effects().await;
            // Vanilla `LivingEntity.tick` reads the tracked arrow count before starting removal
            // (`LivingEntity.java:2754-2767`).
            let arrow_count = self.get_arrow_count();
            if arrow_count > 0 {
                self.tick_arrows(arrow_count);
            }
            self.tick_stingers();

            // Current active item
            {
                let item_in_use = self.item_in_use.lock().await.clone();
                let remaining_use_ticks = item_in_use
                    .is_some()
                    .then(|| self.item_use_time.fetch_sub(1, Ordering::Relaxed));

                if let Some(item) = item_in_use.as_ref()
                    && let Some(player) = caller.get_player()
                {
                    server
                        .item_registry
                        .on_use_tick(item, player, remaining_use_ticks.unwrap_or(0))
                        .await;
                }

                if let Some(item) = item_in_use.as_ref()
                    && remaining_use_ticks.is_some_and(|r| r <= 0)
                    && !server.item_registry.use_on_release(item.item.id, item)
                {
                    // `LivingEntity.updateUsingItem` skips completion for items whose
                    // `useOnRelease` is true (`LivingEntity.java:3472-3474`).
                    // Consume item
                    if let Some(food) = item.get_data_component::<FoodImpl>()
                        && let Some(player) = caller.get_player()
                    {
                        player
                            .hunger_manager
                            .eat(player, food.nutrition as u8, food.saturation)
                            .await;
                        self.entity.world.load().play_bedrock_level_sound(
                            "burp",
                            &self.entity.pos.load(),
                            -1,
                        );
                    }

                    self.apply_consumable_effects(item).await;

                    if let Some(consumable) = item.get_data_component::<ConsumableImpl>() {
                        let world = self.entity.world.load();
                        world.play_sound_event(
                            &consumable.sound_event,
                            SoundCategory::Players,
                            &self.entity.pos.load(),
                        );

                        // Consumable.onConsume, line 90: `user.gameEvent(this.animation ==
                        // ItemUseAnimation.DRINK ? GameEvent.DRINK : GameEvent.EAT)`. Fired
                        // unconditionally for any item with a Consumable component, regardless
                        // of animation, via Item.finishUsingItem (line 216).
                        let game_event = if consumable.animation == ConsumeAnimation::Drink {
                            pumpkin_data::game_event::GameEvent::Drink
                        } else {
                            pumpkin_data::game_event::GameEvent::Eat
                        };
                        crate::world::game_event::emit_game_event(
                            &world,
                            game_event,
                            self.entity.pos.load(),
                            crate::world::game_event::GameEventContext::of_entity(caller.clone()),
                        )
                        .await;
                    }

                    // Handle potion consumption
                    if item.get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>().is_some() {
                        let effects = crate::item::potion::PotionContents::read_potion_effects(item);
                        crate::item::potion::PotionContents::apply_effects_to(self, effects, 1.0, crate::item::potion::PotionApplicationSource::Normal).await;
                    }

                    // SuspiciousStewEffects.onConsume: every entry is applied as a plain
                    // `MobEffectInstance(effect, duration)`, so amplifier 0 and default
                    // visibility, with no duration scaling.
                    if let Some(stew) = item
                        .get_data_component::<pumpkin_data::data_component_impl::SuspiciousStewEffectsImpl>()
                    {
                        for entry in stew.effects.iter() {
                            let Some(effect_type) = StatusEffect::from_minecraft_name(&entry.effect)
                            else {
                                continue;
                            };
                            self.add_effect(pumpkin_data::potion::Effect {
                                effect_type,
                                duration: entry.duration,
                                amplifier: 0,
                                ambient: false,
                                show_particles: true,
                                show_icon: true,
                                blend: false,
                            })
                            .await;
                        }
                    }

                    // OminousBottleAmplifier.onConsume (ConsumableListener, applied outside
                    // the onConsumeEffects list via `stack.getAllOfType(ConsumableListener.class)`
                    // in Consumable.onConsume): always non-ambient, hidden particles/icon shown.
                    if let Some(amplifier) = item.get_data_component::<OminousBottleAmplifierImpl>()
                        && let Ok(amplifier) = u8::try_from(amplifier.amplifier)
                    {
                        self.add_effect(Effect {
                            effect_type: &StatusEffect::BAD_OMEN,
                            duration: 120_000,
                            amplifier,
                            ambient: false,
                            show_particles: false,
                            show_icon: true,
                            blend: false,
                        })
                        .await;
                    }

                    if consumable_clears_all_effects(item) {
                        if let Some(player) = caller.get_player() {
                            // This sends one removal packet per active effect before the
                            // living entity broadcasts the removal to nearby players.
                            player.remove_all_effects().await;
                        } else {
                            let effects: Vec<_> =
                                self.active_effects.lock().await.keys().copied().collect();
                            for effect in effects {
                                self.remove_effect(effect).await;
                            }
                        }
                    }

                    if let Some(player) = caller.get_player() {
                        player
                            .trigger_advancement(crate::entity::player::advancement::trigger::AdvancementTrigger::ConsumeItem {
                                item_id: format!("minecraft:{}", item.item.registry_key),
                            })
                            .await;

                        // Vanilla `Item#finishUsingItem` (default, non-food/non-consumable)
                        // returns the stack unchanged: no decrement happens on natural
                        // use-duration completion unless the item actually has FoodProperties
                        // or a Consumable component. Without this guard, any item with a
                        // finite get_use_duration() that a player holds to completion (e.g.
                        // SpyglassItem at 1200 ticks) gets silently consumed here.
                        if is_consumed_on_finish(item) {
                            let is_potion = item
                                .get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
                                .is_some();
                            // Prefer modifying the exact stack that matches the consumed item:
                            // 1) selected hotbar (held_item)
                            // 2) off-hand
                            // 3) fallback to active_hand if the above didn't match
                            let mut handled = false;
                            // Check main hand (hotbar selected)
                            let mut held = player.inventory.held_item().await;
                            if held.are_items_and_components_equal(item) {
                                if is_potion {
                                    if player.gamemode.load() != GameMode::Creative {
                                        held.decrement(1);
                                        if held.is_empty() {
                                            held = ItemStack::new(1, &Item::GLASS_BOTTLE);
                                        }
                                    }
                                } else {
                                    held.decrement_unless_creative(player.gamemode.load(), 1);
                                }
                                if held.is_empty()
                                    && let Some(remainder) = consumable_remainder(item)
                                {
                                    held = ItemStack::new(1, remainder);
                                }
                                player.inventory.set_held_item(held).await;
                                handled = true;
                            }

                            if !handled {
                                // Check off-hand
                                let mut off_hand = player.inventory.off_hand_item().await;
                                if off_hand.are_items_and_components_equal(item) {
                                    if is_potion {
                                        if player.gamemode.load() != GameMode::Creative {
                                            off_hand.decrement(1);
                                            if off_hand.is_empty() {
                                                off_hand = ItemStack::new(1, &Item::GLASS_BOTTLE);
                                            }
                                        }
                                    } else {
                                        off_hand
                                            .decrement_unless_creative(player.gamemode.load(), 1);
                                    }
                                    if off_hand.is_empty()
                                        && let Some(remainder) = consumable_remainder(item)
                                    {
                                        off_hand = ItemStack::new(1, remainder);
                                    }
                                    player
                                        .inventory
                                        .set_stack_in_hand(Hand::Left, off_hand)
                                        .await;
                                    handled = true;
                                }
                            }

                            if !handled {
                                // Use stored active_hand (as a fallback)
                                let active_hand = *self.active_hand.lock().await;
                                let hand_to_modify = active_hand.unwrap_or(Hand::Right);
                                let mut item_stack = self
                                    .get_stack_in_hand(caller.as_ref(), hand_to_modify)
                                    .await;

                                if is_potion {
                                    if player.gamemode.load() != GameMode::Creative {
                                        item_stack.decrement(1);
                                        if item_stack.is_empty() {
                                            item_stack = ItemStack::new(1, &Item::GLASS_BOTTLE);
                                        }
                                    }
                                } else {
                                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                                }
                                if item_stack.is_empty()
                                    && let Some(remainder) = consumable_remainder(item)
                                {
                                    item_stack = ItemStack::new(1, remainder);
                                }
                                player
                                    .inventory
                                    .set_stack_in_hand(hand_to_modify, item_stack)
                                    .await;
                            }

                            if let Some(cooldown) = item.get_use_cooldown() {
                                let group = cooldown
                                    .cooldown_group
                                    .clone()
                                    .unwrap_or_else(|| item.item.registry_key.to_string());
                                player
                                    .start_cooldown(group, (cooldown.seconds * 20.0) as i32)
                                    .await;
                            }
                        }

                        self.clear_active_hand().await;
                    }
                }

                if self.hurt_cooldown.load(Relaxed) > 0 {
                    self.hurt_cooldown.fetch_sub(1, Relaxed);
                }
                if self.get_last_hurt_by_player_memory_time() > 0 {
                    self.last_hurt_by_player_time.fetch_sub(1, Relaxed);
                }
                if self.is_dead_or_dying() {
                    let time = self
                        .death_time
                        .fetch_update(Relaxed, Relaxed, |time| Some(time.saturating_add(1)))
                        .unwrap_or_else(|time| time)
                        .saturating_add(1);
                    // Players remain part of the world until their client requests a
                    // respawn. Removing one here breaks reconnecting while dead.
                    if self.entity.entity_type == &EntityType::PLAYER {
                        return;
                    }
                    // Only send death particles once (on the exact tick death_time reaches 20)
                    // and then remove the entity, preventing entity_event spam.
                    if time == 20 && !self.entity.removed.swap(true, Ordering::Relaxed) {
                        self.entity.world.load().send_entity_status(
                            &self.entity,
                            EntityStatus::Death,
                            Some(ActorEventType::Death),
                        );
                        self.entity.remove().await;
                    }
                }
            }
        })
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        Some(self)
    }

    fn is_pushable(&self) -> bool {
        self.health.load() > 0.0 && !self.dead.load(Relaxed)
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }
}

const fn equipment_slot_for_hand(hand: Hand) -> EquipmentSlot {
    match hand {
        Hand::Left => EquipmentSlot::OFF_HAND,
        Hand::Right => EquipmentSlot::MAIN_HAND,
    }
}

impl LivingEntity {
    /// Applies data-driven `apply_effects` consume effects after an item completes use.
    /// Vanilla: `Consumable.onConsume` invokes every configured effect server-side.
    async fn apply_consumable_effects(&self, item: &ItemStack) {
        let Some(consumable) = item.get_data_component::<ConsumableImpl>() else {
            return;
        };

        for consume_effect in consumable.effects.iter() {
            match consume_effect {
                ConsumeEffect::ApplyEffects((effects, probability)) => {
                    if !consume_effect_probability_applies(*probability, rand::random()) {
                        continue;
                    }

                    for effect in effects.iter() {
                        let Some(effect_type) =
                            StatusEffect::from_minecraft_name(&effect.effect_id)
                        else {
                            continue;
                        };
                        let Ok(amplifier) = u8::try_from(effect.amplifier) else {
                            continue;
                        };

                        self.add_effect(Effect {
                            effect_type,
                            duration: effect.duration,
                            amplifier,
                            ambient: effect.ambient,
                            show_particles: effect.show_particles,
                            show_icon: effect.show_icon,
                            blend: false,
                        })
                        .await;
                    }
                }
                ConsumeEffect::ClearAllEffects => {
                    self.reset_effects_and_attributes().await;
                }
                ConsumeEffect::RemoveEffects(idset) => {
                    if let pumpkin_data::data_component_impl::IDSet::IDs(ids) = idset {
                        for effect_type in ids.iter() {
                            self.remove_effect(effect_type).await;
                        }
                    }
                }
                ConsumeEffect::PlaySound(sound) => {
                    let world = self.entity.world.load();
                    world.play_sound_event(sound, SoundCategory::Players, &self.entity.pos.load());
                }
                ConsumeEffect::TeleportRandomly(diameter) => {
                    self.teleport_randomly_on_consume(*diameter).await;
                }
            }
        }
    }

    /// `TeleportRandomlyConsumeEffect.apply` (26.2 decompile,
    /// world/item/consume_effects/TeleportRandomlyConsumeEffect.java:38-72): tries up to 16
    /// random offsets within `diameter` and stops at the first successful landing.
    async fn teleport_randomly_on_consume(&self, diameter: f32) {
        let pos = self.entity.pos.load();

        let (min_y, max_y) = {
            let world = self.entity.world.load();
            (
                f64::from(world.dimension.min_y),
                f64::from(world.dimension.min_y + world.dimension.logical_height - 1),
            )
        };

        for _ in 0..16 {
            let (dx, dy, dz) = {
                let mut rng = rand::rng();
                (
                    (rng.random_range(0.0..1.0) - 0.5) * f64::from(diameter),
                    (rng.random_range(0.0..1.0) - 0.5) * f64::from(diameter),
                    (rng.random_range(0.0..1.0) - 0.5) * f64::from(diameter),
                )
            };
            // Clamp to the dimension's playable Y range before randomTeleport searches for ground.
            let target_y = (pos.y + dy).clamp(min_y, max_y);

            // Clone out of the lock first: holding the guard as the `if let` scrutinee would
            // keep it alive for the whole block, across the `.await` below.
            let vehicle = self.entity.vehicle.lock().await.clone();
            if let Some(vehicle) = vehicle {
                vehicle
                    .get_entity()
                    .remove_passenger(self.entity.entity_id)
                    .await;
            }

            if self.random_teleport(pos.x + dx, target_y, pos.z + dz, true) {
                let world = self.entity.world.load();
                let is_fox = self.entity.entity_type == &EntityType::FOX;
                let (sound, category) = if is_fox {
                    (Sound::EntityFoxTeleport, SoundCategory::Neutral)
                } else {
                    (Sound::ItemChorusFruitTeleport, SoundCategory::Players)
                };
                world.play_sound(sound, category, &self.entity.pos.load());
                self.fall_distance.store(0.0);
                break;
            }
        }
    }

    /// `LivingEntity.randomTeleport` (26.2 decompile,
    /// world/entity/LivingEntity.java:3665-3709): walks down from `(x, y, z)` to the first
    /// block that blocks motion, then teleports there if the destination is free of block
    /// collision and liquid, reverting otherwise.
    fn random_teleport(&self, x: f64, y: f64, z: f64, show_particles: bool) -> bool {
        let origin = self.entity.pos.load();
        let world = self.entity.world.load();
        let dimension = self.entity.entity_dimension.load();

        let target_block = BlockPos::new(x.floor() as i32, y.floor() as i32, z.floor() as i32);
        if !world.is_loaded(&target_block) {
            return false;
        }

        let mut target_y = y;
        let mut pos_y = target_block.0.y;
        let mut landed = false;
        while !landed && pos_y > world.dimension.min_y {
            let below = BlockPos::new(target_block.0.x, pos_y - 1, target_block.0.z);
            if world.get_block_state(&below).is_solid() {
                landed = true;
            } else {
                target_y -= 1.0;
                pos_y -= 1;
            }
        }

        if !landed {
            return false;
        }

        self.entity
            .teleport(Vector3::new(x, target_y, z), None, None, world.clone());

        let bb = BoundingBox::new_from_pos(x, target_y, z, &dimension);
        let space_free = world.is_space_empty(bb);
        let liquid_free = !BlockPos::iterate(bb.min_block_pos(), bb.max_block_pos())
            .any(|pos| world.get_fluid(&pos) != &pumpkin_data::fluid::Fluid::EMPTY);

        if !space_free || !liquid_free {
            self.entity.teleport(origin, None, None, world.clone());
            return false;
        }

        if show_particles {
            world.send_entity_status(&self.entity, EntityStatus::Teleport, None);
        }

        true
    }
}

const fn spider_climbing_state(
    entity_type: &EntityType,
    horizontal_collision: bool,
) -> Option<bool> {
    if entity_type.id == EntityType::SPIDER.id || entity_type.id == EntityType::CAVE_SPIDER.id {
        Some(horizontal_collision)
    } else {
        None
    }
}

/// Mirrors vanilla's strict `random < probability` consume-effect gate.
const fn consume_effect_probability_applies(probability: f32, random: f32) -> bool {
    random < probability
}

/// Vanilla `Item#finishUsingItem`: the default implementation returns the
/// stack unchanged. Only the food/consumable override
/// (`ItemUtils#finishUsingItem`) decrements the stack on natural use-duration
/// completion.
fn is_consumed_on_finish(item: &ItemStack) -> bool {
    item.get_data_component::<FoodImpl>().is_some()
        || item.get_data_component::<ConsumableImpl>().is_some()
}

/// Returns whether this consumable has vanilla's `clear_all_effects` consume effect.
fn consumable_clears_all_effects(item: &ItemStack) -> bool {
    item.get_data_component::<ConsumableImpl>()
        .is_some_and(|consumable| {
            consumable
                .effects
                .iter()
                .any(|effect| matches!(effect, ConsumeEffect::ClearAllEffects))
        })
}

/// Returns the item that replaces a consumed single-item container.
///
/// `use_remainder` is a unit data component in the vanilla item data, so the
/// concrete remainder remains an item-specific rule. Keep this mapping here at
/// the shared consumable completion point so main hand, off-hand, and packet
/// inventory synchronization all take the same path.
fn consumable_remainder(item: &ItemStack) -> Option<&'static Item> {
    item.get_data_component::<UseRemainderImpl>()?;

    match item.item.id {
        id if id == Item::MILK_BUCKET.id => Some(&Item::BUCKET),
        id if id == Item::POTION.id => Some(&Item::GLASS_BOTTLE),
        _ => None,
    }
}

/// Vanilla `MobEffect.shouldApplyEffectTickThisTick` for the effects that space their work out:
/// `interval > 0 ? tickCount % interval == 0 : true`. The amplifier is not clamped, and Java
/// shifts by the low five bits of it, so a high enough amplifier wraps rather than saturating.
const fn effect_ticks_this_tick(base_interval: i32, amplifier: u8, tick_count: i32) -> bool {
    let interval = base_interval >> (amplifier as u32 & 31);
    interval <= 0 || tick_count % interval == 0
}

/// `LivingEntity.canBeAffected` and the species overrides in front of it.
///
/// The tag branches are checked in vanilla's order, and the overrides run first because vanilla
/// resolves them before delegating to the base class.
#[must_use]
pub fn effect_applies_to(entity_type: &'static EntityType, effect_type: &StatusEffect) -> bool {
    if entity_type == &EntityType::PARCHED && effect_type.id == StatusEffect::WEAKNESS.id {
        return false;
    }
    if (entity_type == &EntityType::WITHER || entity_type == &EntityType::WITHER_SKELETON)
        && effect_type.id == StatusEffect::WITHER.id
    {
        return false;
    }
    if (entity_type == &EntityType::SPIDER
        || entity_type == &EntityType::CAVE_SPIDER
        || entity_type == &EntityType::NAUTILUS
        || entity_type == &EntityType::ZOMBIE_NAUTILUS)
        && effect_type.id == StatusEffect::POISON.id
    {
        return false;
    }

    if entity_type.has_tag(&tag::EntityType::MINECRAFT_IMMUNE_TO_INFESTED) {
        effect_type.id != StatusEffect::INFESTED.id
    } else if entity_type.has_tag(&tag::EntityType::MINECRAFT_IMMUNE_TO_OOZING) {
        effect_type.id != StatusEffect::OOZING.id
    } else if entity_type.has_tag(&tag::EntityType::MINECRAFT_IGNORES_POISON_AND_REGEN) {
        effect_type.id != StatusEffect::REGENERATION.id && effect_type.id != StatusEffect::POISON.id
    } else {
        true
    }
}

/// Vanilla `OozingMobEffect.numberOfSlimesToSpawn`: the cramming limit minus the slimes already
/// nearby, never below zero and never above what the effect asks for. A `maxEntityCramming` of
/// zero or less disables the check entirely.
fn oozing_slimes_to_spawn(max_entity_cramming: i64, nearby_slimes: usize, requested: i64) -> i64 {
    if max_entity_cramming < 1 {
        return requested;
    }

    let room = max_entity_cramming.saturating_sub(nearby_slimes as i64);
    room.clamp(0, requested)
}

/// Vanilla `HealOrHarmMobEffect.applyEffectTick`: `4 << amplification` hearts of healing or
/// `6 << amplification` of magic damage. Java shifts an `int` by the low five bits of the
/// amplifier, so a large amplifier wraps (and can overflow into a negative amount, which the
/// heal branch clamps away with its own `Math.max`).
fn instant_effect_amount(base: i32, amplifier: u8) -> f32 {
    (base.wrapping_shl(u32::from(amplifier) & 31)) as f32
}

/// Vanilla `AbsorptionMobEffect.onEffectStarted` combined with the clamp in
/// `LivingEntity.setAbsorptionAmount`.
fn absorption_after_application(current: f32, amplifier: u8, max_absorption: f32) -> f32 {
    current
        .max(4.0 * (f32::from(amplifier) + 1.0))
        .clamp(0.0, max_absorption)
}

/// Vanilla `LivingEntity.setLastHurtByPlayer` starts this many ticks of player-kill memory.
const PLAYER_KILL_MEMORY_TICKS: i32 = 100;

/// Vanilla `MobEffectInstance.isShorterDurationThan`: a duration of -1 is infinite, so it is
/// never shorter than anything and everything finite is shorter than it.
const fn effect_is_shorter(effect: &Effect, other: &Effect) -> bool {
    effect.duration != -1 && (effect.duration < other.duration || other.duration == -1)
}

/// Vanilla `MobEffectInstance.update`. `chain[index]` is the instance being taken over and the
/// entries after it are its hidden-effect chain, nearest first. A stronger but shorter instance
/// pushes the one it replaces down the chain, and a weaker but longer one is filed further down
/// it, so both come back once the instances covering them run out. Returns whether the instance
/// at `index` changed, which is what decides if clients and attributes need updating.
fn update_effect_chain(chain: &mut Vec<Effect>, index: usize, take_over: &Effect) -> bool {
    let mut changed = false;
    if take_over.amplifier > chain[index].amplifier {
        if effect_is_shorter(take_over, &chain[index]) {
            let taken_over = chain[index].clone();
            chain.insert(index + 1, taken_over);
        }
        chain[index].amplifier = take_over.amplifier;
        chain[index].duration = take_over.duration;
        changed = true;
    } else if effect_is_shorter(&chain[index], take_over) {
        if take_over.amplifier == chain[index].amplifier {
            chain[index].duration = take_over.duration;
            changed = true;
        } else if index + 1 == chain.len() {
            chain.push(take_over.clone());
        } else {
            update_effect_chain(chain, index + 1, take_over);
        }
    }

    if (!take_over.ambient && chain[index].ambient) || changed {
        chain[index].ambient = take_over.ambient;
        changed = true;
    }
    if take_over.show_particles != chain[index].show_particles {
        chain[index].show_particles = take_over.show_particles;
        changed = true;
    }
    if take_over.show_icon != chain[index].show_icon {
        chain[index].show_icon = take_over.show_icon;
        changed = true;
    }

    changed
}

#[cfg(test)]
mod instant_effect_amount_tests {
    use super::instant_effect_amount;

    #[test]
    fn the_amount_doubles_with_every_level() {
        assert!((instant_effect_amount(4, 0) - 4.0).abs() < f32::EPSILON);
        assert!((instant_effect_amount(4, 1) - 8.0).abs() < f32::EPSILON);
        assert!((instant_effect_amount(6, 0) - 6.0).abs() < f32::EPSILON);
        assert!((instant_effect_amount(6, 2) - 24.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_shift_wraps_the_way_java_does() {
        // Java shifts an int by the low five bits of the amplifier, so 32 behaves like 0 and an
        // amplifier that overflows the sign bit comes back negative rather than panicking.
        assert!((instant_effect_amount(4, 32) - 4.0).abs() < f32::EPSILON);
        assert!(instant_effect_amount(4, 29) < 0.0);
        assert!(instant_effect_amount(4, 29).max(0.0).abs() < f32::EPSILON);
    }
}

#[cfg(test)]
mod effect_tick_cadence_tests {
    use super::effect_ticks_this_tick;

    #[test]
    fn the_interval_halves_with_every_level() {
        assert!(effect_ticks_this_tick(50, 0, 50));
        assert!(!effect_ticks_this_tick(50, 0, 49));
        assert!(effect_ticks_this_tick(50, 1, 25));
        assert!(effect_ticks_this_tick(50, 2, 12));
        assert!(!effect_ticks_this_tick(50, 2, 11));
    }

    #[test]
    fn an_interval_that_reaches_zero_applies_every_tick() {
        // Regeneration VII and beyond: 50 >> 6 is 0, so vanilla stops spacing it out.
        assert!(effect_ticks_this_tick(50, 6, 1));
        assert!(effect_ticks_this_tick(50, 6, 7));
        assert!(effect_ticks_this_tick(25, 5, 3));
    }

    #[test]
    fn the_shift_wraps_the_way_java_does() {
        // Java shifts an int by the low five bits of the amplifier, so 32 behaves like 0.
        assert!(effect_ticks_this_tick(50, 32, 50));
        assert!(!effect_ticks_this_tick(50, 32, 49));
    }
}

#[cfg(test)]
mod can_be_affected_tests {
    use super::effect_applies_to;
    use pumpkin_data::effect::StatusEffect;
    use pumpkin_data::entity::EntityType;

    #[test]
    fn silverfish_shrug_off_infested_but_take_everything_else() {
        assert!(!effect_applies_to(
            &EntityType::SILVERFISH,
            &StatusEffect::INFESTED
        ));
        assert!(effect_applies_to(
            &EntityType::SILVERFISH,
            &StatusEffect::OOZING
        ));
        assert!(effect_applies_to(
            &EntityType::SILVERFISH,
            &StatusEffect::SPEED
        ));
    }

    #[test]
    fn slimes_shrug_off_oozing() {
        assert!(!effect_applies_to(
            &EntityType::SLIME,
            &StatusEffect::OOZING
        ));
        assert!(effect_applies_to(
            &EntityType::SLIME,
            &StatusEffect::INFESTED
        ));
    }

    #[test]
    fn the_undead_ignore_poison_and_regeneration() {
        assert!(!effect_applies_to(
            &EntityType::SKELETON,
            &StatusEffect::POISON
        ));
        assert!(!effect_applies_to(
            &EntityType::SKELETON,
            &StatusEffect::REGENERATION
        ));
        assert!(effect_applies_to(
            &EntityType::SKELETON,
            &StatusEffect::SPEED
        ));
    }

    #[test]
    fn spiders_and_nautiluses_ignore_poison_only() {
        for entity_type in [
            &EntityType::SPIDER,
            &EntityType::CAVE_SPIDER,
            &EntityType::NAUTILUS,
        ] {
            assert!(!effect_applies_to(entity_type, &StatusEffect::POISON));
            assert!(effect_applies_to(entity_type, &StatusEffect::REGENERATION));
        }

        // The zombie nautilus takes the same poison override, but it is also undead and so
        // carries `ignores_poison_and_regen`, which takes regeneration away as well.
        assert!(!effect_applies_to(
            &EntityType::ZOMBIE_NAUTILUS,
            &StatusEffect::POISON
        ));
        assert!(!effect_applies_to(
            &EntityType::ZOMBIE_NAUTILUS,
            &StatusEffect::REGENERATION
        ));
    }

    #[test]
    fn withers_and_wither_skeletons_ignore_wither() {
        assert!(!effect_applies_to(
            &EntityType::WITHER,
            &StatusEffect::WITHER
        ));
        assert!(!effect_applies_to(
            &EntityType::WITHER_SKELETON,
            &StatusEffect::WITHER
        ));
        assert!(effect_applies_to(
            &EntityType::ZOMBIE,
            &StatusEffect::WITHER
        ));
    }

    #[test]
    fn parched_stay_immune_to_their_own_weakness_arrows() {
        assert!(!effect_applies_to(
            &EntityType::PARCHED,
            &StatusEffect::WEAKNESS
        ));
        assert!(effect_applies_to(
            &EntityType::PARCHED,
            &StatusEffect::SLOWNESS
        ));
    }

    #[test]
    fn ordinary_mobs_take_everything() {
        assert!(effect_applies_to(&EntityType::COW, &StatusEffect::POISON));
        assert!(effect_applies_to(
            &EntityType::PLAYER,
            &StatusEffect::REGENERATION
        ));
    }
}

#[cfg(test)]
mod oozing_tests {
    use super::oozing_slimes_to_spawn;

    #[test]
    fn the_effect_spawns_two_slimes_with_room_to_spare() {
        assert_eq!(oozing_slimes_to_spawn(24, 0, 2), 2);
        assert_eq!(oozing_slimes_to_spawn(24, 21, 2), 2);
    }

    #[test]
    fn cramming_limits_the_count() {
        assert_eq!(oozing_slimes_to_spawn(24, 23, 2), 1);
        assert_eq!(oozing_slimes_to_spawn(24, 24, 2), 0);
        assert_eq!(oozing_slimes_to_spawn(24, 30, 2), 0);
    }

    #[test]
    fn a_disabled_cramming_rule_skips_the_check() {
        assert_eq!(oozing_slimes_to_spawn(0, 100, 2), 2);
        assert_eq!(oozing_slimes_to_spawn(-1, 100, 2), 2);
    }
}

#[cfg(test)]
mod absorption_tests {
    use super::absorption_after_application;

    #[test]
    fn a_second_application_does_not_stack_hearts() {
        assert_eq!(absorption_after_application(0.0, 0, 4.0), 4.0);
        assert_eq!(absorption_after_application(4.0, 0, 4.0), 4.0);
    }

    #[test]
    fn a_stronger_application_raises_the_amount() {
        assert_eq!(absorption_after_application(4.0, 1, 8.0), 8.0);
    }

    #[test]
    fn a_weaker_application_keeps_what_is_there() {
        assert_eq!(absorption_after_application(8.0, 0, 8.0), 8.0);
    }

    #[test]
    fn the_max_absorption_attribute_caps_the_result() {
        assert_eq!(absorption_after_application(0.0, 3, 8.0), 8.0);
        assert_eq!(absorption_after_application(0.0, 0, 0.0), 0.0);
    }
}

#[cfg(test)]
mod effect_chain_tests {
    use super::update_effect_chain;
    use pumpkin_data::effect::StatusEffect;
    use pumpkin_data::potion::Effect;

    fn effect(amplifier: u8, duration: i32) -> Effect {
        Effect {
            effect_type: &StatusEffect::STRENGTH,
            duration,
            amplifier,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        }
    }

    #[test]
    fn stronger_but_shorter_hides_the_instance_it_replaces() {
        let mut chain = vec![effect(0, 600)];
        assert!(update_effect_chain(&mut chain, 0, &effect(1, 100)));
        assert_eq!((chain[0].amplifier, chain[0].duration), (1, 100));
        assert_eq!((chain[1].amplifier, chain[1].duration), (0, 600));
    }

    #[test]
    fn stronger_and_longer_replaces_outright() {
        let mut chain = vec![effect(0, 100)];
        assert!(update_effect_chain(&mut chain, 0, &effect(1, 600)));
        assert_eq!((chain[0].amplifier, chain[0].duration), (1, 600));
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn same_amplifier_refreshes_only_a_longer_duration() {
        let mut chain = vec![effect(1, 100)];
        assert!(update_effect_chain(&mut chain, 0, &effect(1, 600)));
        assert_eq!(chain[0].duration, 600);
        assert!(!update_effect_chain(&mut chain, 0, &effect(1, 200)));
        assert_eq!(chain[0].duration, 600);
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn weaker_but_longer_waits_in_the_chain() {
        let mut chain = vec![effect(1, 100)];
        assert!(!update_effect_chain(&mut chain, 0, &effect(0, 600)));
        assert_eq!((chain[0].amplifier, chain[0].duration), (1, 100));
        assert_eq!((chain[1].amplifier, chain[1].duration), (0, 600));
    }

    #[test]
    fn weaker_and_shorter_is_dropped() {
        let mut chain = vec![effect(1, 600)];
        assert!(!update_effect_chain(&mut chain, 0, &effect(0, 100)));
        assert_eq!(chain.len(), 1);
        assert_eq!((chain[0].amplifier, chain[0].duration), (1, 600));
    }

    #[test]
    fn a_second_weaker_instance_merges_into_the_hidden_one() {
        let mut chain = vec![effect(2, 100), effect(0, 600)];
        assert!(!update_effect_chain(&mut chain, 0, &effect(0, 900)));
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[1].duration, 900);
    }

    #[test]
    fn an_infinite_duration_is_never_shorter() {
        let mut chain = vec![effect(0, -1)];
        assert!(!update_effect_chain(&mut chain, 0, &effect(0, 6000)));
        assert_eq!(chain[0].duration, -1);
        assert_eq!(chain.len(), 1);

        let mut chain = vec![effect(0, 6000)];
        assert!(update_effect_chain(&mut chain, 0, &effect(0, -1)));
        assert_eq!(chain[0].duration, -1);
    }

    #[test]
    fn a_stronger_infinite_instance_hides_nothing() {
        let mut chain = vec![effect(0, -1)];
        assert!(update_effect_chain(&mut chain, 0, &effect(1, -1)));
        assert_eq!(chain.len(), 1);
        assert_eq!((chain[0].amplifier, chain[0].duration), (1, -1));
    }

    #[test]
    fn flags_track_the_latest_instance() {
        let mut chain = vec![effect(1, 600)];
        let mut candidate = effect(0, 100);
        candidate.show_icon = false;
        assert!(update_effect_chain(&mut chain, 0, &candidate));
        assert!(!chain[0].show_icon);
        assert_eq!((chain[0].amplifier, chain[0].duration), (1, 600));
    }
}

#[cfg(test)]
mod milk_bucket_tests {
    use super::{
        consumable_clears_all_effects, consumable_remainder, consume_effect_probability_applies,
        is_consumed_on_finish,
    };
    use pumpkin_data::{item::Item, item_stack::ItemStack};

    #[test]
    fn milk_bucket_clears_all_effects_when_consumed() {
        let milk = ItemStack::new(1, &Item::MILK_BUCKET);

        assert!(consumable_clears_all_effects(&milk));
    }

    #[test]
    fn milk_bucket_returns_an_empty_bucket() {
        let milk = ItemStack::new(1, &Item::MILK_BUCKET);

        assert_eq!(
            consumable_remainder(&milk).map(|item| item.id),
            Some(Item::BUCKET.id)
        );
    }

    #[test]
    fn consumable_effect_probability_matches_vanilla_strict_threshold() {
        assert!(!consume_effect_probability_applies(0.0, 0.0));
        assert!(consume_effect_probability_applies(1.0, 0.999));
        assert!(consume_effect_probability_applies(0.5, 0.499));
        assert!(!consume_effect_probability_applies(0.5, 0.5));
    }

    #[test]
    fn non_consumable_items_are_not_consumed_on_finish() {
        let spyglass = ItemStack::new(1, &Item::SPYGLASS);
        assert!(!is_consumed_on_finish(&spyglass));
    }

    #[test]
    fn food_and_consumable_items_are_consumed_on_finish() {
        let milk = ItemStack::new(1, &Item::MILK_BUCKET);
        let apple = ItemStack::new(1, &Item::APPLE);
        assert!(is_consumed_on_finish(&milk));
        assert!(is_consumed_on_finish(&apple));
    }
}

/// Returns `true` if `damage_type` is in `#minecraft:bypasses_armor` (1.21.11).
/// These sources bypass armor entirely (fall, drown, freeze, etc.).
pub(crate) const fn bypasses_armor_durability(damage_type: &DamageType) -> bool {
    // Bitmask lookup: O(1) with two instructions (shift + AND), no array scan.
    // DamageType IDs can exceed 31; use u64 for sufficient range.
    // TODO: Make data-driven once the data pack system can handle it without performance regressions.
    // Compile-time assertions: ensure all bypassing types fit in u64 bitmask.
    const _: () = assert!(
        DamageType::FALL.id < 64
            && DamageType::FLY_INTO_WALL.id < 64
            && DamageType::ON_FIRE.id < 64
            && DamageType::IN_WALL.id < 64
            && DamageType::CRAMMING.id < 64
            && DamageType::DROWN.id < 64
            && DamageType::GENERIC.id < 64
            && DamageType::WITHER.id < 64
            && DamageType::DRAGON_BREATH.id < 64
            && DamageType::STARVE.id < 64
            && DamageType::ENDER_PEARL.id < 64
            && DamageType::FREEZE.id < 64
            && DamageType::STALAGMITE.id < 64
            && DamageType::MAGIC.id < 64
            && DamageType::INDIRECT_MAGIC.id < 64
            && DamageType::OUT_OF_WORLD.id < 64
            && DamageType::GENERIC_KILL.id < 64
            && DamageType::SONIC_BOOM.id < 64
            && DamageType::OUTSIDE_BORDER.id < 64,
        "One or more bypass DamageType IDs exceed u64 bitmask width (>= 64)"
    );
    const BYPASS_MASK: u64 = (1u64 << DamageType::FALL.id)
        | (1u64 << DamageType::FLY_INTO_WALL.id)
        | (1u64 << DamageType::ON_FIRE.id)
        | (1u64 << DamageType::IN_WALL.id)
        | (1u64 << DamageType::CRAMMING.id)
        | (1u64 << DamageType::DROWN.id)
        | (1u64 << DamageType::GENERIC.id)
        | (1u64 << DamageType::WITHER.id)
        | (1u64 << DamageType::DRAGON_BREATH.id)
        | (1u64 << DamageType::STARVE.id)
        | (1u64 << DamageType::ENDER_PEARL.id)
        | (1u64 << DamageType::FREEZE.id)
        | (1u64 << DamageType::STALAGMITE.id)
        | (1u64 << DamageType::MAGIC.id)
        | (1u64 << DamageType::INDIRECT_MAGIC.id)
        | (1u64 << DamageType::OUT_OF_WORLD.id)
        | (1u64 << DamageType::GENERIC_KILL.id)
        | (1u64 << DamageType::SONIC_BOOM.id)
        | (1u64 << DamageType::OUTSIDE_BORDER.id);
    (damage_type.id < 64) && ((BYPASS_MASK >> damage_type.id) & 1 == 1)
}

/// Returns whether the damage source bypasses protection enchantments.
/// The 26.2 tag contains only sonic boom.
pub(crate) const fn bypasses_enchantments(damage_type: &DamageType) -> bool {
    damage_type.id == DamageType::SONIC_BOOM.id
}

/// Returns whether vanilla routes this damage through helmet protection.
pub(crate) const fn damages_helmet(damage_type: &DamageType) -> bool {
    damage_type.id == DamageType::FALLING_ANVIL.id
        || damage_type.id == DamageType::FALLING_BLOCK.id
        || damage_type.id == DamageType::FALLING_STALACTITE.id
}

/// Selects the slots reached by the concrete vanilla `hurtArmor`/`hurtHelmet` overrides
/// (`LivingEntity.java:1207-1210, 1881-1905`; `Player.java:738-745`; `Wolf.java:443-444`;
/// `Horse.java:233-234`).
fn hurt_armor_slot(
    entity_type: &'static EntityType,
    slot: &EquipmentSlot,
    helmet_only: bool,
) -> bool {
    if entity_type == &EntityType::PLAYER {
        return if helmet_only {
            *slot == EquipmentSlot::HEAD
        } else {
            slot.is_armor_slot()
        };
    }

    !helmet_only
        && *slot == EquipmentSlot::BODY
        && (entity_type == &EntityType::WOLF || entity_type == &EntityType::HORSE)
}

/// Returns whether vanilla damage tags prevent shield blocking.
pub(crate) const fn bypasses_shield(damage_type: &DamageType) -> bool {
    matches!(
        damage_type.id,
        id if id == DamageType::ON_FIRE.id
            || id == DamageType::IN_WALL.id
            || id == DamageType::CRAMMING.id
            || id == DamageType::DROWN.id
            || id == DamageType::FLY_INTO_WALL.id
            || id == DamageType::GENERIC.id
            || id == DamageType::WITHER.id
            || id == DamageType::DRAGON_BREATH.id
            || id == DamageType::STARVE.id
            || id == DamageType::FALL.id
            || id == DamageType::ENDER_PEARL.id
            || id == DamageType::FREEZE.id
            || id == DamageType::STALAGMITE.id
            || id == DamageType::MAGIC.id
            || id == DamageType::INDIRECT_MAGIC.id
            || id == DamageType::OUT_OF_WORLD.id
            || id == DamageType::GENERIC_KILL.id
            || id == DamageType::SONIC_BOOM.id
            || id == DamageType::OUTSIDE_BORDER.id
            || id == DamageType::CACTUS.id
            || id == DamageType::CAMPFIRE.id
            || id == DamageType::DRY_OUT.id
            || id == DamageType::FALLING_ANVIL.id
            || id == DamageType::FALLING_STALACTITE.id
            || id == DamageType::HOT_FLOOR.id
            || id == DamageType::SULFUR_CUBE_HOT.id
            || id == DamageType::IN_FIRE.id
            || id == DamageType::LAVA.id
            || id == DamageType::LIGHTNING_BOLT.id
            || id == DamageType::SWEET_BERRY_BUSH.id
    )
}

/// Applies the final air-movement drag. Vanilla skips this entire block when
/// `LivingEntity.shouldDiscardFriction()` is true (`LivingEntity.java:2477-2485`).
fn apply_air_friction(
    velocity: Vector3<f64>,
    horizontal_friction: f64,
    vertical_friction: f64,
    discard_friction: bool,
) -> Vector3<f64> {
    if discard_friction {
        velocity
    } else {
        Vector3::new(
            velocity.x * horizontal_friction,
            velocity.y * vertical_friction,
            velocity.z * horizontal_friction,
        )
    }
}

/// Vanilla `Vec3.xRot` followed by `Vec3.yRot` in `spawnItemParticles`
/// (`LivingEntity.java:3550-3560`).
fn rotate_particle_vector(vector: Vector3<f64>, pitch: f64, yaw: f64) -> Vector3<f64> {
    let (pitch_sin, pitch_cos) = (-pitch).sin_cos();
    let x_rotated = Vector3::new(
        vector.x,
        vector.y * pitch_cos - vector.z * pitch_sin,
        vector.y * pitch_sin + vector.z * pitch_cos,
    );
    let (yaw_sin, yaw_cos) = (-yaw).sin_cos();
    Vector3::new(
        x_rotated.x * yaw_cos + x_rotated.z * yaw_sin,
        x_rotated.y,
        x_rotated.z * yaw_cos - x_rotated.x * yaw_sin,
    )
}

/// `LivingEntity.getFrictionInfluencedSpeed`: the grounded per-tick factor
/// `moveRelative` is called with.
fn friction_influenced_speed(speed: f64, slipperiness: f64) -> f64 {
    speed * 0.216_000_02 / (slipperiness * slipperiness * slipperiness)
}

/// Vanilla `LivingEntity.getArmorValue` floors the effective armor attribute
/// (`LivingEntity.java:1877-1879`).
const fn armor_value_from_attribute(value: f64) -> i32 {
    value.floor() as i32
}

/// Vanilla `LivingEntity.getBlockSpeedFactor` linearly interpolates from the block factor to
/// one using movement efficiency (`LivingEntity.java:511-512`).
fn block_speed_factor(base: f32, efficiency: f32) -> f32 {
    base + (1.0 - base) * efficiency
}

/// Vanilla initializes `removeArrowTime` to `20 * (30 - arrowCount)`
/// (`LivingEntity.java:2759-2763`).
const fn arrow_removal_delay(arrow_count: i32) -> i32 {
    20 * (30 - arrow_count)
}

/// Vanilla `LivingEntity.computeModifiedFriction` (`LivingEntity.java:515-517`).
fn modified_friction(friction: f64, modifier: f64) -> f64 {
    (1.0 - (1.0 - friction) * modifier).clamp(0.0, 1.0)
}

fn head_turn_body_yaw(
    body_yaw: f32,
    target_body_yaw: f32,
    entity_yaw: f32,
    max_head_rotation: f32,
) -> f32 {
    // `LivingEntity.tickHeadTurn` (`LivingEntity.java:3018-3027`) turns the body by 30 percent,
    // then applies the entity-specific maximum head-to-body difference.
    let body_yaw = body_yaw + wrap_degrees(target_body_yaw - body_yaw) * 0.3;
    let head_difference = wrap_degrees(entity_yaw - body_yaw);
    if head_difference.abs() > max_head_rotation {
        body_yaw + head_difference - head_difference.signum() * max_head_rotation
    } else {
        body_yaw
    }
}

/// Vanilla `LivingEntity.updateFallFlyingMovement`, kept pure so its pitch and velocity
/// response can be checked independently of world collision state.
fn fall_flying_velocity(
    mut velocity: Vector3<f64>,
    look: Vector3<f64>,
    pitch: f64,
    gravity: f64,
) -> Vector3<f64> {
    let look_horizontal = look.x.hypot(look.z);
    let horizontal_speed = velocity.x.hypot(velocity.z);
    let lift = pitch.cos().powi(2);

    velocity.y += gravity * (-1.0 + lift * 0.75);
    if velocity.y < 0.0 && look_horizontal > 0.0 {
        let convert = velocity.y * -0.1 * lift;
        velocity.x += look.x * convert / look_horizontal;
        velocity.y += convert;
        velocity.z += look.z * convert / look_horizontal;
    }
    if pitch < 0.0 && look_horizontal > 0.0 {
        let convert = horizontal_speed * -pitch.sin() * 0.04;
        velocity.x -= look.x * convert / look_horizontal;
        velocity.y += convert * 3.2;
        velocity.z -= look.z * convert / look_horizontal;
    }
    if look_horizontal > 0.0 {
        velocity.x += (look.x / look_horizontal * horizontal_speed - velocity.x) * 0.1;
        velocity.z += (look.z / look_horizontal * horizontal_speed - velocity.z) * 0.1;
    }

    velocity.multiply(0.99, 0.98, 0.99)
}

/// Vanilla `LivingEntity.handleFallFlyingCollisions` damage threshold.
fn fall_flying_collision_damage(previous_speed: f64, new_speed: f64) -> Option<f32> {
    let damage = ((previous_speed - new_speed) * 10.0 - 3.0) as f32;
    (damage > 0.0).then_some(damage)
}

/// Vanilla's shield `BlocksAttacks.ItemDamageFunction(3, 1, 1)`: shields take no
/// durability loss below three blocked damage, then lose `floor(1 + damage)` points.
fn shield_block_durability_damage(blocked_damage: f32) -> Option<i32> {
    (blocked_damage >= 3.0).then_some((1.0 + blocked_damage).floor() as i32)
}

/// Vanilla `LivingEntity.updateFallFlying` tick schedule (`LivingEntity.java:3190-3202`).
/// Takes the consecutive glide tick count plus one; returns whether this tick broadcasts
/// the `ELYTRA_GLIDE` game event and whether it additionally damages an equipped glider
/// (every second one-second interval of free fall).
const fn fall_flying_schedule(check_fall_fly_ticks: u32) -> (bool, bool) {
    let glide_event_tick = check_fall_fly_ticks.is_multiple_of(10);
    let damage_glider_tick = glide_event_tick && (check_fall_fly_ticks / 10).is_multiple_of(2);
    (glide_event_tick, damage_glider_tick)
}

/// Vanilla `LivingEntity.canGlideUsing` (`LivingEntity.java:4001-4008`): the stack must
/// carry the `minecraft:glider` component, its `equippable` component must target `slot`,
/// and its next durability point must not break it.
fn can_glide_using(stack: &ItemStack, slot: &EquipmentSlot) -> bool {
    stack.get_data_component::<GliderImpl>().is_some()
        && stack
            .get_data_component::<EquippableImpl>()
            .is_some_and(|equippable| equippable.slot == slot && !next_damage_will_break(stack))
}

/// Vanilla `ItemStack.nextDamageWillBreak`: one more durability point breaks the item.
fn next_damage_will_break(stack: &ItemStack) -> bool {
    !stack.is_empty()
        && stack
            .get_max_damage()
            .is_some_and(|max_damage| stack.get_damage() >= max_damage - 1)
}

fn damage_causes_panic(damage_type: DamageType) -> bool {
    damage_type.has_tag(&tag::DamageType::MINECRAFT_PANIC_CAUSES)
}

// Vanilla `SkeletonHorse.playSwimSound` (`SkeletonHorse.java:103-109`) uses a fixed ground
// volume and caps airborne volume after scaling the water speed.
fn skeleton_swim_sound_volume(on_ground: bool, water_volume: f32) -> f32 {
    if on_ground {
        0.3
    } else {
        (water_volume * 25.0).min(0.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AttackRange.defaultFor` copies the entity-interaction range into both survival and
    /// creative maxima (`AttackRange.java:55-59`).
    #[test]
    fn default_attack_range_uses_entity_interaction_range() {
        let range = default_attack_range(3.5);

        assert_eq!(range.min_reach, 0.0);
        assert_eq!(range.max_reach, 3.5);
        assert_eq!(range.min_creative_reach, 0.0);
        assert_eq!(range.max_creative_reach, 3.5);
        assert_eq!(range.hitbox_margin, 0.0);
        assert_eq!(range.mob_factor, 1.0);
    }

    /// `Mob.playHurtSound` uses the mob sound source, while players use the player category
    /// (`Mob.java:295-299`; `LivingEntity.java:1427-1434`).
    #[test]
    fn hurt_sound_category_matches_entity_kind() {
        assert_eq!(
            hurt_sound_category(true, Some(SoundCategory::Neutral)).to_name(),
            "players"
        );
        assert_eq!(
            hurt_sound_category(false, Some(SoundCategory::Hostile)).to_name(),
            "hostile"
        );
        assert_eq!(hurt_sound_category(false, None).to_name(), "neutral");
    }

    /// `LivingEntity.handleOnClimbable` stops downward motion for a sneaking player outside
    /// scaffolding (`LivingEntity.java:2694-2700`).
    #[test]
    fn sneaking_players_stop_on_ladders_but_not_scaffolding() {
        assert!(suppress_climb_descent(true, true, false));
        assert!(!suppress_climb_descent(true, true, true));
        assert!(!suppress_climb_descent(false, true, false));
        assert!(!suppress_climb_descent(true, false, false));
    }

    /// Vanilla only reaches the concrete `hurtArmor` overrides on Player, Wolf, and Horse
    /// (`LivingEntity.java:1881-1905`; `Player.java:738-745`; `Wolf.java:443-444`;
    /// `Horse.java:233-234`).
    #[test]
    fn hurt_armor_slots_match_concrete_vanilla_overrides() {
        assert!(hurt_armor_slot(
            &EntityType::PLAYER,
            &EquipmentSlot::HEAD,
            true
        ));
        assert!(hurt_armor_slot(
            &EntityType::PLAYER,
            &EquipmentSlot::CHEST,
            false
        ));
        assert!(hurt_armor_slot(
            &EntityType::WOLF,
            &EquipmentSlot::BODY,
            false
        ));
        assert!(hurt_armor_slot(
            &EntityType::HORSE,
            &EquipmentSlot::BODY,
            false
        ));
        assert!(!hurt_armor_slot(
            &EntityType::ZOMBIE,
            &EquipmentSlot::HEAD,
            false
        ));
        assert!(!hurt_armor_slot(
            &EntityType::WOLF,
            &EquipmentSlot::BODY,
            true
        ));
    }

    #[test]
    fn mounted_hitbox_raises_only_its_minimum_y() {
        // `LivingEntity.getHitbox` replaces only minY with the passenger riding position
        // (`LivingEntity.java:1692-1700`).
        let box_before =
            BoundingBox::new(Vector3::new(-0.3, 10.0, -0.3), Vector3::new(0.3, 11.8, 0.3));
        let box_after = hitbox_with_riding_floor(box_before, Some(10.6));
        assert_eq!(box_after.min.y, 10.6);
        assert_eq!(box_after.max.y, 11.8);
        let unchanged = hitbox_with_riding_floor(box_before, None);
        assert_eq!(unchanged.min.y, box_before.min.y);
        assert_eq!(unchanged.max.y, box_before.max.y);
    }

    #[test]
    fn living_portal_position_clears_forward_offset() {
        // `LivingEntity.getRelativePortalPosition` resets the forward coordinate before the
        // destination portal uses it (`LivingEntity.java:3385-3387`).
        let relative = LivingEntity::reset_forward_direction_of_relative_portal_position(
            Vector3::new(0.25, 0.75, -0.5),
        );
        assert_eq!(relative, Vector3::new(0.25, 0.75, 0.0));
    }

    #[test]
    fn body_yaw_turn_wraps_and_limits_head_difference() {
        // `LivingEntity.tickHeadTurn` (`LivingEntity.java:3018-3027`) turns 30 percent toward
        // the target and then limits the head to the requested angle from the body.
        assert!((head_turn_body_yaw(0.0, 90.0, 0.0, 50.0) - 27.0).abs() < 1e-4);
        assert!((head_turn_body_yaw(350.0, 10.0, 10.0, 50.0) - 356.0).abs() < 1e-4);
        assert!((head_turn_body_yaw(0.0, 0.0, 90.0, 50.0) - 40.0).abs() < 1e-4);
        assert!((head_turn_body_yaw(0.0, 0.0, 90.0, 15.0) - 75.0).abs() < 1e-4);
    }

    #[test]
    fn combat_damage_statistics_use_vanilla_rounding() {
        assert_eq!(damage_stat_amount(0.0), 0);
        assert_eq!(damage_stat_amount(0.04), 0);
        assert_eq!(damage_stat_amount(0.05), 1);
        assert_eq!(damage_stat_amount(1.25), 13);
    }

    #[test]
    fn skeleton_horse_swim_volume_matches_vanilla() {
        assert_eq!(skeleton_swim_sound_volume(true, 0.01), 0.3);
        assert_eq!(skeleton_swim_sound_volume(false, 0.002), 0.05);
        assert_eq!(skeleton_swim_sound_volume(false, 0.1), 0.1);
    }

    #[test]
    fn fall_flying_velocity_preserves_forward_glide_with_drag() {
        let velocity = fall_flying_velocity(
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            0.0,
            0.08,
        );
        assert!(velocity.x > 0.9);
        assert!(velocity.y < 0.0);
        assert!(velocity.z.abs() < f64::EPSILON);
    }

    #[test]
    fn fall_flying_velocity_converts_a_dive_into_speed_and_climb_into_lift() {
        let diving = fall_flying_velocity(
            Vector3::new(0.8, -0.1, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            0.5,
            0.08,
        );
        let climbing = fall_flying_velocity(
            Vector3::new(0.8, -0.1, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            -0.5,
            0.08,
        );
        assert!(diving.y < climbing.y);
        assert!(climbing.y > -0.1);
    }

    #[test]
    fn impulse_context_limits_fall_distance_to_impact_height() {
        assert_eq!(impulse_limited_fall_distance(8.0, 70.0, 72.5), 2.5);
        assert_eq!(impulse_limited_fall_distance(2.0, 70.0, 72.5), 2.0);
        assert_eq!(impulse_limited_fall_distance(2.0, 73.0, 72.5), -0.5);
    }

    /// `Entity.checkFallDistanceAccumulation` (`Entity.java:2890-2894`) clamps only after an
    /// upward or slow impulse, and leaves a normal falling distance unchanged.
    #[test]
    fn fall_distance_clamps_after_slow_impulse() {
        assert_eq!(accumulated_fall_distance_after_impulse(0.0, 4.0), 1.0);
        assert_eq!(accumulated_fall_distance_after_impulse(-0.5, 4.0), 4.0);
        assert_eq!(accumulated_fall_distance_after_impulse(-0.6, 0.5), 0.5);
    }

    #[test]
    fn living_state_predicates_match_vanilla_boundaries() {
        // `LivingEntity.hasLandedInLiquid` uses a strict downward-velocity threshold
        // (`LivingEntity.java:404-406`).
        assert!(has_landed_in_liquid_state(-0.01, true, false));
        assert!(has_landed_in_liquid_state(0.0, false, true));
        assert!(!has_landed_in_liquid_state(1.0E-5, true, false));
        assert!(!has_landed_in_liquid_state(0.0, false, false));

        // `LivingEntity.isDeadOrDying` treats either zero health or the death flag as terminal
        // (`LivingEntity.java:1171-1173`).
        assert!(dead_or_dying_state(0.0, false));
        assert!(dead_or_dying_state(20.0, true));
        assert!(!dead_or_dying_state(20.0, false));
    }

    #[test]
    fn fall_flying_collision_damage_requires_meaningful_speed_loss() {
        assert_eq!(fall_flying_collision_damage(1.0, 0.8), None);
        assert_eq!(fall_flying_collision_damage(1.0, 0.5), Some(2.0));
    }

    #[test]
    fn discard_friction_preserves_all_velocity_components() {
        let velocity = Vector3::new(1.0, -2.0, 3.0);
        assert_eq!(apply_air_friction(velocity, 0.5, 0.25, true), velocity);
        assert_eq!(
            apply_air_friction(velocity, 0.5, 0.25, false),
            Vector3::new(0.5, -0.5, 1.5)
        );
    }

    #[test]
    fn shield_durability_uses_the_vanilla_damage_threshold() {
        assert_eq!(shield_block_durability_damage(2.999), None);
        assert_eq!(shield_block_durability_damage(3.0), Some(4));
        assert_eq!(shield_block_durability_damage(3.9), Some(4));
        assert_eq!(shield_block_durability_damage(4.0), Some(5));
    }

    #[test]
    fn fall_one_cm_stat_uses_vanilla_threshold_and_rounding() {
        assert_eq!(fall_one_cm_stat_amount(1.999), None);
        assert_eq!(fall_one_cm_stat_amount(2.0), Some(200));
        assert_eq!(fall_one_cm_stat_amount(2.345), Some(235));
    }

    #[test]
    fn fall_flying_schedule_matches_vanilla_glide_and_damage_intervals() {
        // Glide event every tenth tick; durability damage on alternating intervals
        // (20th, 40th, ... glide tick), matching LivingEntity.java:3190-3202.
        assert_eq!(fall_flying_schedule(1), (false, false));
        assert_eq!(fall_flying_schedule(9), (false, false));
        assert_eq!(fall_flying_schedule(10), (true, false));
        assert_eq!(fall_flying_schedule(15), (false, false));
        assert_eq!(fall_flying_schedule(20), (true, true));
        assert_eq!(fall_flying_schedule(30), (true, false));
        assert_eq!(fall_flying_schedule(40), (true, true));
    }

    #[test]
    fn armor_value_floors_the_effective_attribute() {
        // Vanilla `LivingEntity.getArmorValue` uses Mth.floor (`LivingEntity.java:1877-1879`).
        assert_eq!(armor_value_from_attribute(7.99), 7);
        assert_eq!(armor_value_from_attribute(8.0), 8);
    }

    #[test]
    fn block_speed_factor_interpolates_with_movement_efficiency() {
        // Vanilla `LivingEntity.getBlockSpeedFactor` lerps to one
        // (`LivingEntity.java:511-512`).
        assert_eq!(block_speed_factor(0.4, 0.0), 0.4);
        assert!((block_speed_factor(0.4, 0.5) - 0.7).abs() < 1e-4);
        assert_eq!(block_speed_factor(0.4, 1.0), 1.0);
    }

    #[test]
    fn arrow_removal_delay_scales_with_visible_arrow_count() {
        // Vanilla initializes `removeArrowTime` from the current count
        // (`LivingEntity.java:2759-2763`).
        assert_eq!(arrow_removal_delay(1), 580);
        assert_eq!(arrow_removal_delay(3), 540);
    }

    #[test]
    fn can_glide_using_requires_glider_component_in_matching_slot() {
        let elytra = ItemStack::new(1, &Item::ELYTRA);
        assert!(!elytra.is_empty());
        // The elytra's equippable component targets the chest slot.
        assert!(can_glide_using(&elytra, &EquipmentSlot::CHEST));
        assert!(!can_glide_using(&elytra, &EquipmentSlot::HEAD));
        assert!(!can_glide_using(&elytra, &EquipmentSlot::MAIN_HAND));

        // A non-glider item never passes, even in its own slot.
        let sword = ItemStack::new(1, &Item::IRON_SWORD);
        assert!(!can_glide_using(&sword, &EquipmentSlot::MAIN_HAND));
    }

    #[test]
    fn can_glide_using_rejects_a_glider_one_hit_from_breaking() {
        let mut elytra = ItemStack::new(1, &Item::ELYTRA);
        assert!(!next_damage_will_break(&elytra));

        let max_damage = elytra.get_max_damage().unwrap_or(0);
        elytra.set_damage(max_damage - 1);
        assert!(next_damage_will_break(&elytra));
        // Vanilla `canGlideUsing` stops gliding on the last durability point.
        assert!(!can_glide_using(&elytra, &EquipmentSlot::CHEST));
    }

    #[test]
    fn non_finite_damage_is_clamped_before_combat_state_updates() {
        assert_eq!(normalize_non_finite_damage(7.5), 7.5);
        assert_eq!(normalize_non_finite_damage(f32::NAN), f32::MAX);
        assert_eq!(normalize_non_finite_damage(f32::INFINITY), f32::MAX);
        assert_eq!(normalize_non_finite_damage(f32::NEG_INFINITY), f32::MAX);
    }

    /// Terminal horizontal speed, in blocks per second, of `velocity += input * factor`
    /// followed by `velocity *= slipperiness * 0.91` as `travel_in_air` applies it on a
    /// normal block (slipperiness 0.6). `tick_movement` decays the stored input by 0.98
    /// each tick before travelling, so a continuously re-applied input settles at 0.98.
    fn terminal_blocks_per_second(input: f64, speed: f64) -> f64 {
        let per_tick = 0.98 * input * friction_influenced_speed(speed, 0.6);
        per_tick / (1.0 - 0.6 * 0.91) * 20.0
    }

    /// A player's forward input is 1.0 and its `speed` is the raw `MOVEMENT_SPEED`
    /// attribute (0.1), which reproduces the documented 4.317 blocks/s walk speed. This
    /// pins the consumer side, so the mob case below can only be fixed on the producer side.
    #[test]
    fn player_walk_speed_matches_the_documented_value() {
        let walk = terminal_blocks_per_second(1.0, 0.1);
        assert!((walk - 4.317).abs() < 0.01, "player {walk} blocks/s");
    }

    /// `Mob.setSpeed` stores `speedModifier * MOVEMENT_SPEED` and mirrors it into `zza`,
    /// so the attribute enters the per-tick velocity twice for mobs. Feeding the bare
    /// speed modifier into the input instead (the bug) made a chasing zombie 10.1 and a
    /// spider 12.9 blocks/s, both faster than a walking player.
    #[test]
    fn mob_input_and_speed_both_carry_the_movement_speed_attribute() {
        for (attribute, expected) in [(0.23, 2.284), (0.3, 3.886)] {
            let speed = 1.0 * attribute; // LivingEntity::speed_for_modifier
            let got = terminal_blocks_per_second(speed, speed);
            assert!((got - expected).abs() < 0.01, "{attribute} -> {got}");
            assert!(got < 4.317);
            // The old behaviour: input was the raw modifier.
            assert!(terminal_blocks_per_second(1.0, speed) > 4.317);
        }
    }

    #[test]
    fn active_hand_maps_to_the_matching_equipment_slot() {
        assert!(equipment_slot_for_hand(Hand::Left) == EquipmentSlot::OFF_HAND);
        assert!(equipment_slot_for_hand(Hand::Right) == EquipmentSlot::MAIN_HAND);
    }

    #[test]
    fn enemy_visibility_requires_vanilla_targetability_state() {
        // `LivingEntity.canBeSeenAsEnemy` (`LivingEntity.java:952-958`) rejects an
        // invulnerable, invisible-to-anyone, or explicitly non-targetable living entity.
        assert!(can_be_seen_as_enemy_state(false, true, false));
        assert!(!can_be_seen_as_enemy_state(true, true, false));
        assert!(!can_be_seen_as_enemy_state(false, false, false));
        assert!(!can_be_seen_as_enemy_state(false, true, true));
    }

    #[test]
    fn striders_stand_on_lava_but_travel_through_water() {
        // `Strider.canStandOnFluid` (`Strider.java:180-182`) returns true only for lava;
        // `LivingEntity.shouldTravelInFluid` (`LivingEntity.java:2421-2437`) therefore still
        // sends a strider through water movement.
        assert!(!should_travel_in_fluid(
            &EntityType::STRIDER,
            true,
            false,
            true
        ));
        assert!(should_travel_in_fluid(
            &EntityType::STRIDER,
            true,
            true,
            false
        ));
        assert!(should_travel_in_fluid(
            &EntityType::ZOMBIE,
            true,
            false,
            true
        ));
        assert!(!should_travel_in_fluid(
            &EntityType::ZOMBIE,
            false,
            false,
            false
        ));
    }

    #[test]
    fn damage_resistant_armor_matches_damage_tags() {
        let netherite_boots = ItemStack::new(1, &Item::NETHERITE_BOOTS);
        assert!(armor_resists_damage(&netherite_boots, &DamageType::ON_FIRE));
        assert!(!armor_resists_damage(
            &netherite_boots,
            &DamageType::PLAYER_ATTACK
        ));
    }

    #[test]
    fn panic_goal_uses_the_vanilla_damage_tag() {
        for damage_type in [
            DamageType::CACTUS,
            DamageType::LAVA,
            DamageType::ON_FIRE,
            DamageType::MOB_ATTACK,
            DamageType::PLAYER_ATTACK,
        ] {
            assert!(
                damage_causes_panic(damage_type),
                "{}",
                damage_type.message_id
            );
        }
        for damage_type in [DamageType::FALL, DamageType::DROWN, DamageType::IN_WALL] {
            assert!(
                !damage_causes_panic(damage_type),
                "{}",
                damage_type.message_id
            );
        }
    }

    #[test]
    fn knockback_resistance_scales_damage_knockback() {
        assert_eq!(knockback_strength_with_resistance(0.4, 0.0), 0.4);
        assert!((knockback_strength_with_resistance(0.4, 0.25) - 0.3).abs() < f64::EPSILON);
        assert_eq!(knockback_strength_with_resistance(0.4, 1.0), 0.0);
    }

    /// `LivingEntity.hurtServer` skips `dealDefaultKnockback` for the `no_knockback` tag
    /// (LivingEntity.java:1247-1249). Explosions and lightning are the cases that carry a
    /// source entity, so they are the ones the missing gate actually double-applied.
    #[test]
    fn no_knockback_tag_covers_the_damage_types_with_a_source_entity() {
        for damage_type in [
            DamageType::EXPLOSION,
            DamageType::PLAYER_EXPLOSION,
            DamageType::LIGHTNING_BOLT,
            DamageType::MAGIC,
            DamageType::WITHER,
            DamageType::DRAGON_BREATH,
        ] {
            assert!(
                damage_type.has_tag(&tag::DamageType::MINECRAFT_NO_KNOCKBACK),
                "{} should be in minecraft:no_knockback",
                damage_type.registry_name
            );
        }

        for damage_type in [
            DamageType::MOB_ATTACK,
            DamageType::PLAYER_ATTACK,
            DamageType::ARROW,
            DamageType::MOB_PROJECTILE,
        ] {
            assert!(
                !damage_type.has_tag(&tag::DamageType::MINECRAFT_NO_KNOCKBACK),
                "{} must keep its hurt knockback",
                damage_type.registry_name
            );
        }
    }

    #[test]
    fn spiders_climb_only_during_horizontal_collisions() {
        assert_eq!(spider_climbing_state(&EntityType::SPIDER, true), Some(true));
        assert_eq!(
            spider_climbing_state(&EntityType::SPIDER, false),
            Some(false)
        );
        assert_eq!(
            spider_climbing_state(&EntityType::CAVE_SPIDER, true),
            Some(true)
        );
        assert_eq!(spider_climbing_state(&EntityType::ZOMBIE, true), None);
        assert_eq!(tracked_data::spider::DATA_FLAGS_ID.id.v26_2, 16);
    }

    fn effect(amplifier: u8, duration: i32) -> Effect {
        Effect {
            effect_type: &StatusEffect::SPEED,
            duration,
            amplifier,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        }
    }

    #[test]
    fn stronger_effect_replaces_weaker_effect_even_when_shorter() {
        let mut chain = vec![effect(0, 1_200)];
        assert!(update_effect_chain(&mut chain, 0, &effect(1, 100)));
        assert_eq!((chain[0].amplifier, chain[0].duration), (1, 100));
    }

    #[test]
    fn instant_effects_invert_for_tagged_entities() {
        assert!(EntityType::ZOMBIE.has_tag(&tag::EntityType::MINECRAFT_INVERTED_HEALING_AND_HARM));
        assert!(EntityType::WITHER.has_tag(&tag::EntityType::MINECRAFT_INVERTED_HEALING_AND_HARM));
        assert!(!EntityType::PLAYER.has_tag(&tag::EntityType::MINECRAFT_INVERTED_HEALING_AND_HARM));
        assert!(!LivingEntity::instant_effect_is_damage(
            &StatusEffect::INSTANT_HEALTH,
            false
        ));
        assert!(LivingEntity::instant_effect_is_damage(
            &StatusEffect::INSTANT_HEALTH,
            true
        ));
        assert!(LivingEntity::instant_effect_is_damage(
            &StatusEffect::INSTANT_DAMAGE,
            false
        ));
        assert!(!LivingEntity::instant_effect_is_damage(
            &StatusEffect::INSTANT_DAMAGE,
            true
        ));
    }

    #[test]
    fn longer_effect_replaces_effect_with_same_amplifier() {
        let mut chain = vec![effect(1, 100)];
        assert!(update_effect_chain(&mut chain, 0, &effect(1, 101)));
        assert_eq!(chain[0].duration, 101);
    }

    #[test]
    fn weaker_or_shorter_effect_does_not_replace_active_effect() {
        for candidate in [effect(0, 1_200), effect(1, 99), effect(1, 100)] {
            let mut chain = vec![effect(1, 100)];
            assert!(!update_effect_chain(&mut chain, 0, &candidate));
            assert_eq!((chain[0].amplifier, chain[0].duration), (1, 100));
        }
    }

    // ── bypasses_armor_durability ─────────────────────────────────────

    /// Every member of `minecraft:bypasses_armor` (1.21.11) must return `true`.
    #[test]
    fn bypasses_armor_durability_returns_true_for_tag_members() {
        // Exact contents of the minecraft:bypasses_armor tag in 1.21.11.
        let bypassing: &[DamageType] = &[
            DamageType::ON_FIRE,
            DamageType::IN_WALL,
            DamageType::CRAMMING,
            DamageType::DROWN,
            DamageType::FLY_INTO_WALL,
            DamageType::GENERIC,
            DamageType::WITHER,
            DamageType::DRAGON_BREATH,
            DamageType::STARVE,
            DamageType::FALL,
            DamageType::ENDER_PEARL,
            DamageType::FREEZE,
            DamageType::STALAGMITE,
            DamageType::MAGIC,
            DamageType::INDIRECT_MAGIC,
            DamageType::OUT_OF_WORLD,
            DamageType::GENERIC_KILL,
            DamageType::SONIC_BOOM,
            DamageType::OUTSIDE_BORDER,
        ];
        for dt in bypassing {
            assert!(
                bypasses_armor_durability(dt),
                "{} should bypass armor durability",
                dt.message_id
            );
        }
    }

    /// Physical/combat damage types must NOT bypass armor durability.
    #[test]
    fn bypasses_armor_durability_returns_false_for_physical_sources() {
        let physical: &[DamageType] = &[
            DamageType::MOB_ATTACK,
            DamageType::PLAYER_ATTACK,
            DamageType::ARROW,
            DamageType::CACTUS,
            DamageType::SWEET_BERRY_BUSH,
            DamageType::LAVA,
            DamageType::EXPLOSION,
            DamageType::PLAYER_EXPLOSION,
            DamageType::LIGHTNING_BOLT,
            DamageType::FIREBALL,
            DamageType::THORNS,
            DamageType::TRIDENT,
        ];
        for dt in physical {
            assert!(
                !bypasses_armor_durability(dt),
                "{} should NOT bypass armor durability",
                dt.message_id
            );
        }
    }

    #[test]
    fn silent_movers_emit_no_swim_sound() {
        for entity_type in [
            &EntityType::BAT,
            &EntityType::SQUID,
            &EntityType::GLOW_SQUID,
            &EntityType::GUARDIAN,
            &EntityType::ELDER_GUARDIAN,
            &EntityType::SILVERFISH,
        ] {
            assert!(!LivingEntity::movement_emits_sounds(entity_type));
        }
    }

    #[test]
    fn ordinary_mobs_emit_movement_sounds() {
        assert!(LivingEntity::movement_emits_sounds(&EntityType::COD));
        assert!(LivingEntity::movement_emits_sounds(&EntityType::ZOMBIE));
    }

    #[test]
    fn shulkers_suppress_movement_events_but_ordinary_mobs_emit_them() {
        // `Shulker.getMovementEmission` returns NONE while the base living path returns ALL
        // (`Shulker.java:104-108`; `Entity.java:1533-1535`).
        assert!(!LivingEntity::movement_emits_events(&EntityType::SHULKER));
        assert!(LivingEntity::movement_emits_events(&EntityType::ZOMBIE));
    }

    #[test]
    fn hurt_sound_for_entity_uses_zombie_family_sounds() {
        let cases = [
            (&EntityType::ZOMBIE, Sound::EntityZombieHurt),
            (&EntityType::DROWNED, Sound::EntityDrownedHurt),
            (&EntityType::HUSK, Sound::EntityHuskHurt),
            (
                &EntityType::ZOMBIE_VILLAGER,
                Sound::EntityZombieVillagerHurt,
            ),
        ];

        for (entity_type, expected) in cases {
            assert_eq!(LivingEntity::hurt_sound_for_entity(entity_type), expected);
        }
    }

    #[test]
    fn hurt_sound_for_entity_uses_enderman_hurt_sound() {
        assert_eq!(
            LivingEntity::hurt_sound_for_entity(&EntityType::ENDERMAN),
            Sound::EntityEndermanHurt
        );
    }

    #[test]
    fn hurt_sound_for_entity_uses_skeleton_family_sounds() {
        let cases = [
            (&EntityType::SKELETON, Sound::EntitySkeletonHurt),
            (&EntityType::BOGGED, Sound::EntityBoggedHurt),
            (&EntityType::PARCHED, Sound::EntityParchedHurt),
            (
                &EntityType::WITHER_SKELETON,
                Sound::EntityWitherSkeletonHurt,
            ),
            (&EntityType::STRAY, Sound::EntityStrayHurt),
        ];

        for (entity_type, expected) in cases {
            assert_eq!(LivingEntity::hurt_sound_for_entity(entity_type), expected);
        }
    }

    #[test]
    fn hurt_sound_for_entity_defaults_to_generic_hurt() {
        assert_eq!(
            LivingEntity::hurt_sound_for_entity(&EntityType::CREEPER),
            Sound::EntityGenericHurt
        );
    }

    #[test]
    fn sonic_boom_bypasses_armor_enchantments() {
        assert!(bypasses_enchantments(&DamageType::SONIC_BOOM));
        assert!(!bypasses_enchantments(&DamageType::MAGIC));
    }

    #[test]
    fn falling_damage_sources_use_helmet_protection() {
        assert!(damages_helmet(&DamageType::FALLING_ANVIL));
        assert!(damages_helmet(&DamageType::FALLING_BLOCK));
        assert!(damages_helmet(&DamageType::FALLING_STALACTITE));
        assert!(!damages_helmet(&DamageType::FALL));
    }

    #[test]
    fn shield_bypass_sources_cannot_be_blocked() {
        assert!(bypasses_shield(&DamageType::SONIC_BOOM));
        assert!(bypasses_shield(&DamageType::CAMPFIRE));
        assert!(!bypasses_shield(&DamageType::MOB_ATTACK));
    }

    #[test]
    fn regeneration_particle_metadata_uses_vanilla_argb_color() {
        let effect = Effect {
            effect_type: &StatusEffect::REGENERATION,
            duration: 200,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        };
        let metadata = Metadata::new(
            tracked_data::living_entity::EFFECT_PARTICLES,
            EffectParticles(vec![EffectParticle::from_effect(&effect)]),
        );
        let mut bytes = Vec::new();

        metadata
            .write(
                &mut bytes,
                &pumpkin_util::version::JavaMinecraftVersion::V_26_2,
            )
            .unwrap();

        assert_eq!(bytes, [10, 17, 1, 28, 0xff, 0xcd, 0x5c, 0xab]);
    }
}

#[cfg(test)]
mod active_item_tests {
    use super::active_item_for_state;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;

    #[test]
    fn active_item_prefers_the_use_stack_and_falls_back_to_main_hand() {
        // `LivingEntity.getActiveItem` selects `getUseItem` only while using an item
        // (`LivingEntity.java:2235-2241`).
        let main_hand = ItemStack::new(1, &Item::IRON_SWORD);
        let use_item = ItemStack::new(1, &Item::SHIELD);

        assert!(active_item_for_state(false, Some(&use_item), &main_hand).are_equal(&main_hand));
        assert!(active_item_for_state(true, Some(&use_item), &main_hand).are_equal(&use_item));
        assert!(active_item_for_state(true, None, &main_hand).is_empty());
    }
}

#[cfg(test)]
mod attribute_nbt_tests {
    use super::{
        AttributeInstance, Modifier, ModifierOperation, apply_packed_attributes, pack_attributes,
    };
    use pumpkin_data::attributes::Attributes;
    use pumpkin_nbt::tag::NbtTag;
    use std::collections::HashMap;

    fn instance_of(attribute: &Attributes, base: f64) -> AttributeInstance {
        AttributeInstance::new(base, attribute.min_value, attribute.max_value)
    }

    fn defaults() -> Vec<(Attributes, f64)> {
        vec![
            (Attributes::MAX_HEALTH, Attributes::MAX_HEALTH.default_value),
            (
                Attributes::FOLLOW_RANGE,
                Attributes::FOLLOW_RANGE.default_value,
            ),
        ]
    }

    #[test]
    fn round_trips_base_and_permanent_modifiers() {
        let mut saved: HashMap<u8, AttributeInstance> = HashMap::new();
        let mut health = instance_of(&Attributes::MAX_HEALTH, 20.0);
        health.add_or_replace_modifier(Modifier {
            id: "minecraft:leader_zombie_bonus".to_string(),
            amount: 2.5,
            operation: ModifierOperation::MultiplyTotal,
        });
        saved.insert(Attributes::MAX_HEALTH.id, health);
        let mut follow = instance_of(&Attributes::FOLLOW_RANGE, 42.0);
        follow.add_or_replace_modifier(Modifier {
            id: "minecraft:zombie_random_spawn_bonus".to_string(),
            amount: 1.25,
            operation: ModifierOperation::Add,
        });
        saved.insert(Attributes::FOLLOW_RANGE.id, follow);

        let packed = pack_attributes(&saved, &defaults());
        assert_eq!(packed.len(), 2);

        let mut loaded: HashMap<u8, AttributeInstance> = HashMap::new();
        loaded.insert(
            Attributes::MAX_HEALTH.id,
            instance_of(
                &Attributes::MAX_HEALTH,
                Attributes::MAX_HEALTH.default_value,
            ),
        );
        loaded.insert(
            Attributes::FOLLOW_RANGE.id,
            instance_of(
                &Attributes::FOLLOW_RANGE,
                Attributes::FOLLOW_RANGE.default_value,
            ),
        );
        apply_packed_attributes(&mut loaded, &packed);

        for id in [Attributes::MAX_HEALTH.id, Attributes::FOLLOW_RANGE.id] {
            let before = &saved[&id];
            let after = &loaded[&id];
            assert_eq!(before.base_value.to_bits(), after.base_value.to_bits());
            assert_eq!(before.modifiers.len(), after.modifiers.len());
            for (a, b) in before.modifiers.iter().zip(after.modifiers.iter()) {
                assert_eq!(a.id, b.id);
                assert_eq!(a.amount.to_bits(), b.amount.to_bits());
                assert_eq!(a.operation as i8, b.operation as i8);
            }
            assert_eq!(before.value().to_bits(), after.value().to_bits());
        }
    }

    #[test]
    fn skips_transient_modifiers_and_untouched_attributes() {
        let mut saved: HashMap<u8, AttributeInstance> = HashMap::new();
        let mut speed = instance_of(
            &Attributes::MOVEMENT_SPEED,
            Attributes::MOVEMENT_SPEED.default_value,
        );
        speed.add_or_replace_modifier(Modifier {
            id: "minecraft:enchantment.swift_sneak/legs".to_string(),
            amount: 0.4,
            operation: ModifierOperation::MultiplyTotal,
        });
        speed.add_or_replace_modifier(Modifier {
            id: "minecraft:attacking".to_string(),
            amount: 0.15,
            operation: ModifierOperation::Add,
        });
        saved.insert(Attributes::MOVEMENT_SPEED.id, speed);
        saved.insert(
            Attributes::MAX_HEALTH.id,
            instance_of(
                &Attributes::MAX_HEALTH,
                Attributes::MAX_HEALTH.default_value,
            ),
        );

        assert!(pack_attributes(&saved, &defaults()).is_empty());
    }

    #[test]
    fn packed_shape_matches_vanilla_codec() {
        let mut saved: HashMap<u8, AttributeInstance> = HashMap::new();
        let mut health = instance_of(&Attributes::MAX_HEALTH, 24.0);
        health.add_or_replace_modifier(Modifier {
            id: "minecraft:leader_zombie_bonus".to_string(),
            amount: 0.5,
            operation: ModifierOperation::Add,
        });
        saved.insert(Attributes::MAX_HEALTH.id, health);

        let packed = pack_attributes(&saved, &defaults());
        let NbtTag::Compound(entry) = &packed[0] else {
            panic!("attribute entry is not a compound");
        };
        assert_eq!(entry.get_string("id"), Some("minecraft:max_health"));
        assert_eq!(entry.get_double("base"), Some(24.0));
        let modifiers = entry.get_list("modifiers").expect("modifiers list");
        let NbtTag::Compound(modifier) = &modifiers[0] else {
            panic!("modifier is not a compound");
        };
        assert_eq!(
            modifier.get_string("id"),
            Some("minecraft:leader_zombie_bonus")
        );
        assert_eq!(modifier.get_double("amount"), Some(0.5));
        assert_eq!(modifier.get_string("operation"), Some("add_value"));
    }

    #[test]
    fn unknown_attribute_is_ignored() {
        let mut compound = pumpkin_nbt::compound::NbtCompound::new();
        compound.put_string("id", "modded:not_an_attribute".to_string());
        compound.put_double("base", 5.0);
        let mut loaded: HashMap<u8, AttributeInstance> = HashMap::new();
        apply_packed_attributes(&mut loaded, &[NbtTag::Compound(compound)]);
        assert!(loaded.is_empty());
    }
}

#[cfg(test)]
mod equipment_drop_tests {
    use super::LivingEntity;
    use pumpkin_data::Enchantment;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;

    #[test]
    fn vanishing_curse_prevents_mob_equipment_drop() {
        // `Mob.dropCustomDeathLoot` checks `PREVENT_EQUIPMENT_DROP` before dropping
        // (`Mob.java:904-906`).
        let plain = ItemStack::new(1, &Item::IRON_SWORD);
        assert!(!LivingEntity::item_prevents_equipment_drop(&plain));

        let mut cursed = plain;
        cursed.add_enchantment(&Enchantment::VANISHING_CURSE, 1);
        assert!(LivingEntity::item_prevents_equipment_drop(&cursed));
    }
}

#[cfg(test)]
mod equip_event_tests {
    use super::does_emit_equip_event;
    use pumpkin_data::data_component_impl::EquipmentSlot;

    #[test]
    fn player_equip_events_are_limited_to_humanoid_armor() {
        // `Player.doesEmitEquipEvent` (`Player.java:1664-1666`) excludes hand equipment,
        // while the base `LivingEntity` hook (`LivingEntity.java:685-686`) accepts it.
        assert!(does_emit_equip_event(true, &EquipmentSlot::HEAD));
        assert!(!does_emit_equip_event(true, &EquipmentSlot::MAIN_HAND));
        assert!(does_emit_equip_event(false, &EquipmentSlot::MAIN_HAND));
    }
}
