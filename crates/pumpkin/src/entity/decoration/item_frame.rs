use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::server::Server;
use crate::world::game_event::{GameEventContext, emit_game_event};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockDirection;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::MapIdImpl;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockState};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_protocol::java::client::play::{CSetEntityMetadata, Metadata};
use pumpkin_util::GameMode;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

pub struct ItemFrameEntity {
    entity: Entity,
    item_stack: Mutex<ItemStack>,
    /// Rotation of the displayed item, always in `0..8`.
    rotation: AtomicU8,
    /// The direction the frame faces, stored as the vanilla 3D direction index.
    facing: AtomicU8,
    item_drop_chance: AtomicCell<f32>,
    invisible: AtomicBool,
    fixed: AtomicBool,
    /// `BlockAttachedEntity.checkInterval`: support is revalidated once every 100 ticks.
    check_interval: AtomicU32,
}

impl ItemFrameEntity {
    const DEFAULT_FACING: BlockDirection = BlockDirection::South;

    pub fn new(entity: Entity) -> Self {
        let facing = Self::DEFAULT_FACING.to_index();
        entity.data.store(i32::from(facing), Ordering::Relaxed);
        let frame = Self {
            entity,
            item_stack: Mutex::new(ItemStack::EMPTY.clone()),
            rotation: AtomicU8::new(0),
            facing: AtomicU8::new(facing),
            item_drop_chance: AtomicCell::new(1.0),
            invisible: AtomicBool::new(false),
            fixed: AtomicBool::new(false),
            check_interval: AtomicU32::new(0),
        };
        frame.recalculate_bounding_box(false);
        frame
    }

    pub const fn is_glow(&self) -> bool {
        self.entity.entity_type.id == EntityType::GLOW_ITEM_FRAME.id
    }

    pub const fn get_add_item_sound(&self) -> Sound { if self.is_glow() { Sound::EntityGlowItemFrameAddItem } else { Sound::EntityItemFrameAddItem } }
    pub const fn get_remove_item_sound(&self) -> Sound { if self.is_glow() { Sound::EntityGlowItemFrameRemoveItem } else { Sound::EntityItemFrameRemoveItem } }
    pub const fn get_rotate_item_sound(&self) -> Sound { if self.is_glow() { Sound::EntityGlowItemFrameRotateItem } else { Sound::EntityItemFrameRotateItem } }
    pub const fn get_break_sound(&self) -> Sound { if self.is_glow() { Sound::EntityGlowItemFrameBreak } else { Sound::EntityItemFrameBreak } }
    pub const fn get_place_sound(&self) -> Sound { if self.is_glow() { Sound::EntityGlowItemFramePlace } else { Sound::EntityItemFramePlace } }

    pub fn get_facing(&self) -> BlockDirection {
        BlockDirection::from_index(self.facing.load(Ordering::Relaxed)).unwrap_or(Self::DEFAULT_FACING)
    }

    /// `ItemFrame.createBoundingBox` (`ItemFrame.java:114-123`). Maps make the frame one block
    /// wide and tall; ordinary frames are three quarters of a block in both axes.
    fn frame_box(position: Vector3<f64>, facing: BlockDirection, has_framed_map: bool) -> BoundingBox {
        let width = if has_framed_map { 1.0 } else { 0.75 };
        let half_width = width / 2.0;
        let (half_x, half_y, half_z) = match facing {
            BlockDirection::North | BlockDirection::South => (half_width, half_width, 0.03125),
            BlockDirection::West | BlockDirection::East => (0.03125, half_width, half_width),
            BlockDirection::Down | BlockDirection::Up => (half_width, 0.03125, half_width),
        };
        BoundingBox::new(
            Vector3::new(position.x - half_x, position.y - half_y, position.z - half_z),
            Vector3::new(position.x + half_x, position.y + half_y, position.z + half_z),
        )
    }

    /// Placement and support checks use the empty-frame box (`ItemFrame.java:110-112`).
    pub(crate) fn pop_box(position: Vector3<f64>, facing: BlockDirection) -> BoundingBox {
        Self::frame_box(position, facing, false)
    }

    fn recalculate_bounding_box(&self, has_framed_map: bool) {
        self.entity.bounding_box.store(Self::frame_box(self.entity.pos.load(), self.get_facing(), has_framed_map));
    }

    pub fn set_facing(&self, facing: BlockDirection) {
        let index = facing.to_index();
        self.facing.store(index, Ordering::Relaxed);
        self.entity.data.store(i32::from(index), Ordering::Relaxed);
        // Direction.get2DDataValue() * 90 (Direction.java:33-37).
        let (yaw, pitch) = match facing {
            BlockDirection::West => (90.0, 0.0), BlockDirection::North => (180.0, 0.0),
            BlockDirection::East => (270.0, 0.0), BlockDirection::Up => (0.0, -90.0),
            BlockDirection::Down => (0.0, 90.0), BlockDirection::South => (0.0, 0.0),
        };
        self.entity.set_rotation(yaw, pitch);
        let has_framed_map = self.item_stack.try_lock().is_ok_and(|stack| stack.get_data_component::<MapIdImpl>().is_some());
        self.recalculate_bounding_box(has_framed_map);
    }

    fn play_frame_sound(&self, plain: Sound, glow: Sound) {
        self.entity.world.load().play_sound(if self.is_glow() { glow } else { plain }, SoundCategory::Blocks, &self.entity.pos.load());
    }

    fn update_output_signal(&self) {
        // Vanilla `level.updateNeighbourForOutputSignal(pos, Blocks.AIR)` updates comparators.
        let world = self.entity.world.load_full();
        let position = self.entity.block_pos.load();
        tokio::spawn(async move { world.update_comparators(&position, &Block::AIR).await; });
    }

    fn sync_item(&self, stack: &ItemStack) {
        self.entity.send_meta_data(&[Metadata::new(pumpkin_data::tracked_data::item_frame::ITEM, &ItemStackSerializer::from(stack.clone()))], None);
    }

    fn sync_rotation(&self, rotation: u8) {
        self.entity.send_meta_data(&[Metadata::new(pumpkin_data::tracked_data::item_frame::ROTATION, i32::from(rotation))], None);
    }

    pub fn get_item(&self) -> ItemStack { self.item_stack.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone() }

    pub fn set_item(&self, mut item_stack: ItemStack, update_neighbours: bool) {
        if !item_stack.is_empty() { item_stack.item_count = 1; }
        let play_sound = !item_stack.is_empty();
        *self.item_stack.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = item_stack.clone();
        // `ItemFrame.onItemChanged` (`ItemFrame.java:307-309`) recalculates the hitbox for maps.
        self.recalculate_bounding_box(item_stack.get_data_component::<MapIdImpl>().is_some());
        self.sync_item(&item_stack);
        if play_sound { self.entity.play_sound(self.get_add_item_sound()); }
        if update_neighbours { self.entity.world.load().update_neighbors(&self.entity.block_pos.load(), None); }
        self.update_output_signal();
    }

    pub fn get_rotation(&self) -> u8 { self.rotation.load(Ordering::Relaxed) % 8 }

    pub fn set_rotation(&self, rotation: u8, update_neighbours: bool) {
        let rotation = rotation % 8;
        self.rotation.store(rotation, Ordering::Relaxed);
        self.sync_rotation(rotation);
        if update_neighbours { self.entity.world.load().update_neighbors(&self.entity.block_pos.load(), None); }
        self.update_output_signal();
    }

    pub fn get_drop_chance(&self) -> f32 { self.item_drop_chance.load() }
    pub fn set_drop_chance(&self, chance: f32) { self.item_drop_chance.store(chance); }
    pub fn is_fixed(&self) -> bool { self.fixed.load(Ordering::Relaxed) }
    pub fn set_fixed(&self, fixed: bool) { self.fixed.store(fixed, Ordering::Relaxed); }
    pub fn is_invisible(&self) -> bool { self.invisible.load(Ordering::Relaxed) }
    pub fn set_invisible(&self, invisible: bool) { self.invisible.store(invisible, Ordering::Relaxed); }

    pub fn get_frame_item_stack(&self) -> ItemStack {
        if self.is_glow() { ItemStack::new(1, &Item::GLOW_ITEM_FRAME) } else { ItemStack::new(1, &Item::ITEM_FRAME) }
    }

    pub fn get_frame_item_stack_with_data(&self) -> ItemStack {
        let mut stack = self.get_frame_item_stack();
        if let Some(custom_name) = self.entity.custom_name.load().as_ref().clone() { stack.set_custom_name(custom_name.to_pretty_console()); }
        stack
    }

    pub fn get_pick_result(&self) -> ItemStack { let stack = self.get_item(); if stack.is_empty() { self.get_frame_item_stack_with_data() } else { stack } }

    /// Vanilla: `getItem().isEmpty() ? 0 : getRotation() % 8 + 1`.
    pub fn get_analog_output(&self) -> u8 { if self.get_item().is_empty() { 0 } else { self.rotation.load(Ordering::Relaxed) % 8 + 1 } }

    /// Vanilla `ItemFrame.survives`: fixed frames survive; otherwise check the pop box, other
    /// same-direction hanging entities, and the support block.
    pub(crate) fn survives(&self) -> bool {
        if self.fixed.load(Ordering::Relaxed) { return true; }
        let facing = self.get_facing();
        let world = self.entity.world.load();
        let pop_box = Self::pop_box(self.entity.pos.load(), facing);
        if !world.is_space_empty(pop_box) { return false; }
        if world.get_entities_at_box(&pop_box).iter().any(|entity| {
            let other = entity.get_entity();
            let hanging = other.entity_type == &EntityType::ITEM_FRAME || other.entity_type == &EntityType::GLOW_ITEM_FRAME || other.entity_type == &EntityType::PAINTING;
            other.entity_id != self.entity.entity_id && hanging && other.data.load(Ordering::Relaxed) == i32::from(facing.to_index())
        }) { return false; }
        let support = self.entity.block_pos.load().offset(facing.opposite().to_offset());
        let Some(state_id) = world.get_block_state_id_if_loaded(&support) else { return true; };
        let (block, state) = BlockState::from_id_with_block(state_id);
        state.is_solid() || (facing.is_horizontal() && (block == &Block::REPEATER || block == &Block::COMPARATOR))
    }

    pub fn drop_item(&self, caused_by: Option<&dyn EntityBase>, with_frame: bool) {
        if self.is_fixed() { return; }
        let item_stack = self.get_item();
        *self.item_stack.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = ItemStack::EMPTY.clone();
        self.sync_item(&ItemStack::EMPTY);
        self.update_output_signal();
        let creative = caused_by.is_some_and(|s| s.cast_any().downcast_ref::<Player>().is_some_and(Player::is_creative));
        if creative { return; }
        let world = self.entity.world.load();
        if !world.level_info.load().game_rules.entity_drops { return; }
        let pos = self.entity.block_pos.load();
        if with_frame { world.drop_stack(&pos, self.get_frame_item_stack_with_data()); }
        if !item_stack.is_empty() && rand::random::<f32>() < self.item_drop_chance.load() { world.drop_stack(&pos, item_stack); }
    }

    fn emit_block_change(&self, causer: Option<Arc<dyn EntityBase>>) {
        let world = self.entity.world.load_full();
        let position = self.entity.pos.load();
        tokio::spawn(async move { emit_game_event(&world, GameEvent::BlockChange, position, causer.map_or_else(GameEventContext::none, GameEventContext::of_entity)).await; });
    }
}

impl EntityBase for ItemFrameEntity {
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        let item = self.item_stack.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !item.is_empty() { let mut item_compound = NbtCompound::new(); item.write_item_stack(&mut item_compound); nbt.put_compound("Item", item_compound); }
        nbt.put_float("ItemDropChance", self.item_drop_chance.load());
        nbt.put_byte("ItemRotation", self.rotation.load(Ordering::Relaxed) as i8);
        nbt.put_byte("Facing", self.facing.load(Ordering::Relaxed) as i8);
        nbt.put_bool("Invisible", self.invisible.load(Ordering::Relaxed));
        nbt.put_bool("Fixed", self.fixed.load(Ordering::Relaxed));
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(item_compound) = nbt.get_compound("Item") && let Some(stack) = ItemStack::read_item_stack(item_compound) { *self.item_stack.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = stack; }
        self.rotation.store((nbt.get_byte("ItemRotation").unwrap_or(0) as u8) % 8, Ordering::Relaxed);
        let facing = nbt.get_byte("Facing").unwrap_or(0) as u8 % 6;
        self.facing.store(facing, Ordering::Relaxed);
        self.entity.data.store(i32::from(facing), Ordering::Relaxed);
        self.recalculate_bounding_box(self.get_item().get_data_component::<MapIdImpl>().is_some());
        self.item_drop_chance.store(nbt.get_float("ItemDropChance").unwrap_or(1.0));
        let invisible = nbt.get_bool("Invisible").unwrap_or(false);
        self.invisible.store(invisible, Ordering::Relaxed);
        self.entity.set_invisible(invisible);
        self.fixed.store(nbt.get_bool("Fixed").unwrap_or(false), Ordering::Relaxed);
    }

    fn get_entity(&self) -> &Entity { &self.entity }
    fn get_living_entity(&self) -> Option<&LivingEntity> { None }
    fn is_pickable(&self) -> bool { true }

    /// `ItemFrame.move` (`ItemFrame.java:140-145`): fixed frames ignore entity movement.
    fn move_entity(&self, caller: &dyn EntityBase, motion: Vector3<f64>) { if !self.fixed.load(Ordering::Relaxed) { self.entity.move_entity(caller, motion); } }

    fn init_data_tracker(&self) { self.sync_item(&self.get_item()); self.sync_rotation(self.get_rotation()); }

    fn send_java_spawn_packet(&self, client: &crate::net::java::JavaClient) {
        let spawn_packet = self.entity.create_spawn_packet();
        if let Ok(data) = client.serialize_packet(&spawn_packet) { client.try_enqueue_packet(data); }
        let ver = client.version.load();
        if ver >= CURRENT_MC_VERSION {
            let mut data = Vec::new();
            let meta_item = Metadata::new(pumpkin_data::tracked_data::item_frame::ITEM, ItemStackSerializer::from(self.get_item()));
            let meta_rot = Metadata::new(pumpkin_data::tracked_data::item_frame::ROTATION, self.get_rotation() as i32);
            if meta_item.write(&mut data, &ver).is_ok() && meta_rot.write(&mut data, &ver).is_ok() {
                data.push(255);
                let packet = CSetEntityMetadata::new(self.entity.entity_id.into(), data.into());
                if let Ok(packet) = client.serialize_packet(&packet) { client.try_enqueue_packet(packet); }
            }
        }
    }

    /// Vanilla `ItemFrame.interact`: an empty frame takes the held item; an occupied frame rotates.
    fn interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.is_fixed() { return false; }
        let causer = Some(player.clone() as Arc<dyn EntityBase>);
        if !self.get_item().is_empty() {
            self.set_rotation(self.get_rotation().wrapping_add(1), true);
            self.entity.play_sound(self.get_rotate_item_sound());
            self.emit_block_change(causer);
            return true;
        }
        if item_stack.is_empty() || self.entity.removed.load(Ordering::Relaxed) { return false; }
        self.set_item(item_stack.clone(), true);
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        self.emit_block_change(causer);
        true
    }

    fn damage_with_context(&self, _caller: &dyn EntityBase, _amount: f32, damage_type: DamageType, _position: Option<Vector3<f64>>, source: Option<&dyn EntityBase>, cause: Option<&dyn EntityBase>) -> bool {
        let causer = cause.or(source);
        let creative = causer.and_then(EntityBase::get_player).is_some_and(|player| player.gamemode.load() == GameMode::Creative);
        let bypasses_invuln = damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY);
        if self.is_fixed() && !bypasses_invuln && !creative { return false; }
        if !self.is_fixed() && self.entity.is_invulnerable_to(&damage_type) { return false; }
        let holds_item = !self.get_item().is_empty();
        let is_explosion = damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION);
        if !self.is_fixed() && !is_explosion && holds_item {
            self.drop_item(causer, false);
            self.entity.play_sound(self.get_remove_item_sound());
        } else {
            self.drop_item(causer, true);
            self.entity.play_sound(self.get_break_sound());
            self.entity.remove();
        }
        self.emit_block_change(None);
        true
    }

    /// `BlockAttachedEntity.tick`: drop the frame once its support is gone.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.entity.tick(caller, server);
        if self.check_interval.fetch_add(1, Ordering::Relaxed) < 100 { return; }
        self.check_interval.store(0, Ordering::Relaxed);
        if self.entity.removed.load(Ordering::Relaxed) || self.survives() { return; }
        self.drop_item(None, true);
        self.entity.play_sound(self.get_break_sound());
        self.entity.remove();
        self.emit_block_change(None);
    }

    fn cast_any(&self) -> &dyn std::any::Any { self }
}
