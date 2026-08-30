//! `BeehiveBlockEntity` (`BeehiveBlockEntity.java`).
//!
//! Ported: the occupant list (`stored`, max 3), `Occupant`'s `ticks_in_hive` /
//! `min_ticks_in_hive` scheduling, `addOccupant`, `releaseOccupant` with its facing/blocked
//! spawn offset, the honey-level growth on `HONEY_DELIVERED` release, `emptyAllLivingFromHive`,
//! `isFireNearby`, `isSedated` and the `serverTick` work sound.
//!
//! Documented reductions, all forced by engine gaps rather than choice:
//!
//! - `Occupant.of` runs the entity through `TagValueOutput` and discards `IGNORED_BEE_TAGS`.
//!   Pumpkin's `NBTStorage::write_nbt` writes a much smaller set, so only the keys that are
//!   actually produced and would be wrong on respawn are stripped (`IGNORED_BEE_TAGS` below).
//! - `EntityType.loadEntityRecursive` + `EntityTypeTags.BEEHIVE_INHABITORS` become a plain
//!   `EntityType::from_name` lookup plus a bee-type check; nothing else can occupy a hive here.
//! - `Occupant.setBeeReleaseData` ages the bee down by `ticks_in_hive` and decays its in-love
//!   timer. Pumpkin's `BeeEntity` carries no age or breeding state yet, so that is not ported.
//! - `EnvironmentAttributes.BEES_STAY_IN_HIVE` (`EnvironmentAttributes.java:150`, raised by
//!   `WeatherAttributes.java:22` and the night keyframes in `Timelines.java:157`) has no
//!   registry here; `bees_stay_in_hive` reproduces it as "night or raining".
//! - `DataComponents.BEES` (`applyImplicitComponents` / `collectImplicitComponents`) uses the
//!   item-component pipeline, matching `BeehiveBlockEntity.java:309-321`.

use super::BlockEntity;
use crate::entity::mob::Mob;
use crate::entity::player::Player;
use crate::entity::r#type::from_type;
use crate::entity::{EntityBase, NBTStorage};
use crate::world::World;
use pumpkin_data::block_properties::{
    BeeNestLikeProperties, BlockProperties, CampfireLikeProperties,
};
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

/// `BeehiveBlockEntity.MAX_OCCUPANTS`.
pub const MAX_OCCUPANTS: usize = 3;
/// `BeehiveBlockEntity.MIN_OCCUPATION_TICKS_NECTAR`.
const MIN_OCCUPATION_TICKS_NECTAR: i32 = 2400;
/// `BeehiveBlockEntity.MIN_OCCUPATION_TICKS_NECTARLESS`.
const MIN_OCCUPATION_TICKS_NECTARLESS: i32 = 600;
/// `BeehiveBlock.MAX_HONEY_LEVELS`.
pub const MAX_HONEY_LEVELS: u8 = 5;
/// The countdown `emptyAllLivingFromHive` applies to a bee released from a sedated hive.
const SEDATED_STAY_OUT_TICKS: i32 = 400;

/// The subset of `BeehiveBlockEntity.IGNORED_BEE_TAGS` that Pumpkin's bee actually writes.
/// Keeping any of these would respawn the bee with a duplicate identity, at its old position,
/// or already homed to the hive it is being released from.
const IGNORED_BEE_TAGS: [&str; 11] = [
    "UUID",
    "Pos",
    "Motion",
    "Rotation",
    "OnGround",
    "FallDistance",
    "Fire",
    "hive_pos",
    // Vanilla strips these three too. Keeping `TicksSincePollination` in particular would
    // release a bee that entered because it was tired of looking for nectar still tired, so it
    // would want back in immediately and never pollinate again.
    "TicksSincePollination",
    "CannotEnterHiveTicks",
    "CropsGrownSincePollination",
];

/// `BeehiveBlockEntity.BeeReleaseStatus`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BeeReleaseStatus {
    HoneyDelivered,
    BeeReleased,
    Emergency,
}

/// `BeehiveBlockEntity.Occupant` fused with its mutable `BeeData` wrapper: vanilla keeps a
/// separate `BeeData` only so the immutable record can stay a codec target.
#[derive(Clone, Debug)]
pub struct Occupant {
    /// `Occupant.entityData`, a compound carrying the entity id under `"id"`.
    pub entity_data: NbtCompound,
    /// `BeeData.ticksInHive`.
    pub ticks_in_hive: i32,
    /// `Occupant.minTicksInHive`.
    pub min_ticks_in_hive: i32,
}

impl Occupant {
    /// `BeeData.hasNectar`.
    fn has_nectar(&self) -> bool {
        self.entity_data.get_bool("HasNectar").unwrap_or(false)
    }

    /// `BeeData.tick`.
    const fn tick(&mut self) -> bool {
        let ready = self.ticks_in_hive > self.min_ticks_in_hive;
        self.ticks_in_hive += 1;
        ready
    }

    /// `Occupant.of`.
    #[must_use]
    pub fn of(mut entity_data: NbtCompound, entity_id: &str) -> Self {
        for key in IGNORED_BEE_TAGS {
            entity_data.child_tags.remove(key);
        }
        entity_data.put_string("id", entity_id.to_string());
        let has_nectar = entity_data.get_bool("HasNectar").unwrap_or(false);
        Self {
            entity_data,
            ticks_in_hive: 0,
            min_ticks_in_hive: if has_nectar {
                MIN_OCCUPATION_TICKS_NECTAR
            } else {
                MIN_OCCUPATION_TICKS_NECTARLESS
            },
        }
    }

    fn to_nbt(&self) -> NbtTag {
        let mut compound = NbtCompound::new();
        compound.put_compound("entity_data", self.entity_data.clone());
        compound.put_int("ticks_in_hive", self.ticks_in_hive);
        compound.put_int("min_ticks_in_hive", self.min_ticks_in_hive);
        NbtTag::Compound(compound)
    }

    fn from_nbt(tag: &NbtTag) -> Option<Self> {
        let NbtTag::Compound(compound) = tag else {
            return None;
        };
        Some(Self {
            entity_data: compound.get_compound("entity_data")?.clone(),
            ticks_in_hive: compound.get_int("ticks_in_hive").unwrap_or(0),
            min_ticks_in_hive: compound
                .get_int("min_ticks_in_hive")
                .unwrap_or(MIN_OCCUPATION_TICKS_NECTARLESS),
        })
    }
}

/// `EnvironmentAttributes.BEES_STAY_IN_HIVE`: raised by `WeatherAttributes` while it is raining
/// and by the `Timelines` keyframes at day-time 12542 (on) and 23460 (off).
pub async fn bees_stay_in_hive(world: &World) -> bool {
    if world.weather.lock().await.raining {
        return true;
    }
    let time_of_day = world.get_time_of_day().await.rem_euclid(24_000);
    (12_542..23_460).contains(&time_of_day)
}

/// `CampfireBlock.isSmokeyPos` (`CampfireBlock.java:261`), reduced to a solid-block test in
/// place of the `SHAPE_VIRTUAL_POST` collision join, which Pumpkin has no equivalent for.
#[must_use]
pub fn is_smokey_pos(world: &World, pos: &BlockPos) -> bool {
    for i in 1..=5 {
        let to_check = BlockPos::new(pos.0.x, pos.0.y - i, pos.0.z);
        let (block, state) = world.get_block_and_state(&to_check);
        if is_lit_campfire(block, state.id) {
            return true;
        }
        if state.is_solid_block() {
            let below = BlockPos::new(to_check.0.x, to_check.0.y - 1, to_check.0.z);
            let (below_block, below_state) = world.get_block_and_state(&below);
            return is_lit_campfire(below_block, below_state.id);
        }
    }
    false
}

/// `CampfireBlock.isLitCampfire`.
fn is_lit_campfire(block: &Block, state_id: BlockStateId) -> bool {
    if block.id != Block::CAMPFIRE.id && block.id != Block::SOUL_CAMPFIRE.id {
        return false;
    }
    CampfireLikeProperties::from_state_id(state_id, block).lit
}

/// `BlockTags.BEEHIVES`.
#[must_use]
pub fn is_beehive(block: &Block) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_BEEHIVES)
}

pub struct BeehiveBlockEntity {
    pub position: BlockPos,
    /// `BeehiveBlockEntity.stored`.
    pub bees: Mutex<Vec<Occupant>>,
    /// `BeehiveBlockEntity.savedFlowerPos`.
    pub flower_pos: Mutex<Option<BlockPos>>,
    dirty: AtomicBool,
}

impl BlockEntity for BeehiveBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let bees = nbt
            .get_list("bees")
            .map(|list| list.iter().filter_map(Occupant::from_nbt).collect())
            .unwrap_or_default();
        let flower_pos = nbt
            .get_int_array("flower_pos")
            .and_then(|array| match array {
                &[x, y, z] => Some(BlockPos::new(x, y, z)),
                _ => None,
            });
        Self {
            position,
            bees: Mutex::new(bees),
            flower_pos: Mutex::new(flower_pos),
            dirty: AtomicBool::new(false),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            nbt.put_list(
                "bees",
                self.bees
                    .lock()
                    .await
                    .iter()
                    .map(Occupant::to_nbt)
                    .collect(),
            );
            let flower_pos = *self.flower_pos.lock().await;
            if let Some(pos) = flower_pos {
                nbt.put(
                    "flower_pos",
                    NbtTag::IntArray(vec![pos.0.x, pos.0.y, pos.0.z]),
                );
            }
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        if let Ok(bees) = self.bees.try_lock() {
            nbt.put_list("bees", bees.iter().map(Occupant::to_nbt).collect());
        }
        let flower_pos = self.flower_pos.try_lock().ok().and_then(|guard| *guard);
        if let Some(pos) = flower_pos {
            nbt.put(
                "flower_pos",
                NbtTag::IntArray(vec![pos.0.x, pos.0.y, pos.0.z]),
            );
        }
        Some(nbt)
    }

    /// `BeehiveBlockEntity.serverTick`, plus the fire check `setChanged` runs before every
    /// other mutation.
    fn tick<'a>(&'a self, world: &'a Arc<World>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.bees.lock().await.is_empty() {
                return;
            }

            if self.is_fire_nearby(world) {
                self.empty_all_living_from_hive(world, None, BeeReleaseStatus::Emergency)
                    .await;
                return;
            }

            self.tick_occupants(world).await;

            if !self.bees.lock().await.is_empty() && rand::rng().random::<f64>() < 0.005 {
                world.play_sound(
                    Sound::BlockBeehiveWork,
                    SoundCategory::Blocks,
                    &self.position.to_f64(),
                );
            }
        })
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl BeehiveBlockEntity {
    pub const ID: &'static str = "minecraft:beehive";

    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            bees: Mutex::const_new(Vec::new()),
            flower_pos: Mutex::const_new(None),
            dirty: AtomicBool::new(false),
        }
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// `BeehiveBlockEntity.isEmpty`.
    pub async fn is_empty(&self) -> bool {
        self.bees.lock().await.is_empty()
    }

    /// `BeehiveBlockEntity.isFull`.
    pub async fn is_full(&self) -> bool {
        self.bees.lock().await.len() >= MAX_OCCUPANTS
    }

    /// `BeehiveBlockEntity.getOccupantCount`.
    pub async fn occupant_count(&self) -> usize {
        self.bees.lock().await.len()
    }

    /// Serializes occupants for `DataComponents.BEES`, matching the record fields used by
    /// `collectImplicitComponents` (`BeehiveBlockEntity.java:317-321`, `:366-375`).
    pub(crate) async fn bees_component(&self) -> pumpkin_data::data_component_impl::BeesImpl {
        pumpkin_data::data_component_impl::BeesImpl {
            bees: std::borrow::Cow::Owned(
                self.bees
                    .lock()
                    .await
                    .iter()
                    .filter_map(|occupant| match occupant.to_nbt() {
                        NbtTag::Compound(compound) => Some(compound),
                        _ => None,
                    })
                    .collect(),
            ),
        }
    }

    /// `BeehiveBlockEntity.isSedated`.
    #[must_use]
    pub fn is_sedated(&self, world: &World) -> bool {
        is_smokey_pos(world, &self.position)
    }

    /// `BeehiveBlockEntity.isFireNearby`.
    #[must_use]
    pub fn is_fire_nearby(&self, world: &World) -> bool {
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let pos = BlockPos::new(
                        self.position.0.x + x,
                        self.position.0.y + y,
                        self.position.0.z + z,
                    );
                    let block = world.get_block(&pos);
                    if block.id == Block::FIRE.id || block.id == Block::SOUL_FIRE.id {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// `BeehiveBlockEntity.storeBee`.
    pub async fn store_bee(&self, occupant: Occupant) {
        self.bees.lock().await.push(occupant);
        self.mark_dirty();
    }

    /// `BeehiveBlockEntity.addOccupant`: stores the bee's NBT and removes the live entity.
    ///
    /// Returns `false` when the hive is already full, in which case the bee is left alone.
    pub async fn add_occupant(
        &self,
        world: &Arc<World>,
        bee: &Arc<dyn EntityBase>,
        bee_flower_pos: Option<BlockPos>,
    ) -> bool {
        if self.is_full().await {
            return false;
        }

        let mut entity_data = NbtCompound::new();
        if let Some(living) = bee.get_living_entity() {
            living.write_nbt(&mut entity_data).await;
        }
        bee.write_nbt(&mut entity_data).await;
        self.store_bee(Occupant::of(entity_data, "minecraft:bee"))
            .await;

        if let Some(flower_pos) = bee_flower_pos {
            let mut saved = self.flower_pos.lock().await;
            if saved.is_none() || rand::rng().random::<bool>() {
                *saved = Some(flower_pos);
            }
        }

        world.play_sound(
            Sound::BlockBeehiveEnter,
            SoundCategory::Blocks,
            &self.position.to_f64(),
        );
        world.remove_entity(bee.as_ref()).await;
        true
    }

    /// `BeehiveBlockEntity.emptyAllLivingFromHive`.
    ///
    /// `player` angers the released bees at the harvesting player, or, when the hive is
    /// sedated by a campfire below it, keeps them out of the hive for 400 ticks instead.
    pub async fn empty_all_living_from_hive(
        &self,
        world: &Arc<World>,
        player: Option<&Arc<Player>>,
        release_status: BeeReleaseStatus,
    ) -> Vec<Arc<dyn EntityBase>> {
        self.empty_all_living_from_hive_with_state(world, player, release_status, None)
            .await
    }

    /// `BeehiveBlock.playerDestroy` supplies the pre-break state after removal
    /// (`BeehiveBlock.java:91-108`), because the world now contains air.
    pub(crate) async fn empty_all_living_from_hive_with_state(
        &self,
        world: &Arc<World>,
        player: Option<&Arc<Player>>,
        release_status: BeeReleaseStatus,
        // `playerDestroy` retains the old block state across removal (`BeehiveBlock.java:91-108`).
        release_state: Option<(pumpkin_data::BlockId, BlockStateId)>,
    ) -> Vec<Arc<dyn EntityBase>> {
        let sedated = self.is_sedated(world);
        let released = self
            .release_all_occupants(world, release_status, release_state)
            .await;

        if let Some(player) = player {
            let player_pos = player.get_entity().pos.load();
            for entity in &released {
                let Some(bee) = crate::entity::passive::bee::as_bee(entity) else {
                    continue;
                };
                if entity
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&player_pos)
                    > 16.0
                {
                    continue;
                }
                if sedated {
                    bee.set_stay_out_of_hive_countdown(SEDATED_STAY_OUT_TICKS);
                } else {
                    bee.set_mob_target(Some(player.clone() as Arc<dyn EntityBase>))
                        .await;
                }
            }
        }

        released
    }

    /// Removes the occupants at `indices` from the stored list.
    ///
    /// Releasing has to await (it spawns an entity), so the list cannot be held locked across
    /// it, and a concurrent `add_occupant` may push in the meantime. `add_occupant` only ever
    /// pushes, so the snapshot indices stay valid and the released entries are removed
    /// back-to-front rather than the whole vector being overwritten -- overwriting would
    /// silently drop a bee that entered during the release window, after `add_occupant` had
    /// already removed its live entity.
    async fn remove_released(&self, indices: &[usize]) {
        if indices.is_empty() {
            return;
        }
        let mut bees = self.bees.lock().await;
        for &index in indices.iter().rev() {
            if index < bees.len() {
                bees.remove(index);
            }
        }
        drop(bees);
        self.mark_dirty();
    }

    /// `BeehiveBlockEntity.releaseAllOccupants`.
    async fn release_all_occupants(
        &self,
        world: &Arc<World>,
        release_status: BeeReleaseStatus,
        // `playerDestroy` releases against its pre-break state (`BeehiveBlock.java:91-108`).
        release_state: Option<(pumpkin_data::BlockId, BlockStateId)>,
    ) -> Vec<Arc<dyn EntityBase>> {
        let saved_flower_pos = *self.flower_pos.lock().await;
        let occupants: Vec<Occupant> = self.bees.lock().await.clone();

        let mut spawned = Vec::new();
        let mut released = Vec::new();
        for (index, occupant) in occupants.iter().enumerate() {
            if let Some(entity) = self
                .release_occupant(
                    world,
                    occupant,
                    release_status,
                    saved_flower_pos,
                    release_state,
                )
                .await
            {
                spawned.push(entity);
                released.push(index);
            }
        }

        self.remove_released(&released).await;
        spawned
    }

    /// `BeehiveBlockEntity.tickOccupants`.
    async fn tick_occupants(&self, world: &Arc<World>) {
        let saved_flower_pos = *self.flower_pos.lock().await;

        // The counters are advanced under the lock so a concurrent `add_occupant` cannot lose
        // a tick; the releases themselves happen after it is dropped.
        let ready: Vec<(usize, Occupant)> = {
            let mut bees = self.bees.lock().await;
            let mut ready = Vec::new();
            for (index, occupant) in bees.iter_mut().enumerate() {
                if occupant.tick() {
                    ready.push((index, occupant.clone()));
                }
            }
            ready
        };
        if ready.is_empty() {
            return;
        }

        let mut released = Vec::new();
        for (index, occupant) in &ready {
            let release_status = if occupant.has_nectar() {
                BeeReleaseStatus::HoneyDelivered
            } else {
                BeeReleaseStatus::BeeReleased
            };
            if self
                .release_occupant(world, occupant, release_status, saved_flower_pos, None)
                .await
                .is_some()
            {
                released.push(*index);
            }
        }

        self.remove_released(&released).await;
    }

    /// `BeehiveBlockEntity.releaseOccupant`.
    ///
    /// Returns the spawned entity, or `None` when the release is refused (night/rain outside an
    /// emergency, a blocked hive mouth, or an occupant whose stored type is not a bee).
    async fn release_occupant(
        &self,
        world: &Arc<World>,
        occupant: &Occupant,
        release_status: BeeReleaseStatus,
        saved_flower_pos: Option<BlockPos>,
        // The post-break playerDestroy callback supplies the old hive state
        // (`BeehiveBlock.java:91-108`).
        release_state: Option<(pumpkin_data::BlockId, BlockStateId)>,
    ) -> Option<Arc<dyn EntityBase>> {
        let emergency = release_status == BeeReleaseStatus::Emergency;
        if !emergency && bees_stay_in_hive(world).await {
            return None;
        }

        let (block, state_id) = release_state.map_or_else(
            || world.get_block_and_state_id(&self.position),
            |(block_id, state_id)| (block_id.to_block(), state_id),
        );
        if !is_beehive(block) {
            return None;
        }
        let props = BeeNestLikeProperties::from_state_id(state_id, block);
        let facing = props.facing.to_offset();
        let facing_pos = BlockPos::new(
            self.position.0.x + facing.x,
            self.position.0.y + facing.y,
            self.position.0.z + facing.z,
        );
        let front_blocked = world.get_block_state(&facing_pos).is_solid();
        if front_blocked && !emergency {
            return None;
        }

        let id = occupant.entity_data.get_string("id")?;
        let entity_type = EntityType::from_name(id.strip_prefix("minecraft:").unwrap_or(id))?;
        if entity_type.id != EntityType::BEE.id {
            return None;
        }

        let width = f64::from(entity_type.dimension[0]);
        let height = f64::from(entity_type.dimension[1]);
        let delta = if front_blocked {
            0.0
        } else {
            0.55 + width / 2.0
        };
        let spawn_pos = Vector3::new(
            f64::from(self.position.0.x) + 0.5 + delta * f64::from(facing.x),
            f64::from(self.position.0.y) + 0.5 - height / 2.0,
            f64::from(self.position.0.z) + 0.5 + delta * f64::from(facing.z),
        );

        let entity = from_type(entity_type, spawn_pos, world, uuid::Uuid::new_v4());
        if let Some(living) = entity.get_living_entity() {
            living.read_nbt_non_mut(&occupant.entity_data).await;
        }
        entity.read_nbt_non_mut(&occupant.entity_data).await;
        // `read_nbt_non_mut` restores a saved position; the occupant NBT has none, but the
        // living read still normalises the entity, so the hive-mouth spawn is applied after it.
        entity.get_entity().pos.store(spawn_pos);
        entity.get_entity().velocity.store(Vector3::default());

        if let Some(bee) = crate::entity::passive::bee::as_bee(&entity) {
            bee.set_hive_pos(Some(self.position));
            if let Some(flower_pos) = saved_flower_pos
                && bee.flower_pos.load().is_none()
                && rand::rng().random::<f32>() < 0.9
            {
                bee.flower_pos.store(Some(flower_pos));
            }

            if release_status == BeeReleaseStatus::HoneyDelivered {
                bee.drop_off_nectar();
                self.grow_honey(world, block, state_id).await;
            }
        }

        world.play_sound(
            Sound::BlockBeehiveExit,
            SoundCategory::Blocks,
            &self.position.to_f64(),
        );
        world.spawn_entity(entity.clone()).await;
        Some(entity)
    }

    /// The honey-level half of `BeehiveBlockEntity.releaseOccupant`: `+1`, or `+2` on a
    /// 1-in-100 roll, clamped to `MAX_HONEY_LEVELS`.
    async fn grow_honey(&self, world: &Arc<World>, block: &Block, state_id: BlockStateId) {
        let mut props = BeeNestLikeProperties::from_state_id(state_id, block);
        if props.honey_level >= MAX_HONEY_LEVELS {
            return;
        }
        let mut increase = if rand::rng().random_range(0..100) == 0 {
            2
        } else {
            1
        };
        if props.honey_level + increase > MAX_HONEY_LEVELS {
            increase -= 1;
        }
        props.honey_level += increase;
        world
            .set_block_state(
                &self.position,
                props.to_state_id(block),
                BlockFlags::NOTIFY_ALL,
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::{MIN_OCCUPATION_TICKS_NECTAR, MIN_OCCUPATION_TICKS_NECTARLESS, Occupant};
    use pumpkin_nbt::compound::NbtCompound;

    #[test]
    fn occupant_min_ticks_depends_on_nectar() {
        let mut with_nectar = NbtCompound::new();
        with_nectar.put_bool("HasNectar", true);
        assert_eq!(
            Occupant::of(with_nectar, "minecraft:bee").min_ticks_in_hive,
            MIN_OCCUPATION_TICKS_NECTAR
        );
        assert_eq!(
            Occupant::of(NbtCompound::new(), "minecraft:bee").min_ticks_in_hive,
            MIN_OCCUPATION_TICKS_NECTARLESS
        );
    }

    #[test]
    fn occupant_of_strips_identity_tags() {
        let mut nbt = NbtCompound::new();
        nbt.put_uuid("UUID", uuid::Uuid::new_v4());
        nbt.put(
            "hive_pos",
            pumpkin_nbt::tag::NbtTag::IntArray(vec![1, 2, 3]),
        );
        nbt.put_bool("HasStung", true);
        let occupant = Occupant::of(nbt, "minecraft:bee");
        assert!(!occupant.entity_data.has("UUID"));
        assert!(!occupant.entity_data.has("hive_pos"));
        assert_eq!(occupant.entity_data.get_bool("HasStung"), Some(true));
        assert_eq!(occupant.entity_data.get_string("id"), Some("minecraft:bee"));
    }

    #[test]
    fn occupant_releases_only_after_min_ticks() {
        let mut occupant = Occupant::of(NbtCompound::new(), "minecraft:bee");
        for _ in 0..=MIN_OCCUPATION_TICKS_NECTARLESS {
            assert!(!occupant.tick());
        }
        assert!(occupant.tick());
    }

    #[test]
    fn occupant_nbt_round_trip() {
        let mut nbt = NbtCompound::new();
        nbt.put_bool("HasNectar", true);
        let mut occupant = Occupant::of(nbt, "minecraft:bee");
        occupant.ticks_in_hive = 42;
        let restored = Occupant::from_nbt(&occupant.to_nbt()).expect("round trip");
        assert_eq!(restored.ticks_in_hive, 42);
        assert_eq!(restored.min_ticks_in_hive, MIN_OCCUPATION_TICKS_NECTAR);
        assert!(restored.has_nectar());
    }
}
