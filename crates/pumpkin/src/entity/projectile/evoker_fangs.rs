use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::sound::Sound;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{Entity, EntityBase};
use crate::server::Server;
use crate::world::World;

/// Vanilla: `EvokerFangs`. A short-lived hazard that telegraphs for `warmup_delay_ticks`, then
/// bites once and lingers briefly before despawning.
pub struct EvokerFangsEntity {
    pub entity: Entity,
    pub warmup_ticks: AtomicU32,
    pub life_ticks: AtomicU32,
    owner_id: Option<i32>,
    pub has_bitten: AtomicBool,
}

impl EvokerFangsEntity {
    /// Used when restoring from disk or `/summon`, where there is no casting owner.
    pub const fn orphan(entity: Entity) -> Self {
        Self {
            entity,
            warmup_ticks: AtomicU32::new(0),
            life_ticks: AtomicU32::new(0),
            owner_id: None,
            has_bitten: AtomicBool::new(false),
        }
    }

    /// Vanilla: `EvokerFangs(Level, double, double, double, float, int, LivingEntity)`.
    /// `yaw_radians` matches the angle vanilla passes in (it stores yaw in degrees internally).
    pub fn new_spell(
        world: &Arc<World>,
        pos: Vector3<f64>,
        yaw_radians: f32,
        warmup_delay_ticks: i32,
        owner: &Entity,
    ) -> Self {
        let entity = Entity::new(world.clone(), pos, &EntityType::EVOKER_FANGS);
        entity.set_rotation(yaw_radians.to_degrees(), 0.0);
        Self {
            entity,
            warmup_ticks: AtomicU32::new(warmup_delay_ticks.max(0) as u32),
            life_ticks: AtomicU32::new(0),
            owner_id: Some(owner.entity_id),
            has_bitten: AtomicBool::new(false),
        }
    }

    /// Constructor used by `EvokerEntity`'s fang volleys: the entity is built by the
    /// caller, `yaw_radians` matches vanilla's constructor argument.
    #[must_use]
    pub fn new(
        entity: Entity,
        warmup_delay_ticks: u32,
        yaw_radians: f32,
        owner_id: Option<i32>,
    ) -> Self {
        entity.set_rotation(yaw_radians.to_degrees(), 0.0);
        Self {
            entity,
            warmup_ticks: AtomicU32::new(warmup_delay_ticks),
            life_ticks: AtomicU32::new(0),
            owner_id,
            has_bitten: AtomicBool::new(false),
        }
    }

    fn bite_nearby(&self, caller: &dyn EntityBase) {
        let world = self.entity.world.load();
        let bb = self.entity.bounding_box.load().expand(0.2, 0.0, 0.2);
        let owner = self.owner_id.and_then(|id| world.get_entity_by_id(id));

        for candidate in world.get_all_at_box(&bb) {
            let target_entity = candidate.get_entity();
            if candidate.get_living_entity().is_none()
                || !target_entity.is_alive()
                || target_entity.invulnerable.load(Relaxed)
                || Some(target_entity.entity_id) == self.owner_id
            {
                continue;
            }

            // Scope reduction: vanilla also skips the target when `currentOwner.isAlliedTo`
            // (shared raid/illager team), which Pumpkin has no team/alliance system to check
            // here; only the direct owner is exempted.
            let damage_type = if owner.is_some() {
                DamageType::INDIRECT_MAGIC
            } else {
                DamageType::MAGIC
            };
            candidate
                .damage_with_context(
                    candidate.as_ref(),
                    6.0,
                    damage_type,
                    None,
                    owner.as_deref(),
                    Some(caller),
                );
        }
    }
}

impl EntityBase for EvokerFangsEntity {
    /// Vanilla `EvokerFangs.addAdditionalSaveData` persists only the warmup counter.
    fn write_custom_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
        nbt.put_int("Warmup", self.warmup_ticks.load(Ordering::Relaxed) as i32);
    }

    fn read_custom_nbt(&self, nbt: &pumpkin_nbt::compound::NbtCompound) {
        if let Some(warmup) = nbt.get_int("Warmup") {
            self.warmup_ticks.store(warmup.max(0) as u32, Ordering::Relaxed);
        }
    }
    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = &self.entity;
        let world = entity.world.load();

        let warmup = self.warmup_ticks.load(Ordering::Relaxed);
        let life = self.life_ticks.fetch_add(1, Ordering::Relaxed) + 1;
        if life >= warmup {
            if life == warmup {
                entity.play_sound(Sound::EntityEvokerFangsAttack);
                world.send_entity_status(&self.entity, EntityStatus::StartAttacking, None);
            }
            // Vanilla's server-side bite occurs eight ticks after the warmup ends.
            if life == warmup.saturating_add(8)
                && !self.has_bitten.swap(true, Ordering::SeqCst)
            {
                self.bite_nearby(caller);
            }
            if life > warmup.saturating_add(30) {
                entity.remove();
            }
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
