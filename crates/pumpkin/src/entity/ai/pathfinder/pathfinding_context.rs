use pumpkin_data::{
    Block, BlockState,
    block_properties::{BlockProperties, CampfireLikeProperties},
    fluid::Fluid,
    tag::{self, Taggable},
};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

use crate::{
    entity::ai::pathfinder::{
        node::{Coordinate, PathType},
        path_type_cache::PathTypeCache,
    },
    world::World,
};

use rustc_hash::FxHashMap;
use std::sync::Arc;

pub struct PathfindingContext {
    path_type_cache: Option<PathTypeCache>,
    mob_position: Vector3<i32>,
    world: Arc<World>,
    collision_cache: FxHashMap<Vector3<i32>, bool>,
    source_entity_id: Option<i32>,
    source_root_vehicle_id: Option<i32>,
}

fn collision_height_for_state(state: &BlockState) -> f64 {
    state
        .get_block_collision_shapes()
        .map(|shape| shape.max.y)
        .fold(0.0, f64::max)
}

impl PathfindingContext {
    pub fn new(mob_position: Vector3<i32>, world: Arc<World>) -> Self {
        Self {
            path_type_cache: Some(PathTypeCache::new()),
            mob_position,
            world,
            collision_cache: FxHashMap::default(),
            source_entity_id: None,
            source_root_vehicle_id: None,
        }
    }

    pub fn with_cache(mob_position: Vector3<i32>, world: Arc<World>, cache: PathTypeCache) -> Self {
        Self {
            path_type_cache: Some(cache),
            mob_position,
            world,
            collision_cache: FxHashMap::default(),
            source_entity_id: None,
            source_root_vehicle_id: None,
        }
    }

    pub fn for_entity(
        mob_position: Vector3<i32>,
        world: Arc<World>,
        entity_id: i32,
        root_vehicle_id: i32,
    ) -> Self {
        let mut context = Self::new(mob_position, world);
        context.source_entity_id = Some(entity_id);
        context.source_root_vehicle_id = Some(root_vehicle_id);
        context
    }

    #[must_use]
    pub const fn mob_position(&self) -> Vector3<i32> {
        self.mob_position
    }

    #[must_use]
    pub fn sea_level(&self) -> i32 {
        self.world.sea_level
    }

    pub fn get_path_type_from_state(&mut self, pos: Vector3<i32>) -> PathType {
        if let Some(ref cache) = self.path_type_cache
            && let Some(pt) = cache.get(pos)
        {
            return pt;
        }

        let pt = self.compute_path_type_from_state(pos);

        if let Some(ref mut cache) = self.path_type_cache {
            cache.insert(pos, pt);
        }

        pt
    }

    /// Classifies a block position into a `PathType` for pathfinding.
    #[must_use]
    pub fn compute_path_type_from_state(&self, pos: Vector3<i32>) -> PathType {
        let block_pos = pos.as_blockpos();

        // Single async chunk lookup, then derive block & state from static arrays
        let state_id = self.world.get_block_state_id(&block_pos);
        let block = Block::from_state_id(state_id);
        let state = BlockState::from_id(state_id);

        if block.id == Block::AIR.id
            || block.id == Block::VOID_AIR.id
            || block.id == Block::CAVE_AIR.id
        {
            return PathType::Open;
        }

        if block.has_tag(&tag::Block::MINECRAFT_TRAPDOORS)
            || block.id == Block::LILY_PAD.id
            || block.id == Block::BIG_DRIPLEAF.id
        {
            return PathType::Trapdoor;
        }

        if block.id == Block::POWDER_SNOW.id {
            return PathType::PowderSnow;
        }

        if block.id == Block::CACTUS.id || block.id == Block::SWEET_BERRY_BUSH.id {
            return PathType::DamageOther;
        }

        if block.id == Block::HONEY_BLOCK.id {
            return PathType::StickyHoney;
        }

        if block.id == Block::COCOA.id {
            return PathType::Cocoa;
        }

        if block.id == Block::WITHER_ROSE.id || block.id == Block::POINTED_DRIPSTONE.id {
            return PathType::DamageCautious;
        }

        let fluid = Fluid::from_state_id(state_id);
        if fluid.is_some_and(|f| f.has_tag(&tag::Fluid::MINECRAFT_LAVA)) {
            return PathType::Lava;
        }

        if block.id == Block::FIRE.id
            || block.id == Block::SOUL_FIRE.id
            || block.id == Block::MAGMA_BLOCK.id
            || block.id == Block::LAVA_CAULDRON.id
        {
            return PathType::DamageFire;
        }

        // Vanilla `NodeEvaluator.isBurningBlock` gates campfires on
        // `CampfireBlock.isLitCampfire(state)`, so an extinguished campfire is not a
        // fire hazard and must not poison the surrounding nodes with `DangerFire`.
        if (block.id == Block::CAMPFIRE.id || block.id == Block::SOUL_CAMPFIRE.id)
            && CampfireLikeProperties::from_state_id(state_id, block).lit
        {
            return PathType::DamageFire;
        }

        if block.has_tag(&tag::Block::MINECRAFT_DOORS) {
            if state.collision_shapes.is_empty() {
                return PathType::DoorOpen;
            }

            return if block.id == Block::IRON_DOOR.id {
                PathType::DoorIronClosed
            } else {
                PathType::DoorWoodClosed
            };
        }

        if block.has_tag(&tag::Block::MINECRAFT_RAILS) {
            return PathType::Rail;
        }

        if block.has_tag(&tag::Block::MINECRAFT_LEAVES) {
            return PathType::Leaves;
        }

        if block.has_tag(&tag::Block::MINECRAFT_FENCES)
            || block.has_tag(&tag::Block::MINECRAFT_WALLS)
        {
            return PathType::Fence;
        }

        if block.has_tag(&tag::Block::MINECRAFT_FENCE_GATES) && !state.collision_shapes.is_empty() {
            return PathType::Fence;
        }

        if state.is_full_cube() {
            return PathType::Blocked;
        }

        if fluid.is_some_and(|f| f.has_tag(&tag::Fluid::MINECRAFT_WATER)) {
            return PathType::Water;
        }

        PathType::Open
    }

    /// Wraps the raw block type with below-check and neighbor danger scanning for OPEN nodes.
    pub fn get_land_node_type(&mut self, pos: Vector3<i32>) -> PathType {
        let raw_type = self.get_path_type_from_state(pos);

        // `WalkNodeEvaluator.getLandNodeType` only inspects below and neighboring blocks above
        // the world's bottom edge. At minY, vanilla returns the raw open type instead of reading
        // one block below into the void.
        if raw_type == PathType::Open && pos.y > self.world.get_bottom_y() {
            let below_type = self.get_path_type_from_state(Vector3::new(pos.x, pos.y - 1, pos.z));
            return match below_type {
                PathType::Open | PathType::Water | PathType::Lava | PathType::Walkable => {
                    PathType::Open
                }
                PathType::DamageFire => PathType::DamageFire,
                PathType::DamageOther => PathType::DamageOther,
                PathType::StickyHoney => PathType::StickyHoney,
                PathType::PowderSnow => PathType::DangerPowderSnow,
                PathType::DamageCautious => PathType::DamageCautious,
                PathType::Trapdoor => PathType::DangerTrapdoor,
                _ => self.get_node_type_from_neighbors(pos, PathType::Walkable),
            };
        }

        raw_type
    }

    /// Matches `FlyNodeEvaluator.getPathType`, including its different
    /// treatment of fences below open air and its neighbor hazard scan.
    pub fn get_fly_node_type(&mut self, pos: Vector3<i32>) -> PathType {
        let mut path_type = self.get_path_type_from_state(pos);
        if path_type == PathType::Open && pos.y > self.world.get_bottom_y() {
            let below_pos = Vector3::new(pos.x, pos.y - 1, pos.z);
            let below_type = self.get_path_type_from_state(below_pos);
            path_type = match below_type {
                PathType::DamageFire | PathType::Lava => PathType::DamageFire,
                PathType::DamageOther => PathType::DamageOther,
                PathType::Cocoa => PathType::Cocoa,
                PathType::Fence => {
                    if below_pos == self.mob_position {
                        PathType::Open
                    } else {
                        PathType::Fence
                    }
                }
                PathType::Walkable | PathType::Open | PathType::Water => PathType::Open,
                _ => PathType::Walkable,
            };
        }

        if matches!(path_type, PathType::Walkable | PathType::Open) {
            self.get_node_type_from_neighbors(pos, path_type)
        } else {
            path_type
        }
    }

    /// Scans a 3x3x3 neighborhood for danger blocks and returns the appropriate danger type.
    pub fn get_node_type_from_neighbors(
        &mut self,
        pos: Vector3<i32>,
        fallback: PathType,
    ) -> PathType {
        for dy in -1..=1i32 {
            for dx in -1..=1i32 {
                for dz in -1..=1i32 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }

                    let neighbor_type = self.get_path_type_from_state(Vector3::new(
                        pos.x + dx,
                        pos.y + dy,
                        pos.z + dz,
                    ));

                    if neighbor_type == PathType::DamageOther {
                        return PathType::DangerOther;
                    }
                    if neighbor_type == PathType::DamageFire || neighbor_type == PathType::Lava {
                        return PathType::DangerFire;
                    }
                    if neighbor_type == PathType::Water {
                        return PathType::WaterBorder;
                    }
                    if neighbor_type == PathType::DamageCautious {
                        return PathType::DamageCautious;
                    }
                }
            }
        }

        fallback
    }

    pub fn has_collisions(&mut self, pos: Vector3<i32>) -> bool {
        if let Some(&cached) = self.collision_cache.get(&pos) {
            return cached;
        }

        let block_pos = pos.as_blockpos();
        let state_id = self.world.get_block_state_id(&block_pos);
        let state = BlockState::from_id(state_id);
        let has_collision = state.is_full_cube();

        self.collision_cache.insert(pos, has_collision);
        has_collision
    }

    /// Tests a mob-sized AABB against the actual block collision shapes. Vanilla uses this for
    /// the short ray march around fences and closed doors; checking only `is_full_cube()` misses
    /// partial shapes entirely.
    #[must_use]
    pub async fn has_collision_box(&self, bounding_box: BoundingBox) -> bool {
        let border = self.world.worldborder.lock().await;
        let border_clear = [
            (bounding_box.min.x, bounding_box.min.z),
            (bounding_box.min.x, bounding_box.max.z),
            (bounding_box.max.x, bounding_box.min.z),
            (bounding_box.max.x, bounding_box.max.z),
        ]
        .into_iter()
        .all(|(x, z)| border.contains(x, z));
        drop(border);

        if !border_clear || !self.world.is_space_empty(bounding_box) {
            return true;
        }

        let source_root_vehicle_id = self.source_root_vehicle_id;
        for entity in self.world.get_all_at_box(&bounding_box.expand_all(1.0e-7)) {
            let entity_base = entity.get_entity();
            if self.source_entity_id == Some(entity_base.entity_id)
                || !entity.can_be_collided_with()
            {
                continue;
            }

            if source_root_vehicle_id.is_some()
                && source_root_vehicle_id == Some(entity_base.root_vehicle_id().await)
            {
                continue;
            }

            return true;
        }

        false
    }

    #[must_use]
    pub fn collision_height(&self, pos: Vector3<i32>) -> f64 {
        let state_id = self.world.get_block_state_id(&pos.as_blockpos());
        collision_height_for_state(BlockState::from_id(state_id))
    }

    #[must_use]
    pub fn is_water(&self, pos: Vector3<i32>) -> bool {
        Fluid::from_state_id(self.world.get_block_state_id(&pos.as_blockpos()))
            .is_some_and(|fluid| fluid.has_tag(&tag::Fluid::MINECRAFT_WATER))
    }

    /// Matches `FluidState.isEmpty()` for the pathfinding context.
    #[must_use]
    pub fn is_fluid_empty(&self, pos: Vector3<i32>) -> bool {
        Fluid::from_state_id(self.world.get_block_state_id(&pos.as_blockpos()))
            .is_none_or(|fluid| fluid.id == Fluid::EMPTY.id)
    }

    /// Matches the block-state part of `SwimNodeEvaluator`'s breach check.
    #[must_use]
    pub fn is_air(&self, pos: Vector3<i32>) -> bool {
        BlockState::from_id(self.world.get_block_state_id(&pos.as_blockpos())).is_air()
    }

    /// Matches the block-specific `BlockState.isPathfindable(..., WATER)` rules used by
    /// `SwimNodeEvaluator`. The default is the water fluid state; doors are the important
    /// override because vanilla rejects them even when waterlogged.
    #[must_use]
    pub fn is_pathfindable_for_water(&self, pos: Vector3<i32>) -> bool {
        let state_id = self.world.get_block_state_id(&pos.as_blockpos());
        let block = Block::from_state_id(state_id);
        let always_blocked_for_water = matches!(
            block.id,
            id if id == Block::CHEST.id
                || id == Block::ENDER_CHEST.id
                || id == Block::TRAPPED_CHEST.id
                || id == Block::HOPPER.id
                || id == Block::CONDUIT.id
        );
        Fluid::from_state_id(state_id)
            .is_some_and(|fluid| fluid.has_tag(&tag::Fluid::MINECRAFT_WATER))
            && !always_blocked_for_water
            && !block.has_tag(&tag::Block::MINECRAFT_DOORS)
            && !block.has_tag(&tag::Block::C_FENCE_GATES)
            && !block.has_tag(&tag::Block::MINECRAFT_LANTERNS)
            && !block.has_tag(&tag::Block::MINECRAFT_CAMPFIRES)
            && !block.has_tag(&tag::Block::MINECRAFT_BARS)
            && !block.has_tag(&tag::Block::MINECRAFT_FENCES)
            && !block.has_tag(&tag::Block::MINECRAFT_CHAINS)
            && !block.has_tag(&tag::Block::MINECRAFT_WALLS)
            && !block.has_tag(&tag::Block::C_GLASS_PANES)
    }

    #[must_use]
    pub fn block_has_tag(&self, pos: Vector3<i32>, tag: &'static tag::Tag) -> bool {
        Block::from_state_id(self.world.get_block_state_id(&pos.as_blockpos())).has_tag(tag)
    }

    pub fn clear_caches(&mut self) {
        if let Some(ref mut cache) = self.path_type_cache {
            cache.clear();
        }
        self.collision_cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{Block, collision_height_for_state};

    #[test]
    fn partial_collision_shapes_supply_their_actual_floor_height() {
        let height = collision_height_for_state(Block::STONE_SLAB.default_state);

        assert!(height > 0.0);
        assert!(height < 1.0);
    }
}
