use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU8, Ordering};

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture, living::LivingEntity,
};
use crate::world::game_event::{GameEventContext, emit_game_event};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{
    damage::DamageType,
    data_component_impl::{EquipmentSlot, EquipmentType},
    entity::EntityStatus,
    item::Item,
    particle::Particle,
    sound::{Sound, SoundCategory},
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::math::{
    boundingbox::EntityDimensions, euler_angle::EulerAngle, vector3::Vector3,
};

#[derive(Debug, Clone, Copy)]
pub struct PackedRotation {
    pub head: EulerAngle,
    pub body: EulerAngle,
    pub left_arm: EulerAngle,
    pub right_arm: EulerAngle,
    pub left_leg: EulerAngle,
    pub right_leg: EulerAngle,
}

impl Default for PackedRotation {
    fn default() -> Self {
        Self {
            head: EulerAngle::new(0.0, 0.0, 0.0),
            body: EulerAngle::new(0.0, 0.0, 0.0),
            left_arm: EulerAngle::new(-10.0, 0.0, -10.0),
            right_arm: EulerAngle::new(-15.0, 0.0, 10.0),
            left_leg: EulerAngle::new(-1.0, 0.0, -1.0),
            right_leg: EulerAngle::new(1.0, 0.0, 1.0),
        }
    }
}

impl From<PackedRotation> for NbtTag {
    fn from(val: PackedRotation) -> Self {
        let mut compound = NbtCompound::new();
        compound.put("Head", val.head);
        compound.put("Body", val.body);
        compound.put("LeftArm", val.left_arm);
        compound.put("RightArm", val.right_arm);
        compound.put("LeftLeg", val.left_leg);
        compound.put("RightLeg", val.right_leg);
        Self::Compound(compound)
    }
}

impl From<NbtTag> for PackedRotation {
    #[expect(clippy::unnecessary_fallible_conversions)]
    fn from(tag: NbtTag) -> Self {
        if let NbtTag::Compound(compound) = tag {
            fn get_rotation(
                compound: &NbtCompound,
                key: &'static str,
                default: EulerAngle,
            ) -> EulerAngle {
                compound
                    .get(key)
                    .and_then(|tag| tag.clone().try_into().ok())
                    .unwrap_or(default)
            }

            let default = Self::default();

            Self {
                head: get_rotation(&compound, "Head", default.head),
                body: get_rotation(&compound, "Body", default.body),
                left_arm: get_rotation(&compound, "LeftArm", default.left_arm),
                right_arm: get_rotation(&compound, "RightArm", default.right_arm),
                left_leg: get_rotation(&compound, "LeftLeg", default.left_leg),
                right_leg: get_rotation(&compound, "RightLeg", default.right_leg),
            }
        } else {
            Self::default()
        }
    }
}

pub struct ArmorStandEntity {
    living_entity: LivingEntity,

    armor_stand_flags: AtomicU8,
    last_hit_time: AtomicI64,
    disabled_slots: AtomicI32,

    rotation: AtomicCell<PackedRotation>,
}

impl ArmorStandEntity {
    pub fn new(entity: Entity) -> Self {
        let living_entity = LivingEntity::new(entity);
        let packed_rotation = PackedRotation::default();

        Self {
            living_entity,
            armor_stand_flags: AtomicU8::new(0),
            last_hit_time: AtomicI64::new(0),
            disabled_slots: AtomicI32::new(0),
            rotation: AtomicCell::new(packed_rotation),
        }
    }

    pub fn set_small(&self, small: bool) {
        self.set_bit_field(ArmorStandFlags::Small, small);
        self.refresh_dimensions();
    }

    pub fn is_small(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::Small as u8) != 0
    }

    pub fn set_show_arms(&self, show_arms: bool) {
        self.set_bit_field(ArmorStandFlags::ShowArms, show_arms);
    }

    pub fn should_show_arms(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::ShowArms as u8) != 0
    }

    pub fn set_hide_base_plate(&self, hide_base_plate: bool) {
        self.set_bit_field(ArmorStandFlags::HideBasePlate, hide_base_plate);
    }

    pub fn should_show_base_plate(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::HideBasePlate as u8) == 0
    }

    pub fn set_marker(&self, marker: bool) {
        self.set_bit_field(ArmorStandFlags::Marker, marker);
        self.refresh_dimensions();
    }

    pub fn is_marker(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::Marker as u8) != 0
    }

    fn set_bit_field(&self, bit_field: ArmorStandFlags, set: bool) {
        if set {
            self.armor_stand_flags
                .fetch_or(bit_field as u8, Ordering::Relaxed);
        } else {
            self.armor_stand_flags
                .fetch_and(!(bit_field as u8), Ordering::Relaxed);
        }
    }

    fn refresh_dimensions(&self) {
        let entity = self.get_entity();
        let dimensions = if self.is_marker() {
            EntityDimensions::fixed(0.0, 0.0)
        } else if self.is_small() {
            let base = entity.base_dimension.load();
            EntityDimensions::new(base.width * 0.5, base.height * 0.5, 0.9875)
        } else {
            entity.base_dimension.load()
        };
        entity.entity_dimension.store(dimensions);
        entity
            .bounding_box
            .store(dimensions.make_bounding_box(entity.pos.load()));
    }

    pub fn can_use_slot(&self, slot: &EquipmentSlot) -> bool {
        !matches!(slot, EquipmentSlot::Body(_) | EquipmentSlot::Saddle(_))
            && !self.is_slot_disabled(slot)
    }

    pub fn is_slot_disabled(&self, slot: &EquipmentSlot) -> bool {
        let disabled_slots = self.disabled_slots.load(Ordering::Relaxed);
        let slot_bit = 1 << slot.get_offset_entity_slot_id(0);

        (disabled_slots & slot_bit) != 0
            || (slot.slot_type() == EquipmentType::Hand && !self.should_show_arms())
    }

    pub fn set_slot_disabled(&self, slot: &EquipmentSlot, disabled: bool) {
        let slot_bit = 1 << slot.get_offset_entity_slot_id(0);
        let current = self.disabled_slots.load(Ordering::Relaxed);

        let new_val = if disabled {
            current | slot_bit
        } else {
            current & !slot_bit
        };

        self.disabled_slots.store(new_val, Ordering::Relaxed);
    }

    pub fn is_invisible(&self) -> bool {
        self.get_entity().invisible.load(Ordering::Relaxed)
    }

    /// Vanilla `ArmorStand.setInvisible` (`ArmorStand.java:450-454`) stores a stand-specific
    /// invisibility flag and reapplies it after potion visibility recalculation.
    pub async fn set_invisible(&self, invisible: bool) {
        self.get_entity()
            .persistent_invisible
            .store(invisible, Ordering::Relaxed);
        self.living_entity.update_effect_visibility().await;
    }

    pub fn pack_rotation(&self) -> PackedRotation {
        self.rotation.load()
    }

    pub fn unpack_rotation(&self, packed: &PackedRotation) {
        self.rotation.store(packed.to_owned());
    }

    async fn break_and_drop_items(&self) {
        let entity = self.get_entity();
        let equipment = self.living_entity.entity_equipment.lock().await;
        let equipped_items: Vec<_> = equipment.equipment.values().cloned().collect();
        drop(equipment);

        for mut stack in equipped_items {
            if stack.is_empty() {
                continue;
            }
            let dropped = stack.clone();
            stack.clear();
            drop(stack);
            entity
                .world
                .load()
                .drop_stack(&entity.block_pos.load(), dropped)
                .await;
        }

        //let name = entity.custom_name.unwrap_or(entity.get_name());

        //TODO: i am stupid! let armor_stand_item = ItemStack::new_with_component(1, &Item::ARMOR_STAND, vec![(DataComponent::CustomName, self.get_custom_name())]);
        let armor_stand_item = ItemStack::new(1, &Item::ARMOR_STAND);
        entity
            .world
            .load()
            .drop_stack(&entity.block_pos.load(), armor_stand_item)
            .await;

        Self::on_break(entity);
    }

    fn on_break(entity: &Entity) {
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityArmorStandBreak,
            SoundCategory::Neutral,
            &entity.pos.load(),
        );
    }

    /// Spawns break particles at the armor stand's position.
    // TODO: use oak plank block particles like vanilla (requires block state data in particle system)
    fn spawn_break_particles(entity: &Entity) {
        let world = entity.world.load();
        let pos = entity.pos.load();
        let width = entity.width();
        let height = entity.height();

        // Spawn particles similar to vanilla: 10 particles with offset based on entity size
        world.spawn_particle(
            Vector3::new(pos.x, pos.y + f64::from(height) * 0.6666, pos.z),
            Vector3::new(width / 4.0, height / 4.0, width / 4.0),
            0.05,
            10,
            Particle::Poof,
        );
    }
}

impl NBTStorage for ArmorStandEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.living_entity.write_nbt(nbt).await;
            let disabled_slots = self.disabled_slots.load(Ordering::Relaxed);
            // ...

            nbt.put_bool(
                "Invisible",
                self.get_entity()
                    .persistent_invisible
                    .load(Ordering::Relaxed),
            );
            nbt.put_bool("Small", self.is_small());
            nbt.put_bool("ShowArms", self.should_show_arms());
            nbt.put_int("DisabledSlots", disabled_slots);
            nbt.put_bool("NoBasePlate", !self.should_show_base_plate());
            if self.is_marker() {
                nbt.put_bool("Marker", true);
            }

            nbt.put("Pose", self.pack_rotation());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.living_entity.read_nbt_non_mut(nbt).await;
            let mut flags = 0u8;
            // ...

            self.set_invisible(nbt.get_bool("Invisible").unwrap_or(false))
                .await;

            if let Some(small) = nbt.get_bool("Small")
                && small
            {
                flags |= ArmorStandFlags::Small as u8;
            }

            if let Some(show_arms) = nbt.get_bool("ShowArms")
                && show_arms
            {
                flags |= ArmorStandFlags::ShowArms as u8;
            }

            if let Some(disabled_slots) = nbt.get_int("DisabledSlots") {
                self.disabled_slots.store(disabled_slots, Ordering::Relaxed);
            }

            if let Some(no_base_plate) = nbt.get_bool("NoBasePlate") {
                if !no_base_plate {
                    flags |= ArmorStandFlags::HideBasePlate as u8;
                }
            } else {
                flags |= ArmorStandFlags::HideBasePlate as u8;
            }

            if let Some(marker) = nbt.get_bool("Marker")
                && marker
            {
                flags |= ArmorStandFlags::Marker as u8;
            }

            self.armor_stand_flags.store(flags, Ordering::Relaxed);
            self.refresh_dimensions();

            if let Some(pose_tag) = nbt.get("Pose") {
                let packed: PackedRotation = pose_tag.clone().into();
                self.unpack_rotation(&packed);
            }
        })
    }
}

impl EntityBase for ArmorStandEntity {
    fn get_entity(&self) -> &Entity {
        &self.living_entity.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        Some(&self.living_entity)
    }

    fn is_pickable(&self) -> bool {
        self.get_entity().is_alive() && !self.is_marker()
    }

    /// Vanilla `ArmorStand.isEffectiveAi` (`ArmorStand.java:118-121`) gates the inherited
    /// living movement path on the stand having physics.
    fn is_effective_ai(&self) -> bool {
        !self.is_marker() && !self.get_entity().has_no_gravity()
    }

    /// Vanilla `ArmorStand.canBeSeenByAnyone` (`ArmorStand.java:670-672`). Marker stands and
    /// explicitly invisible stands must not enter living-entity visibility memories.
    fn can_be_seen_by_anyone(&self) -> bool {
        !self.is_invisible() && !self.is_marker()
    }

    /// Vanilla `ArmorStand.skipAttackInteraction` (`ArmorStand.java:577-580`) delegates to
    /// `ServerLevel.mayInteract` (`ServerLevel.java:868-871`).
    fn skip_attack_interaction<'a>(
        &'a self,
        source: &'a crate::entity::player::Player,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let entity = self.get_entity();
            let world = entity.world.load();
            let Some(server) = world.server.upgrade() else {
                return false;
            };
            let block_pos = entity.block_pos.load();
            source
                .is_under_spawn_protection(&server, &world, &block_pos)
                .await
                || !world
                    .worldborder
                    .lock()
                    .await
                    .contains_block(block_pos.0.x, block_pos.0.z)
        })
    }

    /// Vanilla `ArmorStand.thunderHit` (`ArmorStand.java:602-604`) is intentionally empty, so
    /// lightning must not apply the generic living-entity fire and damage callback.
    fn on_lightning_strike<'a>(
        &'a self,
        _caller: &'a dyn EntityBase,
        _lightning: &'a crate::entity::lightning::LightningBoltEntity,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Vanilla `ArmorStand.getPistonPushReaction` (`ArmorStand.java:472-475`). Marker stands are
    /// not moved by pistons; ordinary stands use the default normal reaction.
    fn can_be_pushed_by_piston(&self) -> bool {
        !self.is_marker()
    }

    /// Vanilla `ArmorStand.getFallSounds` (`ArmorStand.java:587-590`). Both fall distances use
    /// the armor-stand fall sound; the generic `LivingEntity` fallback is distance-dependent.
    fn get_fall_sound(&self, _fall_distance: i32) -> Sound {
        Sound::EntityArmorStandFall
    }

    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn kill<'a>(&'a self, _caller: &'a dyn EntityBase) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.get_entity().remove().await;

            // No Arc<dyn EntityBase> available here, so GameEventContext::none().
            emit_game_event(
                &self.get_entity().world.load(),
                pumpkin_data::game_event::GameEvent::EntityDie,
                self.get_entity().pos.load(),
                GameEventContext::none(),
            )
            .await;
        })
    }

    fn damage_with_context<'a>(
        &'a self,
        caller: &'a dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&'a dyn EntityBase>,
        cause: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let entity = self.get_entity();
            if entity.is_removed() {
                return false;
            }

            let world = entity.world.load();

            let mob_griefing_gamerule = {
                let game_rules = &world.level_info.load().game_rules;
                game_rules.mob_griefing
            };

            if !mob_griefing_gamerule && source.is_some_and(|source| source.get_player().is_none())
            {
                return false;
            }

            let bypasses_invulnerability =
                damage_type == DamageType::OUT_OF_WORLD || damage_type == DamageType::GENERIC_KILL;

            if bypasses_invulnerability {
                entity.kill(caller).await;
                return false;
            }

            if entity.is_invulnerable_to(&damage_type).await
                || self.is_invisible()
                || self.is_marker()
            {
                return false;
            }

            let is_explosion = damage_type == DamageType::FIREWORKS
                || damage_type == DamageType::EXPLOSION
                || damage_type == DamageType::PLAYER_EXPLOSION
                || damage_type == DamageType::BAD_RESPAWN_POINT;

            if is_explosion {
                self.break_and_drop_items().await;
                entity.kill(caller).await;
                return false;
            }

            // Vanilla ignites instead of damaging; no health field exists here to chip
            // for the already-on-fire case, so that sub-case is a no-op.
            if damage_type.has_tag(&tag::DamageType::MINECRAFT_IGNITES_ARMOR_STANDS) {
                entity.set_on_fire_for(5.0);
                return false;
            }

            if damage_type.has_tag(&tag::DamageType::MINECRAFT_BURNS_ARMOR_STANDS) {
                return false;
            }

            let can_break = damage_type == DamageType::PLAYER_EXPLOSION
                || damage_type == DamageType::PLAYER_ATTACK
                || damage_type == DamageType::SPEAR
                || damage_type == DamageType::MACE_SMASH;

            let always_kills = damage_type == DamageType::ARROW
                || damage_type == DamageType::TRIDENT
                || damage_type == DamageType::FIREBALL
                || damage_type == DamageType::WITHER_SKULL
                || damage_type == DamageType::WIND_CHARGE;

            if !can_break && !always_kills {
                return false;
            }

            let attacker = cause.or(source);
            if let Some(attacker) = attacker
                && let Some(player) = attacker.get_player()
            {
                if !player.abilities.lock().await.allow_modify_world {
                    return false;
                } else if player.is_creative() {
                    Self::spawn_break_particles(entity);
                    entity.kill(caller).await;
                    return true;
                }
            }

            let time = world.level_time.lock().await.query_gametime();

            if time - self.last_hit_time.load(Ordering::Relaxed) > 5 && !always_kills {
                world.send_entity_status(entity, EntityStatus::ArmorstandWobble, None);
                world.play_sound(
                    Sound::EntityArmorStandHit,
                    SoundCategory::Neutral,
                    &entity.block_pos.load().to_f64(),
                );
                self.last_hit_time.store(time, Ordering::Relaxed);
            } else {
                Self::spawn_break_particles(entity);
                world.play_sound(
                    Sound::EntityArmorStandBreak,
                    SoundCategory::Neutral,
                    &entity.block_pos.load().to_f64(),
                );
                self.break_and_drop_items().await;
                entity.kill(caller).await;
            }

            true
        })
    }

    fn get_gravity(&self) -> f64 {
        0.08
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub enum ArmorStandFlags {
    /// Small armor stand Flag
    Small = 1,
    /// Show arms Flag
    ShowArms = 4,
    /// Hide base plate fLag
    HideBasePlate = 8,
    /// Marker Flag
    Marker = 16,
}
