use pumpkin_data::data_component_impl::IDSetContent;
use pumpkin_data::tag::Taggable;
use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use pumpkin_data::effect::StatusEffect;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::block::entities::{BlockEntity, PropertyDelegate};
use crate::world::World;
use pumpkin_world::inventory::{Clearable, Inventory};

pub struct BeaconBlockEntity {
    pub position: BlockPos,
    pub primary_effect: AtomicI32,
    pub secondary_effect: AtomicI32,
    pub levels: AtomicI32,
    pub dirty: AtomicBool,
    pub payment: Arc<Mutex<ItemStack>>,

    // Vanilla Parity Fields
    pub custom_name: Mutex<Option<String>>,
    pub lock_key: Mutex<Option<String>>,
    pub last_check_y: AtomicI32,
}

impl BeaconBlockEntity {
    pub const ID: &'static str = "minecraft:beacon";

    pub const DATA_LEVELS: usize = 0;
    pub const DATA_PRIMARY: usize = 1;
    pub const DATA_SECONDARY: usize = 2;
    pub const NUM_DATA_VALUES: usize = 3;

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            primary_effect: AtomicI32::new(-1),
            secondary_effect: AtomicI32::new(-1),
            levels: AtomicI32::new(0),
            dirty: AtomicBool::new(false),
            payment: Arc::new(Mutex::new(ItemStack::EMPTY.clone())),
            custom_name: Mutex::new(None),
            lock_key: Mutex::new(None),
            last_check_y: AtomicI32::new(position.0.y - 1),
        }
    }

    /// Scans straight up from the beacon for an opaque, non-bedrock block.
    fn beam_clear(&self, world: &World) -> bool {
        let top_y = world.dimension.min_y + world.dimension.height;
        let mut pos = self.position.up();
        while pos.0.y < top_y {
            let block = world.get_block(&pos);
            if block.id != pumpkin_data::Block::BEDROCK.id {
                let state = world.get_block_state(&pos);
                if state.opacity >= 15 {
                    return false;
                }
            }
            pos = pos.up();
        }
        true
    }

    /// Replicates Java's `updateBase` logic.
    pub fn update_base(&self, world: &Arc<World>) -> i32 {
        let x = self.position.0.x;
        let y = self.position.0.y;
        let z = self.position.0.z;
        let mut levels = 0;

        for step in 1..=4 {
            let layer_y = y - step;
            if layer_y < world.dimension.min_y {
                break;
            }
            let mut valid = true;
            for dx in -step..=step {
                for dz in -step..=step {
                    let pos = BlockPos::new(x + dx, layer_y, z + dz);
                    let block = world.get_block(&pos);
                    if !block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_BEACON_BASE_BLOCKS) {
                        valid = false;
                        break;
                    }
                }
                if !valid {
                    break;
                }
            }
            if !valid {
                break;
            }
            levels = step;
        }
        levels
    }

    /// Replicates Java's `applyEffects` bounding-box and duration mapping.
    pub fn apply_effects(&self, world: &Arc<World>, levels: i32) {
        if levels <= 0 {
            return;
        }
        let primary_id = self.primary_effect.load(Ordering::Relaxed);
        let secondary_id = self.secondary_effect.load(Ordering::Relaxed);
        // -1 is the "no effect" sentinel; effect ids are 0-based, so Speed is valid at 0.
        if primary_id < 0 {
            return;
        }

        let primary_effect = StatusEffect::from_id(primary_id as u16);
        let secondary_effect = if secondary_id >= 0 {
            StatusEffect::from_id(secondary_id as u16)
        } else {
            None
        };
        let range = f64::from(levels * 10 + 10);
        let pos = self.position.0.to_f64();
        let bounding_box = BoundingBox::new(pos, pos.add_raw(1.0, 1.0, 1.0))
            .expand(range, range, range)
            .expand_towards(0.0, world.dimension.height as f64, 0.0);
        let duration_ticks = (9 + levels * 2) * 20;
        let base_amp = i32::from(levels >= 4 && primary_id == secondary_id);

        for player in world.get_players_at_box(&bounding_box) {
            if let Some(effect) = primary_effect {
                player.add_effect(pumpkin_data::potion::Effect {
                    effect_type: effect,
                    duration: duration_ticks,
                    amplifier: base_amp as u8,
                    ambient: true,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                });
            }
            if levels >= 4
                && primary_id != secondary_id
                && let Some(effect) = secondary_effect
            {
                player.add_effect(pumpkin_data::potion::Effect {
                    effect_type: effect,
                    duration: duration_ticks,
                    amplifier: 0,
                    ambient: true,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                });
            }
        }
    }

    /// Vanilla `BeaconBlockEntity.getRequiredLevelsFor`: the pyramid tier an effect needs.
    const fn required_level(effect: Option<&'static StatusEffect>) -> i32 {
        match effect {
            None => 0,
            Some(e) if e.id == StatusEffect::SPEED.id || e.id == StatusEffect::HASTE.id => 1,
            Some(e)
                if e.id == StatusEffect::RESISTANCE.id || e.id == StatusEffect::JUMP_BOOST.id =>
            {
                2
            }
            Some(e) if e.id == StatusEffect::STRENGTH.id => 3,
            Some(e) if e.id == StatusEffect::REGENERATION.id => 4,
            Some(_) => i32::MAX,
        }
    }

    /// Vanilla `BeaconBlockEntity.validateEffects`.
    pub(crate) fn validate_effects(
        primary: Option<&'static StatusEffect>,
        secondary: Option<&'static StatusEffect>,
        levels: i32,
    ) -> bool {
        if secondary.is_some() && levels < 4 {
            return false;
        }
        let primary_level = Self::required_level(primary);
        let secondary_level = Self::required_level(secondary);
        if primary_level > levels || secondary_level > levels || primary_level >= 4 {
            return false;
        }
        secondary_level == 0
            || secondary_level >= 4
            || primary.map(|e| e.id) == secondary.map(|e| e.id)
    }

    /// Vanilla `BeaconMenu.updateEffects`: validates effects and consumes one payment item.
    pub fn update_effects(
        &self,
        world: &Arc<World>,
        primary: Option<i32>,
        secondary: Option<i32>,
    ) -> bool {
        let mut payment = self
            .payment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if payment.is_empty() {
            return false;
        }
        let levels = self.levels.load(Ordering::Relaxed);
        let primary_effect = primary.and_then(|id| StatusEffect::from_id(id as u16));
        let secondary_effect = secondary.and_then(|id| StatusEffect::from_id(id as u16));
        if !Self::validate_effects(primary_effect, secondary_effect, levels) {
            return false;
        }
        self.primary_effect.store(
            primary_effect.map_or(-1, |e| i32::from(e.id)),
            Ordering::Relaxed,
        );
        self.secondary_effect.store(
            secondary_effect.map_or(-1, |e| i32::from(e.id)),
            Ordering::Relaxed,
        );
        payment.decrement(1);
        drop(payment);
        self.mark_dirty();
        let pos = Vector3::new(
            self.position.0.x as f64 + 0.5,
            self.position.0.y as f64 + 0.5,
            self.position.0.z as f64 + 0.5,
        );
        world.play_sound(Sound::BlockBeaconPowerSelect, SoundCategory::Blocks, &pos);
        true
    }
}

impl BlockEntity for BeaconBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn on_block_replaced(self: Arc<Self>, world: &Arc<World>, position: &BlockPos) {
        let sound_position = Vector3::new(
            position.0.x as f64 + 0.5,
            position.0.y as f64 + 0.5,
            position.0.z as f64 + 0.5,
        );
        world.play_sound(
            Sound::BlockBeaconDeactivate,
            SoundCategory::Blocks,
            &sound_position,
        );
        if let Some(inventory) = self.get_inventory() {
            world.scatter_inventory(position, &inventory);
        }
    }

    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self {
        Self {
            position,
            primary_effect: AtomicI32::new(nbt.get_int("primary_effect").unwrap_or(-1)),
            secondary_effect: AtomicI32::new(nbt.get_int("secondary_effect").unwrap_or(-1)),
            levels: AtomicI32::new(nbt.get_int("Levels").unwrap_or(0)),
            dirty: AtomicBool::new(false),
            payment: Arc::new(Mutex::new(ItemStack::EMPTY.clone())),
            custom_name: Mutex::new(
                nbt.get_string("CustomName")
                    .map(std::string::ToString::to_string),
            ),
            lock_key: Mutex::new(
                nbt.get_string("Lock")
                    .map(std::string::ToString::to_string),
            ),
            last_check_y: AtomicI32::new(position.0.y - 1),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("primary_effect", self.primary_effect.load(Ordering::Relaxed));
        nbt.put_int("secondary_effect", self.secondary_effect.load(Ordering::Relaxed));
        nbt.put_int("Levels", self.levels.load(Ordering::Relaxed));
        if let Some(name) = &*self
            .custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            nbt.put_string("CustomName", name.clone());
        }
        if let Some(lock) = &*self
            .lock_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            nbt.put_string("Lock", lock.clone());
        }
    }

    fn tick(&self, world: &Arc<World>) {
        if world.get_time_of_day() % 80 == 0 {
            let levels = self.update_base(world);
            self.levels.store(levels, Ordering::Relaxed);
            if levels > 0 && self.beam_clear(world) {
                self.apply_effects(world, levels);
            }
        }
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        nbt.put_int("primary_effect", self.primary_effect.load(Ordering::Relaxed));
        nbt.put_int("secondary_effect", self.secondary_effect.load(Ordering::Relaxed));
        nbt.put_int("Levels", self.levels.load(Ordering::Relaxed));
        if let Ok(name) = self.custom_name.try_lock()
            && let Some(name) = &*name
        {
            nbt.put_string("CustomName", name.clone());
        }
        if let Ok(lock) = self.lock_key.try_lock()
            && let Some(lock) = &*lock
        {
            nbt.put_string("Lock", lock.clone());
        }
        Some(nbt)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn get_inventory(self: Arc<Self>) -> Option<Arc<dyn Inventory>> {
        Some(self as Arc<dyn Inventory>)
    }

    fn to_property_delegate(self: Arc<Self>) -> Option<Arc<dyn PropertyDelegate>> {
        Some(self as Arc<dyn PropertyDelegate>)
    }
}

/// Vanilla's `BeaconMenu.encodeEffect`: effect id plus one, with 0 meaning no effect.
const fn encode_effect(id: i32) -> i32 {
    if id < 0 { 0 } else { id + 1 }
}

impl PropertyDelegate for BeaconBlockEntity {
    fn get_property(&self, index: i32) -> i32 {
        match index as usize {
            Self::DATA_LEVELS => self.levels.load(Ordering::Relaxed),
            Self::DATA_PRIMARY => encode_effect(self.primary_effect.load(Ordering::Relaxed)),
            Self::DATA_SECONDARY => encode_effect(self.secondary_effect.load(Ordering::Relaxed)),
            _ => 0,
        }
    }

    fn set_property(&self, _index: i32, _value: i32) {}

    fn get_properties_size(&self) -> i32 {
        Self::NUM_DATA_VALUES as i32
    }
}

impl Inventory for BeaconBlockEntity {
    fn size(&self) -> usize {
        1
    }

    fn is_empty(&self) -> bool {
        self.payment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    fn get_stack(&self, slot: usize) -> ItemStack {
        if slot == 0 {
            self.payment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        } else {
            ItemStack::EMPTY.clone()
        }
    }

    fn remove_stack(&self, slot: usize) -> ItemStack {
        if slot == 0 {
            let mut removed = ItemStack::EMPTY.clone();
            let mut payment = self
                .payment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::swap(&mut removed, &mut *payment);
            self.mark_dirty();
            removed
        } else {
            ItemStack::EMPTY.clone()
        }
    }

    fn remove_stack_specific(&self, slot: usize, amount: u8) -> ItemStack {
        if slot == 0 {
            let mut payment = self
                .payment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if payment.is_empty() {
                return ItemStack::EMPTY.clone();
            }
            let result = payment.split(amount);
            self.mark_dirty();
            result
        } else {
            ItemStack::EMPTY.clone()
        }
    }

    fn set_stack(&self, slot: usize, stack: ItemStack) {
        if slot == 0 {
            *self
                .payment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = stack;
            self.mark_dirty();
        }
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Clearable for BeaconBlockEntity {
    fn clear(&self) {
        if let Ok(mut payment) = self.payment.try_lock() {
            *payment = ItemStack::EMPTY.clone();
        }
    }
}
