use pumpkin_util::math::vector3::Vector3;
use std::{
    f64,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

use crate::{
    entity::{
        Entity, EntityBase, EntityBaseFuture, NBTStorage,
        living::LivingEntity,
        projectile::{ProjectileHit, ThrownItemEntity},
        projectile_deflection::ProjectileDeflectionType,
    },
    server::Server,
};

// WindCharge.java RADIUS = 1.2F.
const EXPLOSION_POWER: f32 = 1.2;
// BreezeWindCharge.java RADIUS = 3.0F.
const BREEZE_EXPLOSION_POWER: f32 = 3.0;
const DEFAULT_DEFLECT_COOLDOWN: u8 = 5;
/// `AbstractWindCharge.JUMP_SCALE` (`AbstractWindCharge.java:33`): the distance the block-hit
/// explosion is pushed back out along the hit face.
const JUMP_SCALE: f64 = 0.25;
/// `AbstractWindCharge.tick` (`AbstractWindCharge.java:149`) explodes above `getMaxY() + 30`.
const MAX_Y_EXPLODE_MARGIN: i32 = 30;
pub const WIND_CHARGE_GRAVITY: f64 = 0.0;

/// A kind to differentiate both types of wind charges from each other.
enum WindChargeKind {
    /// Represents a wind charge spawned by a player or dispenser.
    /// This wind charge also has a deflect cooldown counter.
    Normal { deflect_cooldown: AtomicU8 },
    /// Represents a wind charge spawned by a breeze.
    Breeze,
}

pub struct WindChargeEntity {
    kind: WindChargeKind,
    pub thrown_item_entity: ThrownItemEntity,
}

use crate::world::SimpleExplosionDamageCalculator;
use pumpkin_data::tag;
use std::sync::LazyLock;

pub static WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR: LazyLock<Arc<SimpleExplosionDamageCalculator>> =
    LazyLock::new(|| {
        Arc::new(SimpleExplosionDamageCalculator::new(
            true,
            false,
            Some(1.22),
            Some(&tag::Block::MINECRAFT_BLOCKS_WIND_CHARGE_EXPLOSIONS),
        ))
    });

pub static BREEZE_WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR: LazyLock<
    Arc<SimpleExplosionDamageCalculator>,
> = LazyLock::new(|| {
    Arc::new(SimpleExplosionDamageCalculator::new(
        true,
        false,
        None,
        Some(&tag::Block::MINECRAFT_BLOCKS_WIND_CHARGE_EXPLOSIONS),
    ))
});

impl WindChargeEntity {
    #[must_use]
    pub const fn new_normal(thrown_item_entity: ThrownItemEntity) -> Self {
        Self {
            kind: WindChargeKind::Normal {
                deflect_cooldown: AtomicU8::new(DEFAULT_DEFLECT_COOLDOWN),
            },
            thrown_item_entity,
        }
    }

    #[must_use]
    pub const fn new_breeze(thrown_item_entity: ThrownItemEntity) -> Self {
        Self {
            kind: WindChargeKind::Breeze,
            thrown_item_entity,
        }
    }

    pub const fn deflect_cooldown(&self) -> Option<&AtomicU8> {
        if let WindChargeKind::Normal {
            deflect_cooldown, ..
        } = &self.kind
        {
            Some(deflect_cooldown)
        } else {
            None
        }
    }

    pub async fn create_explosion(&self, position: Vector3<f64>) {
        // WindCharge.java RADIUS = 1.2F vs BreezeWindCharge.java RADIUS = 3.0F.
        let (power, calculator) = match self.kind {
            WindChargeKind::Normal { .. } => (
                EXPLOSION_POWER,
                WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR.clone(),
            ),
            WindChargeKind::Breeze => (
                BREEZE_EXPLOSION_POWER,
                BREEZE_WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR.clone(),
            ),
        };
        self.get_entity()
            .world
            .load()
            // `AbstractWindCharge.explode` passes the wind charge as the explosion source
            // (`AbstractWindCharge.java:92-109`; `WindCharge.java:23-27`).
            .explode_with_calculator_from(
                position,
                power,
                crate::world::ExplosionInteraction::Trigger,
                Some(self.get_entity().entity_type),
                Some(calculator),
            )
            .await;
    }

    /// Sets this projectile's velocity from a direction vector, power, and spread.
    /// Mirrors `Projectile.spawnProjectileUsingShoot`.
    pub fn set_velocity(&self, x: f64, y: f64, z: f64, power: f64, uncertainty: f64) {
        self.thrown_item_entity
            .set_velocity(x, y, z, power, uncertainty);
    }

    pub fn deflect(
        &mut self,
        deflection: &ProjectileDeflectionType,
        deflector: Option<&dyn EntityBase>,
        _from_attack: bool,
    ) -> bool {
        if let Some(cooldown) = self.deflect_cooldown()
            && cooldown.load(Ordering::Relaxed) > 0
        {
            return false;
        }

        deflection.deflect(self, deflector);

        /* TODO: Does this need to be implemented?
        if self.get_entity().world().is_client() {
            self.set_owner();
            self.on_Deflected(from_attack);
        }
         */
        true
    }
}

impl NBTStorage for WindChargeEntity {}

impl EntityBase for WindChargeEntity {
    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn is_pickable(&self) -> bool {
        true
    }

    fn tick<'a>(
        &'a self,
        caller: &'a Arc<dyn EntityBase>,
        server: &'a Server,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            // `AbstractWindCharge.tick` (`AbstractWindCharge.java:148-155`): a wind charge more
            // than 30 blocks above the build limit explodes where it is instead of flying on
            // forever. It has no gravity and full inertia, so nothing else ever stops it.
            let entity = self.get_entity();
            let world = entity.world.load();
            if entity.block_pos.load().0.y > world.get_top_y() + MAX_Y_EXPLODE_MARGIN {
                let pos = entity.pos.load();
                self.create_explosion(pos).await;
                entity.remove().await;
                return;
            }

            self.thrown_item_entity.process_tick(caller, server).await;

            if let Some(cooldown) = self.deflect_cooldown() {
                let cooldown_ticks = cooldown.load(Ordering::Relaxed);
                if cooldown_ticks > 0 {
                    cooldown.store(cooldown_ticks - 1, Ordering::Relaxed);
                }
            }
        })
    }

    fn get_entity(&self) -> &Entity {
        &self.thrown_item_entity.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }
    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: ProjectileHit) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let explosion_pos = match &hit {
                ProjectileHit::Entity { entity, .. } => {
                    let _ = entity
                        .damage(self, 1.0, pumpkin_data::damage::DamageType::WIND_CHARGE)
                        .await;
                    // `AbstractWindCharge.onHitEntity` (`AbstractWindCharge.java:92`) explodes at
                    // `this.position()` - the projectile's own position, not the hit location.
                    self.get_entity().pos.load()
                }
                // `AbstractWindCharge.onHitBlock` (`AbstractWindCharge.java:106-108`) shifts the
                // impact point by `JUMP_SCALE` along the hit face's unit vector, putting the
                // explosion centre just outside the block instead of inside it.
                ProjectileHit::Block { face, hit_pos, .. } => {
                    let offset = face.to_offset();
                    hit_pos.add_raw(
                        f64::from(offset.x) * JUMP_SCALE,
                        f64::from(offset.y) * JUMP_SCALE,
                        f64::from(offset.z) * JUMP_SCALE,
                    )
                }
            };
            self.create_explosion(explosion_pos).await;
        })
    }
}
