use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU8, Ordering};

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture, living::LivingEntity,
};
use crate::world::game_event::{GameEventContext, emit_game_event};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{
    attributes::Attributes,
    damage::DamageType,
    data_component_impl::{EquipmentSlot, EquipmentType},
    entity::EntityStatus,
    item::Item,
    particle::Particle,
    sound::{Sound, SoundCategory},
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::{
    Hand,
    math::{euler_angle::EulerAngle, vector3::Vector3},
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

    fn is_slot_locked(&self, slot: &EquipmentSlot, offset: i32) -> bool {
        let disabled_slots = self.disabled_slots.load(Ordering::Relaxed);
        let slot_bit = 1 << slot.get_offset_entity_slot_id(offset);
        (disabled_slots & slot_bit) != 0
    }

    fn equipment_slot_for_item(&self, item_stack: &ItemStack) -> &'static EquipmentSlot {
        item_stack
            .get_data_component::<pumpkin_data::data_component_impl::EquippableImpl>()
            .map_or(&EquipmentSlot::MAIN_HAND, |equippable| {
                if self.can_use_slot(equippable.slot) {
                    equippable.slot
                } else {
                    &EquipmentSlot::MAIN_HAND
                }
            })
    }

    async fn has_item_in_slot(&self, slot: &EquipmentSlot) -> bool {
        let item = self.living_entity.entity_equipment.lock().await.get(slot);
        !item.lock().await.is_empty()
    }

    async fn clicked_slot(&self, location: Option<Vector3<f64>>) -> EquipmentSlot {
        let Some(location) = location else {
            return EquipmentSlot::MAIN_HAND;
        };
        let small = self.is_small();
        let scale = self.living_entity.get_attribute_value(&Attributes::SCALE);
        let click_y = armor_stand_click_y(location.y, scale, small);

        if click_y >= 0.1
            && click_y < 0.1 + if small { 0.8 } else { 0.45 }
            && self.has_item_in_slot(&EquipmentSlot::FEET).await
        {
            EquipmentSlot::FEET
        } else if click_y >= 0.9 + if small { 0.3 } else { 0.0 }
            && click_y < 0.9 + if small { 1.0 } else { 0.7 }
            && self.has_item_in_slot(&EquipmentSlot::CHEST).await
        {
            EquipmentSlot::CHEST
        } else if click_y >= 0.4
            && click_y < 0.4 + if small { 1.0 } else { 0.8 }
            && self.has_item_in_slot(&EquipmentSlot::LEGS).await
        {
            EquipmentSlot::LEGS
        } else if click_y >= 1.6 && self.has_item_in_slot(&EquipmentSlot::HEAD).await {
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
        player: &crate::entity::player::Player,
        slot: &EquipmentSlot,
        item_stack: &mut ItemStack,
    ) -> bool {
        let mut equipment = self.living_entity.entity_equipment.lock().await;
        let equipped = equipment.get(slot).lock().await.clone();
        if (!equipped.is_empty() && self.is_slot_locked(slot, 8))
            || (equipped.is_empty() && self.is_slot_locked(slot, 16))
        {
            return false;
        }

        let Some((new_equipped, new_hand)) =
            armor_stand_swap(player.is_creative(), item_stack, &equipped)
        else {
            return false;
        };
        equipment.put(slot, new_equipped.clone()).await;
        drop(equipment);

        *item_stack = new_hand;
        self.living_entity
            .send_equipment_changes(&[(slot.clone(), new_equipped)]);
        true
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

    fn interact_with_hand<'a>(
        &'a self,
        player: &'a std::sync::Arc<crate::entity::player::Player>,
        item_stack: &'a mut ItemStack,
        _hand: Hand,
        location: Option<Vector3<f64>>,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            if self.is_marker() || item_stack.item.id == Item::NAME_TAG.id {
                return false;
            }
            if player.gamemode.load() == pumpkin_util::GameMode::Spectator {
                return true;
            }

            let item_slot = self.equipment_slot_for_item(item_stack);
            if item_stack.is_empty() {
                let clicked_slot = self.clicked_slot(location).await;
                let target_slot = if self.is_slot_disabled(&clicked_slot) {
                    item_slot
                } else {
                    &clicked_slot
                };
                return self.has_item_in_slot(target_slot).await
                    && self.swap_item(player, target_slot, item_stack).await;
            }

            if self.is_slot_disabled(item_slot) {
                return false;
            }
            self.swap_item(player, item_slot, item_stack).await
        })
    }

    fn is_pickable(&self) -> bool {
        self.get_entity().is_alive() && !self.is_marker()
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

/// `ArmorStand.swapItem`: returns the stack to store in the stand and the stack
/// that remains in the player's interacting hand.
fn armor_stand_swap(
    player_is_creative: bool,
    held: &ItemStack,
    equipped: &ItemStack,
) -> Option<(ItemStack, ItemStack)> {
    if player_is_creative && equipped.is_empty() && !held.is_empty() {
        return Some((held.copy_with_count(1), held.clone()));
    }
    if held.is_empty() || held.item_count <= 1 {
        return Some((held.clone(), equipped.clone()));
    }
    if !equipped.is_empty() {
        return None;
    }

    let mut remaining = held.clone();
    let inserted = remaining.split(1);
    Some((inserted, remaining))
}

const fn armor_stand_click_y(location_y: f64, scale: f64, small: bool) -> f64 {
    location_y / (scale * if small { 0.5 } else { 1.0 })
}

#[cfg(test)]
mod tests {
    use super::{armor_stand_click_y, armor_stand_swap};
    use pumpkin_data::{item::Item, item_stack::ItemStack};

    #[test]
    fn survival_inserts_one_item_from_a_larger_stack() {
        let held = ItemStack::new(3, &Item::IRON_HELMET);
        let (equipped, remaining) = armor_stand_swap(false, &held, ItemStack::EMPTY).unwrap();

        assert_eq!(equipped.item.id, Item::IRON_HELMET.id);
        assert_eq!(equipped.item_count, 1);
        assert_eq!(remaining.item.id, Item::IRON_HELMET.id);
        assert_eq!(remaining.item_count, 2);
    }

    #[test]
    fn single_item_swaps_with_the_existing_equipment() {
        let held = ItemStack::new(1, &Item::IRON_HELMET);
        let equipped = ItemStack::new(1, &Item::CARVED_PUMPKIN);
        let (new_equipped, new_hand) = armor_stand_swap(false, &held, &equipped).unwrap();

        assert_eq!(new_equipped.item.id, Item::IRON_HELMET.id);
        assert_eq!(new_hand.item.id, Item::CARVED_PUMPKIN.id);
    }

    #[test]
    fn creative_copies_one_item_into_an_empty_slot() {
        let held = ItemStack::new(3, &Item::IRON_HELMET);
        let (equipped, remaining) = armor_stand_swap(true, &held, ItemStack::EMPTY).unwrap();

        assert_eq!(equipped.item_count, 1);
        assert_eq!(remaining.item_count, 3);
    }

    #[test]
    fn larger_stack_cannot_replace_existing_equipment() {
        let held = ItemStack::new(2, &Item::IRON_HELMET);
        let equipped = ItemStack::new(1, &Item::CARVED_PUMPKIN);

        assert!(armor_stand_swap(false, &held, &equipped).is_none());
    }

    #[test]
    fn click_height_uses_entity_and_baby_scale() {
        assert_eq!(armor_stand_click_y(0.4, 1.0, false), 0.4);
        assert_eq!(armor_stand_click_y(0.4, 1.0, true), 0.8);
        assert_eq!(armor_stand_click_y(0.8, 2.0, false), 0.4);
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
