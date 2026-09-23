use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_util::math::vector3::Vector3;

use crate::{
    entity::{
        Entity, EntityBase, NBTStorage,
        living::LivingEntity,
        projectile::{ProjectileHit, ThrownItemEntity},
    },
    server::Server,
};

/// `LlamaSpit.getDefaultGravity` (`LlamaSpit.java:37-40`).
pub const LLAMA_SPIT_GRAVITY: f64 = 0.06;

/// A llama's ranged spit attack. `LlamaSpit.java`.
pub struct LlamaSpitEntity {
    pub thrown: ThrownItemEntity,
}

impl LlamaSpitEntity {
    /// Bare constructor for generic entity-type lookup (`entity::type::from_type`), with no
    /// owner and a resting velocity -- mirrors `SnowballEntity::new`.
    pub fn new(entity: Entity) -> Self {
        entity.set_velocity(Vector3::new(0.0, 0.1, 0.0));
        let thrown = ThrownItemEntity {
            entity,
            owner_id: None,
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: LLAMA_SPIT_GRAVITY,
        };
        Self { thrown }
    }

    /// `LlamaSpit(Level, Llama)` (`LlamaSpit.java:29-35`): the spit spawns beside the shooter's
    /// mouth, offset `(bbWidth + 1) * 0.5` along the body yaw, at `eyeY - 0.1`.
    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, LLAMA_SPIT_GRAVITY);

        let owner_pos = shooter.pos.load();
        let body_yaw_rad = f64::from(shooter.body_yaw.load()).to_radians();
        let bb_width = f64::from(shooter.entity_dimension.load().width);
        let offset = f64::midpoint(bb_width, 1.0);
        let x = owner_pos.x - offset * body_yaw_rad.sin();
        let y = owner_pos.y + shooter.get_eye_height() - 0.1;
        let z = owner_pos.z + offset * body_yaw_rad.cos();
        thrown.entity.pos.store(Vector3::new(x, y, z));

        Self { thrown }
    }
}

impl NBTStorage for LlamaSpitEntity {}

impl EntityBase for LlamaSpitEntity {
    fn tick(&self, caller: &Arc<dyn EntityBase>, server: &Server) {
        // `LlamaSpit.tick` (`LlamaSpit.java:52-55`): discarded once in water.
        if self.get_entity().touching_water.load(Ordering::Relaxed) {
            self.get_entity().remove();
            return;
        }
        self.thrown.process_tick(caller, server);
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    /// `LlamaSpit.onHitEntity` (`LlamaSpit.java:65-72`): 1 damage from the `spit` damage source,
    /// attributed to the owner, and only when the owner is a living entity.
    fn on_hit(&self, hit: ProjectileHit) {
        if let ProjectileHit::Entity {
            ref entity,
            hit_pos,
            ..
        } = hit
        {
            let world = self.get_entity().world.load();
            let Some(owner) = self
                .thrown
                .owner_id
                .and_then(|id| world.get_entity_by_id(id))
                .filter(|owner| owner.get_living_entity().is_some())
            else {
                return;
            };
            let _ = entity.damage_with_context(
                entity.as_ref(),
                1.0,
                DamageType::SPIT,
                Some(hit_pos),
                Some(owner.as_ref()),
                None,
            );
        }
    }
}
