use std::sync::atomic::{AtomicBool, Ordering};

use std::sync::Arc;

use pumpkin_data::{Block, damage::DamageType, effect::StatusEffect, potion::Effect, tag::Taggable, tracked_data};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::{Difficulty, math::vector3::Vector3};

use crate::{
    entity::{
        Entity, EntityBase,
        projectile::{ProjectileHit, ThrownItemEntity},
    },
    server::Server,
    world::{World, ExplosionInteraction, explosion::{DefaultExplosionDamageCalculator, Explosion, ExplosionDamageCalculator}},
};

/// `WitherSkull.getBlockExplosionResistance` (`WitherSkull.java:50-55`) applies the same
/// protection as `WitherBoss.canDestroy` (`WitherBoss.java:355-357`): dangerous skulls cap
/// destructible block resistance at 0.8, while wither-immune blocks remain protected.
struct WitherSkullExplosionCalculator {
    dangerous: bool,
}

impl ExplosionDamageCalculator for WitherSkullExplosionCalculator {
    fn get_block_explosion_resistance(
        &self,
        explosion: &Explosion,
        world: &World,
        pos: &pumpkin_util::math::position::BlockPos,
        block: &Block,
        fluid: &pumpkin_data::fluid::FluidState,
    ) -> Option<f32> {
        let resistance = DefaultExplosionDamageCalculator
            .get_block_explosion_resistance(explosion, world, pos, block, fluid)?;
        if self.dangerous
            && !block.default_state.is_air()
            && !block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_WITHER_IMMUNE)
        {
            Some(resistance.min(0.8))
        } else {
            Some(resistance)
        }
    }
}

/// `AbstractHurtingProjectile.applyInertia` (`AbstractHurtingProjectile.java:102-127`) uses
/// acceleration followed by the inertia selected by `getInertia` (`AbstractHurtingProjectile.java:155-157`).
pub const AIR_INERTIA: f64 = 0.95;
/// `AbstractHurtingProjectile.getLiquidInertia` (`AbstractHurtingProjectile.java:159-161`).
pub const WATER_INERTIA: f64 = 0.8;
/// `WitherSkull.getInertia` (`WitherSkull.java:41-44`).
pub const DANGEROUS_INERTIA: f64 = 0.73;
/// `AbstractHurtingProjectile.accelerationPower` (`AbstractHurtingProjectile.java:24`).
const ACCELERATION_POWER: f64 = 0.1;

const GRAVITY: f64 = 0.0;
const OWNER_DAMAGE: f32 = 8.0;
const NO_OWNER_DAMAGE: f32 = 5.0;
const OWNER_HEAL_ON_KILL: f32 = 5.0;

pub struct WitherSkullEntity {
    pub thrown: ThrownItemEntity,
    pub dangerous: AtomicBool,
}

impl WitherSkullEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner_id: None,
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: GRAVITY,
        };

        Self {
            thrown,
            dangerous: AtomicBool::new(false),
        }
    }

    #[must_use]
    pub fn new_shot(
        entity: Entity,
        shooter: &Entity,
        dangerous: bool,
        direction: Vector3<f64>,
    ) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        let speed = 0.95;
        let vel = direction.normalize().multiply(speed, speed, speed);
        thrown.entity.velocity.store(vel);

        let len = vel.horizontal_length();
        thrown.entity.set_rotation(
            vel.x.atan2(vel.z) as f32 * 57.295_776,
            vel.y.atan2(len) as f32 * 57.295_776,
        );

        Self {
            thrown,
            dangerous: AtomicBool::new(dangerous),
        }
    }

    #[must_use]
    pub fn is_dangerous(&self) -> bool {
        self.dangerous.load(Ordering::Relaxed)
    }

    pub fn set_dangerous(&self, dangerous: bool) {
        self.dangerous.store(dangerous, Ordering::Relaxed);
        self.thrown.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::wither_skull::DATA_DANGEROUS,
                dangerous,
            )],
            None,
        );
    }
}

impl EntityBase for WitherSkullEntity {
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("dangerous", self.is_dangerous());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(dangerous) = nbt.get_bool("dangerous") {
            self.dangerous.store(dangerous, Ordering::Relaxed);
        }
    }

    fn init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                tracked_data::wither_skull::DATA_DANGEROUS,
                self.is_dangerous(),
            )],
            None,
        );
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = self.get_entity();
        let velocity = entity.velocity.load();
        let inertia = if entity.touching_water.load(Ordering::Relaxed) {
            WATER_INERTIA
        } else if self.is_dangerous() {
            DANGEROUS_INERTIA
        } else {
            AIR_INERTIA
        };
        let velocity = velocity
            .normalize()
            .multiply(ACCELERATION_POWER, ACCELERATION_POWER, ACCELERATION_POWER)
            .add(&velocity)
            .multiply(inertia, inertia, inertia);
        entity.velocity.store(velocity);
        self.thrown.process_tick(caller);
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: ProjectileHit) {
        let world = self.get_entity().world.load();

        if let ProjectileHit::Entity { ref entity, .. } = hit {
            let owner = self
                .thrown
                .owner_id
                .and_then(|id| world.get_entity_by_id(id));
            let owner_living = owner.as_ref().and_then(|o| o.get_living_entity());
            let was_hurt = if let Some(owner_living) = owner_living {
                let hurt = entity.damage(self, OWNER_DAMAGE, DamageType::WITHER_SKULL);
                if hurt && !entity.get_entity().is_alive() {
                    owner_living.heal(OWNER_HEAL_ON_KILL);
                }
                hurt
            } else {
                entity.damage(self, NO_OWNER_DAMAGE, DamageType::MAGIC)
            };

            if was_hurt && let Some(living) = entity.get_living_entity() {
                let difficulty = world.level_info.load().difficulty;
                let duration = match difficulty {
                    Difficulty::Hard => 800,   // 40 seconds
                    Difficulty::Normal => 200, // 10 seconds
                    Difficulty::Easy | Difficulty::Peaceful => 0,
                };

                if duration > 0 {
                    let effect = Effect {
                        effect_type: &StatusEffect::WITHER,
                        duration,
                        amplifier: 1,
                        ambient: false,
                        show_particles: true,
                        show_icon: true,
                        blend: true,
                    };
                    if let Some(player) = entity.get_player() {
                        player.add_effect(effect.clone());
                    }
                    living.add_effect(effect);
                }
            }
        }

        let hit_pos = hit.hit_pos();
        world.explode_with_calculator(
            hit_pos,
            1.0,
            ExplosionInteraction::Mob,
            Some(Arc::new(WitherSkullExplosionCalculator {
                dangerous: self.is_dangerous(),
            })),
        );
    }
}
