use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::entity::EntityType;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::CEntityStatus;
use pumpkin_util::math::position::BlockPos;

use crate::{
    block::entities::mob_spawner::{
        BaseSpawnerConfig, MobSpawnerBlockEntity, base_spawner_server_tick,
    },
    entity::Entity,
    world::World,
};

/// Port of vanilla `MinecartSpawner`
/// (net/minecraft/world/entity/vehicle/minecart/MinecartSpawner.java:16-77): a
/// minecart carrying an anonymous `BaseSpawner` field
/// (MinecartSpawner.java:17-22) that is server-ticked every entity tick at the
/// cart's current block position (MinecartSpawner.java:40-44, 68-72) instead of
/// a fixed block position.
pub(super) struct SpawnerMinecart {
    /// `BaseSpawner.spawnDelay` (BaseSpawner.java:48); the -1 sentinel seeds a
    /// fresh delay (BaseSpawner.java:90-92).
    delay: AtomicI32,
    config: AtomicCell<BaseSpawnerConfig>,
    entity_type: AtomicCell<Option<&'static EntityType>>,
}

impl SpawnerMinecart {
    pub(super) const fn new() -> Self {
        Self {
            // `BaseSpawner` field initialisers (BaseSpawner.java:41-47, 48-59).
            delay: AtomicI32::new(MobSpawnerBlockEntity::DEFAULT_DELAY),
            config: AtomicCell::new(BaseSpawnerConfig {
                min_delay: MobSpawnerBlockEntity::DEFAULT_MIN_SPAWN_DELAY,
                max_delay: MobSpawnerBlockEntity::DEFAULT_MAX_SPAWN_DELAY,
                spawn_count: MobSpawnerBlockEntity::DEFAULT_SPAWN_COUNT,
                spawn_range: MobSpawnerBlockEntity::DEFAULT_SPAWN_RANGE,
                max_nearby_entities: MobSpawnerBlockEntity::DEFAULT_MAX_NEARBY_ENTITIES,
                required_player_range: MobSpawnerBlockEntity::DEFAULT_REQUIRED_PLAYER_RANGE,
            }),
            entity_type: AtomicCell::new(None),
        }
    }

    /// Saved through `MinecartSpawner.addAdditionalSaveData`
    /// (MinecartSpawner.java:57-61) → `BaseSpawner.save`
    /// (BaseSpawner.java:220-230).
    pub(super) fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_short("Delay", self.delay.load(Ordering::Relaxed) as i16);
        let config = self.config.load();
        nbt.put_short("MinSpawnDelay", config.min_delay as i16);
        nbt.put_short("MaxSpawnDelay", config.max_delay as i16);
        nbt.put_short("SpawnCount", config.spawn_count as i16);
        nbt.put_short("MaxNearbyEntities", config.max_nearby_entities as i16);
        nbt.put_short("RequiredPlayerRange", config.required_player_range as i16);
        nbt.put_short("SpawnRange", config.spawn_range as i16);
        if let Some(entity_type) = self.entity_type.load() {
            let mut spawn_entry = NbtCompound::new();

            let mut entity_nbt = NbtCompound::new();
            entity_nbt.put_string("id", format!("minecraft:{}", entity_type.resource_name));

            spawn_entry.put_compound("entity", entity_nbt);

            nbt.put_compound("SpawnData", spawn_entry);
        }
    }

    /// Loaded through `MinecartSpawner.readAdditionalSaveData`
    /// (MinecartSpawner.java:51-55) → `BaseSpawner.load`
    /// (BaseSpawner.java:206-218).
    pub(super) fn read_nbt(&self, nbt: &NbtCompound) {
        self.delay.store(
            nbt.get_short("Delay")
                .unwrap_or(MobSpawnerBlockEntity::DEFAULT_DELAY as i16) as i32,
            Ordering::Relaxed,
        );
        let mut config = self.config.load();
        config.min_delay = nbt
            .get_short("MinSpawnDelay")
            .unwrap_or(MobSpawnerBlockEntity::DEFAULT_MIN_SPAWN_DELAY as i16)
            as i32;
        config.max_delay = nbt
            .get_short("MaxSpawnDelay")
            .unwrap_or(MobSpawnerBlockEntity::DEFAULT_MAX_SPAWN_DELAY as i16)
            as i32;
        config.spawn_count =
            nbt.get_short("SpawnCount")
                .unwrap_or(MobSpawnerBlockEntity::DEFAULT_SPAWN_COUNT as i16) as i32;
        config.max_nearby_entities = nbt
            .get_short("MaxNearbyEntities")
            .unwrap_or(MobSpawnerBlockEntity::DEFAULT_MAX_NEARBY_ENTITIES as i16)
            as i32;
        config.required_player_range = nbt
            .get_short("RequiredPlayerRange")
            .unwrap_or(MobSpawnerBlockEntity::DEFAULT_REQUIRED_PLAYER_RANGE as i16)
            as i32;
        config.spawn_range =
            nbt.get_short("SpawnRange")
                .unwrap_or(MobSpawnerBlockEntity::DEFAULT_SPAWN_RANGE as i16) as i32;
        self.config.store(config);
        self.entity_type.store(
            nbt.get_compound("SpawnData")
                .and_then(|data| data.get_compound("entity"))
                .and_then(|entity| entity.get_string("id"))
                .and_then(|id| {
                    let name = id.strip_prefix("minecraft:").unwrap_or(id);
                    EntityType::from_name(name)
                }),
        );
    }

    /// `MinecartSpawner.tick` (MinecartSpawner.java:68-72): run the carried
    /// spawner's server tick each cart tick from the cart's current block
    /// position.
    ///
    /// When the shared core reports that vanilla reached a `delay()` call site,
    /// the notification goes out as an *entity* event on the cart, because
    /// `MinecartSpawner.broadcastEvent` (MinecartSpawner.java:19-21) overrides
    /// the `BaseSpawner` notification with
    /// `level.broadcastEntityEvent(this, (byte) id)`; clients route it back in
    /// through `handleEntityEvent` (MinecartSpawner.java:63-66) →
    /// `onEventTriggered` (BaseSpawner.java:249-259).
    pub(super) async fn tick(&self, world: &Arc<World>, position: BlockPos, entity: &Entity) {
        if base_spawner_server_tick(
            world,
            position,
            &self.delay,
            self.config.load(),
            self.entity_type.load(),
        )
        .await
        {
            world.broadcast_packet_all(&CEntityStatus::new(entity.entity_id, 1));
        }
    }
}
