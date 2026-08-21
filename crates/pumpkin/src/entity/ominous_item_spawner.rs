use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering::Relaxed},
};

use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_protocol::java::client::play::{CSetEntityMetadata, Metadata};
use pumpkin_util::math::vector3::Vector3;
use rand::{RngExt, rng};
use tokio::sync::Mutex;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture, item::ItemEntity,
    living::LivingEntity,
};
use crate::server::Server;
use crate::world::game_event::{GameEventContext, emit_game_event};

/// `OminousItemSpawner.SPAWN_ITEM_DELAY_MIN` (`OminousItemSpawner.java:24`).
pub const SPAWN_ITEM_DELAY_MIN: i64 = 60;
/// `OminousItemSpawner.SPAWN_ITEM_DELAY_MAX` (`OminousItemSpawner.java:25`).
pub const SPAWN_ITEM_DELAY_MAX: i64 = 120;
/// `OminousItemSpawner.TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND` (`OminousItemSpawner.java:29`).
pub const TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND: i64 = 36;

const TAG_ITEM: &str = "item";
const TAG_SPAWN_ITEM_AFTER_TICKS: &str = "spawn_item_after_ticks";

/// `minecraft:ominous_item_spawner`.
///
/// The invisible entity an ominous trial spawner uses to hand out its reward: it
/// hovers holding the pending stack, plays
/// `TRIAL_SPAWNER_ABOUT_TO_SPAWN_ITEM` 36 ticks before the drop, then drops the
/// item and removes itself (`OminousItemSpawner.java:53-63`).
///
/// It extends plain `Entity`, not a mob, and sets `noPhysics = true`
/// (`OminousItemSpawner.java:32-35`).
pub struct OminousItemSpawnerEntity {
    pub entity: Entity,
    /// `DATA_ITEM` (`OminousItemSpawner.java:28`), NBT `item`.
    item: Mutex<ItemStack>,
    /// Vanilla `Entity.tickCount`, incremented by `super.tick()` before
    /// `tickServer` reads it. Transient: vanilla never saves it either.
    tick_count: AtomicI64,
    /// `OminousItemSpawner.spawnItemAfterTicks` (`:30`), NBT
    /// `spawn_item_after_ticks`.
    spawn_item_after_ticks: AtomicI64,
}

impl OminousItemSpawnerEntity {
    /// The plain constructor (`OminousItemSpawner.java:32-35`): no item, delay 0.
    /// A restored entity gets both from NBT.
    pub fn new(entity: Entity) -> Arc<Self> {
        Self::new_with(entity, ItemStack::EMPTY.clone(), 0)
    }

    /// `OminousItemSpawner.create` (`OminousItemSpawner.java:37-42`): rolls the
    /// delay and stores the stack to be handed out.
    pub fn create(entity: Entity, item: ItemStack) -> Arc<Self> {
        Self::new_with(entity, item, Self::random_spawn_delay())
    }

    fn new_with(entity: Entity, item: ItemStack, spawn_item_after_ticks: i64) -> Arc<Self> {
        // `this.noPhysics = true` (`OminousItemSpawner.java:34`).
        entity.no_clip.store(true, Relaxed);
        Arc::new(Self {
            entity,
            item: Mutex::new(item),
            tick_count: AtomicI64::new(0),
            spawn_item_after_ticks: AtomicI64::new(spawn_item_after_ticks),
        })
    }

    /// `level.getRandom().nextIntBetweenInclusive(60, 120)`
    /// (`OminousItemSpawner.java:39`) -- inclusive at BOTH ends, 61 possible
    /// values.
    fn random_spawn_delay() -> i64 {
        rng().random_range(SPAWN_ITEM_DELAY_MIN..=SPAWN_ITEM_DELAY_MAX)
    }

    /// The tick on which `TRIAL_SPAWNER_ABOUT_TO_SPAWN_ITEM` plays:
    /// `this.tickCount == this.spawnItemAfterTicks - 36L`
    /// (`OminousItemSpawner.java:54`).
    const fn about_to_spawn_sound_tick(spawn_item_after_ticks: i64) -> i64 {
        spawn_item_after_ticks - TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND
    }

    pub async fn get_item(&self) -> ItemStack {
        self.item.lock().await.clone()
    }

    /// `OminousItemSpawner.addAdditionalSaveData` (`:120-127`): the stack is
    /// only stored when non-empty, the delay always.
    fn write_spawner_nbt(nbt: &mut NbtCompound, item: &ItemStack, spawn_item_after_ticks: i64) {
        if !item.is_empty() {
            let mut item_compound = NbtCompound::new();
            item.write_item_stack(&mut item_compound);
            nbt.put_compound(TAG_ITEM, item_compound);
        }
        nbt.put_long(TAG_SPAWN_ITEM_AFTER_TICKS, spawn_item_after_ticks);
    }

    /// `OminousItemSpawner.readAdditionalSaveData` (`:114-118`). A missing delay
    /// reads back as 0 (`getLongOr(..., 0L)`), which makes the spawner fire on
    /// its very first tick -- that is vanilla's behaviour, not an oversight.
    fn read_spawner_nbt(nbt: &NbtCompound) -> (ItemStack, i64) {
        let item = nbt
            .get_compound(TAG_ITEM)
            .and_then(ItemStack::read_item_stack)
            .unwrap_or_else(|| ItemStack::EMPTY.clone());
        let ticks = nbt.get_long(TAG_SPAWN_ITEM_AFTER_TICKS).unwrap_or(0);
        (item, ticks)
    }

    /// `OminousItemSpawner.spawnItem` (`OminousItemSpawner.java:71-88`).
    ///
    /// TODO: the projectile branch. Vanilla routes any stack whose item is a
    /// `ProjectileItem` through `OminousItemSpawner.spawnProjectile`
    /// (`:90-105`), which dispenses it straight DOWN with the item's own
    /// `ProjectileItem.createDispenseConfig` power/uncertainty/dispense-event
    /// and sets this spawner as the projectile's owner. That is the common case
    /// for ominous trial spawner loot, so this gap is the hot path, not a corner
    /// case. Implementing it needs a reusable `ProjectileItem` abstraction:
    /// today the only dispense logic here is `DispenserBlock::dispense`
    /// (`block/blocks/redstone/dispenser.rs:256`), a private per-item `if`
    /// chain bound to a `DispenseContext` (block position and facing), with the
    /// power/uncertainty constants hardcoded per item. Until an item-side
    /// `create_dispense_config` / `as_projectile` pair exists, every stack takes
    /// the `ItemEntity` branch below.
    async fn spawn_item(&self) {
        let stack = {
            let mut item = self.item.lock().await;
            if item.is_empty() {
                return;
            }
            let stack = item.clone();
            // Vanilla `setItem(ItemStack.EMPTY)` at the end of `spawnItem`.
            *item = ItemStack::EMPTY.clone();
            stack
        };

        let world = self.entity.world.load();
        let pos = self.entity.pos.load();

        let item_entity = Entity::new(world.clone(), pos, &EntityType::ITEM);
        // `new ItemEntity(level, x, y, z, stack)` (`ItemEntity.java:61-66`) sets
        // the random drop velocity and leaves `pickupDelay` at its default 0 --
        // it never calls `setDefaultPickUpDelay`, so the 10-tick delay
        // `ItemEntity::new` applies here would be wrong.
        let velocity = Vector3::new(
            rng().random::<f64>().mul_add(0.2, -0.1),
            0.2,
            rng().random::<f64>().mul_add(0.2, -0.1),
        );
        let spawned: Arc<dyn EntityBase> = Arc::new(ItemEntity::new_with_velocity(
            item_entity,
            stack,
            velocity,
            0,
        ));
        world.spawn_entity(spawned.clone()).await;

        // `level.levelEvent(3021, blockPosition(), 1)` (`OminousItemSpawner.java:84`).
        world.sync_world_event(
            WorldEvent::ParticlesTrialSpawnerSpawnItem,
            self.entity.block_pos.load(),
            1,
        );
        // `level.gameEvent(spawnedEntity, GameEvent.ENTITY_PLACE, position())` (`:85`).
        emit_game_event(
            &world,
            GameEvent::EntityPlace,
            pos,
            GameEventContext::of_entity(spawned),
        )
        .await;

        self.init_data_tracker().await;
    }
}

impl NBTStorage for OminousItemSpawnerEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.entity.write_nbt(nbt).await;
            let item = self.item.lock().await;
            Self::write_spawner_nbt(nbt, &item, self.spawn_item_after_ticks.load(Relaxed));
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.entity.read_nbt_non_mut(nbt).await;
            let (item, ticks) = Self::read_spawner_nbt(nbt);
            *self.item.lock().await = item;
            self.spawn_item_after_ticks.store(ticks, Relaxed);
        })
    }
}

impl EntityBase for OminousItemSpawnerEntity {
    /// `OminousItemSpawner.tick` / `tickServer` (`OminousItemSpawner.java:44-63`).
    /// The client half (`tickClient` `:65-69` and `addParticles` `:153-172`) is
    /// client-side only and has no server counterpart.
    fn tick<'a>(
        &'a self,
        caller: &'a Arc<dyn EntityBase>,
        server: &'a Server,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            // `super.tick()` first: vanilla increments `tickCount` inside it,
            // before `tickServer` compares against it.
            self.entity.tick(caller, server).await;
            let tick_count = self.tick_count.fetch_add(1, Relaxed) + 1;
            let spawn_item_after_ticks = self.spawn_item_after_ticks.load(Relaxed);

            if tick_count == Self::about_to_spawn_sound_tick(spawn_item_after_ticks) {
                // `level.playSound(null, blockPosition(), ..)` centers on the block.
                self.entity.world.load().play_sound(
                    Sound::BlockTrialSpawnerAboutToSpawnItem,
                    SoundCategory::Neutral,
                    &self.entity.block_pos.load().to_centered_f64(),
                );
            }

            if tick_count >= spawn_item_after_ticks {
                self.spawn_item().await;
                // `kill(level)` (`OminousItemSpawner.java:61`) is
                // `Entity.kill` (`Entity.java:405-408`): remove, then fire
                // `GameEvent.ENTITY_DIE` with this entity as the source. The
                // default `EntityBase::kill` for non-living entities only
                // removes, so the event is emitted here.
                let world = self.entity.world.load();
                let pos = self.entity.pos.load();
                self.entity.remove().await;
                emit_game_event(
                    &world,
                    GameEvent::EntityDie,
                    pos,
                    GameEventContext::of_entity(caller.clone()),
                )
                .await;
            }
        })
    }

    fn init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async {
            self.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::ominous_item_spawner::ITEM,
                    &ItemStackSerializer::from(self.item.lock().await.clone()),
                )],
                None,
            );
        })
    }

    /// `Entity.kill` (`Entity.java:405-408`): remove, then fire
    /// `GameEvent.ENTITY_DIE`. The non-living default in `EntityBase::kill`
    /// only removes, so an outside kill would otherwise skip the event.
    fn kill<'a>(&'a self, _caller: &'a dyn EntityBase) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let world = self.entity.world.load();
            let pos = self.entity.pos.load();
            self.entity.remove().await;
            // No `Arc<dyn EntityBase>` available here, as in `ArmorStandEntity::kill`.
            emit_game_event(&world, GameEvent::EntityDie, pos, GameEventContext::none()).await;
        })
    }

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

    /// `OminousItemSpawner.getPistonPushReaction` is `IGNORE` (`:144-147`) and
    /// `isIgnoringBlockTriggers` is true (`:149-152`); it is also never pushed
    /// by fluids, having `noPhysics`.
    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn send_java_spawn_packet<'a>(
        &'a self,
        client: &'a crate::net::java::JavaClient,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let spawn_packet = self.entity.create_spawn_packet();
            if let Ok(data) = client.serialize_packet(&spawn_packet) {
                client.enqueue_packet(data).await;
            }

            if client.version.load() >= CURRENT_MC_VERSION {
                let metadata = Metadata::new(
                    pumpkin_data::tracked_data::ominous_item_spawner::ITEM,
                    ItemStackSerializer::from(self.item.lock().await.clone()),
                );
                let mut data = Vec::new();
                if metadata.write(&mut data, &client.version.load()).is_ok() {
                    data.push(255);
                    let meta_packet =
                        CSetEntityMetadata::new(self.entity.entity_id.into(), data.into());
                    if let Ok(meta_data) = client.serialize_packet(&meta_packet) {
                        client.enqueue_packet(meta_data).await;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OminousItemSpawnerEntity, SPAWN_ITEM_DELAY_MAX, SPAWN_ITEM_DELAY_MIN,
        TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND,
    };
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_nbt::compound::NbtCompound;

    /// `nextIntBetweenInclusive(60, 120)` is inclusive at both ends: 61 values.
    #[test]
    fn spawn_delay_range_is_inclusive_at_both_ends() {
        assert_eq!(
            (SPAWN_ITEM_DELAY_MIN..=SPAWN_ITEM_DELAY_MAX).count(),
            61,
            "60..=120 must cover 61 values, not 60"
        );

        let mut saw_min = false;
        let mut saw_max = false;
        for _ in 0..20_000 {
            let delay = OminousItemSpawnerEntity::random_spawn_delay();
            assert!(
                (SPAWN_ITEM_DELAY_MIN..=SPAWN_ITEM_DELAY_MAX).contains(&delay),
                "rolled {delay} outside 60..=120"
            );
            saw_min |= delay == SPAWN_ITEM_DELAY_MIN;
            saw_max |= delay == SPAWN_ITEM_DELAY_MAX;
        }
        assert!(saw_min && saw_max, "both endpoints must be reachable");
    }

    #[test]
    fn about_to_spawn_sound_fires_36_ticks_early() {
        assert_eq!(
            OminousItemSpawnerEntity::about_to_spawn_sound_tick(120),
            120 - TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND
        );
        assert_eq!(OminousItemSpawnerEntity::about_to_spawn_sound_tick(60), 24);
        // A delay shorter than the lead-in simply never matches a tick count,
        // exactly as vanilla's `==` comparison does.
        assert!(OminousItemSpawnerEntity::about_to_spawn_sound_tick(10) < 0);
    }

    #[test]
    fn nbt_round_trips_item_and_delay() {
        let mut nbt = NbtCompound::new();
        let stack = ItemStack::new(3, &Item::DIAMOND);
        OminousItemSpawnerEntity::write_spawner_nbt(&mut nbt, &stack, 97);

        let (item, ticks) = OminousItemSpawnerEntity::read_spawner_nbt(&nbt);
        assert_eq!(ticks, 97);
        assert_eq!(item.item_count, 3);
        assert_eq!(item.item.id, Item::DIAMOND.id);
    }

    #[test]
    fn empty_item_is_not_written_but_delay_always_is() {
        let mut nbt = NbtCompound::new();
        OminousItemSpawnerEntity::write_spawner_nbt(&mut nbt, ItemStack::EMPTY, 60);

        assert!(nbt.get_compound("item").is_none());
        assert_eq!(nbt.get_long("spawn_item_after_ticks"), Some(60));

        let (item, ticks) = OminousItemSpawnerEntity::read_spawner_nbt(&nbt);
        assert!(item.is_empty());
        assert_eq!(ticks, 60);
    }

    /// Vanilla's `getLongOr("spawn_item_after_ticks", 0L)`.
    #[test]
    fn missing_delay_reads_back_as_zero() {
        let nbt = NbtCompound::new();
        let (item, ticks) = OminousItemSpawnerEntity::read_spawner_nbt(&nbt);
        assert!(item.is_empty());
        assert_eq!(ticks, 0);
    }
}
