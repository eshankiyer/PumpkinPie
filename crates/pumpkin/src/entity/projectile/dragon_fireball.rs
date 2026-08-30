use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use pumpkin_data::entity::EntityType;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::{
    entity::{
        Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
        area_effect_cloud::AreaEffectCloudEntity,
        projectile::{
            ProjectileHit, ThrownItemEntity,
            fireball::{AIR_INERTIA, DEFLECTION_SCALE, INITIAL_ACCELERATION_POWER, WATER_INERTIA},
        },
        projectile_deflection::ProjectileDeflectionType,
    },
    server::Server,
};

/// `DragonFireball.SPLASH_RANGE` (DragonFireball.java:19).
pub const SPLASH_RANGE: f32 = 4.0;
/// The vertical half of `getBoundingBox().inflate(4.0, 2.0, 4.0)` (DragonFireball.java:33).
const SPLASH_VERTICAL_RANGE: f64 = 2.0;
/// `cloud.setRadius(3.0F)` (DragonFireball.java:41).
const CLOUD_RADIUS: f32 = 3.0;
/// `cloud.setDuration(600)` (DragonFireball.java:42).
const CLOUD_DURATION: i32 = 600;
/// The radius the cloud grows to over `CLOUD_DURATION`, implied by
/// `setRadiusPerTick((7.0F - getRadius()) / getDuration())` (DragonFireball.java:43).
const CLOUD_MAX_RADIUS: f32 = 7.0;
/// `AreaEffectCloud` defaults for reapplication delay and wait time.
const CLOUD_REAPPLICATION_DELAY: i32 = 20;
const CLOUD_WAIT_TIME: i32 = 20;
/// `new MobEffectInstance(MobEffects.INSTANT_DAMAGE, 1, 1)` (DragonFireball.java:45).
const CLOUD_EFFECT_DURATION: i32 = 1;
const CLOUD_EFFECT_AMPLIFIER: u8 = 1;
/// `this.distanceToSqr(entity) < 16.0` (DragonFireball.java:48).
const REPOSITION_RANGE_SQUARED: f64 = 16.0;

/// `setRadiusPerTick((7.0F - cloud.getRadius()) / cloud.getDuration())` (DragonFireball.java:43).
fn cloud_radius_per_tick() -> f32 {
    (CLOUD_MAX_RADIUS - CLOUD_RADIUS) / CLOUD_DURATION as f32
}

/// The guard from `DragonFireball.onHit` (DragonFireball.java:31):
/// `hitResult.getType() != ENTITY || !this.ownedBy(hitResult.getEntity())`. A fireball that
/// lands on its own shooter produces no cloud at all.
const fn should_splash(hit_entity_id: Option<i32>, owner_id: Option<i32>) -> bool {
    match (hit_entity_id, owner_id) {
        (Some(hit), Some(owner)) => hit != owner,
        _ => true,
    }
}

/// `DragonFireball.onHit` (DragonFireball.java:46-53): the cloud spawns on the impact point
/// unless a living entity from the splash box is within 4 blocks, in which case the *first*
/// such entity's position wins.
fn cloud_position(impact: Vector3<f64>, nearby: &[Vector3<f64>]) -> Vector3<f64> {
    nearby
        .iter()
        .copied()
        .find(|pos| impact.squared_distance_to_vec(pos) < REPOSITION_RANGE_SQUARED)
        .unwrap_or(impact)
}

/// Vanilla `DragonFireball` (DragonFireball.java:18).
///
/// An `AbstractHurtingProjectile` that leaves a lingering dragon-breath cloud where it
/// lands. Unlike `Fireball` it carries no item stack and no explosion power, so the only
/// persisted field is the inherited `acceleration_power`
/// (`AbstractHurtingProjectile.java:166`).
pub struct DragonFireballEntity {
    pub thrown: ThrownItemEntity,
    pub acceleration_power: AtomicU64,
}

impl DragonFireballEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner_id: None,
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: 0.0,
        };

        Self {
            thrown,
            acceleration_power: AtomicU64::new(INITIAL_ACCELERATION_POWER.to_bits()),
        }
    }

    /// `AbstractHurtingProjectile(type, mob, direction, level)` (AbstractHurtingProjectile.java:46-50)
    /// followed by `assignDirectionalMovement(direction, accelerationPower)` (:180-182).
    ///
    /// The shooter is passed as an id rather than an `Entity` because vanilla positions this
    /// projectile explicitly with `snapTo` (DragonStrafePlayerPhase.java:76) instead of
    /// leaving it at the shooter's eye position.
    #[must_use]
    pub fn new_shot(entity: Entity, owner_id: i32, direction: Vector3<f64>) -> Self {
        let mut this = Self::new(entity);
        this.thrown.owner_id = Some(owner_id);
        // `Projectile.getAddEntityPacket` (`Projectile.java:346-349`): the spawn packet's
        // generic "data" int carries the owner's entity id.
        this.thrown.entity.data.store(owner_id, Ordering::Relaxed);
        this.thrown
            .entity
            .velocity
            .store(direction.normalize() * INITIAL_ACCELERATION_POWER);
        this
    }

    pub fn get_acceleration_power(&self) -> f64 {
        f64::from_bits(self.acceleration_power.load(Ordering::Relaxed))
    }

    pub fn set_acceleration_power(&self, power: f64) {
        self.acceleration_power
            .store(power.to_bits(), Ordering::Relaxed);
    }

    /// `AbstractHurtingProjectile.onDeflection` (AbstractHurtingProjectile.java:185-193).
    pub fn on_deflection(&self, _deflection: &ProjectileDeflectionType, by_attack: bool) {
        if by_attack {
            self.set_acceleration_power(INITIAL_ACCELERATION_POWER);
        } else {
            let current = self.get_acceleration_power();
            self.set_acceleration_power(current * DEFLECTION_SCALE);
        }
    }
}

impl NBTStorage for DragonFireballEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            nbt.put_double("acceleration_power", self.get_acceleration_power());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            if let Some(accel) = nbt.get_double("acceleration_power") {
                self.set_acceleration_power(accel);
            }
        })
    }
}

impl EntityBase for DragonFireballEntity {
    fn is_pickable(&self) -> bool {
        true
    }

    fn tick<'a>(
        &'a self,
        caller: &'a Arc<dyn EntityBase>,
        server: &'a Server,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            // `AbstractHurtingProjectile.applyInertia` (AbstractHurtingProjectile.java:102-127).
            let entity = self.get_entity();
            let mut velocity = entity.velocity.load();

            let inertia = if entity.touching_water.load(Ordering::Relaxed) {
                WATER_INERTIA
            } else {
                AIR_INERTIA
            };

            let accel = self.get_acceleration_power();
            let speed = velocity.length();
            if speed > 1e-6 {
                let norm = velocity.normalize();
                velocity = norm
                    .multiply(accel, accel, accel)
                    .add(&velocity)
                    .multiply(inertia, inertia, inertia);
                entity.velocity.store(velocity);
            }

            self.thrown.process_tick(caller, server).await;
        })
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    /// `DragonFireball.onHit` (DragonFireball.java:29-60).
    ///
    /// Deviation: vanilla only `discard()`s inside the guarded branch, so a fireball that
    /// lands on its own shooter keeps flying. `ThrownItemEntity::process_tick` removes the
    /// projectile after every hit, so here the owner hit merely produces no cloud.
    fn on_hit(&self, hit: ProjectileHit) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            let world = entity.world.load();

            let hit_entity_id = match &hit {
                ProjectileHit::Entity { entity, .. } => Some(entity.get_entity().entity_id),
                ProjectileHit::Block { .. } => None,
            };
            if !should_splash(hit_entity_id, self.thrown.owner_id) {
                return;
            }

            let impact = hit.hit_pos();
            let dimension = entity.entity_dimension.load();
            let splash_box = BoundingBox::new_from_pos(impact.x, impact.y, impact.z, &dimension)
                .expand(
                    f64::from(SPLASH_RANGE),
                    SPLASH_VERTICAL_RANGE,
                    f64::from(SPLASH_RANGE),
                );
            // `getEntitiesOfClass(LivingEntity.class, ...)`: ender dragon parts report no
            // living entity, so they drop out here the way they do in vanilla.
            let nearby: Vec<Vector3<f64>> = world
                .get_all_at_box(&splash_box)
                .into_iter()
                .filter(|other| other.get_living_entity().is_some())
                .map(|other| other.get_entity().pos.load())
                .collect();
            let cloud_pos = cloud_position(impact, &nearby);

            // `level().levelEvent(2006, blockPosition(), isSilent() ? -1 : 1)` (DragonFireball.java:55).
            world.sync_world_event(
                WorldEvent::ParticlesDragonFireballSplash,
                BlockPos::floored_v(impact),
                i32::from(if entity.is_silent() { -1i8 } else { 1i8 }),
            );

            let cloud_entity =
                Entity::new(world.clone(), cloud_pos, &EntityType::AREA_EFFECT_CLOUD);
            // DragonFireball.java:40-45 uses DragonBreath with power 1 and a 0.25 potion
            // duration scale for the spawned area-effect cloud.
            let cloud = AreaEffectCloudEntity::create_with_options(
                cloud_entity,
                pumpkin_data::item_stack::ItemStack::new(
                    0,
                    &pumpkin_data::item::Item::DRAGON_BREATH,
                ),
                vec![(
                    &pumpkin_data::effect::StatusEffect::INSTANT_DAMAGE,
                    CLOUD_EFFECT_DURATION,
                    CLOUD_EFFECT_AMPLIFIER,
                    false,
                    true,
                    true,
                )],
                CLOUD_DURATION,
                CLOUD_RADIUS,
                CLOUD_REAPPLICATION_DELAY,
                CLOUD_WAIT_TIME,
                0.0,
                0,
                cloud_radius_per_tick(),
                Some((
                    pumpkin_protocol::codec::var_int::VarInt(
                        pumpkin_data::particle::Particle::DragonBreath as i32,
                    ),
                    1.0f32.to_be_bytes().to_vec(),
                )),
                0.25,
            );
            world.spawn_entity(cloud).await;
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn splash_range_matches_vanilla() {
        assert!((SPLASH_RANGE - 4.0).abs() < f32::EPSILON);
        assert!((SPLASH_VERTICAL_RANGE - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn radius_per_tick_grows_three_to_seven_over_the_duration() {
        let per_tick = cloud_radius_per_tick();
        assert!((per_tick - (7.0 - 3.0) / 600.0).abs() < f32::EPSILON);
        let final_radius = CLOUD_RADIUS + per_tick * CLOUD_DURATION as f32;
        assert!((final_radius - CLOUD_MAX_RADIUS).abs() < 1e-4);
    }

    #[test]
    fn owner_hit_produces_no_cloud() {
        assert!(!should_splash(Some(7), Some(7)));
    }

    #[test]
    fn non_owner_and_block_hits_splash() {
        assert!(should_splash(Some(8), Some(7)));
        assert!(should_splash(None, Some(7)));
        assert!(should_splash(Some(8), None));
        assert!(should_splash(None, None));
    }

    #[test]
    fn cloud_stays_at_impact_without_a_nearby_living_entity() {
        let impact = Vector3::new(10.0, 64.0, 10.0);
        assert_eq!(cloud_position(impact, &[]), impact);
        // 4.5 blocks away: inside the 4/2/4 splash box on x, but outside the 16.0 radius.
        let far = Vector3::new(14.5, 64.0, 10.0);
        assert_eq!(cloud_position(impact, &[far]), impact);
    }

    #[test]
    fn cloud_snaps_to_the_first_living_entity_inside_four_blocks() {
        let impact = Vector3::new(10.0, 64.0, 10.0);
        let far = Vector3::new(14.5, 64.0, 10.0);
        let near = Vector3::new(11.0, 64.0, 11.0);
        let nearer = Vector3::new(10.5, 64.0, 10.0);
        // vanilla breaks on the first match in iteration order, not on the closest.
        assert_eq!(cloud_position(impact, &[far, near, nearer]), near);
    }
}
