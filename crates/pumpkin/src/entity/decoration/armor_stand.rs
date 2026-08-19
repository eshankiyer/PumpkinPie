use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicI64, AtomicU8, Ordering},
};

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture, living::LivingEntity,
    player::Player,
};
use crate::world::game_event::{GameEventContext, emit_game_event};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{
    damage::DamageType,
    data_component_impl::{EquipmentSlot, EquipmentType, EquippableImpl},
    entity::EntityStatus,
    item::Item,
    particle::Particle,
    sound::{Sound, SoundCategory},
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::math::{euler_angle::EulerAngle, vector3::Vector3};

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
    }

    pub fn is_marker(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::Marker as u8) != 0
    }

    fn set_bit_field(&self, bit_field: ArmorStandFlags, set: bool) {
        let current = self.armor_stand_flags.load(Ordering::Relaxed);
        let new_value = if set {
            current | bit_field as u8
        } else {
            current & !(bit_field as u8)
        };
        self.armor_stand_flags.store(new_value, Ordering::Relaxed);
    }

    pub fn can_use_slot(&self, slot: &EquipmentSlot) -> bool {
        Self::supports_slot(slot) && !self.is_slot_disabled(slot)
    }

    pub fn is_slot_disabled(&self, slot: &EquipmentSlot) -> bool {
        self.is_slot_masked(slot, 0)
            || (slot.slot_type() == EquipmentType::Hand && !self.should_show_arms())
    }

    pub fn set_slot_disabled(&self, slot: &EquipmentSlot, disabled: bool) {
        let slot_bit = Self::slot_mask(slot, 0);
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

    pub fn pack_rotation(&self) -> PackedRotation {
        self.rotation.load()
    }

    pub fn unpack_rotation(&self, packed: &PackedRotation) {
        self.rotation.store(packed.to_owned());
    }

    fn swap_held_item(equipped: &mut ItemStack, held: &mut ItemStack, creative: bool) -> bool {
        if creative && equipped.is_empty() && !held.is_empty() {
            *equipped = held.copy_with_count(1);
            return true;
        }

        if held.is_empty() || held.item_count <= 1 {
            std::mem::swap(equipped, held);
            return true;
        }

        if !equipped.is_empty() {
            return false;
        }

        *equipped = held.split(1);
        true
    }

    const fn supports_slot(slot: &EquipmentSlot) -> bool {
        !matches!(slot, EquipmentSlot::Body(_) | EquipmentSlot::Saddle(_))
    }

    fn equipment_slot_for_item(&self, item_stack: &ItemStack) -> EquipmentSlot {
        let slot = item_stack
            .get_data_component::<EquippableImpl>()
            .map_or_else(
                || EquipmentSlot::MAIN_HAND,
                |equippable| equippable.slot.clone(),
            );
        if self.can_use_slot(&slot) {
            slot
        } else {
            EquipmentSlot::MAIN_HAND
        }
    }

    fn is_slot_masked(&self, slot: &EquipmentSlot, offset: i32) -> bool {
        self.disabled_slots.load(Ordering::Relaxed) & Self::slot_mask(slot, offset) != 0
    }

    const fn slot_mask(slot: &EquipmentSlot, offset: i32) -> i32 {
        1 << (slot.get_slot_index() + offset)
    }

    async fn has_item_in_slot(&self, slot: &EquipmentSlot) -> bool {
        let stack = self.living_entity.entity_equipment.lock().await.get(slot);
        !stack.lock().await.is_empty()
    }

    async fn clicked_slot(&self, location: Vector3<f64>) -> EquipmentSlot {
        let small = self.is_small();
        let y = location.y;
        if y >= 0.1
            && y < 0.1 + if small { 0.8 } else { 0.45 }
            && self.has_item_in_slot(&EquipmentSlot::FEET).await
        {
            EquipmentSlot::FEET
        } else if y >= 0.9 + if small { 0.3 } else { 0.0 }
            && y < 0.9 + if small { 1.0 } else { 0.7 }
            && self.has_item_in_slot(&EquipmentSlot::CHEST).await
        {
            EquipmentSlot::CHEST
        } else if y >= 0.4
            && y < 0.4 + if small { 1.0 } else { 0.8 }
            && self.has_item_in_slot(&EquipmentSlot::LEGS).await
        {
            EquipmentSlot::LEGS
        } else if y >= 1.6 && self.has_item_in_slot(&EquipmentSlot::HEAD).await {
            EquipmentSlot::HEAD
        } else if !self.has_item_in_slot(&EquipmentSlot::MAIN_HAND).await
            && self.has_item_in_slot(&EquipmentSlot::OFF_HAND).await
        {
            EquipmentSlot::OFF_HAND
        } else {
            EquipmentSlot::MAIN_HAND
        }
    }

    async fn swap_item(
        &self,
        player: &Arc<Player>,
        slot: &EquipmentSlot,
        held: &mut ItemStack,
    ) -> bool {
        let equipped = self
            .living_entity
            .entity_equipment
            .lock()
            .await
            .get_or_insert(slot);
        let mut equipped = equipped.lock().await;
        if (!equipped.is_empty() && self.is_slot_masked(slot, 8))
            || (equipped.is_empty() && self.is_slot_masked(slot, 16))
        {
            return false;
        }
        if !Self::swap_held_item(
            &mut equipped,
            held,
            player.gamemode.load() == pumpkin_util::GameMode::Creative,
        ) {
            return false;
        }
        let updated = equipped.clone();
        drop(equipped);
        self.living_entity
            .send_equipment_changes(&[(slot.clone(), updated)]);
        true
    }

    async fn interact_at_location(
        &self,
        player: &Arc<Player>,
        held: &mut ItemStack,
        target_position: Option<Vector3<f64>>,
    ) -> bool {
        if self.is_marker() || held.item == &Item::NAME_TAG {
            return false;
        }
        if player.gamemode.load() == pumpkin_util::GameMode::Spectator {
            return true;
        }

        let item_slot = self.equipment_slot_for_item(held);
        if held.is_empty() {
            let clicked_slot = match target_position {
                Some(position) => self.clicked_slot(position).await,
                None => EquipmentSlot::MAIN_HAND,
            };
            let target_slot = if self.is_slot_disabled(&clicked_slot) {
                item_slot
            } else {
                clicked_slot
            };
            self.has_item_in_slot(&target_slot).await
                && self.swap_item(player, &target_slot, held).await
        } else if self.is_slot_disabled(&item_slot) {
            true
        } else {
            self.swap_item(player, &item_slot, held).await
        }
    }

    async fn break_and_drop_items(&self) {
        let entity = self.get_entity();
        let equipment = self.living_entity.entity_equipment.lock().await;
        let equipped_items: Vec<_> = equipment.equipment.values().cloned().collect();
        drop(equipment);

        for equipped_item in equipped_items {
            let mut stack = equipped_item.lock().await;
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

            nbt.put_bool("Invisible", self.is_invisible());
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

            if let Some(invisible) = nbt.get_bool("Invisible")
                && invisible
            {
                self.get_entity().set_invisible(invisible).await;
            }

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

    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move { self.interact_at_location(player, item_stack, None).await })
    }

    fn interact_at<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
        target_position: Option<Vector3<f64>>,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            self.interact_at_location(player, item_stack, target_position)
                .await
        })
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
                world.send_entity_status(entity, EntityStatus::ArmorstandWobble);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equips_one_item_from_a_survival_stack() {
        let mut equipped = ItemStack::EMPTY.clone();
        let mut held = ItemStack::new(3, &Item::IRON_HELMET);

        assert!(ArmorStandEntity::swap_held_item(
            &mut equipped,
            &mut held,
            false
        ));
        assert!(equipped.item == &Item::IRON_HELMET);
        assert_eq!(equipped.item_count, 1);
        assert_eq!(held.item_count, 2);
    }

    #[test]
    fn creative_equip_copies_one_item_without_consuming_held_stack() {
        let mut equipped = ItemStack::EMPTY.clone();
        let mut held = ItemStack::new(3, &Item::IRON_HELMET);

        assert!(ArmorStandEntity::swap_held_item(
            &mut equipped,
            &mut held,
            true
        ));
        assert!(equipped.item == &Item::IRON_HELMET);
        assert_eq!(equipped.item_count, 1);
        assert_eq!(held.item_count, 3);
    }

    #[test]
    fn refuses_to_replace_equipment_with_a_multi_item_stack() {
        let mut equipped = ItemStack::new(1, &Item::IRON_HELMET);
        let mut held = ItemStack::new(2, &Item::DIAMOND_HELMET);

        assert!(!ArmorStandEntity::swap_held_item(
            &mut equipped,
            &mut held,
            false
        ));
        assert!(equipped.item == &Item::IRON_HELMET);
        assert_eq!(held.item_count, 2);
    }

    #[test]
    fn disabled_slot_masks_use_equipment_slot_indices() {
        assert_eq!(ArmorStandEntity::slot_mask(&EquipmentSlot::HEAD, 0), 1 << 4);
        assert_eq!(
            ArmorStandEntity::slot_mask(&EquipmentSlot::HEAD, 8),
            1 << 12
        );
        assert_eq!(
            ArmorStandEntity::slot_mask(&EquipmentSlot::HEAD, 16),
            1 << 20
        );
        assert_eq!(
            ArmorStandEntity::slot_mask(&EquipmentSlot::OFF_HAND, 0),
            1 << 5
        );
    }
}
