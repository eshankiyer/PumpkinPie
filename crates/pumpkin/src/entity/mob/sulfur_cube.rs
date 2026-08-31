// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::{codec::var_int::VarInt, java::client::play::Metadata};
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase, NBTStorage, NbtFuture,
    ageable::{AgeableData, AgeableMob},
    ai::control::{Control, MoveControlTrait},
    ai::goal::{Controls, Goal, GoalFuture},
    item::ItemEntity,
    mob::{Mob, MobEntity},
    player::Player,
};
use crate::world::World;

/// SulfurCube.java: a peaceful, ageable `AbstractCubeMob`.
///
/// It swallows items into its `EquipmentSlot.BODY` slot. The swallowed item's
/// `SulfurCubeArchetype` (a data-driven registry keyed by item tags under
/// `sulfur_cube_archetype/*`) is meant to change its buoyancy, knockback response,
/// contact damage and push sounds, and can prime an explosive fuse identical to
/// Primed TNT.
///
/// That archetype registry drives brand new attributes (`bounciness`, `friction_modifier`,
/// `air_drag_modifier`, `explosion_knockback_resistance`) through the movement/physics
/// engine, none of which this codebase's movement code consumes yet. Wiring that up is a
/// physics-engine change, not an entity change, so it is deferred here: `explosion_data`
/// is always absent, `can_explode` is always `false`, and the swallowed item only changes
/// appearance/equipment state, not movement or explosion behavior. Bucket capture is handled by
/// the shared bucket item path, which maps sulfur cubes by entity type and stores only the data
/// components currently supported by Pumpkin.
pub struct SulfurCubeEntity {
    entity: Arc<MobEntity>,
    ageable_data: AgeableData,
    jump_delay: AtomicI32,
    target_yaw: AtomicCell<f32>,
    was_on_ground: AtomicBool,
    pub squish: AtomicCell<f32>,
    pub target_squish: AtomicCell<f32>,
    pub o_squish: AtomicCell<f32>,
    speed_modifier: AtomicCell<f64>,
    has_split: AtomicBool,
    pickup_timer: AtomicI32,
    // SulfurCube.java:87, 355-362: cooldown for push sounds, decremented by server AI.
    push_sound_cooldown: AtomicI32,
    fuse: AtomicI32,
    from_bucket: AtomicBool,
}

impl SulfurCubeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let cube = Self {
            entity: Arc::new(mob_entity),
            ageable_data: AgeableData::default(),
            jump_delay: AtomicI32::new(0),
            target_yaw: AtomicCell::new(0.0),
            was_on_ground: AtomicBool::new(false),
            squish: AtomicCell::new(0.0),
            target_squish: AtomicCell::new(0.0),
            o_squish: AtomicCell::new(0.0),
            speed_modifier: AtomicCell::new(0.0),
            has_split: AtomicBool::new(false),
            pickup_timer: AtomicI32::new(0),
            push_sound_cooldown: AtomicI32::new(0),
            fuse: AtomicI32::new(-1),
            from_bucket: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(cube);

        {
            let mut move_control = mob_arc.entity.move_control.lock().unwrap();
            *move_control = Box::new(SulfurCubeMoveControl::new(Arc::downgrade(&mob_arc)));

            let mut goal_selector = mob_arc.entity.goals_selector.lock().unwrap();
            goal_selector.add_goal(1, Box::new(SulfurCubeFloatGoal::new(mob_arc.clone())));
            goal_selector.add_goal(2, Box::new(SulfurCubeTemptGoal::new(mob_arc.clone())));
            goal_selector.add_goal(
                3,
                Box::new(SulfurCubeSearchForItemsGoal::new(mob_arc.clone())),
            );
            goal_selector.add_goal(
                4,
                Box::new(SulfurCubeRandomDirectionGoal::new(mob_arc.clone())),
            );
            goal_selector.add_goal(
                5,
                Box::new(SulfurCubeKeepOnJumpingGoal::new(mob_arc.clone())),
            );
        };

        mob_arc.set_size(2, true);

        mob_arc
    }

    pub fn set_size(&self, size: i32, update_health: bool) {
        let actual_size = size.clamp(1, 127);
        let entity = &self.entity.living_entity.entity;
        let size_changed = entity.data.swap(actual_size, Ordering::Relaxed) != actual_size;

        let living_entity = &self.entity.living_entity;
        living_entity.set_attribute_base(
            &Attributes::MAX_HEALTH,
            f64::from(Self::max_health_for_size(actual_size)),
        );
        living_entity.set_attribute_base(
            &Attributes::MOVEMENT_SPEED,
            f64::from(0.2 + 0.1 * actual_size as f32),
        );

        if update_health {
            let max_health = living_entity.get_attribute_value(&Attributes::MAX_HEALTH) as f32;
            living_entity.health.store(max_health);
        }

        let scaled_dimensions = EntityDimensions {
            width: entity.entity_type.dimension[0] * actual_size as f32,
            height: entity.entity_type.dimension[1] * actual_size as f32,
            eye_height: entity.entity_type.eye_height * actual_size as f32,
            fixed: false,
        };
        entity.base_dimension.store(scaled_dimensions);
        entity.entity_dimension.store(scaled_dimensions);

        let pos = entity.pos.load();
        let new_bb = BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &scaled_dimensions);
        entity.bounding_box.store(new_bb);

        // SulfurCube.java:678-683 (setSize): shrinking to size 1 forces the baby flag.
        if update_health && actual_size == 1 && !self.is_baby() {
            self.set_baby(true);
        }

        // Vanilla `AbstractCubeMob.setSize` writes the size into synched entity data, which
        // only broadcasts when the value actually changed. Without this the client keeps the
        // default size of 1 and renders/collides at the wrong scale.
        if size_changed {
            self.send_size_meta_data(actual_size);
        }
    }

    /// Broadcasts the cube-scoped size tracker. Split out so `mob_init_data_tracker` can
    /// publish the size at spawn time, when `set_size` ran before the entity had any viewers.
    fn send_size_meta_data(&self, size: i32) {
        self.entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::sulfur_cube::ID_SIZE,
                VarInt(size),
            )],
            None,
        );
    }

    pub fn get_size(&self) -> i32 {
        self.entity
            .living_entity
            .entity
            .data
            .load(Ordering::Relaxed)
    }

    pub fn is_tiny(&self) -> bool {
        self.get_size() <= 1
    }

    /// SulfurCube.java:133-147 (`getFuse`/`isPrimed`): expose the persisted fuse state through
    /// the same small queries used by split and interaction logic.
    pub fn get_fuse(&self) -> i32 {
        self.fuse.load(Ordering::Relaxed)
    }

    pub fn is_primed(&self) -> bool {
        self.get_fuse() >= 0
    }

    /// SulfurCube.java:159-161 (`setFromBucket`/`fromBucket`).
    pub fn set_from_bucket(&self, from_bucket: bool) {
        self.from_bucket.store(from_bucket, Ordering::Relaxed);
    }

    pub fn from_bucket(&self) -> bool {
        self.from_bucket.load(Ordering::Relaxed)
    }

    /// SulfurCube.java:928-930 (`canBePickedFromInside`): a sulfur cube carrying an item
    /// cannot be caught by an empty bucket.
    pub async fn can_be_picked_from_inside(&self) -> bool {
        !self.has_body_item().await
    }

    /// SulfurCube.java:669-675 (`setSpawnSize`): unlike the generic cube-mob random size
    /// roll, spawn size is only ever 1 (baby) or 2 (adult).
    pub fn set_spawn_size(&self) {
        if self.is_baby() {
            self.set_size(1, true);
        } else {
            self.set_size(2, true);
        }
    }

    pub(crate) async fn has_body_item(&self) -> bool {
        let equipment = self.entity.living_entity.entity_equipment.lock().await;
        let stack = equipment.get(&EquipmentSlot::BODY);
        !stack.is_empty()
    }

    pub(crate) async fn can_breathe_underwater(&self) -> bool {
        self.has_body_item().await
    }

    async fn can_hold_item(&self, item_stack: &ItemStack) -> bool {
        if self.is_baby() {
            return false;
        }
        !self.has_body_item().await && is_swallowable_item(item_stack)
    }

    /// SulfurCube.java:493-522 (`equipItem`).
    pub(crate) async fn equip_item(&self, item_stack: &ItemStack) -> bool {
        if self.is_baby() {
            return false;
        }

        let equipment = self.entity.living_entity.entity_equipment.clone();
        let body_stack = equipment.lock().await.get(&EquipmentSlot::BODY);
        if !body_stack.is_empty() && body_stack.item.id == item_stack.item.id {
            return false;
        }

        let previous = {
            let mut equipment = equipment.lock().await;
            equipment.put(&EquipmentSlot::BODY, item_stack.copy_with_count(1))
        };

        if !previous.is_empty() {
            let entity = &self.entity.living_entity.entity;
            let world = entity.world.load();
            world.drop_stack(&entity.block_pos.load(), previous).await;
        }

        let new_body = equipment.lock().await.get(&EquipmentSlot::BODY);
        self.entity
            .living_entity
            .send_equipment_changes(&[(EquipmentSlot::BODY, new_body)]);

        self.play_sound(Sound::EntitySulfurCubeAbsorb);
        true
    }

    /// SulfurCube.java:607-614 (`shear`).
    async fn shear(&self) {
        let equipment = self.entity.living_entity.entity_equipment.clone();
        let ejected = {
            let mut equipment = equipment.lock().await;
            equipment.put(&EquipmentSlot::BODY, ItemStack::EMPTY.clone())
        };

        if ejected.is_empty() {
            return;
        }

        let entity = &self.entity.living_entity.entity;
        let world = entity.world.load();
        world.drop_stack(&entity.block_pos.load(), ejected).await;

        self.entity
            .living_entity
            .send_equipment_changes(&[(EquipmentSlot::BODY, ItemStack::EMPTY.clone())]);

        self.play_sound(Sound::EntitySulfurCubeEject);
        self.pickup_timer.store(100, Ordering::Relaxed);
    }

    fn play_sound(&self, sound: Sound) {
        let entity = &self.entity.living_entity.entity;
        let world = entity.world.load();
        world.play_sound(sound, SoundCategory::Neutral, &entity.pos.load());
    }

    /// Mirrors `LivingEntity::hurt_sound`'s existing `EntityType::SLIME` special case
    /// (`living.rs`), which picks a size-dependent hurt sound instead of the generic
    /// `EntityType::hurt_sound` field.
    pub(crate) const fn hurt_sound_for_size(size: i32) -> Sound {
        if size <= 1 {
            Sound::EntitySmallSulfurCubeHurt
        } else {
            Sound::EntitySulfurCubeHurt
        }
    }

    fn get_squish_sound(&self) -> Sound {
        Self::squish_sound_for(
            self.get_size(),
            futures::executor::block_on(self.has_body_item()),
        )
    }

    /// SulfurCube.java:569-576 (`getSquishSound`): tiny cubes squish, while an adult carrying
    /// an item bounces instead of using the ordinary adult squish sound.
    const fn squish_sound_for(size: i32, has_body_item: bool) -> Sound {
        if size <= 1 {
            Sound::EntitySmallSulfurCubeSquish
        } else if has_body_item {
            Sound::EntitySulfurCubeBounce
        } else {
            Sound::EntitySulfurCubeSquish
        }
    }

    fn get_jump_sound(&self) -> Sound {
        if self.is_tiny() {
            Sound::EntitySmallSulfurCubeJump
        } else {
            Sound::EntitySulfurCubeJump
        }
    }

    fn get_sound_volume(&self) -> f32 {
        0.4 * self.get_size() as f32
    }

    fn get_sound_pitch(&self) -> f32 {
        let pitch_adjuster = if self.is_tiny() { 1.4 } else { 0.8 };
        (rand::random_range(0.0..1.0) - rand::random_range(0.0..1.0)) * 0.2 + 1.0 * pitch_adjuster
    }

    fn get_jump_delay() -> i32 {
        rand::random_range(10..30)
    }

    fn rot_lerp(start: f32, end: f32, max_step: f32) -> f32 {
        let mut diff = (end - start).rem_euclid(360.0);
        if diff > 180.0 {
            diff -= 360.0;
        }
        start + diff.clamp(-max_step, max_step)
    }

    /// SulfurCube.java:664-666 (`getSplitCount`): a fixed 2-way split, suppressed while
    /// primed (`isPrimed`, always false here since the archetype/explosion system is
    /// deferred).
    fn get_split_count(&self) -> i32 {
        Self::split_count_for_fuse(self.fuse.load(Ordering::Relaxed))
    }

    /// SulfurCube.java:918-920 (`setcubeMobHealth`): `4 * actualSize`.
    const fn max_health_for_size(size: i32) -> i32 {
        4 * size
    }

    /// SulfurCube.java:664-666 (`getSplitCount`).
    const fn split_count_for_fuse(fuse: i32) -> i32 {
        if fuse >= 0 { 0 } else { 2 }
    }

    /// Vanilla `SulfurCube.ageBoundaryReached` (`SulfurCube.java:706-710`) restores the
    /// adult cube size when a baby reaches age zero. The shared ageable clock updates age,
    /// but does not know this species-specific size rule.
    fn age_boundary_reached(&self) {
        if !self.is_baby() {
            self.set_size(2, true);
        }
    }

    /// SulfurCube.java:731-767 (`playerTouch`/`playerPush`): pushes an overlapping player or
    /// the player's root vehicle when the cube is carrying an item.
    async fn player_push(&self, player: &Player) {
        if !self.has_body_item().await {
            return;
        }

        let player_entity = player.get_entity();
        let player_is_passenger = player_entity.has_vehicle().await;
        let (pusher_position, pusher_height) = if player_is_passenger {
            let mut vehicle = player_entity.vehicle.lock().await.clone();
            let mut position = player_entity.pos.load();
            let mut height = player_entity.height();
            while let Some(current) = vehicle {
                let current_entity = current.get_entity();
                position = current_entity.pos.load();
                height = current_entity.height();
                // `vehicle` is moved out by this loop's own `while let` each iteration, so
                // there is nothing to `clone_from` into; this is a fresh assignment, not a
                // clone-over-existing-value.
                #[expect(clippy::assigning_clones)]
                {
                    vehicle = current_entity.vehicle.lock().await.clone();
                }
            }
            (position, height)
        } else {
            (player_entity.pos.load(), player_entity.height())
        };

        let entity = &self.entity.living_entity.entity;
        let cube_position = entity.pos.load();
        let cube_to_pusher = Vector3::new(
            cube_position.x - pusher_position.x,
            0.0,
            cube_position.z - pusher_position.z,
        );
        let cube_height = f64::from(entity.height());
        let pusher_bottom = pusher_position.y;
        let pusher_top = pusher_bottom + f64::from(pusher_height);

        if cube_to_pusher.horizontal_length() >= 1.3
            || pusher_bottom > cube_position.y + cube_height
            || pusher_top <= cube_position.y
        {
            return;
        }

        let knockback = (1.0
            - self
                .entity
                .living_entity
                .get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE))
        .max(0.0);
        let player_speed = (player_entity.velocity.load().length()
            * 2.0
            * if player_is_passenger { 0.16 } else { 0.3 })
        .clamp(0.0, 0.5);
        let push_velocity = player_push_velocity(
            cube_to_pusher,
            knockback,
            entity.on_ground.load(Ordering::Relaxed),
            player_speed,
        );

        if push_velocity.length_squared() > 0.2 * 0.2
            && self.push_sound_cooldown.load(Ordering::Relaxed) <= 0
        {
            self.push_sound_cooldown.store(10, Ordering::Relaxed);
            self.play_sound(Sound::EntitySulfurCubeRegularPush);
        }
        entity.add_velocity(push_velocity);
    }
}

fn is_swallowable_item(item_stack: &ItemStack) -> bool {
    item_stack
        .item
        .has_tag(&tag::Item::MINECRAFT_SULFUR_CUBE_SWALLOWABLE)
}

fn is_food_item(item_stack: &ItemStack) -> bool {
    item_stack
        .item
        .has_tag(&tag::Item::MINECRAFT_SULFUR_CUBE_FOOD)
}

/// SulfurCube.java:731-767 (`playerPush`): scales the normalized horizontal push
/// direction and applies the grounded vertical impulse before adding it to the cube.
fn player_push_velocity(
    cube_to_pusher: Vector3<f64>,
    knockback: f64,
    on_ground: bool,
    player_speed: f64,
) -> Vector3<f64> {
    let direction = cube_to_pusher.normalize();
    let scale = player_speed * knockback;
    Vector3::new(
        direction.x * scale,
        if on_ground { 0.3 * scale } else { 0.0 },
        direction.z * scale,
    )
}

impl AgeableMob for SulfurCubeEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl NBTStorage for SulfurCubeEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.entity.living_entity.write_nbt(nbt).await;
            nbt.put_int("Size", self.get_size() - 1);
            nbt.put_bool("wasOnGround", self.was_on_ground.load(Ordering::Relaxed));
            self.write_ageable_nbt(nbt);
            nbt.put_int("pickup_timer", self.pickup_timer.load(Ordering::Relaxed));
            nbt.put_bool("from_bucket", self.from_bucket());
            nbt.put_int("fuse", self.get_fuse());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.entity.living_entity.read_nbt_non_mut(nbt).await;
            self.set_size(nbt.get_int("Size").unwrap_or(0) + 1, false);
            self.was_on_ground.store(
                nbt.get_bool("wasOnGround").unwrap_or(false),
                Ordering::Relaxed,
            );
            self.read_ageable_nbt(nbt);
            self.pickup_timer
                .store(nbt.get_int("pickup_timer").unwrap_or(0), Ordering::Relaxed);
            self.set_from_bucket(nbt.get_bool("from_bucket").unwrap_or(false));
            self.fuse
                .store(nbt.get_int("fuse").unwrap_or(-1), Ordering::Relaxed);
        })
    }
}

impl Mob for SulfurCubeEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity
    }

    /// Vanilla `SulfurCube.isInvulnerableToPiercingWeapon` (`SulfurCube.java:923-925`) keeps
    /// an unprimed invulnerable cube out of piercing-weapon hits, but allows a primed cube.
    fn mob_is_invulnerable_to_piercing_weapon(&self) -> bool {
        self.entity
            .living_entity
            .entity
            .invulnerable
            .load(Ordering::Relaxed)
            && !self.is_primed()
    }

    fn light_level_dependent_magic_value(&self, _world: &World) -> f32 {
        1.0
    }

    /// `set_size` runs from `new` and from NBT load, both before the entity has any viewers,
    /// so its broadcast reaches nobody. This is the first point at which nearby players
    /// exist. Chains the baby flag from `Mob`'s default so overriding does not drop it.
    fn mob_init_data_tracker(&self) -> crate::entity::EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.send_size_meta_data(self.get_size());
            if self.is_baby() {
                self.entity.living_entity.entity.send_meta_data(
                    &[Metadata::new(tracked_data::sulfur_cube::BABY_ID, true)],
                    None,
                );
            }
        })
    }

    fn mob_tick<'a>(
        &'a self,
        _caller: &'a Arc<dyn EntityBase>,
    ) -> crate::entity::EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let was_baby = self.is_baby();
            self.ageable_ai_step();
            if was_baby && !self.is_baby() {
                self.age_boundary_reached();
            }

            self.o_squish.store(self.squish.load());
            self.squish
                .store(self.squish.load() + (self.target_squish.load() - self.squish.load()) * 0.5);

            if self.pickup_timer.load(Ordering::Relaxed) > 0 {
                self.pickup_timer.fetch_sub(1, Ordering::Relaxed);
            }

            // SulfurCube.java:355-362 (`customServerAiStep`): push sounds have a
            // server-side cooldown measured in ticks.
            if self.push_sound_cooldown.load(Ordering::Relaxed) > 0 {
                self.push_sound_cooldown.fetch_sub(1, Ordering::Relaxed);
            }

            let on_ground = self
                .entity
                .living_entity
                .entity
                .on_ground
                .load(Ordering::Relaxed);
            let was_on_ground = self.was_on_ground.load(Ordering::Relaxed);

            if on_ground && !was_on_ground {
                let world = self.entity.living_entity.entity.world.load();
                world.play_sound_fine(
                    self.get_squish_sound(),
                    SoundCategory::Neutral,
                    &self.entity.living_entity.entity.pos.load(),
                    self.get_sound_volume(),
                    ((rand::random_range(0.0..1.0) - rand::random_range(0.0..1.0)) * 0.2 + 1.0)
                        / 0.8,
                );

                self.target_squish.store(-0.5);
            } else if !on_ground && was_on_ground {
                self.target_squish.store(1.0);
            }

            self.was_on_ground.store(on_ground, Ordering::Relaxed);
            self.target_squish.store(self.target_squish.load() * 0.6);
            self.speed_modifier.store(0.0);
        })
    }

    fn mob_player_collision<'a>(
        &'a self,
        player: &'a Arc<Player>,
    ) -> crate::entity::EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            // SulfurCube.java:731-767 (`playerTouch`/`playerPush`): a cube carrying
            // an item pushes an overlapping player or the player's root vehicle.
            self.player_push(player).await;
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> crate::entity::EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            if self.is_baby() {
                if is_food_item(item_stack) && self.can_age_up() {
                    let age = self.get_age();
                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                    self.age_up(Self::get_speed_up_seconds_when_feeding(-age), true);
                    self.age_boundary_reached();
                    self.play_sound(Sound::EntitySmallSulfurCubeEat);
                    return true;
                }
                return self
                    .entity
                    .mob_interact(player, item_stack, self.can_be_leashed())
                    .await;
            }

            if item_stack.item.id == Item::SHEARS.id && self.has_body_item().await {
                self.shear().await;
                return true;
            }

            if self.can_hold_item(item_stack).await {
                let equipped = self.equip_item(item_stack).await;
                if equipped {
                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                }
                return equipped;
            }

            self.entity
                .mob_interact(player, item_stack, self.can_be_leashed())
                .await
        })
    }

    fn post_tick(&self) -> crate::entity::EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            if self.entity.living_entity.dead.load(Ordering::Relaxed)
                && self.get_size() > 1
                && self
                    .has_split
                    .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                let size = self.get_size();
                let world = self.entity.living_entity.entity.world.load();
                let pos = self.entity.living_entity.entity.pos.load();
                let half_size = size / 2;
                let count = self.get_split_count();

                let width = self
                    .entity
                    .living_entity
                    .entity
                    .entity_dimension
                    .load()
                    .width;
                let xz_offset = width / 2.0;

                for i in 0..count {
                    let xd = ((i % 2) as f32 - 0.5) * xz_offset;
                    let zd = ((i / 2) as f32 - 0.5) * xz_offset;

                    let new_pos = Vector3::new(pos.x + xd as f64, pos.y + 0.5, pos.z + zd as f64);
                    let new_entity = Entity::new(world.clone(), new_pos, &EntityType::SULFUR_CUBE);
                    let cube = Self::new(new_entity);
                    cube.set_size(half_size, true);
                    cube.set_baby(true);
                    cube.entity
                        .living_entity
                        .entity
                        .yaw
                        .store(rand::random_range(0.0..360.0));
                    world.spawn_entity(cube).await;
                }
            }
        })
    }
}

pub struct SulfurCubeMoveControl {
    cube: Weak<SulfurCubeEntity>,
}

impl SulfurCubeMoveControl {
    #[must_use]
    pub const fn new(cube: Weak<SulfurCubeEntity>) -> Self {
        Self { cube }
    }
}

impl Control for SulfurCubeMoveControl {}

impl MoveControlTrait for SulfurCubeMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(cube) = self.cube.upgrade() else {
            return;
        };

        // SulfurCube.java:949-960 (`SulfurCubeMobMoveControl`): movement is suppressed
        // entirely while the body slot holds a swallowed item.
        if futures::executor::block_on(cube.has_body_item()) {
            return;
        }

        let mob_entity = mob.get_mob_entity();
        let living_entity = &mob_entity.living_entity;
        let entity = &living_entity.entity;

        let current_yaw = entity.yaw.load();
        let new_yaw = SulfurCubeEntity::rot_lerp(current_yaw, cube.target_yaw.load(), 90.0);
        entity.yaw.store(new_yaw);
        entity.head_yaw.store(new_yaw);
        entity.body_yaw.store(new_yaw);

        // the slime-style move control this mirrors calls setSpeed(speedModifier * MOVEMENT_SPEED) like the
        // base MoveControl, so the forward input carries the attribute too.
        let speed_modifier = cube.speed_modifier.load();
        let scaled_speed = living_entity.speed_for_modifier(speed_modifier);
        let mut movement_input = Vector3::new(0.0, 0.0, 0.0);

        let on_ground = entity.on_ground.load(Ordering::Relaxed);

        if on_ground {
            if speed_modifier > 0.0 {
                let current_delay = cube.jump_delay.load(Ordering::Relaxed);
                if current_delay <= 0 {
                    let next_delay = SulfurCubeEntity::get_jump_delay();
                    cube.jump_delay.store(next_delay, Ordering::Relaxed);
                    mob_entity.jump_requested.store(true, Ordering::SeqCst);
                    let world = entity.world.load();
                    world.play_sound_fine(
                        cube.get_jump_sound(),
                        SoundCategory::Neutral,
                        &entity.pos.load(),
                        cube.get_sound_volume(),
                        cube.get_sound_pitch(),
                    );
                    movement_input.z = scaled_speed;
                } else {
                    cube.jump_delay.store(current_delay - 1, Ordering::Relaxed);
                }
            }
        } else if speed_modifier > 0.0 {
            movement_input.z = scaled_speed;
        }
        living_entity.speed.store(movement_input.z);
        living_entity.movement_input.store(movement_input);
    }
}

pub struct SulfurCubeFloatGoal {
    cube: Arc<SulfurCubeEntity>,
}

impl SulfurCubeFloatGoal {
    pub const fn new(cube: Arc<SulfurCubeEntity>) -> Self {
        Self { cube }
    }
}

impl Goal for SulfurCubeFloatGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let entity = &self.cube.entity.living_entity.entity;
            entity.touching_water.load(Ordering::Relaxed)
                || entity.touching_lava.load(Ordering::Relaxed)
        })
    }

    fn tick<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if rand::random_range(0.0..1.0) < 0.8 {
                self.cube
                    .entity
                    .jump_requested
                    .store(true, Ordering::SeqCst);
            }
            self.cube.speed_modifier.store(1.2);
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::JUMP | Controls::MOVE
    }
}

pub struct SulfurCubeRandomDirectionGoal {
    cube: Arc<SulfurCubeEntity>,
    chosen_degrees: f32,
    next_randomize_time: i32,
}

impl SulfurCubeRandomDirectionGoal {
    pub const fn new(cube: Arc<SulfurCubeEntity>) -> Self {
        Self {
            cube,
            chosen_degrees: 0.0,
            next_randomize_time: 0,
        }
    }
}

impl Goal for SulfurCubeRandomDirectionGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            self.cube
                .entity
                .living_entity
                .entity
                .on_ground
                .load(Ordering::Relaxed)
                || self
                    .cube
                    .entity
                    .living_entity
                    .entity
                    .touching_water
                    .load(Ordering::Relaxed)
                || self
                    .cube
                    .entity
                    .living_entity
                    .entity
                    .touching_lava
                    .load(Ordering::Relaxed)
        })
    }

    fn tick<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.next_randomize_time -= 1;
            if self.next_randomize_time <= 0 {
                self.next_randomize_time = rand::random_range(40..100);
                self.chosen_degrees = rand::random_range(0.0..360.0);
            }
            self.cube.target_yaw.store(self.chosen_degrees);
        })
    }

    fn controls(&self) -> Controls {
        Controls::LOOK
    }
}

pub struct SulfurCubeKeepOnJumpingGoal {
    cube: Arc<SulfurCubeEntity>,
}

impl SulfurCubeKeepOnJumpingGoal {
    pub const fn new(cube: Arc<SulfurCubeEntity>) -> Self {
        Self { cube }
    }
}

impl Goal for SulfurCubeKeepOnJumpingGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let vehicle = self.cube.entity.living_entity.entity.vehicle.lock().await;
            vehicle.is_none()
        })
    }

    fn tick<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.cube.speed_modifier.store(1.0);
        })
    }

    fn controls(&self) -> Controls {
        Controls::JUMP | Controls::MOVE
    }
}

/// SulfurCube.java:962-996 (`SulfurCubeSearchForItemsGoal`).
///
/// Vanilla only uses this goal to look at and walk toward the nearest swallowable item;
/// actual absorption happens through `Mob`'s generic per-tick bounding-box item-pickup
/// pass (`pickUpItem`). This codebase has no such generic pass, so absorption is folded
/// directly into this goal's `tick` once the cube is close enough to the item, an
/// externally equivalent simplification.
pub struct SulfurCubeSearchForItemsGoal {
    cube: Arc<SulfurCubeEntity>,
    target_item: tokio::sync::Mutex<Option<Arc<ItemEntity>>>,
}

impl SulfurCubeSearchForItemsGoal {
    pub fn new(cube: Arc<SulfurCubeEntity>) -> Self {
        Self {
            cube,
            target_item: tokio::sync::Mutex::new(None),
        }
    }
}

impl Goal for SulfurCubeSearchForItemsGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if self.cube.is_baby() || self.cube.pickup_timer.load(Ordering::Relaxed) > 0 {
                return false;
            }

            let entity = &self.cube.entity.living_entity.entity;
            let world = entity.world.load();
            let pos = entity.pos.load();

            let mut nearest: Option<(Arc<ItemEntity>, f64)> = None;
            for candidate in world.get_nearby_entities(pos, 8.0).values() {
                if !candidate.get_entity().is_alive() {
                    continue;
                }
                let Ok(item_entity) = Arc::downcast::<ItemEntity>(candidate.clone()) else {
                    continue;
                };

                let is_swallowable = {
                    let stack = item_entity.get_item_stack().lock().await;
                    !stack.is_empty() && is_swallowable_item(&stack)
                };
                if !is_swallowable {
                    continue;
                }

                let dist = pos.squared_distance_to_vec(&item_entity.get_entity().pos.load());
                if nearest.as_ref().is_none_or(|(_, best)| dist < *best) {
                    nearest = Some((item_entity, dist));
                }
            }

            let found = nearest.is_some();
            *self.target_item.lock().await = nearest.map(|(item, _)| item);
            found
        })
    }

    fn tick<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let target = self.target_item.lock().await.clone();
            let Some(item_entity) = target else {
                return;
            };

            let entity = &self.cube.entity.living_entity.entity;
            let item_pos = item_entity.get_entity().pos.load();
            let my_pos = entity.pos.load();
            let dx = item_pos.x - my_pos.x;
            let dz = item_pos.z - my_pos.z;
            let yaw = dx.atan2(dz).to_degrees() as f32;
            self.cube.target_yaw.store(yaw);
            self.cube.speed_modifier.store(1.0);

            if my_pos.squared_distance_to_vec(&item_pos) <= 1.5 * 1.5 {
                let can_hold = {
                    let stack = item_entity.get_item_stack().lock().await;
                    !stack.is_empty() && self.cube.can_hold_item(&stack).await
                };
                if can_hold {
                    let picked = {
                        let mut stack = item_entity.get_item_stack().lock().await;
                        stack.split(1)
                    };
                    if self.cube.equip_item(&picked).await {
                        let remaining_empty = item_entity.get_item_stack().lock().await.is_empty();
                        if remaining_empty {
                            let world = entity.world.load();
                            world.remove_entity(&*item_entity).await;
                        }
                    }
                }
            }
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            !self.cube.is_baby()
                && self.cube.pickup_timer.load(Ordering::Relaxed) <= 0
                && self.target_item.lock().await.is_some()
        })
    }

    fn controls(&self) -> Controls {
        Controls::LOOK | Controls::MOVE
    }
}

/// SulfurCube.java:998-1018 (`SulfurCubeTemptGoal`): babies are tempted by
/// `sulfur_cube_food` (slimeballs), adults by anything swallowable.
///
/// `SulfurCube.java:125` constructs this as `TemptGoal.ForNonPathfinders(this, 1.0, ...,
/// false, 1.0)` -- a `stopDistance` of 1.0, unlike vanilla's 2.5 default.
const STOP_DISTANCE_SQUARED: f64 = 1.0;

pub struct SulfurCubeTemptGoal {
    cube: Arc<SulfurCubeEntity>,
    target_player: tokio::sync::Mutex<Option<Arc<Player>>>,
}

impl SulfurCubeTemptGoal {
    pub fn new(cube: Arc<SulfurCubeEntity>) -> Self {
        Self {
            cube,
            target_player: tokio::sync::Mutex::new(None),
        }
    }

    fn is_tempting(&self, item_stack: &ItemStack) -> bool {
        if item_stack.is_empty() {
            return false;
        }
        if self.cube.is_baby() {
            is_food_item(item_stack)
        } else {
            is_swallowable_item(item_stack)
        }
    }

    async fn is_holding_tempt_item(&self, player: &Player) -> bool {
        let main = player.inventory.held_item().await;
        if self.is_tempting(&main) {
            return true;
        }
        let off = player.inventory.off_hand_item().await;
        self.is_tempting(&off)
    }
}

impl Goal for SulfurCubeTemptGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let entity = &self.cube.entity.living_entity.entity;
            let pos = entity.pos.load();
            let world = entity.world.load();

            let mut nearest: Option<(Arc<Player>, f64)> = None;
            for player in world.get_nearby_players(pos, 10.0) {
                if !self.is_holding_tempt_item(&player).await {
                    continue;
                }
                let dist = pos.squared_distance_to_vec(&player.get_entity().pos.load());
                if nearest.as_ref().is_none_or(|(_, best)| dist < *best) {
                    nearest = Some((player, dist));
                }
            }

            let found = nearest.is_some();
            *self.target_player.lock().await = nearest.map(|(player, _)| player);
            found
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(player) = self.target_player.lock().await.clone() else {
                return false;
            };
            let entity = &self.cube.entity.living_entity.entity;
            let dist = entity
                .pos
                .load()
                .squared_distance_to_vec(&player.get_entity().pos.load());
            dist <= 10.0 * 10.0 && self.is_holding_tempt_item(&player).await
        })
    }

    fn tick<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(player) = self.target_player.lock().await.clone() else {
                return;
            };
            let entity = &self.cube.entity.living_entity.entity;
            let my_pos = entity.pos.load();
            let player_pos = player.get_entity().pos.load();
            let dx = player_pos.x - my_pos.x;
            let dz = player_pos.z - my_pos.z;
            let yaw = dx.atan2(dz).to_degrees() as f32;
            self.cube.target_yaw.store(yaw);

            // `TemptGoal.tick`: stop navigating once within `stopDistance` (1.0 here).
            if my_pos.squared_distance_to_vec(&player_pos) < STOP_DISTANCE_SQUARED {
                self.cube.speed_modifier.store(0.0);
            } else {
                self.cube.speed_modifier.store(1.0);
            }
        })
    }

    fn stop<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            *self.target_player.lock().await = None;
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_count_is_zero_only_while_primed() {
        assert_eq!(SulfurCubeEntity::split_count_for_fuse(-1), 2);
        assert_eq!(SulfurCubeEntity::split_count_for_fuse(5), 0);
        assert_eq!(SulfurCubeEntity::split_count_for_fuse(0), 0);
    }

    #[test]
    fn max_health_is_four_times_size() {
        assert_eq!(SulfurCubeEntity::max_health_for_size(1), 4);
        assert_eq!(SulfurCubeEntity::max_health_for_size(2), 8);
    }

    #[test]
    fn uses_small_hurt_sound_only_for_size_one() {
        assert_eq!(
            SulfurCubeEntity::hurt_sound_for_size(1),
            Sound::EntitySmallSulfurCubeHurt
        );
        assert_eq!(
            SulfurCubeEntity::hurt_sound_for_size(2),
            Sound::EntitySulfurCubeHurt
        );
    }

    #[test]
    fn carrying_item_uses_adult_bounce_sound() {
        assert_eq!(
            SulfurCubeEntity::squish_sound_for(2, true),
            Sound::EntitySulfurCubeBounce
        );
        assert_eq!(
            SulfurCubeEntity::squish_sound_for(2, false),
            Sound::EntitySulfurCubeSquish
        );
        assert_eq!(
            SulfurCubeEntity::squish_sound_for(1, true),
            Sound::EntitySmallSulfurCubeSquish
        );
    }

    #[test]
    // SulfurCube.java:731-767 (`playerPush`): verify the grounded impulse and horizontal scale.
    fn player_push_velocity_has_grounded_vertical_impulse() {
        assert_eq!(
            player_push_velocity(Vector3::new(1.0, 0.0, 0.0), 1.0, true, 0.5),
            Vector3::new(0.5, 0.15, 0.0)
        );
        assert_eq!(
            player_push_velocity(Vector3::new(0.0, 0.0, -2.0), 0.5, false, 0.5),
            Vector3::new(0.0, 0.0, -0.25)
        );
    }
}
