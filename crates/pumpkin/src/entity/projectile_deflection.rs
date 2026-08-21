use pumpkin_data::sound::{Sound, SoundCategory};

use crate::entity::EntityBase;
use std::sync::atomic::Ordering;

#[derive(Clone, Copy)]
pub enum ProjectileDeflectionType {
    None,
    Simple,
    Redirected,
    TransferVelocityDirection,
    /// `Breeze.PROJECTILE_DEFLECTION` (`Breeze.java:62-66`): the breeze deflect sound, then
    /// `ProjectileDeflection.REVERSE` -- which is exactly what `Simple` already implements.
    BreezeDeflect,
}

impl ProjectileDeflectionType {
    /// Takes the projectile by shared reference: every field written here lives behind an
    /// atomic, and the dispatch site only ever holds an `Arc<dyn EntityBase>`.
    pub fn deflect(&self, projectile: &dyn EntityBase, hit_entity: Option<&dyn EntityBase>) {
        match self {
            Self::None => {}
            Self::BreezeDeflect => {
                if let Some(hit_entity) = hit_entity {
                    let entity = hit_entity.get_entity();
                    entity.world.load().play_sound(
                        Sound::EntityBreezeDeflect,
                        SoundCategory::Hostile,
                        &entity.pos.load(),
                    );
                }
                Self::Simple.deflect(projectile, hit_entity);
            }
            Self::Simple => {
                let vel = rand::random::<f32>().mul_add(20.0, 170.0);

                let current_velocity = projectile
                    .get_entity()
                    .velocity
                    .load()
                    .multiply(-0.5, -0.5, -0.5);

                let entity = projectile.get_entity();
                entity.velocity.store(current_velocity);

                let yaw = entity.yaw.load() + vel;
                let pitch = entity.pitch.load();
                entity.set_rotation(yaw, pitch);
                // TODO: Add entity.lastYaw += vel
                entity.velocity_dirty.store(true, Ordering::Relaxed);
            }
            Self::Redirected => {
                if let Some(hit_entity) = hit_entity {
                    let rotation_vector = hit_entity.get_entity().rotation();

                    let entity = projectile.get_entity();
                    entity.velocity.store(rotation_vector.to_f64());
                    entity.velocity_dirty.store(true, Ordering::Relaxed);
                }
            }
            Self::TransferVelocityDirection => {
                if let Some(hit_entity) = hit_entity {
                    let hit_velocity = hit_entity.get_entity().velocity.load().normalize();

                    let entity = projectile.get_entity();
                    entity.velocity.store(hit_velocity);
                    entity.velocity_dirty.store(true, Ordering::Relaxed);
                }
            }
        }
    }
}
