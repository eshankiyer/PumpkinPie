use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    entity::{Entity, EntityBase, living::LivingEntity, player::Player},
    server::Server,
};
use pumpkin_data::damage::DamageType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_protocol::IdOr;
use pumpkin_protocol::java::client::play::{CEntityVelocity, CSoundEffect, Metadata};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::ProjectileHit;
use super::arrow::ArrowPickup;

pub struct TridentEntity {
    pub entity: Entity,
    pub owner_id: Option<i32>,
    pub item_stack: Arc<Mutex<ItemStack>>,
    pub pickup: ArrowPickup,
    pub in_ground: AtomicBool,
    pub in_ground_time: AtomicU32,
    pub life: AtomicU32,
    pub shake_time: AtomicU8,
    pub has_hit: AtomicBool,
    pub last_block_pos: Arc<std::sync::RwLock<Option<BlockPos>>>,
    /// `ThrownTrident.dealtDamage` (`ThrownTrident.java:36`).
    pub dealt_damage: AtomicBool,
    /// `ThrownTrident.tick` sets no-physics while returning (`ThrownTrident.java:83`).
    pub no_physics: AtomicBool,
    /// `ThrownTrident.clientSideReturnTridentTickCount` (`ThrownTrident.java:37`).
    pub return_tick_count: AtomicU32,
    /// `ThrownTrident.ID_LOYALTY` (`ThrownTrident.java:32`), initialized from the stack.
    pub loyalty: AtomicU8,
}

impl TridentEntity {
    const BASE_DAMAGE: f64 = 8.0;
    const AIR_INERTIA: f64 = 0.99;
    // `ThrownTrident.getWaterInertia` (`ThrownTrident.java:56-60`) keeps the arrow's 0.99 drag.
    const WATER_INERTIA: f64 = 0.99;
    const GRAVITY: f64 = 0.05;
    const DESPAWN_TIME: u32 = 1200;

    pub fn new(entity: Entity, owner_id: Option<i32>) -> Self {
        // `Projectile.getAddEntityPacket` (`Projectile.java:346-349`) carries the owner id.
        entity.data.store(owner_id.unwrap_or(0), Ordering::Relaxed);
        Self {
            entity,
            owner_id,
            item_stack: Arc::new(Mutex::new(ItemStack::new(1, &Item::TRIDENT))),
            pickup: ArrowPickup::Disallowed,
            in_ground: AtomicBool::new(false),
            in_ground_time: AtomicU32::new(0),
            life: AtomicU32::new(0),
            shake_time: AtomicU8::new(0),
            has_hit: AtomicBool::new(false),
            last_block_pos: Arc::new(std::sync::RwLock::new(None)),
            dealt_damage: AtomicBool::new(false),
            no_physics: AtomicBool::new(false),
            return_tick_count: AtomicU32::new(0),
            loyalty: AtomicU8::new(0),
        }
    }

    pub fn new_shot(
        entity: Entity,
        shooter: &Entity,
        item_stack: ItemStack,
        pickup: ArrowPickup,
    ) -> Self {
        // `ThrownTrident.<init>` (`ThrownTrident.java:43-47`) reads Loyalty from the thrown stack.
        let loyalty = item_stack
            .get_enchantment_level(&pumpkin_data::Enchantment::LOYALTY)
            .clamp(0, i32::from(u8::MAX)) as u8;
        let mut owner_pos = shooter.pos.load();
        owner_pos.y = owner_pos.y + f64::from(shooter.entity_dimension.load().eye_height) - 0.1;
        entity.pos.store(owner_pos);
        entity.set_velocity(Vector3::new(0.0, 0.1, 0.0));
        entity.data.store(shooter.entity_id, Ordering::Relaxed);

        Self {
            entity,
            owner_id: Some(shooter.entity_id),
            item_stack: Arc::new(Mutex::new(item_stack)),
            pickup,
            in_ground: AtomicBool::new(false),
            in_ground_time: AtomicU32::new(0),
            life: AtomicU32::new(0),
            shake_time: AtomicU8::new(0),
            has_hit: AtomicBool::new(false),
            last_block_pos: Arc::new(std::sync::RwLock::new(None)),
            dealt_damage: AtomicBool::new(false),
            no_physics: AtomicBool::new(false),
            return_tick_count: AtomicU32::new(0),
            loyalty: AtomicU8::new(loyalty),
        }
    }

    pub fn set_velocity_from_rotation(
        &self,
        pitch: f32,
        yaw: f32,
        roll: f32,
        speed: f32,
        divergence: f32,
    ) {
        let yaw_rad = yaw.to_radians();
        let pitch_rad = pitch.to_radians();
        let roll_rad = (pitch + roll).to_radians();

        let x = -yaw_rad.sin() * pitch_rad.cos();
        let y = -roll_rad.sin();
        let z = yaw_rad.cos() * pitch_rad.cos();

        self.set_velocity(
            f64::from(x),
            f64::from(y),
            f64::from(z),
            f64::from(speed),
            f64::from(divergence),
        );
    }

    pub fn set_velocity(&self, x: f64, y: f64, z: f64, power: f64, uncertainty: f64) {
        fn next_triangular(mode: f64, deviation: f64) -> f64 {
            deviation.mul_add(rand::random::<f64>() - rand::random::<f64>(), mode)
        }

        let velocity = Vector3::new(x, y, z)
            .normalize()
            .add_raw(
                next_triangular(0.0, 0.017_227_5 * uncertainty),
                next_triangular(0.0, 0.017_227_5 * uncertainty),
                next_triangular(0.0, 0.017_227_5 * uncertainty),
            )
            .multiply(power, power, power);

        self.entity.velocity.store(velocity);
        let len = velocity.horizontal_length();
        self.entity.set_rotation(
            velocity.x.atan2(velocity.z) as f32 * 57.295_776,
            velocity.y.atan2(len) as f32 * 57.295_776,
        );
    }

    fn should_skip_collision(&self, self_ent: &Entity, other: &Arc<dyn EntityBase>) -> bool {
        let other_ent = other.get_entity();

        // Don't collide with self
        if other_ent.entity_id == self_ent.entity_id {
            return true;
        }

        if !other.can_be_hit_by_projectile() {
            return true;
        }

        // Skip owner for initial frames (5 ticks)
        if Some(other_ent.entity_id) == self.owner_id && self_ent.age.load(Ordering::Relaxed) < 5 {
            return true;
        }

        // Skip other projectiles and item entities
        if other_ent.entity_type == &pumpkin_data::entity::EntityType::ARROW
            || other_ent.entity_type == &pumpkin_data::entity::EntityType::TRIDENT
            || other_ent.entity_type == &pumpkin_data::entity::EntityType::ITEM
            || other_ent.entity_type == &pumpkin_data::entity::EntityType::FALLING_BLOCK
        {
            return true;
        }

        false
    }

    /// `ThrownTrident.tick` (`ThrownTrident.java:63-97`), the Loyalty return leg. Returning
    /// tridents set no-physics and bypass the ordinary collision sweep.
    fn tick_loyalty_return(&self, world: &Arc<crate::world::World>) -> bool {
        let entity = self.get_entity();
        // `ThrownTrident.java:64-66`: four stuck ticks count as dealt damage for Loyalty.
        if self.in_ground_time.load(Ordering::Relaxed) > 4 {
            self.dealt_damage.store(true, Ordering::Relaxed);
        }
        let loyalty = self.loyalty.load(Ordering::Relaxed);
        if loyalty == 0
            || !(self.dealt_damage.load(Ordering::Relaxed) || self.no_physics.load(Ordering::Relaxed))
        {
            return false;
        }
        let Some(owner) = self.owner_id.and_then(|id| world.get_entity_by_id(id)) else {
            return false;
        };
        let owner_entity = owner.get_entity();
        let owner_player = world.get_player_by_id(owner_entity.entity_id);
        // `ThrownTrident.isAcceptibleReturnOwner` (`ThrownTrident.java:99-102`).
        let acceptable = owner_entity.is_alive()
            && owner.get_living_entity().is_none_or(|living| living.health.load() > 0.0)
            && owner_player.as_ref().is_none_or(|player| !player.is_spectator());
        if !acceptable {
            if self.pickup == ArrowPickup::Allowed {
                let stack = self.item_stack.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
                world.drop_stack(&BlockPos::floored(entity.pos.load().x, entity.pos.load().y, entity.pos.load().z), stack);
            }
            entity.remove();
            return true;
        }
        let eye_pos = owner_entity.pos.load().add_raw(
            0.0,
            f64::from(owner_entity.entity_dimension.load().eye_height),
            0.0,
        );
        let to_owner = eye_pos.sub(&entity.pos.load());
        // `ThrownTrident.tick` (`ThrownTrident.java:78-81`) absorbs into non-player owners.
        if owner_player.is_none()
            && to_owner.length() < f64::from(owner_entity.entity_dimension.load().width) + 1.0
        {
            entity.remove();
            return true;
        }
        self.no_physics.store(true, Ordering::Relaxed);
        self.in_ground.store(false, Ordering::Relaxed);
        let pos = entity.pos.load();
        entity.set_pos(Vector3::new(pos.x, pos.y + to_owner.y * 0.015 * f64::from(loyalty), pos.z));
        let accel = 0.05 * f64::from(loyalty);
        let mut velocity = entity.velocity.load()
            .multiply(0.95, 0.95, 0.95)
            .add(&to_owner.normalize().multiply(accel, accel, accel));
        // `ThrownTrident.tick` falls through to `super.tick()` (`ThrownTrident.java:96`), so the
        // returning trident retains AbstractArrow gravity and drag.
        velocity.y -= Self::GRAVITY;
        let inertia = if entity.touching_water.load(Ordering::Relaxed) { Self::WATER_INERTIA } else { Self::AIR_INERTIA };
        velocity = velocity.multiply(inertia, inertia, inertia);
        entity.velocity.store(velocity);
        entity.set_pos(entity.pos.load().add(&velocity));
        world.broadcast_to_chunk(entity.chunk_pos.load(), &CEntityVelocity::new(entity.entity_id.into(), velocity));
        if self.return_tick_count.fetch_add(1, Ordering::Relaxed) == 0 {
            world.broadcast_to_chunk(entity.chunk_pos.load(), &CSoundEffect::new(
                IdOr::Id(Sound::ItemTridentReturn as u16), SoundCategory::Neutral,
                &entity.pos.load(), 10.0, 1.0, 0.0,
            ));
        }
        true
    }

    /// The block-then-entity raycast sweep of `AbstractArrow.tick`.
    fn sweep_collision(
        &self,
        world: &Arc<crate::world::World>,
        start_pos: Vector3<f64>,
        velocity: Vector3<f64>,
    ) -> Option<ProjectileHit> {
        let entity = self.get_entity();
        let new_pos = start_pos.add(&velocity);
        let search_box = BoundingBox::new(
            Vector3::new(start_pos.x.min(new_pos.x), start_pos.y.min(new_pos.y), start_pos.z.min(new_pos.z)),
            Vector3::new(start_pos.x.max(new_pos.x), start_pos.y.max(new_pos.y), start_pos.z.max(new_pos.z)),
        ).expand(0.3, 0.3, 0.3);
        let mut closest_t = 1.0;
        let mut hit = None;
        let (block_cols, block_positions) = world.get_block_collisions(search_box, entity);
        for (idx, bb) in block_cols.iter().enumerate() {
            if let Some(t) = calculate_ray_intersection(&start_pos, &velocity, bb) && t < closest_t {
                closest_t = t;
                let mut curr = 0;
                for (len, pos) in &block_positions {
                    curr += len;
                    if idx < curr {
                        let hit_pos = start_pos.add(&velocity.multiply(t, t, t));
                        hit = Some(ProjectileHit::Block { pos: *pos, face: get_hit_face(hit_pos, *pos), hit_pos, normal: velocity.normalize().multiply(-1.0, -1.0, -1.0) });
                        break;
                    }
                }
            }
        }
        for cand in world.get_entities_at_box(&search_box) {
            if self.should_skip_collision(entity, &cand) { continue; }
            let ebb = cand.get_entity().bounding_box.load().expand(0.3, 0.3, 0.3);
            if let Some(t) = calculate_ray_intersection(&start_pos, &velocity, &ebb) && t < closest_t {
                closest_t = t;
                let hit_pos = start_pos.add(&velocity.multiply(t, t, t));
                hit = Some(ProjectileHit::Entity { entity: cand, hit_pos, normal: velocity.normalize().multiply(-1.0, -1.0, -1.0) });
            }
        }
        hit
    }
}

impl EntityBase for TridentEntity {
    fn write_custom_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
        // `ThrownTrident.addAdditionalSaveData` (`ThrownTrident.java:194-205`,
        // `ThrownTrident.java:204`) and `AbstractArrow.addAdditionalSaveData`
        // (`AbstractArrow.java:606-635`, `AbstractArrow.java:608-612`).
        let mut item = pumpkin_nbt::compound::NbtCompound::new();
        self.item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .copy_with_count(1)
            .write_item_stack(&mut item);
        nbt.put_compound("item", item);
        nbt.put_bool("DealtDamage", self.dealt_damage.load(Ordering::Relaxed));
        nbt.put_short("life", self.life.load(Ordering::Relaxed) as i16);
        // `AbstractArrow.java:626` masks the stored shake byte to 255 on load.
        nbt.put_byte("shake", self.shake_time.load(Ordering::Relaxed) as i8);
        nbt.put_bool("inGround", self.in_ground.load(Ordering::Relaxed));
        nbt.put_byte("pickup", self.pickup.to_byte() as i8);
    }

    fn read_custom_nbt(&self, nbt: &pumpkin_nbt::compound::NbtCompound) {
        // `ThrownTrident.readAdditionalSaveData` (`ThrownTrident.java:198`) re-derives Loyalty
        // from the serialized item rather than storing the enchantment level separately.
        if let Some(stack) = nbt.get_compound("item").and_then(ItemStack::read_item_stack).map(|s| s.copy_with_count(1)) {
            let loyalty = stack.get_enchantment_level(&pumpkin_data::Enchantment::LOYALTY).clamp(0, i32::from(u8::MAX)) as u8;
            self.loyalty.store(loyalty, Ordering::Relaxed);
            *self.item_stack.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = stack;
        }
        self.dealt_damage.store(nbt.get_bool("DealtDamage").unwrap_or(false), Ordering::Relaxed);
        self.life.store(u32::from(nbt.get_short("life").unwrap_or(0).max(0) as u16), Ordering::Relaxed);
        self.shake_time.store(nbt.get_byte("shake").unwrap_or(0) as u8, Ordering::Relaxed);
        self.in_ground.store(nbt.get_bool("inGround").unwrap_or(false), Ordering::Relaxed);
        self.pickup = ArrowPickup::from_byte(nbt.get_byte("pickup").unwrap_or(0) as u8);
    }

    /// `ThrownTrident.defineSynchedData` (`ThrownTrident.java:56-60`) and constructor foil/
    /// loyalty state (`ThrownTrident.java:44-46`).
    fn init_data_tracker(&self) {
        let has_foil = self.item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_data_component::<pumpkin_data::data_component_impl::EnchantmentsImpl>()
            .is_some_and(|e| !e.enchantment.is_empty());
        self.entity.send_meta_data(&[Metadata::new(
            pumpkin_data::tracked_data::thrown_trident::ID_LOYALTY,
            self.loyalty.load(Ordering::Relaxed),
        ), Metadata::new(
            pumpkin_data::tracked_data::thrown_trident::ID_FOIL,
            has_foil,
        )], None);
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = self.get_entity();
        let world = entity.world.load();

        // Handle shake time
        let shake = self.shake_time.load(Ordering::Relaxed);
        if shake > 0 {
            self.shake_time.store(shake - 1, Ordering::Relaxed);
        }

        if self.tick_loyalty_return(&world) {
            return;
        }

        if self.in_ground.load(Ordering::Relaxed) {
            let _in_ground_time = self.in_ground_time.fetch_add(1, Ordering::Relaxed);
            let life = self.life.fetch_add(1, Ordering::Relaxed);

            // Despawn after enough time
            if life >= Self::DESPAWN_TIME {
                entity.remove();
            }
            return;
        }

        // Trident is flying
        let start_pos = entity.pos.load();
        let mut velocity = entity.velocity.load();

        // Apply gravity
        velocity.y -= Self::GRAVITY;

        // Apply inertia (air resistance or water drag)
        let inertia = if entity.touching_water.load(Ordering::Relaxed) {
            Self::WATER_INERTIA
        } else {
            Self::AIR_INERTIA
        };
        velocity = velocity.multiply(inertia, inertia, inertia);

        entity.velocity.store(velocity);

        // Update rotation based on velocity
        let len = velocity.horizontal_length();
        entity.set_rotation(
            velocity.x.atan2(velocity.z) as f32 * 57.295_776,
            velocity.y.atan2(len) as f32 * 57.295_776,
        );

        // Move trident
        let new_pos = start_pos.add(&velocity);
        entity.set_pos(new_pos);

        // Broadcast velocity update
        let packet = CEntityVelocity::new(entity.entity_id.into(), velocity);
        let chunk_pos = entity.chunk_pos.load();
        world.broadcast_to_chunk(chunk_pos, &packet);

        // Check for collisions using the `AbstractArrow.tick` ray sweep.
        let hit = self.sweep_collision(&world, start_pos, velocity);
        if let Some(h) = hit
            && !self.has_hit.swap(true, Ordering::SeqCst)
        {
            caller.on_hit(h);
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }
    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: ProjectileHit) {
        let entity = self.get_entity();
        let world = entity.world.load();

        match hit {
            ProjectileHit::Block { pos, face, hit_pos, .. } => {
                self.in_ground.store(true, Ordering::Relaxed);
                self.shake_time.store(7, Ordering::Relaxed);
                *self
                    .last_block_pos
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pos);

                // `TargetBlock` receives persistent projectile power before the trident settles;
                // dispatch the existing async target-block hook without delaying the sync hit.
                if world.get_block(&pos) == &pumpkin_data::Block::TARGET {
                    let world = world.clone();
                    let owner_id = self.owner_id;
                    tokio::spawn(async move {
                        crate::entity::projectile::on_target_block_hit(
                            &world,
                            &pos,
                            face,
                            hit_pos,
                            owner_id,
                            crate::block::blocks::redstone::target_block::TargetBlock::PERSISTENT_PROJECTILE_DELAY,
                        )
                        .await;
                    });
                }

                // Stop the trident
                entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
                entity.set_pos(hit_pos);

                // Play sound
                let sound_packet = CSoundEffect::new(
                    IdOr::Id(Sound::ItemTridentHitGround as u16),
                    SoundCategory::Neutral,
                    &hit_pos,
                    1.0,
                    1.0,
                    0.0,
                );
                let chunk_pos = entity.chunk_pos.load();
                world.broadcast_to_chunk(chunk_pos, &sound_packet);
            }
            ProjectileHit::Entity {
                entity: target,
                hit_pos,
                ..
            } => {
                // `ThrownTrident.onHitEntity` (`ThrownTrident.java:122`) starts at fixed damage 8.
                let mut damage = Self::BASE_DAMAGE;

                // `EnchantmentHelper.getDamageBonus` (`EnchantmentHelper.java:415-421`) applies
                // Impaling to entities tagged sensitive_to_impaling, not merely wet entities.
                if let Some(enchantments) = self
                    .item_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get_data_component::<pumpkin_data::data_component_impl::EnchantmentsImpl>()
                {
                    for (enchantment, level) in enchantments.enchantment.iter() {
                        if **enchantment == pumpkin_data::Enchantment::IMPALING {
                            let target_type = target.get_entity().entity_type;
                            for effect in crate::enchantment::effects_for(enchantment) {
                                if let crate::enchantment::EnchantmentEffect::Damage(condition, value) = effect
                                    && condition.applies(target_type)
                                {
                                    damage += f64::from(value.calculate(*level));
                                }
                            }
                        }
                    }
                }

                // `ThrownTrident.onHitEntity` (`ThrownTrident.java:129`) sets dealtDamage before
                // hurt, including when the target is invulnerable.
                self.dealt_damage.store(true, Ordering::Relaxed);
                target.damage(self, damage as f32, DamageType::TRIDENT);

                // Play hit sound
                let sound_packet = CSoundEffect::new(
                    IdOr::Id(Sound::ItemTridentHit as u16),
                    SoundCategory::Neutral,
                    &hit_pos,
                    1.0,
                    1.0,
                    0.0,
                );
                world.broadcast_packet_all(&sound_packet);

                // Channeling's post-attack effect is gated on thundering weather and sky access.
                let channeling_level = self
                    .item_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get_enchantment_level(&pumpkin_data::Enchantment::CHANNELING);
                if channeling_level > 0
                    && world.is_thundering()
                    && world.can_see_sky(&BlockPos::floored(hit_pos.x, hit_pos.y, hit_pos.z))
                {
                    let lightning = crate::entity::Entity::new(
                        entity.world.load_full(),
                        hit_pos,
                        &pumpkin_data::entity::EntityType::LIGHTNING_BOLT,
                    );
                    world.spawn_entity(Arc::new(lightning));
                }

                // Standard bounce/fall-back behavior
                entity.velocity.store(Vector3::new(0.0, -0.1, 0.0));
                self.has_hit.store(false, Ordering::Relaxed); // Let it hit the ground
            }
        }
    }

    fn on_player_collision(&self, player: &Arc<Player>) {
        // `ThrownTrident.tryPickup` (`ThrownTrident.java:173-175`) lets Loyalty-returning
        // tridents be collected by their owner regardless of normal pickup restrictions.
        let returning_to_owner = self.no_physics.load(Ordering::Relaxed)
            && self.owner_id == Some(player.living_entity.entity.entity_id);
        // Can only pick up when on the ground
        if !returning_to_owner && !self.in_ground.load(Ordering::Relaxed) {
            return;
        }

        if player.living_entity.health.load() <= 0.0 {
            return;
        }

        if !returning_to_owner {
            match self.pickup {
                ArrowPickup::Disallowed => return,
                ArrowPickup::CreativeOnly if !player.is_creative() => return,
                _ => {}
            }
        }

        let mut stack = self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if player.is_creative() || player.inventory.insert_stack_anywhere(&mut stack) {
            player.living_entity.pickup(&self.entity, 1);
            self.get_entity().remove();
        }
    }
}

/// Ray intersection algorithm for AABBs, returning a t value
fn calculate_ray_intersection(
    start: &Vector3<f64>,
    dir: &Vector3<f64>,
    bb: &BoundingBox,
) -> Option<f64> {
    let mut t_min = 0.0f64;
    let mut t_max = 1.0f64;

    let b_min = [bb.min.x, bb.min.y, bb.min.z];
    let b_max = [bb.max.x, bb.max.y, bb.max.z];
    let s = [start.x, start.y, start.z];
    let d = [dir.x, dir.y, dir.z];

    for i in 0..3 {
        if d[i].abs() < 1e-9 {
            if s[i] < b_min[i] || s[i] > b_max[i] {
                return None;
            }
        } else {
            let t1 = (b_min[i] - s[i]) / d[i];
            let t2 = (b_max[i] - s[i]) / d[i];
            t_min = t_min.max(t1.min(t2));
            t_max = t_max.min(t1.max(t2));
        }
    }

    (0.0..=1.0).contains(&t_min).then_some(t_min)
}

/// Get the face of the block that was hit
fn get_hit_face(hit_pos: Vector3<f64>, block_pos: BlockPos) -> pumpkin_data::BlockDirection {
    let local = hit_pos.sub(&block_pos.0.to_f64());
    let eps = 1.0e-4;

    if local.x <= eps {
        pumpkin_data::BlockDirection::West
    } else if local.x >= 1.0 - eps {
        pumpkin_data::BlockDirection::East
    } else if local.y <= eps {
        pumpkin_data::BlockDirection::Down
    } else if local.y >= 1.0 - eps {
        pumpkin_data::BlockDirection::Up
    } else if local.z <= eps {
        pumpkin_data::BlockDirection::North
    } else {
        pumpkin_data::BlockDirection::South
    }
}
