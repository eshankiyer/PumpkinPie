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
    AtomicBool, AtomicU8, AtomicU64,
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
    ConsumableImpl, ConsumeAnimation, ConsumeEffect, UseRemainderImpl,
};
use pumpkin_data::data_component_impl::{
    AttributeModifiersImpl, BlocksAttacksImpl, DamageResistantImpl, DamageResistantType,
    DeathProtectionImpl, EnchantmentsImpl, EquipmentSlot, EquippableImpl, FoodImpl,
    OminousBottleAmplifierImpl,
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
use pumpkin_util::text::TextComponent;
use rand::{RngExt, SeedableRng, rngs::StdRng};
use std::sync::RwLock;
use tokio::sync::Mutex;
use uuid::Uuid;

fn knockback_strength_with_resistance(strength: f64, resistance: f64) -> f64 {
    strength * (1.0 - resistance.clamp(0.0, 1.0))
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
    fn write_metadata(&self, writer: &mut impl std::io::Write) -> Result<(), WritingError> {
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
    pub fn knockback_with_resistance(&self, strength: f64, x: f64, z: f64) {
        let resistance = self.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE);
        self.entity.knockback(
            knockback_strength_with_resistance(strength, resistance),
            x,
            z,
        );
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
            air_random: std::sync::Mutex::new(StdRng::seed_from_u64(rand::rng().random())),
            air_metadata_initialized: AtomicBool::new(false),
            entity,
            hurt_cooldown: AtomicI32::new(0),
            last_hurt_by_player_time: AtomicI32::new(0),
            last_damage_taken: AtomicCell::new(0.0),
            absorption: AtomicCell::new(0.0),
            fall_distance: AtomicCell::new(0.0),
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
        let mut changes: Vec<(EquipmentSlot, ItemStack, ItemStack)> = Vec::new();
        {
            let mut last = self.last_equipment_items.lock().await;
            for (slot, current) in current_items {
                let previous = last
                    .get(&slot)
                    .cloned()
                    .unwrap_or_else(|| ItemStack::EMPTY.clone());
                if !previous.are_equal(&current) {
                    last.insert(slot.clone(), current.clone());
                    changes.push((slot, previous, current));
                }
            }
        }

        if changes.is_empty() {
            return;
        }

        let mut touched: Vec<Attributes> = Vec::new();
        for (slot, previous, current) in changes {
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

    /// Forgets the equipment snapshot so the next tick re-derives every enchantment attribute
    /// modifier from scratch. Needed wherever the attribute map itself is rebuilt, otherwise an
    /// unchanged snapshot would suppress the re-application.
    pub async fn clear_equipment_attribute_snapshot(&self) {
        self.last_equipment_items.lock().await.clear();
        self.soul_speed_active.store(false, Relaxed);
    }

    /// Soul Speed's `minecraft:location_changed` effects (`soul_speed.json`).
    ///
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
        self.item_use_time.store(duration, Ordering::Relaxed);
        *self.item_in_use.lock().await = Some(stack);
        *self.active_hand.lock().await = Some(hand);
        self.set_living_flag(Self::USING_ITEM_FLAG, true);
        self.set_living_flag(Self::OFF_HAND_ACTIVE_FLAG, hand == Hand::Left);
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
        // Must be at least 0
        let new_abs = new_abs.max(0.0);

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
        inst.dirty.store(true, Ordering::Relaxed);
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
            inst.base_value = new_base;
            inst.dirty.store(true, Ordering::Relaxed);
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

    #[expect(
        clippy::too_many_lines,
        reason = "effect application also synchronizes attributes"
    )]
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

            // Effects that modify attributes (ex. speed) should also update the
            // entity's attribute instances (server-side) and then notify clients.
            if !effect.effect_type.attribute_modifiers.is_empty() {
                // Apply each attribute modifier into the local AttributeInstance
                for m in effect.effect_type.attribute_modifiers {
                    let id = m.id.to_string();
                    let op = match m.operation {
                        Operation::AddValue => ModifierOperation::Add,
                        Operation::AddMultipliedBase => ModifierOperation::MultiplyBase,
                        Operation::AddMultipliedTotal => ModifierOperation::MultiplyTotal,
                    };
                    let scaled_amount = m.base_value * (f64::from(effect.amplifier) + 1.);
                    let mod_inst = Modifier {
                        id,
                        amount: scaled_amount,
                        operation: op,
                    };

                    self.update_attribute(m.attribute, |inst| {
                        inst.add_or_replace_modifier(mod_inst.clone());
                    });
                }

                // Recompute packet modifiers from active effects for each affected attribute
                let mut touched_attrs: Vec<pumpkin_data::attributes::Attributes> = Vec::new();
                for m in effect.effect_type.attribute_modifiers {
                    if !touched_attrs.iter().any(|a| a.id == m.attribute.id) {
                        touched_attrs.push(m.attribute.clone());
                    }
                }

                if !touched_attrs.is_empty() {
                    crate::entity::attributes::send_attribute_updates_for_living(
                        self,
                        touched_attrs,
                    )
                    .await;
                }
            }

            // Apply invisible effect
            if effect.effect_type == &StatusEffect::INVISIBILITY {
                self.entity.set_invisible(true).await;
            }

            // Apply glowing effect
            if effect.effect_type == &StatusEffect::GLOWING {
                self.entity.set_glowing(true).await;
            }
        }

        // Broadcast effect to nearby players
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
        self.sync_effect_particles().await;

        true
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

    pub async fn remove_effect(&self, effect_type: &'static StatusEffect) -> bool {
        // Remove the effect
        let succeeded = self
            .active_effects
            .lock()
            .await
            .remove(&effect_type)
            .is_some();

        if !succeeded {
            return false;
        }

        self.hidden_effects.lock().await.remove(&effect_type);

        // Broadcast effect removal
        self.entity
            .world
            .load()
            .send_remove_mob_effect(&self.entity, effect_type);

        // Remove attribute modifiers, if any
        if !effect_type.attribute_modifiers.is_empty() {
            let mut touched_attrs = Vec::new();

            for m in effect_type.attribute_modifiers {
                let id = m.id.to_string();

                // Clean local server state
                self.update_attribute(m.attribute, |inst| {
                    inst.remove_modifier(&id);
                });

                // Track unique attributes for the packet update
                if !touched_attrs
                    .iter()
                    .any(|a: &Attributes| a.id == m.attribute.id)
                {
                    touched_attrs.push(m.attribute.clone());
                }
            }

            // Sync the clean state to the client
            if !touched_attrs.is_empty() {
                crate::entity::attributes::send_attribute_updates_for_living(self, touched_attrs)
                    .await;
            }
        }

        // Vanilla has no absorption reset on removal: dropping the effect drops its
        // MAX_ABSORPTION modifier, and `LivingEntity.onAttributeUpdated` clamps the current
        // amount down to whatever maximum is left instead of zeroing it outright.
        if effect_type == &StatusEffect::ABSORPTION {
            let max_absorption = self.get_attribute_value(&Attributes::MAX_ABSORPTION) as f32;
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

        if succeeded {
            self.sync_effect_particles().await;
        }

        succeeded
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
        let is_alive = !self.dead.load(Relaxed) && self.health.load() > 0.0;
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

                velo.y += 0.04;

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

        let custom_travel = if no_ai {
            false
        } else if let Some(mob) = caller.get_mob()
            && mob.get_entity().entity_id == self.entity.entity_id
        {
            mob.custom_travel(caller).await
        } else {
            false
        };

        if !no_ai && !custom_travel {
            let touching_water = self.entity.touching_water.load(SeqCst);

            // Strider is the only entity that has canWalkOnFluid = false

            if (touching_water || self.entity.touching_lava.load(SeqCst))
                && should_swim_in_fluids
                && self.entity.entity_type != &EntityType::STRIDER
            {
                self.travel_in_fluid(caller, touching_water).await;
            } else {
                // TODO: Gliding

                self.travel_in_air(caller).await;
            }
        }

        // TODO: Apply Soul Speed boot durability when tick_block_underneath is implemented.
        //self.entity.tick_block_underneath(&caller);

        let suffocating = self.entity.tick_block_collisions(caller, server).await;

        if suffocating {
            self.damage(&**caller, 1.0, DamageType::IN_WALL).await;
        }
    }

    async fn travel_in_air<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) {
        // applyMovementInput

        // LivingEntity.getFrictionInfluencedSpeed uses `getSpeed()`, not the raw attribute.
        let effective_speed = self.speed.load();

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
                slipperiness * 0.91,
            )
        } else {
            let speed = if let Some(player) = caller.get_player() {
                player.get_off_ground_speed().await
            } else {
                // TODO: If the passenger is a player, ogs = movement_speed * 0.1

                0.02
            };

            (speed, 0.91)
        };

        self.entity
            .update_velocity_from_input(self.movement_input.load(), speed);

        self.apply_climbing_speed();

        self.make_move(caller).await;

        let mut velo = self.entity.velocity.load();

        let can_powder_snow_climb = if self.entity.was_in_powder_snow.load(Relaxed) {
            crate::block::blocks::powder_snow::can_entity_walk_on_powder_snow(caller.as_ref()).await
        } else {
            false
        };

        if (self.entity.horizontal_collision.load(SeqCst) || self.jumping.load(SeqCst))
            && (self.climbing.load(Relaxed) || can_powder_snow_climb)
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

        velo.x *= friction;

        velo.z *= friction;

        velo.y *= caller.get_y_velocity_drag().unwrap_or_else(|| {
            if caller.is_flutterer() {
                friction
            } else {
                0.98
            }
        });

        self.entity.velocity.store(velo);
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
            if self.entity.horizontal_collision.load(SeqCst) && self.climbing.load(Relaxed) {
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
        let world = self.entity.world.load();
        let actual_dy = self.entity.pos.load().y - old_y;
        let probe_box = self.entity.bounding_box.load().shift(Vector3::new(
            velo.x,
            velo.y + 0.6 - actual_dy,
            velo.z,
        ));
        if self.entity.horizontal_collision.load(SeqCst)
            && !world.check_fluid_collision(probe_box)
            && world.is_space_empty(probe_box)
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

    fn apply_climbing_speed(&self) {
        if self.climbing.load(Relaxed) {
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

            // TODO
            // if velo.y < 0.0
            //     && self.entity.entity_type == &EntityType::PLAYER
            //     && self.entity.sneaking.load(Relaxed)
            // {
            //     let block = self
            //         .entity
            //         .world
            //         .read()
            //         .await
            //         .get_block(&self.entity.block_pos.load())
            //         .await;

            //     if let Some(props) = block.properties(block.default_state.id) {
            //         if props.name() == "ScaffoldingLikeProperties" {
            //             velo.y = 0.0;
            //         }
            //     }
            // }

            self.entity.velocity.store(velo);
        }
    }

    pub fn get_swim_height(&self) -> f64 {
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
        if ground {
            let fall_distance = self.fall_distance.swap(0.0);
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
        if may_fly || self.is_immune_to_fall_damage() {
            return;
        }

        // Fetches the safe fall distance attribute
        let safe_fall_distance = self.get_attribute_value(&Attributes::SAFE_FALL_DISTANCE) as f32;
        let unsafe_fall_distance = fall_distance + 1.0E-6 - safe_fall_distance;

        let damage = (unsafe_fall_distance * damage_per_distance).floor();
        if damage > 0.0 {
            let check_damage = self.damage(caller, damage, DamageType::FALL).await; // Fall
            if check_damage {
                self.entity
                    .play_sound(Self::get_fall_sound(fall_distance as i32));
            }
        }
    }

    const fn get_fall_sound(distance: i32) -> Sound {
        if distance > 4 {
            Sound::EntityGenericBigFall
        } else {
            Sound::EntityGenericSmallFall
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
                killed_by_player: Some(self.last_hurt_by_player_time.load(Relaxed) > 0),
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
            if world.level_info.load().game_rules.mob_drops && (is_monster || !is_baby) {
                self.drop_loot(params.clone()).await;
            }

            // `LivingEntity.die` (`world/entity/LivingEntity.java:1474`) calls
            // `createWitherRose(killer)` directly after `dropAllDeathLoot`, with `killer`
            // being `getKillCredit()` (:1438) - `cause` here.
            self.create_wither_rose(cause).await;

            // Award experience
            if params.killed_by_player.unwrap_or(false)
                && world.level_info.load().game_rules.mob_drops
            {
                let amount = dyn_self.get_experience_reward(cause);
                if amount > 0 {
                    ExperienceOrbEntity::spawn(&world, self.entity.pos.load(), amount).await;
                }
            }
            self.entity.pose.store(EntityPose::Dying);

            self.drop_equipment(looting_level).await;

            // Broadcast death message if it's a player and the gamerule is enabled
            self.broadcast_death_message(&*dyn_self, damage_type, source, cause)
                .await;

            self.reset_effects_and_attributes().await;
        }
    }

    async fn drop_equipment(&self, looting_level: u32) {
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
            if let Some(max_damage) = item.get_max_damage() {
                let mut rng = rand::rng();
                let inner = rng.random_range(0..(max_damage - 3).max(1));
                let outer = rng.random_range(0..=inner);
                item.set_damage((max_damage - outer).max(0));
            }
            world.drop_stack(&block_pos, item).await;
        }
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
            if dyn_self.get_player().is_some() {
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

    async fn drop_loot(&self, params: LootContextParameters) {
        if let Some(loot_table) = &self.get_entity().entity_type.loot_table {
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
        let max_absorption = self.get_attribute_value(&Attributes::MAX_ABSORPTION) as f32;
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
    ) {
        // Formula: armor loses floor(incoming_damage / 4) durability, minimum 1.
        let armor_damage = (damage_amount / 4.0).floor().max(1.0) as i32;
        let mut equipment_updates = Vec::new();

        // TODO: Implement DAMAGE_RESISTANT component checks (e.g. netherite vs fire).

        let helmet_only = damage_type.id == DamageType::FALLING_ANVIL.id
            || damage_type.id == DamageType::FALLING_BLOCK.id
            || damage_type.id == DamageType::FALLING_STALACTITE.id;

        let armor_slots: Vec<(usize, ItemStack, EquipmentSlot)> = {
            let equipment_lock = self.entity_equipment.lock().await;
            self.equipment_slots
                .iter()
                .filter(|(_, slot)| {
                    slot.is_armor_slot() && (!helmet_only || **slot == EquipmentSlot::HEAD)
                })
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
                let slot_result = stack.damage_item(armor_damage);
                if slot_result != pumpkin_data::item_stack::DamageResult::Untouched {
                    if slot_result == pumpkin_data::item_stack::DamageResult::Broken {
                        let world = self.entity.world.load();
                        world.send_entity_status(
                            &self.entity,
                            super::equipment_break_status(&slot),
                            None,
                        );
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

    pub fn can_take_damage(&self) -> bool {
        !self.entity.invulnerable.load(Ordering::Relaxed) && self.is_part_of_game()
    }

    pub fn is_part_of_game(&self) -> bool {
        !self.is_spectator() && self.entity.is_alive()
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
        let pos = self.entity.pos.load();
        let eye_y = self.entity.get_eye_y();
        let block_pos = BlockPos::floored(pos.x, eye_y, pos.z);
        let (fluid, state) = world.get_fluid_and_fluid_state(&block_pos);

        if !fluid.has_tag(&tag::Fluid::MINECRAFT_WATER) {
            return false;
        }

        let surface_y = f64::from(block_pos.0.y) + world.get_fluid_height(&block_pos, fluid, state);

        // EntityFluidInteraction.isEyeInFluid uses an inclusive top boundary:
        // eyeY <= blockY + fluidState.getHeight(...).
        surface_y >= eye_y
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

        if self.dead.load(Relaxed) || self.health.load() <= 0.0 {
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
        if self.dead.load(Relaxed) || self.health.load() <= 0.0 {
            return;
        }

        if eye_in_water && !in_bubble_column {
            let can_breathe_underwater = self.can_breathe_underwater(caller).await;
            let has_water_breathing = self.has_effect(&StatusEffect::WATER_BREATHING).await;
            let has_conduit_power = self.has_effect(&StatusEffect::CONDUIT_POWER).await;
            let has_breath_of_the_nautilus =
                self.has_effect(&StatusEffect::BREATH_OF_THE_NAUTILUS).await;
            if self.dead.load(Relaxed) || self.health.load() <= 0.0 || self.entity.is_removed() {
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

    /// `Entity.getSwimSound` (Entity.java:1263-1265) returns `entity.generic.swim` for every
    /// entity; the two nautilus species override it.
    fn swim_sound(caller: &Arc<dyn EntityBase>) -> Sound {
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
    /// sound (Entity.java:889-893). Pumpkin has no step-sound path, so only the water branch
    /// plays; `nextStep` still advances in the other case (`Entity.nextStep`,
    /// Entity.java:1259-1261) so a mob that walks ashore does not fire the instant it gets back
    /// in. Volume comes from `Entity.waterSwimSound` (Entity.java:1428-1437) and the pitch
    /// spread from `Entity.playSwimSound` (Entity.java:1475-1477).
    ///
    /// Players are skipped: their client simulates its own movement and plays this sound
    /// locally, which vanilla accounts for by excluding the player from its own
    /// `Player.playSound` broadcast, a distinction `World::play_sound` here does not draw.
    fn tick_swim_sound(&self, caller: &Arc<dyn EntityBase>) {
        if self.entity.entity_type == &EntityType::PLAYER {
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
        if !self.entity.touching_water.load(Relaxed)
            || self.entity.on_ground.load(Relaxed)
            || self.entity.is_silent()
            || !Self::movement_emits_sounds(self.entity.entity_type)
        {
            return;
        }
        let velocity = self.entity.velocity.load();
        let volume = ((velocity.x * velocity.x)
            .mul_add(
                0.2,
                velocity
                    .y
                    .mul_add(velocity.y, velocity.z * velocity.z * 0.2),
            )
            .sqrt() as f32
            * 0.35)
            .min(1.0);
        let mut rng = rand::rng();
        let pitch = (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.4, 1.0);
        self.entity.world.load().play_sound_fine(
            Self::swim_sound(caller),
            SoundCategory::Neutral,
            &self.entity.pos.load(),
            volume,
            pitch,
        );
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
            let max_abs = self.get_attribute_value(&Attributes::MAX_ABSORPTION) as f32;
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
            // Vanilla saves the effects' permanent attribute modifiers alongside the effects
            // themselves (`LivingEntity.readAdditionalSaveData`: the `attributes` tag is applied
            // right before `active_effects`), so a reloaded Speed or Strength still moves the
            // attribute. Pumpkin does not persist attributes, so the modifiers are rebuilt from
            // the effects that were just loaded instead of silently going missing.
            for effect in loaded_effects {
                self.restore_effect_attribute_modifiers(&effect);
                if effect.effect_type == &StatusEffect::INVISIBILITY {
                    self.entity.set_invisible(true).await;
                } else if effect.effect_type == &StatusEffect::GLOWING {
                    self.entity.set_glowing(true).await;
                }
            }

            let mut equipment = self.entity_equipment.lock().await;
            for (key, slot) in [
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
                for (index, slot) in slot.iter().enumerate() {
                    let Some(compound) = items.get(index).and_then(NbtTag::extract_compound) else {
                        continue;
                    };
                    let Some(stack) = ItemStack::read_item_stack(compound) else {
                        continue;
                    };
                    equipment.put(slot, stack);
                }
            }
        })
        // todo more...
    }
}

impl EntityBase for LivingEntity {
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

            if amount < 0.0 {
                return false;
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
            amount = damage_event.damage;

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
                            (resisted * 10.0) as i32,
                        )
                        .await;
                }
                if let Some(attacker_player) = cause.and_then(|c| c.get_player()) {
                    attacker_player
                        .increment_stat(
                            StatisticCategory::Custom,
                            CustomStatistic::DamageDealtResisted as i32,
                            (resisted * 10.0) as i32,
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

                    if let Some(attacker_player) = cause.and_then(|c| c.get_player()) {
                        let held_item = attacker_player.inventory().held_item().await;
                        let is_axe = held_item.is_axe();
                        if is_axe {
                            let mut disable_chance = 0.25;
                            let is_sprinting = attacker_player
                                .living_entity
                                .entity
                                .sprinting
                                .load(Ordering::Relaxed);
                            if is_sprinting {
                                disable_chance = 1.0;
                            }

                            if rand::random::<f32>() < disable_chance
                                && let Some(victim_player) = caller.get_player()
                            {
                                victim_player
                                    .start_cooldown("minecraft:shield".to_string(), 100)
                                    .await;
                                self.clear_active_hand().await;

                                world.broadcast_packet_all(&CEntityStatus::new(
                                    self.entity.entity_id,
                                    30,
                                ));
                            }
                        }
                    }

                    let active_hand = self.active_hand.lock().await;
                    if let Some(hand) = *active_hand
                        && amount >= 3.0
                    {
                        let slot = equipment_slot_for_hand(hand);

                        let mut equipment_guard = self.entity_equipment.lock().await;
                        if let Some(stack) = equipment_guard.equipment.get_mut(&slot) {
                            let durability_damage = (amount / 1.0).floor().max(1.0) as i32;
                            if stack.damage_item(durability_damage) == DamageResult::Broken {
                                if let Some(player) = caller.get_player() {
                                    player
                                        .increment_stat(
                                            StatisticCategory::Broken,
                                            stack.item.id as i32,
                                            1,
                                        )
                                        .await;
                                }
                                world.send_entity_status(
                                    &self.entity,
                                    crate::entity::equipment_break_status(&slot),
                                    None,
                                );
                                *stack = ItemStack::EMPTY.clone();
                                let broken_stack = stack.clone();
                                drop(equipment_guard);

                                self.send_equipment_changes(&[(slot, broken_stack)]);
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

            if damages_helmet(&damage_type) {
                amount *= 0.75;
            }

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
                if let Some(attacker_player) = cause.and_then(|c| c.get_player()) {
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
                // `Mob.playHurtSound` (Mob.java:295-299) resets the idle-sound timer, so a mob
                // that was just hit does not chirp immediately afterwards.
                if let Some(mob) = caller.get_mob() {
                    mob.get_mob_entity()
                        .ambient_sound_time
                        .store(-mob.get_ambient_sound_interval(), Relaxed);
                }
                world.play_sound(
                    self.hurt_sound(),
                    SoundCategory::Players,
                    &self.entity.pos.load(),
                );

                if let Some(source) = source {
                    let source_pos = source.get_entity().pos.load();
                    let target_pos = self.entity.pos.load();
                    let dx = source_pos.x - target_pos.x;
                    let dz = source_pos.z - target_pos.z;
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
                            (absorbed * 10.0) as i32,
                        )
                        .await;
                }

                if let Some(attacker_player) = cause.and_then(|c| c.get_player()) {
                    attacker_player
                        .increment_stat(
                            StatisticCategory::Custom,
                            CustomStatistic::DamageDealtAbsorbed as i32,
                            (absorbed * 10.0) as i32,
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
                            (remaining * 10.0) as i32,
                        )
                        .await;
                }

                if let Some(attacker_player) = cause.and_then(|c| c.get_player()) {
                    attacker_player
                        .increment_stat(
                            StatisticCategory::Custom,
                            CustomStatistic::DamageDealt as i32,
                            (remaining * 10.0) as i32,
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
                            world.send_entity_status(
                                &self.entity,
                                crate::entity::equipment_break_status(&slot),
                                None,
                            );
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
                self.damage_armor_items(caller, raw_increment, &damage_type)
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
            if let Some(mob) = caller.get_mob()
                && mob.get_entity().entity_id == self.entity.entity_id
            {
                mob.update_swimming().await;
            }
            self.tick_equipment_attributes(caller.as_ref()).await;
            self.tick_soul_speed(caller.as_ref()).await;
            let was_alive_before_air =
                !self.dead.load(Relaxed) && self.health.load() > 0.0 && !self.entity.is_removed();
            if self.entity.entity_type == &EntityType::PLAYER
                && was_alive_before_air
                && let Some(player) = caller.cast_any().downcast_ref::<Player>()
            {
                player.breath_manager.tick(player).await;
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
                self.tick_auto_spin_attack(caller, previous_bounding_box)
                    .await;
                self.push_entities(caller).await;
                self.tick_swim_sound(caller);
            }

            // TODO
            let player = caller.get_player();
            let is_player = player.is_some();

            if !is_player {
                self.entity.send_pos_rot();
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
                {
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
                if self.last_hurt_by_player_time.load(Relaxed) > 0 {
                    self.last_hurt_by_player_time.fetch_sub(1, Relaxed);
                }
                if self.health.load() <= 0.0 {
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

/// `LivingEntity.getFrictionInfluencedSpeed`: the grounded per-tick factor
/// `moveRelative` is called with.
fn friction_influenced_speed(speed: f64, slipperiness: f64) -> f64 {
    speed * 0.216_000_02 / (slipperiness * slipperiness * slipperiness)
}

fn damage_causes_panic(damage_type: DamageType) -> bool {
    damage_type.has_tag(&tag::DamageType::MINECRAFT_PANIC_CAUSES)
}

#[cfg(test)]
mod tests {
    use super::*;

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
