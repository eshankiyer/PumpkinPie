//! Port of `net.minecraft.world.entity.ai.memory` (26.2 decompile at
//! `/home/eshanki/pumpkin-vanilla-26.2/decompiled`).
//!
//! Stage 0 of the Brain/Memory/Activity port. This module is the *always-live* half of a
//! `Brain`: it is guarded by a short-lived `std::sync::Mutex` and is never taken out of its
//! mutex for the duration of a tick, because memories are legitimately written from outside
//! the owning mob's AI tick (damage handlers, game-event listeners, item pickup). See
//! `super::Brain` for the split and `BRAIN_DESIGN.md` section 4.2 for the derivation.
//!
//! Deviations from vanilla, all deliberate:
//! - `GlobalPos` (dimension + `BlockPos`) is stored as a bare `BlockPos`. Pumpkin has no
//!   `GlobalPos`; the only consumer in this stage is Allay's liked note block, which vanilla
//!   also range-checks per dimension (`AllayAi.java:131`). Cross-dimension correctness is
//!   therefore NOT covered.
//! - `MemoryModuleType.PATH` is not represented at all. Vanilla's `MoveToTargetSink` stores a
//!   live `Path` in it (`MoveToTargetSink.java:92,101`); Pumpkin's `Navigator` owns its path
//!   internally and exposes no equivalent handle, so the slot would be unreadable.
//! - Entity-valued memories hold `Weak<dyn EntityBase>` rather than a strong `Arc`, so a
//!   stale memory cannot keep a despawned entity alive. A failed `upgrade()` is treated the
//!   same as vanilla's "the sensor stopped re-populating this memory".

use std::any::Any;
use std::sync::{Arc, Weak};

use pumpkin_data::damage::DamageType;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

use crate::entity::EntityBase;

/// `MemoryStatus` (`memory/MemoryStatus.java`), the full enum.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemoryStatus {
    ValuePresent,
    ValueAbsent,
    /// The slot exists on this brain at all. `Brain.checkMemory` returns `false` for an
    /// unregistered type regardless of the requested status (`Brain.java:242-249`), which is
    /// why `MemoryStore` tracks registration separately from "has a value".
    Registered,
}

/// Dense, compiler-checked index for every memory type this port covers.
///
/// Vanilla declares ~120 `MemoryModuleType` constants (`memory/MemoryModuleType.java:33-152`)
/// in a registry that assigns each a stable identity. Only the subset reachable from Allay's
/// brain is declared here; adding a mob means adding variants, and `COUNT`/`ALL` must be kept
/// in sync (there is a `debug_assert` covering that in `MemoryStore::new`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemoryKeyId {
    WalkTarget,
    LookTarget,
    CantReachWalkTargetSince,
    IsPanicking,
    HurtBy,
    LikedPlayer,
    LikedNoteblockPosition,
    LikedNoteblockCooldownTicks,
    ItemPickupCooldownTicks,
    NearestVisibleWantedItem,
    NearestLivingEntities,
    NearestVisibleLivingEntities,
}

impl MemoryKeyId {
    pub const ALL: [Self; 12] = [
        Self::WalkTarget,
        Self::LookTarget,
        Self::CantReachWalkTargetSince,
        Self::IsPanicking,
        Self::HurtBy,
        Self::LikedPlayer,
        Self::LikedNoteblockPosition,
        Self::LikedNoteblockCooldownTicks,
        Self::ItemPickupCooldownTicks,
        Self::NearestVisibleWantedItem,
        Self::NearestLivingEntities,
        Self::NearestVisibleLivingEntities,
    ];
    pub const COUNT: usize = Self::ALL.len();

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Registry id of the memory types vanilla registers *with* a codec, i.e. those for which
    /// `MemoryModuleType.canSerialize()` holds (`memory/MemoryModuleType.java:84,137-140`).
    /// Every other type here is registered without one and is never saved.
    ///
    /// The liked note block's id is `liked_noteblock`, not [`MemoryKey::NAME`].
    #[must_use]
    pub const fn serialized_name(self) -> Option<&'static str> {
        match self {
            Self::IsPanicking => Some("minecraft:is_panicking"),
            Self::LikedPlayer => Some("minecraft:liked_player"),
            Self::LikedNoteblockPosition => Some("minecraft:liked_noteblock"),
            Self::LikedNoteblockCooldownTicks => Some("minecraft:liked_noteblock_cooldown_ticks"),
            Self::ItemPickupCooldownTicks => Some("minecraft:item_pickup_cooldown_ticks"),
            _ => None,
        }
    }
}

/// Typed key into a `MemoryStore`, the Rust analogue of `MemoryModuleType<U>`
/// (`memory/MemoryModuleType.java:153-179`).
///
/// Implementors are zero-sized; the value type is
/// carried in the associated type so callers never downcast.
pub trait MemoryKey: 'static {
    type Value: Send + 'static;
    const ID: MemoryKeyId;
    const NAME: &'static str;
}

macro_rules! memory_keys {
    ($($key:ident => $id:ident : $value:ty, $name:literal;)*) => {
        $(
            pub struct $key;
            impl MemoryKey for $key {
                type Value = $value;
                const ID: MemoryKeyId = MemoryKeyId::$id;
                const NAME: &'static str = $name;
            }
        )*
    };
}

memory_keys! {
    WalkTargetMemory => WalkTarget: WalkTarget, "walk_target";
    LookTargetMemory => LookTarget: PositionTracker, "look_target";
    CantReachWalkTargetSinceMemory => CantReachWalkTargetSince: i64, "cant_reach_walk_target_since";
    IsPanickingMemory => IsPanicking: bool, "is_panicking";
    HurtByMemory => HurtBy: DamageType, "hurt_by";
    LikedPlayerMemory => LikedPlayer: Uuid, "liked_player";
    LikedNoteblockPositionMemory => LikedNoteblockPosition: BlockPos, "liked_noteblock_position";
    LikedNoteblockCooldownTicksMemory => LikedNoteblockCooldownTicks: i32, "liked_noteblock_cooldown_ticks";
    ItemPickupCooldownTicksMemory => ItemPickupCooldownTicks: i32, "item_pickup_cooldown_ticks";
    NearestVisibleWantedItemMemory => NearestVisibleWantedItem: Weak<dyn EntityBase>, "nearest_visible_wanted_item";
    NearestLivingEntitiesMemory => NearestLivingEntities: Vec<Weak<dyn EntityBase>>, "nearest_living_entities";
    NearestVisibleLivingEntitiesMemory => NearestVisibleLivingEntities: NearestVisibleLivingEntities, "nearest_visible_living_entities";
}

/// `PositionTracker` (`behavior/PositionTracker.java`) with its two concrete implementations,
/// `BlockPosTracker` and `EntityTracker`, collapsed into one enum.
///
/// The accessors return `Option` where vanilla returns a bare value: an `Entity` variant holds
/// a `Weak`, and a dead weak means the tracked entity left the world. Vanilla cannot hit that
/// case because a `Behavior` holding a Java reference keeps the entity reachable.
#[derive(Clone)]
pub enum PositionTracker {
    /// `BlockPosTracker(BlockPos)` (`behavior/BlockPosTracker.java:11-14`): the tracked point
    /// is the block *center*.
    Block { block_pos: BlockPos },
    /// `EntityTracker(entity, trackEyeHeight)` (`behavior/EntityTracker.java:16-24`).
    Entity {
        entity: Weak<dyn EntityBase>,
        track_eye_height: bool,
    },
}

impl PositionTracker {
    #[must_use]
    pub const fn of_block(block_pos: BlockPos) -> Self {
        Self::Block { block_pos }
    }

    /// `BlockPosTracker(Vec3)` (`behavior/BlockPosTracker.java:16-19`) keeps the exact vector
    /// as the tracked position rather than re-centering. Pumpkin's `Navigator` only ever
    /// consumes a destination vector, so the distinction is preserved by storing the containing
    /// block and accepting the re-centering; flagged because `RandomStroll` picks a non-centered
    /// point in vanilla.
    #[must_use]
    pub const fn of_position(pos: Vector3<f64>) -> Self {
        Self::Block {
            block_pos: BlockPos::new(
                pos.x.floor() as i32,
                pos.y.floor() as i32,
                pos.z.floor() as i32,
            ),
        }
    }

    #[must_use]
    pub fn of_entity(entity: &std::sync::Arc<dyn EntityBase>, track_eye_height: bool) -> Self {
        Self::Entity {
            entity: std::sync::Arc::downgrade(entity),
            track_eye_height,
        }
    }

    /// `PositionTracker.currentPosition()`.
    #[must_use]
    pub fn current_position(&self) -> Option<Vector3<f64>> {
        match self {
            Self::Block { block_pos } => Some(Vector3::new(
                f64::from(block_pos.0.x) + 0.5,
                f64::from(block_pos.0.y) + 0.5,
                f64::from(block_pos.0.z) + 0.5,
            )),
            Self::Entity {
                entity,
                track_eye_height,
            } => {
                let entity = entity.upgrade()?;
                let entity = entity.get_entity();
                let pos = entity.pos.load();
                Some(if *track_eye_height {
                    Vector3::new(pos.x, entity.get_eye_y(), pos.z)
                } else {
                    pos
                })
            }
        }
    }

    /// `PositionTracker.currentBlockPosition()`.
    #[must_use]
    pub fn current_block_position(&self) -> Option<BlockPos> {
        match self {
            Self::Block { block_pos } => Some(*block_pos),
            Self::Entity { entity, .. } => {
                let entity = entity.upgrade()?;
                Some(entity.get_entity().block_pos.load())
            }
        }
    }

    /// `PositionTracker.isVisibleBy(LivingEntity)`.
    ///
    /// DEVIATION: vanilla's `EntityTracker.isVisibleBy` (`behavior/EntityTracker.java:37-48`)
    /// requires the target to appear in the observer's `NEAREST_VISIBLE_LIVING_ENTITIES`
    /// memory. That memory needs a `NearestLivingEntitiesSensor`, which this stage does not
    /// port, so the check degrades to "the entity still exists and is alive". A mob will
    /// therefore keep looking at a target through a wall where vanilla would drop it.
    #[must_use]
    pub fn is_visible_by(&self) -> bool {
        match self {
            Self::Block { .. } => true,
            Self::Entity { entity, .. } => entity
                .upgrade()
                .is_some_and(|entity| entity.get_entity().is_alive()),
        }
    }
}

/// `WalkTarget` (`memory/WalkTarget.java:10-43`). The single memory that drives movement:
/// `MoveToTargetSink` is the only thing that reads it and hands it to the navigator.
#[derive(Clone)]
pub struct WalkTarget {
    pub target: PositionTracker,
    pub speed_modifier: f32,
    pub close_enough_dist: i32,
}

impl WalkTarget {
    #[must_use]
    pub const fn new(target: PositionTracker, speed_modifier: f32, close_enough_dist: i32) -> Self {
        Self {
            target,
            speed_modifier,
            close_enough_dist,
        }
    }
}

/// `NearestVisibleLivingEntities` (`memory/NearestVisibleLivingEntities.java:14-71`).
///
/// The living entities a mob saw on its last scan, nearest first
/// (`NearestLivingEntitySensor.java:21`), each flagged with whether line of sight existed at
/// scan time.
///
/// DEVIATION: vanilla computes visibility lazily per query through a per-scan cached
/// predicate (`NearestVisibleLivingEntities.java:26-28`) that re-tests
/// `Sensor.isEntityTargetable`; this port evaluates the flag once during the sensor scan and
/// stores it, because the flag's inputs (positions, blocks between) are exactly what the scan
/// already read. Both forms are allowed up to one 20-tick scan interval of staleness by
/// vanilla's own design (`sensing/Sensor.java:14`). A despawned entity's `Weak` fails to
/// upgrade and is skipped at query time, matching the module-wide weak-reference rule.
#[derive(Clone, Default)]
pub struct NearestVisibleLivingEntities {
    entities: Vec<(Weak<dyn EntityBase>, bool)>,
}

impl NearestVisibleLivingEntities {
    /// `NearestVisibleLivingEntities.empty()` (`NearestVisibleLivingEntities.java:31-33`).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            entities: Vec::new(),
        }
    }

    /// Built once per scan by [`crate::entity::ai::brain::sensor::nearest_living_entities`],
    /// with entries already sorted nearest-first.
    #[must_use]
    pub fn new(entities: Vec<(Weak<dyn EntityBase>, bool)>) -> Self {
        Self { entities }
    }

    /// `findClosest(filter)` (`NearestVisibleLivingEntities.java:40-48`): the nearest entity
    /// passing `filter` whose stored visibility flag held.
    #[must_use]
    pub fn find_closest(
        &self,
        filter: impl Fn(&dyn EntityBase) -> bool,
    ) -> Option<Arc<dyn EntityBase>> {
        self.entities.iter().find_map(|(weak, visible)| {
            if !*visible {
                return None;
            }
            let entity = weak.upgrade()?;
            filter(entity.as_ref()).then_some(entity)
        })
    }

    /// `findAll(filter)` (`NearestVisibleLivingEntities.java:50-52`), preserving the
    /// nearest-first order.
    #[must_use]
    pub fn find_all(&self, filter: impl Fn(&dyn EntityBase) -> bool) -> Vec<Arc<dyn EntityBase>> {
        self.entities
            .iter()
            .filter_map(|(weak, visible)| {
                if !*visible {
                    return None;
                }
                let entity = weak.upgrade()?;
                filter(entity.as_ref()).then_some(entity)
            })
            .collect()
    }

    /// `contains(targetEntity)` (`NearestVisibleLivingEntities.java:58-60`), compared by
    /// entity id.
    #[must_use]
    pub fn contains(&self, other: &Arc<dyn EntityBase>) -> bool {
        let other_id = other.get_entity().entity_id;
        self.entities.iter().any(|(weak, visible)| {
            *visible
                && weak
                    .upgrade()
                    .is_some_and(|entity| entity.get_entity().entity_id == other_id)
        })
    }
}

/// `MemorySlot<T>` (`memory/MemorySlot.java`). `time_to_live: None` is vanilla's
/// `NEVER_EXPIRE` (`Long.MAX_VALUE`, `:7`).
struct MemorySlot {
    value: Option<Box<dyn Any + Send>>,
    time_to_live: Option<i64>,
}

impl MemorySlot {
    const fn empty() -> Self {
        Self {
            value: None,
            time_to_live: None,
        }
    }

    /// `MemorySlot.tick()` (`memory/MemorySlot.java:16-24`): only ticks when a value is
    /// present *and* the slot can expire, clears at `<= 0` before decrementing.
    fn tick(&mut self) {
        if self.value.is_none() {
            return;
        }
        if let Some(ttl) = self.time_to_live {
            if ttl <= 0 {
                self.value = None;
                self.time_to_live = None;
            } else {
                self.time_to_live = Some(ttl - 1);
            }
        }
    }

    const fn has_value(&self) -> bool {
        self.value.is_some()
    }
}

/// The memory map of a `Brain` (`Brain.java:40`).
///
/// LOCK ORDER: this store's mutex is a leaf. Never acquire it while holding the mob's
/// `navigator`, `look_control` or `move_control` lock; read what you need into locals, drop
/// this guard, then take the controller lock. Every accessor here is a plain field access, so
/// no guard ever needs to survive an `.await`.
pub struct MemoryStore {
    slots: Vec<MemorySlot>,
    registered: Vec<bool>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        debug_assert_eq!(MemoryKeyId::COUNT, MemoryKeyId::ALL.len());
        Self {
            slots: (0..MemoryKeyId::COUNT)
                .map(|_| MemorySlot::empty())
                .collect(),
            registered: vec![false; MemoryKeyId::COUNT],
        }
    }

    /// `Brain.registerMemory` (`Brain.java:78-89`), called for every memory a sensor writes
    /// and every memory a behavior lists as a required memory.
    pub fn register(&mut self, id: MemoryKeyId) {
        self.registered[id.index()] = true;
    }

    #[must_use]
    pub fn is_registered(&self, id: MemoryKeyId) -> bool {
        self.registered[id.index()]
    }

    #[must_use]
    pub fn get<K: MemoryKey>(&self) -> Option<&K::Value> {
        self.slots[K::ID.index()]
            .value
            .as_ref()
            .and_then(|value| value.downcast_ref::<K::Value>())
    }

    pub fn set<K: MemoryKey>(&mut self, value: K::Value) {
        // `Brain.setMemoryInternal` clears empty collections instead of storing them
        // (`Brain.java:201-213`).
        if is_empty_collection(&value) {
            self.erase_by_id(K::ID);
        } else {
            self.slots[K::ID.index()] = MemorySlot {
                value: Some(Box::new(value)),
                time_to_live: None,
            };
        }
    }

    pub fn set_with_expiry<K: MemoryKey>(&mut self, value: K::Value, time_to_live: i64) {
        // The expiry overload applies the same empty-collection clearing rule
        // (`Brain.java:186-199`).
        if is_empty_collection(&value) {
            self.erase_by_id(K::ID);
        } else {
            self.slots[K::ID.index()] = MemorySlot {
                value: Some(Box::new(value)),
                time_to_live: Some(time_to_live),
            };
        }
    }

    pub fn erase<K: MemoryKey>(&mut self) {
        self.erase_by_id(K::ID);
    }

    pub fn erase_by_id(&mut self, id: MemoryKeyId) {
        self.slots[id.index()] = MemorySlot::empty();
    }

    #[must_use]
    pub fn has_value<K: MemoryKey>(&self) -> bool {
        self.slots[K::ID.index()].has_value()
    }

    /// `Brain.getTimeUntilExpiry` (`Brain.java:225-227`) via `MemorySlot.timeToLive`
    /// (`memory/MemorySlot.java:59-61`). A slot with no expiry (`time_to_live: None`) reports
    /// vanilla's `NEVER_EXPIRE` sentinel (`Long.MAX_VALUE`).
    #[must_use]
    pub fn time_until_expiry<K: MemoryKey>(&self) -> i64 {
        self.slots[K::ID.index()].time_to_live.unwrap_or(i64::MAX)
    }

    /// `Brain.checkMemory` (`Brain.java:242-249`). Note the leading null check: an
    /// unregistered memory fails *every* status, including `REGISTERED`.
    #[must_use]
    pub fn check(&self, id: MemoryKeyId, status: MemoryStatus) -> bool {
        if !self.registered[id.index()] {
            return false;
        }
        match status {
            MemoryStatus::Registered => true,
            MemoryStatus::ValuePresent => self.slots[id.index()].has_value(),
            MemoryStatus::ValueAbsent => !self.slots[id.index()].has_value(),
        }
    }

    /// `Brain.forgetOutdatedMemories` (`Brain.java:397-399`), the first of the four steps of
    /// `Brain.tick`.
    pub fn tick_expiry(&mut self) {
        for slot in &mut self.slots {
            slot.tick();
        }
    }

    /// `Brain.clearMemories` (`Brain.java:154-156`).
    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            *slot = MemorySlot::empty();
        }
    }

    /// The `memories` map of `Brain.pack()` (`Brain.java:113-135`): every registered slot with
    /// a value whose type has a codec, as `{value, ttl?}` (`ExpirableValue.java:21-28`). `ttl`
    /// is written only when the slot can expire (`memory/MemorySlot.java:69-79`).
    ///
    /// `dimension` fills the `GlobalPos` of the liked note block, which Pumpkin stores as a
    /// bare `BlockPos` (see the module docs).
    #[must_use]
    pub fn write_memories_nbt(&self, dimension: &str) -> NbtCompound {
        let mut memories = NbtCompound::new();
        for id in MemoryKeyId::ALL {
            let Some(name) = id.serialized_name() else {
                continue;
            };
            if !self.is_registered(id) {
                continue;
            }
            let slot = &self.slots[id.index()];
            let Some(value) = slot.value.as_deref() else {
                continue;
            };
            let mut entry = NbtCompound::new();
            if !write_memory_value(id, value, dimension, &mut entry) {
                continue;
            }
            if let Some(ttl) = slot.time_to_live {
                entry.put_long("ttl", ttl);
            }
            memories.put_compound(name, entry);
        }
        memories
    }

    /// The restore loop of the `Brain` constructor (`Brain.java:96-98`). Each entry goes
    /// through `setMemoryInternal`, which silently drops types this brain never registered
    /// (`Brain.java:186-213`); an entry that fails to decode is skipped.
    ///
    /// A liked note block saved in another dimension is skipped: vanilla keeps it but erases
    /// it on first use through the dimension check of `GlobalPos.isCloseEnough`
    /// (`AllayAi.java:113-117`), and a bare `BlockPos` cannot carry the dimension.
    pub fn read_memories_nbt(&mut self, memories: &NbtCompound, dimension: &str) {
        for id in MemoryKeyId::ALL {
            let Some(name) = id.serialized_name() else {
                continue;
            };
            if !self.is_registered(id) {
                continue;
            }
            let Some(entry) = memories.get_compound(name) else {
                continue;
            };
            let Some(value) = read_memory_value(id, entry, dimension) else {
                continue;
            };
            // `MemorySlot.set(value)` stores `NEVER_EXPIRE` (`Long.MAX_VALUE`), which this port
            // represents as `None`.
            let time_to_live = entry.get_long("ttl").filter(|ttl| *ttl != i64::MAX);
            self.slots[id.index()] = MemorySlot {
                value: Some(value),
                time_to_live,
            };
        }
    }

    /// `Brain.memories.isEmpty()` (`Brain.java:454-455`): registration creates the slots, so
    /// an empty memory store has no registered memory modules.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.registered.iter().all(|registered| !registered)
    }
}

/// `Brain.isEmptyCollection` (`Brain.java:186-213`) applies to the collection-valued memory
/// keys represented in this port.
fn is_empty_collection(value: &dyn Any) -> bool {
    value
        .downcast_ref::<Vec<Weak<dyn EntityBase>>>()
        .is_some_and(Vec::is_empty)
        || value
            .downcast_ref::<NearestVisibleLivingEntities>()
            .is_some_and(|entities| entities.entities.is_empty())
}

/// Encodes one memory value under `value` with its type's codec
/// (`memory/MemoryModuleType.java:84,137-140`). Returns `false` for a type without a codec.
fn write_memory_value(
    id: MemoryKeyId,
    value: &(dyn Any + Send),
    dimension: &str,
    entry: &mut NbtCompound,
) -> bool {
    match id {
        // `Codec.BOOL`.
        MemoryKeyId::IsPanicking => value.downcast_ref::<bool>().is_some_and(|value| {
            entry.put_bool("value", *value);
            true
        }),
        // `UUIDUtil.CODEC`: four-int array.
        MemoryKeyId::LikedPlayer => value.downcast_ref::<Uuid>().is_some_and(|value| {
            entry.put_uuid("value", *value);
            true
        }),
        // `GlobalPos.CODEC`: `{dimension, pos: int[3]}` (`GlobalPos.java:13-17`,
        // `BlockPos.java:33-37`).
        MemoryKeyId::LikedNoteblockPosition => {
            value.downcast_ref::<BlockPos>().is_some_and(|pos| {
                let mut global_pos = NbtCompound::new();
                global_pos.put_string("dimension", dimension.to_string());
                global_pos.put("pos", NbtTag::IntArray(vec![pos.0.x, pos.0.y, pos.0.z]));
                entry.put_compound("value", global_pos);
                true
            })
        }
        // `Codec.INT`.
        MemoryKeyId::LikedNoteblockCooldownTicks | MemoryKeyId::ItemPickupCooldownTicks => {
            value.downcast_ref::<i32>().is_some_and(|value| {
                entry.put_int("value", *value);
                true
            })
        }
        _ => false,
    }
}

/// Decodes the `value` field of one saved memory, the inverse of [`write_memory_value`].
fn read_memory_value(
    id: MemoryKeyId,
    entry: &NbtCompound,
    dimension: &str,
) -> Option<Box<dyn Any + Send>> {
    match id {
        MemoryKeyId::IsPanicking => entry
            .get_bool("value")
            .map(|value| Box::new(value) as Box<dyn Any + Send>),
        MemoryKeyId::LikedPlayer => entry
            .get_uuid("value")
            .map(|value| Box::new(value) as Box<dyn Any + Send>),
        MemoryKeyId::LikedNoteblockPosition => {
            let global_pos = entry.get_compound("value")?;
            // `Level.RESOURCE_KEY_CODEC` reads a bare path as `minecraft:<path>`.
            let saved = global_pos.get_string("dimension")?;
            let same_dimension = if saved.contains(':') {
                saved == dimension
            } else {
                dimension.strip_prefix("minecraft:") == Some(saved)
            };
            if !same_dimension {
                return None;
            }
            match global_pos.get_int_array("pos")? {
                [x, y, z] => Some(Box::new(BlockPos::new(*x, *y, *z)) as Box<dyn Any + Send>),
                _ => None,
            }
        }
        MemoryKeyId::LikedNoteblockCooldownTicks | MemoryKeyId::ItemPickupCooldownTicks => entry
            .get_int("value")
            .map(|value| Box::new(value) as Box<dyn Any + Send>),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_target_preserves_close_enough_distance() {
        // `WalkTarget` stores the constructor value returned by `getCloseEnoughDist`
        // (`WalkTarget.java:27-31,41-43`).
        let target = WalkTarget::new(PositionTracker::of_block(BlockPos::new(1, 2, 3)), 1.0, 4);
        assert_eq!(target.close_enough_dist, 4);
    }

    #[test]
    fn set_get_erase_roundtrip() {
        let mut store = MemoryStore::new();
        store.register(MemoryKeyId::LikedNoteblockCooldownTicks);
        assert!(!store.has_value::<LikedNoteblockCooldownTicksMemory>());
        store.set::<LikedNoteblockCooldownTicksMemory>(600);
        assert_eq!(store.get::<LikedNoteblockCooldownTicksMemory>(), Some(&600));
        store.erase::<LikedNoteblockCooldownTicksMemory>();
        assert!(!store.has_value::<LikedNoteblockCooldownTicksMemory>());
    }

    #[test]
    fn unregistered_memory_fails_every_status() {
        let store = MemoryStore::new();
        assert!(!store.check(MemoryKeyId::WalkTarget, MemoryStatus::Registered));
        assert!(!store.check(MemoryKeyId::WalkTarget, MemoryStatus::ValueAbsent));
        assert!(!store.check(MemoryKeyId::WalkTarget, MemoryStatus::ValuePresent));
    }

    #[test]
    fn registered_empty_slot_is_absent_not_present() {
        let mut store = MemoryStore::new();
        store.register(MemoryKeyId::WalkTarget);
        assert!(store.check(MemoryKeyId::WalkTarget, MemoryStatus::Registered));
        assert!(store.check(MemoryKeyId::WalkTarget, MemoryStatus::ValueAbsent));
        assert!(!store.check(MemoryKeyId::WalkTarget, MemoryStatus::ValuePresent));
    }

    #[test]
    fn expiry_clears_after_exactly_ttl_ticks() {
        let mut store = MemoryStore::new();
        store.register(MemoryKeyId::ItemPickupCooldownTicks);
        store.set_with_expiry::<ItemPickupCooldownTicksMemory>(7, 3);
        // MemorySlot.tick decrements while ttl > 0 and clears on the tick where it is <= 0,
        // so a ttl of 3 survives 3 ticks and is gone on the 4th.
        for _ in 0..3 {
            store.tick_expiry();
            assert!(store.has_value::<ItemPickupCooldownTicksMemory>());
        }
        store.tick_expiry();
        assert!(!store.has_value::<ItemPickupCooldownTicksMemory>());
    }

    #[test]
    fn never_expiring_memory_survives_ticks() {
        let mut store = MemoryStore::new();
        store.register(MemoryKeyId::LikedPlayer);
        store.set::<LikedPlayerMemory>(Uuid::nil());
        for _ in 0..100 {
            store.tick_expiry();
        }
        assert!(store.has_value::<LikedPlayerMemory>());
    }

    #[test]
    fn slots_are_independent() {
        let mut store = MemoryStore::new();
        store.register(MemoryKeyId::LikedNoteblockCooldownTicks);
        store.register(MemoryKeyId::ItemPickupCooldownTicks);
        store.set::<LikedNoteblockCooldownTicksMemory>(1);
        store.set::<ItemPickupCooldownTicksMemory>(2);
        assert_eq!(store.get::<LikedNoteblockCooldownTicksMemory>(), Some(&1));
        assert_eq!(store.get::<ItemPickupCooldownTicksMemory>(), Some(&2));
        store.erase::<LikedNoteblockCooldownTicksMemory>();
        assert_eq!(store.get::<ItemPickupCooldownTicksMemory>(), Some(&2));
    }

    #[test]
    fn memories_nbt_roundtrip() {
        let player = Uuid::from_u128(0x0123_4567_89ab_cdef_0011_2233_4455_6677);
        let mut store = MemoryStore::new();
        store.register(MemoryKeyId::LikedPlayer);
        store.register(MemoryKeyId::ItemPickupCooldownTicks);
        store.register(MemoryKeyId::LikedNoteblockPosition);
        store.register(MemoryKeyId::WalkTarget);
        store.set::<LikedPlayerMemory>(player);
        store.set_with_expiry::<ItemPickupCooldownTicksMemory>(7, 5);
        store.set::<LikedNoteblockPositionMemory>(BlockPos::new(1, -2, 3));
        store.set::<WalkTargetMemory>(WalkTarget::new(
            PositionTracker::of_block(BlockPos::new(0, 0, 0)),
            1.0,
            0,
        ));

        let memories = store.write_memories_nbt("minecraft:overworld");
        // `walk_target` has no codec, and a never-expiring slot writes no `ttl`.
        assert!(!memories.has("minecraft:walk_target"));
        assert!(
            !memories
                .get_compound("minecraft:liked_player")
                .is_some_and(|entry| entry.has("ttl"))
        );
        assert_eq!(
            memories
                .get_compound("minecraft:item_pickup_cooldown_ticks")
                .and_then(|entry| entry.get_long("ttl")),
            Some(5)
        );

        let mut restored = MemoryStore::new();
        restored.register(MemoryKeyId::LikedPlayer);
        restored.register(MemoryKeyId::ItemPickupCooldownTicks);
        restored.read_memories_nbt(&memories, "minecraft:overworld");
        assert_eq!(restored.get::<LikedPlayerMemory>(), Some(&player));
        assert_eq!(restored.time_until_expiry::<LikedPlayerMemory>(), i64::MAX);
        assert_eq!(restored.get::<ItemPickupCooldownTicksMemory>(), Some(&7));
        assert_eq!(
            restored.time_until_expiry::<ItemPickupCooldownTicksMemory>(),
            5
        );
        // Not registered on the restoring brain, so `setMemoryInternal` drops it.
        assert!(!restored.has_value::<LikedNoteblockPositionMemory>());

        restored.register(MemoryKeyId::LikedNoteblockPosition);
        restored.read_memories_nbt(&memories, "minecraft:the_nether");
        assert!(!restored.has_value::<LikedNoteblockPositionMemory>());
        restored.read_memories_nbt(&memories, "minecraft:overworld");
        assert_eq!(
            restored.get::<LikedNoteblockPositionMemory>(),
            Some(&BlockPos::new(1, -2, 3))
        );
    }

    #[test]
    fn empty_collection_memory_is_cleared() {
        // `Brain.setMemoryInternal` turns an empty `Collection` into an empty slot
        // (`Brain.java:186-213`); both nearest-entity memories use that rule.
        let mut store = MemoryStore::new();
        store.register(MemoryKeyId::NearestLivingEntities);
        store.register(MemoryKeyId::NearestVisibleLivingEntities);

        store.set::<NearestLivingEntitiesMemory>(Vec::new());
        store.set_with_expiry::<NearestVisibleLivingEntitiesMemory>(
            NearestVisibleLivingEntities::empty(),
            20,
        );

        assert!(!store.has_value::<NearestLivingEntitiesMemory>());
        assert!(!store.has_value::<NearestVisibleLivingEntitiesMemory>());
    }
}
