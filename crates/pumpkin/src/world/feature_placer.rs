//! Runs a worldgen [`ConfiguredFeature`] against a live [`World`].
//!
//! Vanilla's `Feature.place` takes a `WorldGenLevel`, an interface a live `ServerLevel`
//! implements just as `WorldGenRegion` does, which is how bone-mealed moss
//! (`BonemealableFeaturePlacerBlock.performBonemeal`, `BonemealableFeaturePlacerBlock.java:44-49`),
//! nether vegetation (`NyliumBlock.performBonemeal`, `NyliumBlock.java:57-71`) and sapling
//! growth (`TreeGrower.growTree`, `TreeGrower.java:127-179`) all place real features at runtime.
//!
//! Pumpkin's analogue of `WorldGenLevel` is
//! [`GenerationCache`](pumpkin_world::generation::proto_chunk::GenerationCache). [`FeatureCache`]
//! implements it over a live world:
//!
//! * **Reads** come from the overlay of blocks this placement has already written, falling back
//!   to the loaded chunk. Overlay-first matters: features re-read what they just placed to decide
//!   whether the next block can go down.
//! * **Writes** are buffered in placement order and only reach the world on [`FeatureCache::commit`],
//!   through the ordinary `World::set_block_state` path, so clients, neighbour updates and block
//!   entities all behave exactly as for any other block change.
//!
//! **Unloaded chunks.** Vanilla's `ServerLevel` would simply load the chunk. A live tick has no
//! business blocking on chunk I/O, so this adapter is bounded to the currently loaded region, the
//! same way `WorldGenRegion` is bounded to its region: the moment a read or a write touches an
//! unloaded chunk the placement is marked escaped, and because every write is buffered, `commit`
//! can then discard the whole thing and report failure. That is all-or-nothing - never half a
//! tree. Positions outside the build height are not an escape; they read as air and swallow
//! writes, which is what vanilla's out-of-bounds `setBlock` does. In practice the player who
//! triggered the growth keeps the surrounding chunks loaded.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::chunk::Biome;
use pumpkin_data::fluid::{Fluid, FluidState};
use pumpkin_data::{Block, BlockState, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::HeightMap;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::RandomGenerator;
use pumpkin_world::chunk::ChunkHeightmapType;
use pumpkin_world::generation::feature::configured_features::CONFIGURED_FEATURES;
use pumpkin_world::generation::height_limit::HeightLimitView;
use pumpkin_world::generation::proto_chunk::GenerationCache;
use pumpkin_world::world::{BlockAccessor, BlockFlags};

use crate::world::{World, WorldPortal};

/// `feature_name` is threaded through `ConfiguredFeature::generate` purely so nested features can
/// pass it on; no feature reads it (every implementation binds it as `_feature_name`). Runtime
/// placement has no placed feature at all, so it hands over a fixed placeholder.
const RUNTIME_PLACED_FEATURE: pumpkin_data::placed_feature::PlacedFeature =
    pumpkin_data::placed_feature::PlacedFeature::Acacia;

/// A [`GenerationCache`] backed by a live world, buffering its writes. See the module docs.
pub struct FeatureCache<'a> {
    world: &'a Arc<World>,
    /// Blocks written so far, for read-back.
    overlay: HashMap<BlockPos, BlockStateId>,
    /// The same writes in placement order; `HashMap` iteration order would place blocks before
    /// the blocks they rest on.
    writes: Vec<(BlockPos, BlockStateId)>,
    block_entities: Vec<(BlockPos, NbtCompound)>,
    /// Set when any access left the loaded region; makes [`Self::commit`] discard everything.
    escaped: AtomicBool,
}

impl<'a> FeatureCache<'a> {
    #[must_use]
    pub fn new(world: &'a Arc<World>) -> Self {
        Self {
            world,
            overlay: HashMap::new(),
            writes: Vec::new(),
            block_entities: Vec::new(),
            escaped: AtomicBool::new(false),
        }
    }

    fn read(&self, pos: BlockPos) -> BlockStateId {
        if let Some(id) = self.overlay.get(&pos) {
            return *id;
        }
        if !self.world.is_in_build_limit(pos) {
            return Block::AIR.default_state.id;
        }
        let Some(id) = self.world.get_block_state_id_if_loaded(&pos) else {
            self.escaped.store(true, Ordering::Relaxed);
            return Block::AIR.default_state.id;
        };
        id
    }

    fn write(&mut self, pos: BlockPos, state_id: BlockStateId) {
        if !self.world.is_in_build_limit(pos) {
            return;
        }
        if !self.world.is_loaded(&pos) {
            self.escaped.store(true, Ordering::Relaxed);
            return;
        }
        self.overlay.insert(pos, state_id);
        self.writes.push((pos, state_id));
    }

    /// Buffers a block write, for callers that need to prepare the site inside the same
    /// transaction as the feature - `TreeGrower.growTree` clears the sapling out of the way
    /// before placing (`TreeGrower.java:137-140`, `TreeGrower.java:168`) and puts it back if the
    /// feature declines (`TreeGrower.java:145-148`, `TreeGrower.java:176`). Here the restore is
    /// implicit: nothing was written unless [`Self::commit`] runs.
    pub fn set_block(&mut self, pos: BlockPos, state_id: BlockStateId) {
        self.write(pos, state_id);
    }

    /// Runs a configured feature against this cache, vanilla's
    /// `ConfiguredFeature.place(level, generator, random, pos)`. Returns what the feature
    /// reported; nothing reaches the world until [`Self::commit`].
    pub fn place(
        &mut self,
        feature: pumpkin_data::configured_feature::ConfiguredFeature,
        pos: BlockPos,
        random: &mut RandomGenerator,
    ) -> bool {
        let Some(configured) = CONFIGURED_FEATURES.get(&feature) else {
            return false;
        };
        let portal = WorldPortal(self.world.clone());
        let min_y = self.world.dimension.min_y as i8;
        let height = self.world.dimension.height as u16;
        configured.generate(
            self,
            &portal,
            min_y,
            height,
            RUNTIME_PLACED_FEATURE,
            random,
            pos,
        )
    }

    /// Heightmaps are read from the chunk, so they do not see this placement's own buffered
    /// writes. Vanilla's heightmaps are updated by `setBlock`, but no feature reads a column it
    /// has already written, so the difference is unobservable here.
    ///
    /// `World::get_heightmap_height` returns the Y of the topmost matching block; every
    /// `GenerationCache` height query is exclusive, matching vanilla `Heightmap.getFirstAvailable`.
    fn heightmap_exclusive(&self, kind: ChunkHeightmapType, x: i32, z: i32) -> i32 {
        self.world.get_heightmap_height(kind, x, z) + 1
    }

    /// Applies the buffered placement, or discards it if anything escaped the loaded region.
    /// Returns whether the world was changed.
    pub async fn commit(self) -> bool {
        if self.escaped.load(Ordering::Relaxed) || self.writes.is_empty() {
            return false;
        }
        for (pos, state_id) in self.writes {
            self.world
                .set_block_state(&pos, state_id, BlockFlags::NOTIFY_ALL)
                .await;
        }
        for (pos, nbt) in &self.block_entities {
            self.world.add_block_entity_nbt(*pos, nbt);
        }
        true
    }
}

impl HeightLimitView for FeatureCache<'_> {
    fn height(&self) -> u16 {
        self.world.dimension.height as u16
    }

    fn bottom_y(&self) -> i8 {
        self.world.dimension.min_y as i8
    }

    fn sea_level(&self) -> i32 {
        self.world.sea_level
    }
}

impl BlockAccessor for FeatureCache<'_> {
    fn get_block(&self, position: &BlockPos) -> &'static Block {
        Block::from_state_id(self.read(*position))
    }

    fn get_block_state(&self, position: &BlockPos) -> &'static BlockState {
        BlockState::from_id(self.read(*position))
    }

    fn get_block_state_id(&self, position: &BlockPos) -> BlockStateId {
        self.read(*position)
    }

    fn get_block_and_state(&self, position: &BlockPos) -> (&'static Block, &'static BlockState) {
        BlockState::from_id_with_block(self.read(*position))
    }

    fn get_fluid(&self, position: &BlockPos) -> Fluid {
        GenerationCache::get_fluid_and_fluid_state(self, &position.0).0
    }
}

impl GenerationCache for FeatureCache<'_> {
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId {
        self.read(BlockPos(*pos))
    }

    fn get_fluid_and_fluid_state(&self, position: &Vector3<i32>) -> (Fluid, FluidState) {
        let state_id = self.read(BlockPos(*position));
        let Some(fluid) = Fluid::from_state_id(state_id) else {
            return (Fluid::EMPTY, Fluid::EMPTY.states[0].clone());
        };
        let state = fluid
            .states
            .iter()
            .find(|state| state.block_state_id == state_id)
            .unwrap_or(&fluid.states[fluid.default_state_index as usize]);
        (fluid.clone(), state.clone())
    }

    fn set_block_state(&mut self, pos: &Vector3<i32>, block_state: &BlockState) {
        self.write(BlockPos(*pos), block_state.id);
    }

    fn add_block_entity(&mut self, pos: &Vector3<i32>, nbt: NbtCompound) {
        self.block_entities.push((BlockPos(*pos), nbt));
    }

    fn top_motion_blocking_block_height_exclusive(&self, x: i32, z: i32) -> i32 {
        self.heightmap_exclusive(ChunkHeightmapType::MotionBlocking, x, z)
    }

    fn top_motion_blocking_block_no_leaves_height_exclusive(&self, x: i32, z: i32) -> i32 {
        self.heightmap_exclusive(ChunkHeightmapType::MotionBlockingNoLeaves, x, z)
    }

    fn get_top_y(&self, heightmap: &HeightMap, x: i32, z: i32) -> i32 {
        match heightmap {
            HeightMap::WorldSurfaceWg | HeightMap::WorldSurface => {
                self.top_block_height_exclusive(x, z)
            }
            HeightMap::OceanFloorWg | HeightMap::OceanFloor => {
                self.ocean_floor_height_exclusive(x, z)
            }
            HeightMap::MotionBlocking => self.top_motion_blocking_block_height_exclusive(x, z),
            HeightMap::MotionBlockingNoLeaves => {
                self.top_motion_blocking_block_no_leaves_height_exclusive(x, z)
            }
        }
    }

    fn top_block_height_exclusive(&self, x: i32, z: i32) -> i32 {
        self.heightmap_exclusive(ChunkHeightmapType::WorldSurface, x, z)
    }

    fn ocean_floor_height_exclusive(&self, x: i32, z: i32) -> i32 {
        self.heightmap_exclusive(ChunkHeightmapType::OceanFloor, x, z)
    }

    fn is_air(&self, local_pos: &Vector3<i32>) -> bool {
        BlockState::from_id(self.read(BlockPos(*local_pos))).is_air()
    }

    fn get_biome_for_terrain_gen(&self, x: i32, y: i32, z: i32) -> &'static Biome {
        self.world
            .get_biome(&BlockPos::new(x, y, z))
            .unwrap_or(&Biome::PLAINS)
    }
}

/// Places a configured feature into the live world at `pos`, vanilla's
/// `ConfiguredFeature.place(level, generator, random, pos)`.
///
/// Returns whether the world changed. A feature that declines to generate, or one that reaches
/// outside the loaded region, leaves the world untouched.
pub async fn place_configured_feature(
    world: &Arc<World>,
    feature: pumpkin_data::configured_feature::ConfiguredFeature,
    pos: BlockPos,
    random: &mut RandomGenerator,
) -> bool {
    let mut cache = FeatureCache::new(world);
    if !cache.place(feature, pos, random) {
        return false;
    }
    cache.commit().await
}
