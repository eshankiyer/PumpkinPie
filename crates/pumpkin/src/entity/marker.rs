use std::sync::Mutex;
use std::sync::{Arc, atomic::Ordering};

use crate::{
    entity::{Entity, EntityBase, EntityBaseFuture, NBTStorage, living::LivingEntity},
    net::{bedrock::BedrockClient, java::JavaClient},
    server::Server,
};
use pumpkin_data::damage::DamageType;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::math::vector3::Vector3;

/// `minecraft:marker`.
///
/// Vanilla `Marker` (`Marker.java`) has empty overrides for `tick`,
/// `defineSynchedData`, `readAdditionalSaveData`, `addAdditionalSaveData`
/// (:20-34), sets `noPhysics = true` in its constructor (:17), and `hurtServer`
/// is `final` and always returns false (:67-69). Its `getAddEntityPacket`
/// override (:37-39) throws because vanilla never tracks it to clients
/// (`EntityTypes.java:662-663`, `clientTrackingRange(0)`); Pumpkin has no
/// per-type client tracking range, so the spawn packets are suppressed directly
/// in `send_java_spawn_packet`/`send_bedrock_spawn_packet` below.
pub struct MarkerEntity {
    pub entity: Entity,
    pub data: Mutex<NbtCompound>,
}

impl MarkerEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        entity.no_physics.store(true, Ordering::Relaxed);
        Arc::new(Self {
            entity,
            data: Mutex::new(NbtCompound::new()),
        })
    }
}

impl NBTStorage for MarkerEntity {
    fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.write_nbt(nbt);
        let data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !data.is_empty() {
            nbt.put("data", NbtTag::Compound(data.clone()));
        }
    }

    fn read_nbt(&mut self, nbt: &mut NbtCompound) {
        self.entity.read_nbt(nbt);
        if let Some(data) = nbt.get_compound("data") {
            *self
                .data
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = data.clone();
        }
    }

    fn read_nbt_non_mut(&self, nbt: &NbtCompound) {
        self.entity.read_nbt_non_mut(nbt);
        if let Some(data) = nbt.get_compound("data") {
            *self
                .data
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = data.clone();
        }
    }
}

impl EntityBase for MarkerEntity {
    fn tick(&self, _caller: &Arc<dyn EntityBase>, _server: &Server) {}

    fn init_data_tracker(&self) {}

    fn get_entity(&self) -> &Entity {
        &self.entity
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

    fn is_pushable(&self) -> bool {
        false
    }

    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn can_hit(&self) -> bool {
        false
    }

    fn is_immune_to_explosion(&self) -> bool {
        true
    }

    /// Mirrors vanilla's `final hurtServer` (`Marker.java:67-69`): always rejects damage.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        false
    }

    fn send_java_spawn_packet<'a>(&'a self, _client: &'a JavaClient) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {})
    }

    fn send_bedrock_spawn_packet<'a>(
        &'a self,
        _client: &'a BedrockClient,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {})
    }
}
