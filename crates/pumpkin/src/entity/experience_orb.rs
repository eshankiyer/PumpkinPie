use core::f32;
use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicU32, Ordering},
};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

use crate::{entity::EntityBaseFuture, server::Server, world::World};

use super::{Entity, EntityBase, NBTStorage, living::LivingEntity, player::Player};

pub struct ExperienceOrbEntity {
    entity: Entity,
    amount: AtomicU32,
    orb_age: AtomicU32,
    count: AtomicU32,
    tick_count: AtomicU32,
    /// Vanilla `ExperienceOrb.health` (`ExperienceOrb.java:42`), default 5.
    health: AtomicI32,
}

impl ExperienceOrbEntity {
    pub fn new(entity: Entity, amount: u32) -> Self {
        Self::new_with_direction(entity, Vector3::default(), amount)
    }

    /// Vanilla `ExperienceOrb`'s directional constructor (`net/minecraft/world/entity/ExperienceOrb.java:51-72`).
    pub fn new_with_direction(entity: Entity, rough_direction: Vector3<f64>, amount: u32) -> Self {
        entity.yaw.store(rand::random::<f32>() * 360.0);
        let mut random_movement = Vector3::new(
            rand::random_range(-0.2..0.2),
            rand::random_range(0.0..0.4),
            rand::random_range(-0.2..0.2),
        );
        if rough_direction.length_squared() > 0.0 && rough_direction.dot(&random_movement) < 0.0 {
            random_movement = random_movement.multiply(-1.0, -1.0, -1.0);
        }
        // `AABB.getSize` (`world/phys/AABB.java:267-272`): the average of the bounding box's
        // x/y/z extents, not the maximum. The entity's footprint is square, so x-size and
        // z-size both equal `width`.
        let dimensions = entity.entity_dimension.load();
        let size = (2.0 * f64::from(dimensions.width) + f64::from(dimensions.height)) / 3.0;
        entity.set_pos(
            entity.pos.load()
                + rough_direction
                    .normalize()
                    .multiply(size * 0.5, size * 0.5, size * 0.5),
        );
        entity.velocity.store(random_movement);
        // Vanilla `ExperienceOrb.unstuckIfPossible` (`ExperienceOrb.java:63-68,78-84`) nudges
        // an orb spawned inside a collision toward the nearest free side before it is added.
        if !entity
            .world
            .load()
            .is_space_empty(entity.bounding_box.load())
        {
            let position = entity.pos.load();
            entity.push_out_of_blocks(Vector3::new(
                position.x,
                f64::midpoint(
                    entity.bounding_box.load().min.y,
                    entity.bounding_box.load().max.y,
                ),
                position.z,
            ));
        }
        Self {
            entity,
            amount: AtomicU32::new(amount),
            orb_age: AtomicU32::new(0),
            count: AtomicU32::new(1),
            tick_count: AtomicU32::new(0),
            health: AtomicI32::new(5),
        }
    }

    pub async fn spawn(world: &Arc<World>, position: Vector3<f64>, amount: u32) {
        Self::spawn_with_direction(world, position, Vector3::default(), amount).await;
    }

    /// Vanilla `ExperienceOrb.awardWithDirection` (`net/minecraft/world/entity/ExperienceOrb.java:196-203`).
    pub async fn spawn_with_direction(
        world: &Arc<World>,
        position: Vector3<f64>,
        rough_direction: Vector3<f64>,
        amount: u32,
    ) {
        let mut amount = amount;
        while amount > 0 {
            let i = Self::round_to_orb_size(amount);
            amount -= i;
            // `ExperienceOrb.awardWithDirection` folds each split into a nearby orb of the same
            // value before creating a new entity, which keeps a mob farm's ground from filling
            // with orbs and keeps the pile alive by resetting the age it despawns on.
            if Self::try_merge_to_existing(world, position, i) {
                continue;
            }
            let entity = Entity::new(world.clone(), position, &EntityType::EXPERIENCE_ORB);
            let orb = Arc::new(Self::new_with_direction(entity, rough_direction, i));
            world.spawn_entity(orb).await;
        }
    }

    /// Vanilla `ExperienceOrb.tryMergeToExisting`: look in a one-block box for an orb of the same
    /// value whose entity id is congruent to a random one modulo forty, and grow it instead.
    fn try_merge_to_existing(world: &Arc<World>, position: Vector3<f64>, value: u32) -> bool {
        let half = Vector3::new(0.5, 0.5, 0.5);
        let box_ = BoundingBox::new(position - half, position + half);
        let id = rand::random_range(0..40);

        for other in world.get_entities_at_box(&box_) {
            let Some(orb) = other.cast_any().downcast_ref::<Self>() else {
                continue;
            };
            if orb.entity.is_removed()
                || orb.amount.load(Ordering::Relaxed) != value
                || orb.entity.entity_id.wrapping_sub(id) % 40 != 0
            {
                continue;
            }

            orb.count.fetch_add(1, Ordering::Relaxed);
            orb.orb_age.store(0, Ordering::Relaxed);
            return true;
        }

        false
    }

    const fn round_to_orb_size(value: u32) -> u32 {
        if value >= 2477 {
            2477
        } else if value >= 1237 {
            1237
        } else if value >= 617 {
            617
        } else if value >= 307 {
            307
        } else if value >= 149 {
            149
        } else if value >= 73 {
            73
        } else if value >= 37 {
            37
        } else if value >= 17 {
            17
        } else if value >= 7 {
            7
        } else if value >= 3 {
            3
        } else {
            1
        }
    }

    /// Port of vanilla's `scanForMerges`. Merges compatible orbs (same `amount`, entity ID
    /// difference divisible by 40) within `bounding_box.expand_all(0.5)` into `self`.
    async fn scan_for_merges(&self) {
        let bounding_box = self.entity.bounding_box.load().expand_all(0.5);
        let world = self.entity.world.load();

        for other in world.get_entities_at_box(&bounding_box) {
            let Some(other_orb) = other.cast_any().downcast_ref::<Self>() else {
                continue;
            };
            if std::ptr::eq(self, other_orb) || other_orb.entity.is_removed() {
                continue;
            }
            if other_orb.amount.load(Ordering::Relaxed) != self.amount.load(Ordering::Relaxed)
                || other_orb
                    .entity
                    .entity_id
                    .wrapping_sub(self.entity.entity_id)
                    % 40
                    != 0
            {
                continue;
            }

            let other_count = other_orb.count.load(Ordering::Relaxed);
            self.count.fetch_add(other_count, Ordering::Relaxed);
            let other_age = other_orb.orb_age.load(Ordering::Relaxed);
            self.orb_age.fetch_min(other_age, Ordering::Relaxed);
            other_orb.entity.remove().await;
        }
    }
}

impl NBTStorage for ExperienceOrbEntity {
    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut pumpkin_nbt::compound::NbtCompound,
    ) -> super::NbtFuture<'a, ()> {
        Box::pin(async move {
            // Vanilla `ExperienceOrb.addAdditionalSaveData`.
            nbt.put_short("Health", self.health.load(Ordering::Relaxed) as i16);
            nbt.put_short("Age", self.orb_age.load(Ordering::Relaxed) as i16);
            nbt.put_short("Value", self.amount.load(Ordering::Relaxed) as i16);
            nbt.put_int("Count", self.count.load(Ordering::Relaxed) as i32);
        })
    }

    fn read_nbt_non_mut<'a>(
        &'a self,
        nbt: &'a pumpkin_nbt::compound::NbtCompound,
    ) -> super::NbtFuture<'a, ()> {
        Box::pin(async move {
            self.health.store(
                i32::from(nbt.get_short("Health").unwrap_or(5)),
                Ordering::Relaxed,
            );
            self.orb_age.store(
                nbt.get_short("Age").unwrap_or(0).max(0) as u32,
                Ordering::Relaxed,
            );
            self.amount.store(
                nbt.get_short("Value").unwrap_or(0).max(0) as u32,
                Ordering::Relaxed,
            );
            self.count.store(
                nbt.get_int("Count").unwrap_or(1).max(1) as u32,
                Ordering::Relaxed,
            );
        })
    }
}

impl EntityBase for ExperienceOrbEntity {
    /// Vanilla `ExperienceOrb.isAttackable` (`ExperienceOrb.java:375-378`).
    /// Experience orbs are collected by touching them, not admitted to player attacks.
    fn is_attackable(&self) -> bool {
        false
    }

    /// Vanilla `ExperienceOrb.hurtServer` (`ExperienceOrb.java:252-266`).
    fn damage_with_context<'a>(
        &'a self,
        _caller: &'a dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&'a dyn EntityBase>,
        _cause: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            if self.entity.is_invulnerable_to(&damage_type).await {
                return false;
            }
            self.entity.mark_hurt();
            // `(int)(this.health - damage)` is float arithmetic truncated toward zero.
            let previous = self
                .health
                .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |health| {
                    Some((health as f32 - amount) as i32)
                })
                .unwrap_or_else(|unchanged| unchanged);
            let remaining = (previous as f32 - amount) as i32;
            if remaining <= 0 {
                self.entity.remove().await;
            }
            true
        })
    }

    fn tick<'a>(
        &'a self,
        caller: &'a Arc<dyn EntityBase>,
        server: &'a Server,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let entity = &self.entity;
            entity.tick(caller, server).await;

            // Vanilla `tickCount` is incremented by `Entity.baseTick`, separate from `age`.
            let tick_count = self.tick_count.fetch_add(1, Ordering::Relaxed) + 1;

            let bounding_box = entity.bounding_box.load();
            let world = entity.world.load();
            let mut velo = entity.velocity.load();

            // `ExperienceOrb.tick` (`ExperienceOrb.java:97-140`): water replaces gravity, gravity
            // only applies while not inside a collision, and lava kicks the orb around.
            let colliding = !world.is_space_empty(bounding_box.expand(-1.0e-7, -1.0e-7, -1.0e-7));
            if entity.is_eye_in_fluid(&world, &tag::Fluid::MINECRAFT_WATER) {
                // `ExperienceOrb.setUnderwaterMovement` (`ExperienceOrb.java:234-237`).
                velo = Vector3::new(
                    velo.x * f64::from(0.99f32),
                    (velo.y + f64::from(5.0e-4f32)).min(f64::from(0.06f32)),
                    velo.z * f64::from(0.99f32),
                );
            } else if !colliding {
                velo.y -= self.get_gravity();
            }

            if world
                .get_fluid(&entity.block_pos.load())
                .has_tag(&tag::Fluid::MINECRAFT_LAVA)
            {
                velo = Vector3::new(
                    f64::from((rand::random::<f32>() - rand::random::<f32>()) * 0.2),
                    f64::from(0.2f32),
                    f64::from((rand::random::<f32>() - rand::random::<f32>()) * 0.2),
                );
            }
            entity.velocity.store(velo);

            if tick_count % 20 == 1 {
                self.scan_for_merges().await;
            }

            // `followNearbyPlayer`: the nearest living, non-spectator player within 8 blocks.
            let following = world.get_closest_player_where(entity.pos.load(), 8.0, |player| {
                !player.is_spectator() && !player.living_entity.is_dead_or_dying()
            });
            if let Some(player) = &following {
                let player_entity = player.get_entity();
                let target = player_entity.pos.load()
                    + Vector3::new(0.0, player_entity.get_eye_height() / 2.0, 0.0);
                let delta = target - entity.pos.load();
                let distance = delta.length();
                if distance > 1.0e-4 {
                    let power = (1.0 - distance / 8.0).max(0.0);
                    velo += delta.normalize() * (power * power * 0.1);
                }
                entity.velocity.store(velo);
            } else if colliding
                && !world.is_space_empty(
                    bounding_box
                        .shift(velo)
                        .expand(-1.0e-7, -1.0e-7, -1.0e-7),
                )
            {
                // Stuck in a block with nowhere to fall: `Entity.moveTowardsClosestSpace`, then
                // `needsSync` re-sends the new velocity.
                let position = entity.pos.load();
                entity.push_out_of_blocks(Vector3::new(
                    position.x,
                    f64::midpoint(bounding_box.min.y, bounding_box.max.y),
                    position.z,
                ));
                entity.velocity_dirty.store(true, Ordering::SeqCst);
                velo = entity.velocity.load();
            }

            let fall_speed = velo.y;
            entity.move_entity(caller, velo).await;

            entity.tick_block_collisions(caller, server).await;

            // `ExperienceOrb.tick`: air drag of 0.98 on every axis, multiplied by the slipperiness
            // of the block below when grounded, then a small bounce off the floor. Without it an
            // orb kept accelerating and the pull toward a player made it overshoot and oscillate
            // instead of converging.
            let on_ground = entity.on_ground.load(Ordering::Relaxed);
            // `ExperienceOrb.tick` uses the inherited `Entity.getAirDrag` (`Entity.java:1529-1531`).
            let mut friction = entity.get_air_drag();
            if on_ground {
                friction *= f64::from(entity.get_block_with_y_offset(0.999_999).1.slipperiness);
            }
            let mut damped = entity.velocity.load() * friction;
            if on_ground && fall_speed < -self.get_gravity() {
                damped.y = -fall_speed * 0.4;
            }
            entity.velocity.store(damped);

            if self.orb_age.fetch_add(1, Ordering::Relaxed) + 1 >= 6000 {
                self.entity.remove().await;
            }
        })
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn on_player_collision<'a>(&'a self, player: &'a Arc<Player>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if player.living_entity.health.load() > 0.0 {
                let mut delay = player.experience_pick_up_delay.lock().await;
                if *delay == 0 {
                    *delay = 2;
                    player.living_entity.pickup(&self.entity, 1);
                    let remaining = player
                        .apply_mending_from_xp(self.amount.load(Ordering::Relaxed) as i32)
                        .await;
                    if remaining > 0 {
                        player.add_experience_points(remaining).await;
                    }
                    if self.count.fetch_sub(1, Ordering::Relaxed) <= 1 {
                        self.entity.remove().await;
                    }
                }
            }
        })
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn get_gravity(&self) -> f64 {
        0.03
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::ExperienceOrbEntity;

    #[test]
    fn experience_value_uses_vanilla_thresholds() {
        // Vanilla `ExperienceOrb.getExperienceValue` (`ExperienceOrb.java:351-373`) selects the
        // largest orb value that does not exceed the remaining experience.
        for (remaining, expected) in [
            (1, 1),
            (3, 3),
            (6, 3),
            (7, 7),
            (148, 73),
            (149, 149),
            (2476, 1237),
            (2477, 2477),
        ] {
            assert_eq!(ExperienceOrbEntity::round_to_orb_size(remaining), expected);
        }
    }
}
