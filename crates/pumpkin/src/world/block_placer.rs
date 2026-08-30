use pumpkin_data::{BlockDirection, BlockState, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::HeightMap;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use pumpkin_world::generation::structure::template::BlockPlacer;
use pumpkin_world::level::Level;
use pumpkin_world::world::BlockFlags;
use std::collections::HashMap;

use crate::world::World;

pub struct WorldBlockPlacer<'a> {
    world: &'a std::sync::Arc<World>,
    pub block_entity_nbts: Vec<NbtCompound>,
    pub changed_positions: Vec<(BlockPos, BlockStateId)>,
    lighting_changes: Vec<(BlockPos, BlockStateId, BlockStateId)>,
}

impl<'a> WorldBlockPlacer<'a> {
    #[must_use]
    pub const fn new(world: &'a std::sync::Arc<World>) -> Self {
        Self {
            world,
            block_entity_nbts: Vec::new(),
            changed_positions: Vec::new(),
            lighting_changes: Vec::new(),
        }
    }

    pub async fn finalize(&mut self) {
        // BlockPlacer writes the live chunk directly so a structure can be applied
        // as one batch. Vanilla still runs the light engine after those block-state
        // changes; update each final position once, in a stable order, before the
        // caller flushes the queued block and light packets.
        let mut changes = HashMap::new();
        for &(position, old_state, new_state) in &self.lighting_changes {
            changes
                .entry(position)
                .and_modify(|change: &mut (BlockStateId, BlockStateId)| change.1 = new_state)
                .or_insert((old_state, new_state));
        }
        let mut lighting_positions: Vec<_> = changes
            .into_iter()
            .map(|(position, (old_state, new_state))| (position, old_state, new_state))
            .collect();
        lighting_positions
            .sort_by_key(|(position, _, _)| (position.0.x, position.0.y, position.0.z));
        self.world
            .level
            .light_engine
            .update_lighting_batch_with_states(&self.world.level, &lighting_positions);

        // Vanilla updates the placed shape boundary, then refreshes each placed state and its
        // neighbors (`StructureTemplate.java:356-380`). Apply the existing world hooks after the
        // batch write and before block entities are installed.
        for (position, state_id) in self.changed_positions.clone() {
            let new_state_id = self
                .world
                .update_from_neighbor_shapes(state_id, &position)
                .await;
            if new_state_id != state_id {
                self.world
                    .set_block_state(&position, new_state_id, BlockFlags::NOTIFY_LISTENERS)
                    .await;
                for (changed_position, changed_state_id) in &mut self.changed_positions {
                    if *changed_position == position {
                        *changed_state_id = new_state_id;
                    }
                }
            }
            for direction in BlockDirection::all() {
                let neighbor_position = position.offset(direction.to_offset());
                self.world
                    .replace_with_state_for_neighbor_update(
                        &neighbor_position,
                        direction.opposite(),
                        BlockFlags::NOTIFY_LISTENERS,
                    )
                    .await;
            }
            self.world.update_neighbors(&position, None).await;
        }

        for nbt in &self.block_entity_nbts {
            if let Some(block_entity) = crate::block::entities::block_entity_from_nbt(nbt) {
                self.world.add_block_entity(block_entity);
            }
        }
    }
}

impl BlockPlacer for WorldBlockPlacer<'_> {
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId {
        self.world
            .get_block_state_id(&BlockPos::new(pos.x, pos.y, pos.z))
    }

    fn set_block_state(&mut self, pos: &Vector3<i32>, state: &BlockState) {
        let block_pos = BlockPos::new(pos.x, pos.y, pos.z);
        let old_state = Level::set_block_state(&self.world.level, &block_pos, state.id);
        self.changed_positions.push((block_pos, state.id));
        if old_state != state.id {
            self.lighting_changes.push((block_pos, old_state, state.id));
        }
    }

    fn add_block_entity(&mut self, nbt: NbtCompound) {
        self.block_entity_nbts.push(nbt);
    }

    fn get_top_y(&self, heightmap: &HeightMap, x: i32, z: i32) -> i32 {
        let chunk_heightmap = match heightmap {
            HeightMap::WorldSurfaceWg | HeightMap::WorldSurface => ChunkHeightmapType::WorldSurface,
            HeightMap::OceanFloorWg | HeightMap::OceanFloor => ChunkHeightmapType::OceanFloor,
            HeightMap::MotionBlocking => ChunkHeightmapType::MotionBlocking,
            HeightMap::MotionBlockingNoLeaves => ChunkHeightmapType::MotionBlockingNoLeaves,
        };
        self.world.get_heightmap_height(chunk_heightmap, x, z)
    }
}
