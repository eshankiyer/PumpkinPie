// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use pumpkin_util::math::vector3::Vector3;

use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;

use crate::entity::ai::pathfinder::binary_heap::BinaryHeap;
use crate::entity::ai::pathfinder::node::Coordinate;
use crate::entity::ai::pathfinder::node::Node;
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::ai::pathfinder::node_evaluator::{MobData, NodeEvaluator};
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::ai::pathfinder::pathfinding_context::PathfindingContext;
use crate::entity::ai::pathfinder::walk_node_evaluator::WalkNodeEvaluator;
use pumpkin_data::{
    Block, BlockDirection,
    attributes::Attributes,
    data_component_impl::EquipmentSlot,
    item::Item,
    tag::{self, Taggable},
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::wrap_degrees;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::atomic::{AtomicBool, Ordering};

pub mod binary_heap;
pub mod node;
pub mod node_evaluator;
pub mod path;
pub mod path_type_cache;
pub mod pathfinding_context;
pub mod walk_node_evaluator;

pub struct NavigatorGoal {
    pub current_progress: Vector3<f64>,
    pub destination: Vector3<f64>,
    pub speed: f64,
}

impl NavigatorGoal {
    #[must_use]
    pub const fn new(
        current_progress: Vector3<f64>,
        destination: Vector3<f64>,
        speed: f64,
    ) -> Self {
        Self {
            current_progress,
            destination,
            speed,
        }
    }
}

pub struct Navigator {
    current_goal: Option<NavigatorGoal>,
    evaluator: WalkNodeEvaluator,
    current_path: Option<Path>,
    // Stuck detection
    ticks_on_current_node: u32,
    last_node_index: usize,
    total_ticks: u32,
    path_start_pos: Option<Vector3<f64>>,
    path_type_overrides: FxHashMap<PathType, f32>,
    mob_width: f32,
    mob_height: f32,
    /// Current `Mob.getMaxFallDistance` value (`Mob.java:834-846`) used by walk-node evaluation.
    max_fall_distance: f32,
    // Smart re-pathing cooldown
    repath_cooldown: u32,
    // Reusable allocations to avoid per-pathfind heap allocations
    open_set: BinaryHeap,
    neighbors_buf: Vec<Node>,
    /// Thread-safe status check to avoid deadlocks when components (like `LookControl`) need to
    /// check navigation status.
    pub is_idle: AtomicBool,
    navigation_kind: NavigationKind,
    turtle_travel: bool,
    wall_climber: bool,
    wall_climber_target: Option<BlockPos>,
    wall_climber_direct: bool,
    /// `GroundPathNavigation.avoidSun` (`GroundPathNavigation.java:20`). Set by
    /// `RestrictSunGoal` while the mob is in daylight without head armor; while set, paths are
    /// truncated before their first sky-exposed node (see `Navigator::trim_avoiding_sun`).
    avoid_sun: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NavigationKind {
    #[default]
    Ground,
    Water,
    Flying,
    Amphibious,
}

impl Default for Navigator {
    fn default() -> Self {
        Self {
            current_goal: None,
            evaluator: WalkNodeEvaluator::default(),
            current_path: None,
            ticks_on_current_node: 0,
            last_node_index: 0,
            total_ticks: 0,
            path_start_pos: None,
            path_type_overrides: FxHashMap::default(),
            mob_width: 0.6,
            mob_height: 1.95,
            max_fall_distance: 3.0,
            repath_cooldown: 0,
            open_set: BinaryHeap::new(),
            neighbors_buf: Vec::new(),
            is_idle: AtomicBool::new(true),
            navigation_kind: NavigationKind::Ground,
            turtle_travel: false,
            wall_climber: false,
            wall_climber_target: None,
            wall_climber_direct: false,
            avoid_sun: false,
        }
    }
}

const TARGET_DISTANCE_MULTIPLIER: f32 = 1.5;
const NODE_REACH_XZ: f64 = 0.5;
const NODE_REACH_Y: f64 = 1.0;
const MAX_YAW_TURN_PER_TICK: f32 = 90.0;

/// Mirrors `GroundPathNavigation.findSurfacePosition` for targets supplied as an
/// entity position. Ground navigation searches for the walkable surface in the
/// target column instead of treating the entity's raw block as the node.
fn find_surface_position(world: &crate::world::World, mut pos: BlockPos) -> BlockPos {
    let min_y = world.get_bottom_y();
    let max_y = world.get_top_y();

    if world.get_block_state(&pos).is_air() {
        let mut column = BlockPos::new(pos.0.x, pos.0.y - 1, pos.0.z);
        while column.0.y >= min_y && world.get_block_state(&column).is_air() {
            column.0.y -= 1;
        }
        if column.0.y >= min_y {
            return BlockPos::new(pos.0.x, column.0.y + 1, pos.0.z);
        }

        column = BlockPos::new(pos.0.x, pos.0.y + 1, pos.0.z);
        while column.0.y <= max_y && world.get_block_state(&column).is_air() {
            column.0.y += 1;
        }
        pos = column;
    }

    if !world.get_block_state(&pos).is_solid() {
        return pos;
    }

    let mut column = BlockPos::new(pos.0.x, pos.0.y + 1, pos.0.z);
    while column.0.y <= max_y && world.get_block_state(&column).is_solid() {
        column.0.y += 1;
    }
    column
}

impl Navigator {
    pub(crate) const fn navigation_kind(&self) -> NavigationKind {
        self.navigation_kind
    }

    /// `WalkNodeEvaluator` consumes the mob's current safe fall distance while accepting a
    /// neighbor (`Mob.java:834-846`; `WalkNodeEvaluator.java:352`).
    pub(crate) const fn set_max_fall_distance(&mut self, distance: f32) {
        self.max_fall_distance = distance;
    }

    /// Vanilla `PathNavigation.canFloat` (`PathNavigation.java:401-403`).
    #[must_use]
    pub fn can_float(&self) -> bool {
        self.evaluator.can_float()
    }

    pub fn set_progress(&mut self, goal: NavigatorGoal) {
        self.is_idle.store(false, Ordering::Relaxed);
        if self.wall_climber {
            self.wall_climber_target = Some(BlockPos(goal.destination.floor_to_i32()));
            self.wall_climber_direct = false;
        }
        self.current_goal = Some(goal);
        self.current_path = None;
    }

    /// Updates a direct navigation goal without discarding an unchanged path.
    pub fn set_progress_if_changed(&mut self, goal: NavigatorGoal) {
        let changed = self.current_goal.as_ref().is_none_or(|current| {
            current
                .destination
                .squared_distance_to_vec(&goal.destination)
                > 0.25
                || (current.speed - goal.speed).abs() > f64::EPSILON
        });
        if changed {
            self.set_progress(goal);
        }
    }

    /// Starts navigation with a path that was already computed by a goal.
    ///
    /// Vanilla goals such as `MeleeAttackGoal` and `AvoidEntityGoal` retain the path produced by
    /// `createPath` and hand that exact path to `PathNavigation.moveTo` in `start`. Keeping the
    /// path here avoids throwing away that result and immediately replacing it with a direct
    /// destination goal.
    pub fn set_path(&mut self, goal: NavigatorGoal, path: Path) {
        if !path.is_valid() || path.is_done() {
            self.set_progress(goal);
            return;
        }
        let path_start_pos = goal.current_progress;
        self.is_idle.store(false, Ordering::Relaxed);
        if self.wall_climber {
            self.wall_climber_target = Some(BlockPos(goal.destination.floor_to_i32()));
            self.wall_climber_direct = false;
        }
        self.current_goal = Some(goal);
        self.current_path = Some(path);
        self.ticks_on_current_node = 0;
        self.last_node_index = 0;
        self.total_ticks = 0;
        self.path_start_pos = Some(path_start_pos);
        self.repath_cooldown = 0;
    }

    /// Speed modifier of the active navigation goal, or `None` when idle. Stands in for
    /// vanilla's `MoveControl.getSpeedModifier()`, which `Rabbit.setLandingDelay` reads.
    #[must_use]
    pub fn speed(&self) -> Option<f64> {
        self.current_goal.as_ref().map(|goal| goal.speed)
    }

    pub const fn set_speed(&mut self, speed: f64) {
        if let Some(goal) = &mut self.current_goal {
            goal.speed = speed;
        }
    }

    pub fn stop(&mut self) {
        self.is_idle.store(true, Ordering::Relaxed);
        self.current_goal = None;
        self.current_path = None;
        self.wall_climber_target = None;
        self.wall_climber_direct = false;
        self.ticks_on_current_node = 0;
        self.total_ticks = 0;
        self.path_start_pos = None;
    }

    pub fn set_pathfinding_malus(&mut self, path_type: PathType, malus: f32) {
        self.path_type_overrides.insert(path_type, malus);
    }

    /// Vanilla `PathNavigation.isStableDestination`, dispatched per navigation type:
    /// `PathNavigation.java:388-391` (ground), `WaterBoundPathNavigation.java:47-49`,
    /// `FlyingPathNavigation.java:69-71` and `AmphibiousPathNavigation.java:42-44`, plus
    /// `Turtle.TurtlePathNavigation.isStableDestination` (`Turtle.java:556-560`), which requires
    /// water while the turtle is heading to its travel position.
    pub(crate) fn is_stable_destination(
        &self,
        world: &crate::world::World,
        pos: &BlockPos,
    ) -> bool {
        match self.navigation_kind {
            NavigationKind::Ground => world.get_block_state(&pos.down()).is_solid_render(),
            NavigationKind::Water => !world.get_block_state(pos).is_solid_render(),
            NavigationKind::Flying => world.get_block_state(pos).is_side_solid(BlockDirection::Up),
            NavigationKind::Amphibious => {
                if self.turtle_travel {
                    Block::from_state_id(world.get_block_state(pos).id).id == Block::WATER.id
                } else {
                    !world.get_block_state(&pos.down()).is_air()
                }
            }
        }
    }

    /// Vanilla `GoalUtils.hasMalus` (`GoalUtils.java:40-42`): the mob's per-type override if it
    /// has one, otherwise the `PathType`'s own static malus.
    pub(crate) fn has_pathfinding_malus(
        &self,
        world: &std::sync::Arc<crate::world::World>,
        pos: &BlockPos,
    ) -> bool {
        let mut context = PathfindingContext::new(pos.0, world.clone());
        // `GoalUtils.hasMalus` uses the static walk evaluator. The active amphibious
        // evaluator's temporary WATER/WATER_BORDER costs must not leak into this test.
        let path_type = context.get_land_node_type(pos.0);
        let malus = self
            .path_type_overrides
            .get(&path_type)
            .copied()
            .unwrap_or_else(|| path_type.get_malus());
        malus != 0.0
    }

    /// Matches `MoveControl.isWalkable` for the active navigation evaluator. Vanilla asks
    /// for the path type at the next strafe block and falls back to forward movement unless it
    /// is exactly WALKABLE.
    pub(crate) fn is_strafe_walkable_with_kind(
        world: &std::sync::Arc<crate::world::World>,
        pos: &BlockPos,
        navigation_kind: NavigationKind,
    ) -> bool {
        let mut context = PathfindingContext::new(pos.0, world.clone());
        match navigation_kind {
            NavigationKind::Ground => context.get_land_node_type(pos.0) == PathType::Walkable,
            NavigationKind::Flying => context.get_fly_node_type(pos.0) == PathType::Walkable,
            // SwimNodeEvaluator returns WATER, BREACH, or BLOCKED, never
            // WALKABLE, so vanilla's exact check falls back to forwards.
            NavigationKind::Water => false,
            NavigationKind::Amphibious => {
                context.get_path_type_from_state(pos.0) != PathType::Water
                    && context.get_land_node_type(pos.0) == PathType::Walkable
            }
        }
    }

    /// Vanilla `PathNavigation::setCanOpenDoors` (`GroundPathNavigation.java`'s inherited base),
    /// which just forwards to `NodeEvaluator::setCanOpenDoors`.
    pub fn set_can_open_doors(&mut self, can_open_doors: bool) {
        self.evaluator.set_can_open_doors(can_open_doors);
    }

    /// Selects the `FlyNodeEvaluator` behavior used by vanilla `FlyingPathNavigation`.
    pub const fn set_flying(&mut self, flying: bool) {
        self.evaluator.set_flying(flying);
        if flying {
            self.navigation_kind = NavigationKind::Flying;
        }
    }

    pub fn set_can_float(&mut self, can_float: bool) {
        self.evaluator.set_can_float(can_float);
    }

    pub const fn set_amphibious(&mut self, amphibious: bool) {
        self.evaluator.set_amphibious(amphibious);
        if amphibious {
            self.navigation_kind = NavigationKind::Amphibious;
        }
    }

    /// Selects vanilla `WaterBoundPathNavigation` stability semantics for aquatic mobs.
    pub const fn set_water_bound(&mut self, water_bound: bool) {
        if water_bound {
            self.evaluator.set_water_bound(true);
            self.navigation_kind = NavigationKind::Water;
        }
    }

    pub const fn set_allow_breaching(&mut self, allow_breaching: bool) {
        self.evaluator.set_allow_breaching(allow_breaching);
    }

    pub const fn set_frog(&mut self, frog: bool) {
        self.evaluator.set_frog(frog);
    }

    pub const fn set_turtle_travel(&mut self, traveling: bool) {
        self.turtle_travel = traveling;
    }

    /// Selects vanilla `WallClimberNavigation` fallback behavior used by spiders.
    pub const fn set_wall_climber(&mut self, wall_climber: bool) {
        self.wall_climber = wall_climber;
    }

    /// `GroundPathNavigation.setAvoidSun` (`GroundPathNavigation.java:138-140`). While set,
    /// every navigation tick truncates the current path before its first sky-exposed node
    /// (see `Navigator::trim_avoiding_sun`).
    pub const fn set_avoid_sun(&mut self, avoid_sun: bool) {
        self.avoid_sun = avoid_sun;
    }

    /// `GroundPathNavigation.trimPath` (`GroundPathNavigation.java:116-131`).
    ///
    /// Runs on ground navigators while `avoid_sun` is set. If the mob itself already stands in
    /// open sky the path is left alone (vanilla's early `return`); otherwise the path is cut at
    /// its first node that can see the sky, so following it never walks the mob into daylight.
    fn trim_avoiding_sun(&mut self, entity: &LivingEntity) {
        if !self.avoid_sun || self.navigation_kind != NavigationKind::Ground {
            return;
        }
        let world = entity.entity.world.load();
        let pos = entity.entity.pos.load();
        // Vanilla checks `BlockPos.containing(mob.getX(), mob.getY() + 0.5, mob.getZ())`.
        let mob_pos = BlockPos::new(
            pos.x.floor() as i32,
            (pos.y + 0.5).floor() as i32,
            pos.z.floor() as i32,
        );
        if world.can_see_sky(&mob_pos) {
            return;
        }
        let Some(path) = &mut self.current_path else {
            return;
        };
        for i in 0..path.get_node_count() {
            let Some(node) = path.get_node(i) else {
                continue;
            };
            if world.can_see_sky(&node.pos) {
                path.truncate_nodes(i);
                return;
            }
        }
    }

    #[must_use]
    pub fn get_pathfinding_malus(&self, path_type: PathType) -> f32 {
        self.path_type_overrides
            .get(&path_type)
            .copied()
            .unwrap_or_else(|| path_type.get_malus())
    }

    pub const fn set_mob_dimensions(&mut self, width: f32, height: f32) {
        self.mob_width = width;
        self.mob_height = height;
    }

    /// Vanilla `PathNavigation.createPath(Entity, 0)` as used by `TargetGoal.canReach`.
    ///
    /// This deliberately leaves the active path alone: `TargetGoal` only probes whether the
    /// navigation graph can reach the target, while the target goal itself does not start moving
    /// the mob.
    pub(crate) fn path_probe(&self) -> Self {
        // TargetGoal probes navigation without replacing the active navigation path. Keep the
        // evaluator configuration (door, fluid, flying, and malus settings) but use fresh search
        // scratch state for this independent query.
        Self {
            current_goal: None,
            evaluator: {
                let mut evaluator = WalkNodeEvaluator::default();
                evaluator.set_can_pass_doors(self.evaluator.can_pass_doors());
                evaluator.set_can_open_doors(self.evaluator.can_open_doors());
                evaluator.set_can_float(self.evaluator.can_float());
                evaluator.set_amphibious(self.evaluator.is_amphibious());
                evaluator.set_flying(self.evaluator.is_flying());
                evaluator.set_water_bound(self.evaluator.is_water_bound());
                evaluator.set_allow_breaching(self.evaluator.allows_breaching());
                evaluator.set_frog(self.evaluator.is_frog());
                evaluator
            },
            current_path: None,
            ticks_on_current_node: 0,
            last_node_index: 0,
            total_ticks: 0,
            path_start_pos: None,
            path_type_overrides: self.path_type_overrides.clone(),
            mob_width: self.mob_width,
            mob_height: self.mob_height,
            max_fall_distance: self.max_fall_distance,
            repath_cooldown: 0,
            open_set: BinaryHeap::new(),
            neighbors_buf: Vec::new(),
            is_idle: AtomicBool::new(true),
            navigation_kind: self.navigation_kind,
            turtle_travel: self.turtle_travel,
            wall_climber: self.wall_climber,
            wall_climber_target: None,
            wall_climber_direct: false,
            avoid_sun: self.avoid_sun,
        }
    }

    pub(crate) async fn can_reach_entity_for_mob(
        &mut self,
        mob: &dyn Mob,
        target: &LivingEntity,
    ) -> bool {
        let target_pos = target.entity.block_pos.load();
        let destination = Vector3::new(
            f64::from(target_pos.0.x),
            f64::from(target_pos.0.y),
            f64::from(target_pos.0.z),
        );
        let Some(path) = self
            .compute_path_with_reach_for_mob(mob, destination, 0)
            .await
        else {
            return false;
        };
        let Some(last) = path.get_end_node() else {
            return false;
        };

        let dx = last.pos.0.x - target_pos.0.x;
        let dz = last.pos.0.z - target_pos.0.z;
        dx * dx + dz * dz <= 2
    }

    pub async fn can_reach_within(
        &mut self,
        entity: &LivingEntity,
        destination: Vector3<f64>,
        distance: f32,
    ) -> bool {
        self.compute_path(entity, destination)
            .await
            .is_some_and(|path| path.can_reach() || path.get_dist_to_target() <= distance)
    }

    pub(crate) async fn can_reach_within_for_mob(
        &mut self,
        mob: &dyn Mob,
        destination: Vector3<f64>,
        distance: f32,
    ) -> bool {
        self.compute_path_with_reach_for_mob(mob, destination, 0)
            .await
            .is_some_and(|path| path.can_reach() || path.get_dist_to_target() <= distance)
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn compute_path(
        &mut self,
        entity: &LivingEntity,
        destination: Vector3<f64>,
    ) -> Option<Path> {
        self.compute_path_with_reach(entity, destination, 0).await
    }

    /// Finds a path using the same target reach allowance as vanilla's
    /// `PathNavigation.createPath(target, reachRange)`.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn compute_path_with_reach(
        &mut self,
        entity: &LivingEntity,
        destination: Vector3<f64>,
        reach_range: i32,
    ) -> Option<Path> {
        let dimensions = entity.entity.entity_dimension.load();
        self.mob_width = dimensions.width;
        self.mob_height = dimensions.height;

        // GroundPathNavigation.canUpdatePath: airborne ground mobs do not create a new path
        // until they land or enter liquid. Flying and amphibious navigators are exempt.
        if !self.evaluator.is_flying()
            && !self.evaluator.is_amphibious()
            && !self.evaluator.is_water_bound()
            && !entity.entity.on_ground.load(Ordering::Relaxed)
            && !entity.entity.touching_water.load(Ordering::Relaxed)
            && !entity.entity.touching_lava.load(Ordering::Relaxed)
            && !entity.entity.has_vehicle().await
        {
            return None;
        }
        if self.evaluator.is_water_bound()
            && !self.evaluator.allows_breaching()
            && !entity.entity.touching_water.load(Ordering::Relaxed)
            && !entity.entity.touching_lava.load(Ordering::Relaxed)
        {
            return None;
        }
        let start_pos_f = entity.entity.pos.load();
        let start_block_vec = start_pos_f.floor_to_i32();
        let mob_position = Vector3::new(start_block_vec.x, start_block_vec.y, start_block_vec.z);

        let world = entity.entity.world.load_full();

        // PathNavigation.getMaxPathLength() is at least the required path length (16 blocks),
        // even when FOLLOW_RANGE is smaller. PathFinder visits floor(maxPathLength * 16) nodes.
        let max_path_length =
            (entity.get_attribute_value(&Attributes::FOLLOW_RANGE) as f32).max(16.0);
        let max_iterations = (max_path_length * 16.0).floor() as usize;

        let root_vehicle_id = entity.entity.root_vehicle_id().await;
        let context = PathfindingContext::for_entity(
            mob_position,
            world.clone(),
            entity.entity.entity_id,
            root_vehicle_id,
        );
        let mut mob_data = MobData::new(start_pos_f, self.mob_width, self.mob_height, 1.0);
        // `WalkNodeEvaluator` compares downward steps with `Mob.getMaxFallDistance`
        // (`Mob.java:834-846`; `WalkNodeEvaluator.java:352`).
        mob_data.max_fall_distance = self.max_fall_distance;
        mob_data.bounding_box = entity.entity.bounding_box.load();
        mob_data.fall_distance = entity.fall_distance.load();
        mob_data.is_descending = entity.entity.sneaking.load(Ordering::Relaxed);
        mob_data.can_walk_on_powder_snow = entity
            .entity
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_POWDER_SNOW_WALKABLE_MOBS)
            || entity
                .entity_equipment
                .lock()
                .await
                .equipment
                .get(&EquipmentSlot::FEET)
                .is_some_and(|boots| boots.item == &Item::LEATHER_BOOTS);
        mob_data.on_ground = entity.entity.on_ground.load(Ordering::Relaxed);
        mob_data.in_water = entity.entity.touching_water.load(Ordering::Relaxed);
        mob_data.set_pathfinding_malus(PathType::DangerFire, 16.0);
        mob_data.set_pathfinding_malus(PathType::DamageFire, -1.0);
        // SwimNodeEvaluator and AmphibiousNodeEvaluator both use the vanilla
        // water cost of 0.0. Ground mobs retain the normal water penalty.
        mob_data.set_pathfinding_malus(
            PathType::Water,
            if self.evaluator.is_water_bound() || self.evaluator.is_amphibious() {
                0.0
            } else {
                8.0
            },
        );
        if self.evaluator.is_amphibious() {
            mob_data.set_pathfinding_malus(PathType::Walkable, 6.0);
            mob_data.set_pathfinding_malus(PathType::WaterBorder, 4.0);
        }
        mob_data.set_pathfinding_malus(PathType::Lava, -1.0);
        mob_data.set_pathfinding_malus(PathType::DangerOther, 8.0);

        // Apply per-mob pathfinding malus overrides
        for (&path_type, &malus) in &self.path_type_overrides {
            mob_data.set_pathfinding_malus(path_type, malus);
        }

        self.evaluator.prepare(context, mob_data);

        let mut start_node = self.evaluator.get_start().await?;

        // Vanilla NodeEvaluator floors navigation coordinates before resolving the target node.
        let mut target_pos = if self.evaluator.is_amphibious() {
            BlockPos::floored(destination.x, destination.y + 0.5, destination.z)
        } else {
            BlockPos(destination.floor_to_i32())
        };
        if !self.evaluator.is_amphibious()
            && !self.evaluator.is_flying()
            && !self.evaluator.is_water_bound()
        {
            target_pos = find_surface_position(world.as_ref(), target_pos);
        }
        let mut target = self.evaluator.get_target(target_pos);

        start_node.g = 0.0;
        let start_dist = start_node.distance(&target);
        target.update_best(start_dist, &start_node);
        // Start node uses raw distance (no 1.5x multiplier - that's only for neighbors)
        start_node.h = start_dist;
        start_node.f = start_node.h;
        start_node.walked_dist = 0.0;
        start_node.came_from = None;

        let start_pos = start_node.pos.0;

        // Map to store closed nodes for path reconstruction
        let mut closed_set: FxHashMap<Vector3<i32>, Node> = FxHashMap::default();

        // Reuse the navigator's open_set and neighbors_buf
        self.open_set.clear();
        self.open_set.insert(start_node);

        let mut iterations = 0usize;
        let mut reached = false;

        while !self.open_set.is_empty() {
            iterations += 1;
            if iterations >= max_iterations {
                break;
            }

            let Some(current) = self.open_set.pop() else {
                break;
            };
            if current.distance_manhattan(&target) <= reach_range as f32 {
                target.reached = true;
                reached = true;
                closed_set.insert(current.pos.0, current);
                break;
            }

            let euclidean_from_start = {
                let dx = (current.pos.0.x - start_pos.x) as f32;
                let dy = (current.pos.0.y - start_pos.y) as f32;
                let dz = (current.pos.0.z - start_pos.z) as f32;
                (dx * dx + dy * dy + dz * dz).sqrt()
            };

            if euclidean_from_start >= max_path_length {
                closed_set.insert(current.pos.0, current);
                continue;
            }

            self.neighbors_buf.clear();
            self.evaluator
                .get_neighbors(&current, &mut self.neighbors_buf)
                .await;

            for mut neighbor in self.neighbors_buf.drain(..) {
                let step_cost = current.distance(&neighbor);
                neighbor.walked_dist = current.walked_dist + step_cost;
                let tentative_g = current.g + step_cost + neighbor.cost_malus;

                let in_heap = self.open_set.contains(&neighbor);
                if neighbor.walked_dist < max_path_length
                    && (!in_heap
                        || self
                            .open_set
                            .get_node(&neighbor)
                            .is_some_and(|existing| tentative_g < existing.g))
                {
                    neighbor.came_from = Some(current.pos.0);
                    neighbor.g = tentative_g;
                    let dist_to_target = neighbor.distance(&target);
                    target.update_best(dist_to_target, &neighbor);
                    neighbor.h = dist_to_target * TARGET_DISTANCE_MULTIPLIER;
                    neighbor.f = neighbor.g + neighbor.h;

                    if in_heap {
                        self.open_set.update_node(&neighbor, neighbor);
                    } else {
                        self.open_set.insert(neighbor);
                    }
                }
            }

            closed_set.insert(current.pos.0, current);
        }

        // Also store any remaining open set nodes for path reconstruction
        for node in self.open_set.drain() {
            closed_set.entry(node.pos.0).or_insert(node);
        }

        if let Some(best_node) = target.best_node {
            let mut path_nodes: Vec<Node> = Vec::new();
            let mut current_pos = best_node.pos.0;
            path_nodes.push(best_node);
            let mut visited: FxHashSet<Vector3<i32>> = FxHashSet::default();
            visited.insert(current_pos);
            while let Some(node) = closed_set.get(&current_pos) {
                if let Some(prev_pos) = node.came_from {
                    if prev_pos == current_pos || !visited.insert(prev_pos) {
                        break; // Self-reference or cycle detected
                    }
                    if let Some(&prev_node) = closed_set.get(&prev_pos) {
                        path_nodes.push(prev_node);
                        current_pos = prev_pos;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            path_nodes.reverse();

            let path_target = target.node.pos.0;
            return Some(Path::new(path_nodes, path_target, reached));
        }

        None
    }

    /// Runs the evaluator lifecycle callbacks around a mob-owned path search. Vanilla invokes
    /// these from `WalkNodeEvaluator.prepare/done` (`WalkNodeEvaluator.java:39-49`), including
    /// searches made by temporary navigation probes.
    pub(crate) async fn compute_path_with_reach_for_mob(
        &mut self,
        mob: &dyn Mob,
        destination: Vector3<f64>,
        reach_range: i32,
    ) -> Option<Path> {
        mob.on_pathfinding_start(self);
        let path = self
            .compute_path_with_reach(
                &mob.get_mob_entity().living_entity,
                destination,
                reach_range,
            )
            .await;
        mob.on_pathfinding_done(self);
        path
    }

    fn needs_new_path(&self, goal: &NavigatorGoal) -> bool {
        if self.current_path.is_none() {
            return true;
        }
        if self.repath_cooldown > 0 {
            return false;
        }
        self.current_path.as_ref().is_some_and(|p| {
            let path_target = p.get_target();
            let goal_target = goal.destination.floor_to_i32();
            let dx = f64::from(path_target.x - goal_target.x);
            let dy = f64::from(path_target.y - goal_target.y);
            let dz = f64::from(path_target.z - goal_target.z);
            let distance_sq = dx * dx + dy * dy + dz * dz;
            // Adaptive threshold based on remaining distance
            let remaining = p.get_remaining_distance().clamp(4.0, 16.0);
            let threshold = remaining * 0.5;
            distance_sq > f64::from(threshold * threshold)
        })
    }

    #[allow(clippy::too_many_lines)]
    pub async fn tick(&mut self, entity: &LivingEntity, mob: &dyn Mob) {
        let Some(goal) = self.current_goal.take() else {
            // Idle: stop the mob
            self.is_idle.store(true, Ordering::Relaxed);
            entity.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
            return;
        };

        if goal.current_progress == goal.destination {
            self.is_idle.store(true, Ordering::Relaxed);
            self.current_path = None;
            self.wall_climber_target = None;
            self.wall_climber_direct = false;
            entity.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
            return;
        }

        if self.wall_climber_direct {
            if self
                .wall_climber_target
                .is_some_and(|target| self.wall_climber_target_reached(entity, target))
            {
                self.wall_climber_target = None;
                self.wall_climber_direct = false;
                self.is_idle.store(true, Ordering::Relaxed);
                entity.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
                self.current_goal = None;
                return;
            }

            self.is_idle.store(false, Ordering::Relaxed);
            entity.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
            self.current_goal = Some(goal);
            return;
        }

        self.total_ticks += 1;
        if self.repath_cooldown > 0 {
            self.repath_cooldown -= 1;
        }

        if !self.wall_climber_direct && self.needs_new_path(&goal) {
            self.current_path = self
                .compute_path_with_reach_for_mob(mob, goal.destination, 0)
                .await;
            self.ticks_on_current_node = 0;
            self.last_node_index = 0;
            self.path_start_pos = Some(entity.entity.pos.load());
            self.repath_cooldown = 15; // ~0.75 seconds cooldown before recomputing
            if self.current_path.is_some() {
                self.is_idle.store(false, Ordering::Relaxed);
            }
        }

        if self.current_path.is_none() {
            // No path could be found to the goal: matches vanilla `PathNavigation.isDone()`
            // (`path == null`). Without this, `is_idle` stays false forever, the owning
            // goal's `should_continue` (`!navigator.is_idle()`) never returns false, and the
            // mob is stuck retrying this tick forever instead of the goal ending and a new
            // one starting.
            if self.wall_climber {
                self.wall_climber_direct = true;
                self.is_idle.store(false, Ordering::Relaxed);
            } else {
                self.is_idle.store(true, Ordering::Relaxed);
            }
            entity.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
            self.current_goal = Some(goal);
            return;
        }

        if let Some(path) = &mut self.current_path
            && (path.is_done() || !path.is_valid())
        {
            // Path finished or was invalidated: same "must signal idle" reasoning as
            // above (vanilla `PathNavigation.isDone()`'s `path.isDone()` half). This is
            // the case that fires every tick after a path completes normally, so without
            // it a mob freezes in place permanently after reaching its very first target.
            if self.wall_climber {
                self.current_path = None;
                self.wall_climber_direct = true;
                self.is_idle.store(false, Ordering::Relaxed);
            } else {
                self.is_idle.store(true, Ordering::Relaxed);
            }
            entity.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
            self.current_goal = Some(goal);
            return;
        }

        // Vanilla runs `GroundPathNavigation.trimPath` every navigation tick before following
        // the path (`PathNavigation.tick` -> `trimPath`), so the avoid-sun cut applies to an
        // already-computed path the moment `RestrictSunGoal` starts, not only on repath.
        self.trim_avoiding_sun(entity);

        if let Some(path) = &mut self.current_path {
            let current_node_index = path.get_next_node_index();
            if current_node_index == self.last_node_index {
                self.ticks_on_current_node += 1;
            } else {
                self.ticks_on_current_node = 0;
                self.last_node_index = current_node_index;
            }

            if self.ticks_on_current_node > 100 {
                self.current_path = None;
                self.ticks_on_current_node = 0;
                if self.wall_climber {
                    self.wall_climber_direct = true;
                    self.is_idle.store(false, Ordering::Relaxed);
                } else {
                    self.is_idle.store(true, Ordering::Relaxed);
                }
                entity.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
                self.current_goal = Some(goal);
                return;
            }

            if self.total_ticks.is_multiple_of(100) {
                if let Some(start_pos) = self.path_start_pos {
                    let current_pos = entity.entity.pos.load();
                    let dx = current_pos.x - start_pos.x;
                    let dy = current_pos.y - start_pos.y;
                    let dz = current_pos.z - start_pos.z;
                    let dist_sq = dx * dx + dy * dy + dz * dz;
                    if dist_sq < 2.0 * 2.0 {
                        self.current_path = None;
                        self.ticks_on_current_node = 0;
                        if self.wall_climber {
                            self.wall_climber_direct = true;
                            self.is_idle.store(false, Ordering::Relaxed);
                        } else {
                            self.is_idle.store(true, Ordering::Relaxed);
                        }
                        entity.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
                        self.current_goal = Some(goal);
                        return;
                    }
                }
                self.path_start_pos = Some(entity.entity.pos.load());
            }

            let on_ground = entity.entity.on_ground.load(Ordering::Relaxed);
            let frog_navigation = self.evaluator.is_frog();

            if (self.evaluator.is_amphibious() || self.evaluator.is_water_bound())
                && path.get_next_node_index() + 1 < path.get_node_count()
                && let Some(next) = path.get_next_entity_pos(self.mob_width)
            {
                let current = path.get_next_node_pos().unwrap();
                let position = entity.entity.pos.load();
                let mob_position = Vector3::new(
                    position.x,
                    position.y + f64::from(entity.entity.height()) * 0.5,
                    position.z,
                );
                let world = entity.entity.world.load();
                let dx = mob_position.x - (f64::from(current.x) + 0.5);
                let dy = mob_position.y - f64::from(current.y);
                let dz = mob_position.z - (f64::from(current.z) + 0.5);
                let close_to_current = dx.mul_add(dx, dy.mul_add(dy, dz * dz)) < 4.0;
                let can_move_directly = if close_to_current
                    && (self.evaluator.is_water_bound()
                        || entity.entity.touching_water.load(Ordering::Relaxed))
                {
                    Self::can_move_directly(
                        &world,
                        mob_position,
                        Vector3::new(
                            f64::from(next.0),
                            f64::from(next.1) + f64::from(entity.entity.height()) * 0.5,
                            f64::from(next.2),
                        ),
                    )
                    .await
                } else {
                    false
                };
                if can_move_directly {
                    path.advance();
                } else if close_to_current
                    && path
                        .get_next_node()
                        .is_some_and(|node| Self::can_cut_corner(node.path_type, frog_navigation))
                {
                    let current_node = Vector3::new(
                        f64::from(current.x) + 0.5,
                        f64::from(current.y),
                        f64::from(current.z) + 0.5,
                    );
                    let next_node = path
                        .get_node_pos(path.get_next_node_index() + 1)
                        .map(|pos| {
                            Vector3::new(
                                f64::from(pos.x) + 0.5,
                                f64::from(pos.y),
                                f64::from(pos.z) + 0.5,
                            )
                        });
                    if let Some(next_node) = next_node {
                        let current_delta = current_node - mob_position;
                        let next_delta = next_node - mob_position;
                        let current_distance = current_delta.length_squared();
                        let next_distance = next_delta.length_squared();
                        let closer_to_next = next_distance < current_distance;
                        let within_current = current_distance < 0.5;
                        if (closer_to_next || within_current)
                            && current_distance > 0.0
                            && next_distance > 0.0
                            && current_delta.dot(&next_delta)
                                / (current_distance.sqrt() * next_distance.sqrt())
                                < 0.0
                        {
                            path.advance();
                        }
                    }
                }
            }

            if let Some(next_block) = path.get_next_node_pos() {
                let target_pos = Vector3::new(
                    f64::from(next_block.x) + 0.5,
                    f64::from(next_block.y),
                    f64::from(next_block.z) + 0.5,
                );

                let current_pos = entity.entity.pos.load();
                let dx = target_pos.x - current_pos.x;
                let dy = target_pos.y - current_pos.y;
                let dz = target_pos.z - current_pos.z;

                let horizontal_dist_sq = dx * dx + dz * dz;
                let horizontal_dist = horizontal_dist_sq.sqrt();

                // Skip node if we're above it on the same XZ column and airborne (falling toward it)
                if !on_ground && horizontal_dist < NODE_REACH_XZ && dy < -0.5 {
                    path.advance();
                    if path.is_done() {
                        if self.wall_climber {
                            self.current_path = None;
                            self.wall_climber_direct = true;
                            self.is_idle.store(false, Ordering::Relaxed);
                        } else {
                            self.is_idle.store(true, Ordering::Relaxed);
                        }
                    }
                    self.current_goal = Some(goal);
                    return;
                }

                let node_reach_y = if self.evaluator.is_water_bound() {
                    0.5
                } else {
                    NODE_REACH_Y
                };
                if horizontal_dist < NODE_REACH_XZ && dy.abs() < node_reach_y {
                    path.advance();
                    if path.is_done() {
                        if self.wall_climber {
                            self.current_path = None;
                            self.wall_climber_direct = true;
                            self.is_idle.store(false, Ordering::Relaxed);
                        } else {
                            self.is_idle.store(true, Ordering::Relaxed);
                        }
                    }
                    self.current_goal = Some(goal);
                    return;
                }

                let desired_yaw = wrap_degrees((dz.atan2(dx) as f32).to_degrees() - 90.0);
                let current_yaw = entity.entity.yaw.load();
                let yaw_diff = wrap_degrees(desired_yaw - current_yaw);
                let target_yaw =
                    current_yaw + yaw_diff.clamp(-MAX_YAW_TURN_PER_TICK, MAX_YAW_TURN_PER_TICK);
                entity.entity.yaw.store(target_yaw);
                entity.entity.head_yaw.store(target_yaw);
                entity.entity.body_yaw.store(target_yaw);

                // Mob.setSpeed(speedModifier * MOVEMENT_SPEED), which also sets zza.
                let speed = entity.speed_for_modifier(goal.speed);
                entity.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
                if !self.evaluator.is_amphibious()
                    || !entity.entity.touching_water.load(Ordering::Relaxed)
                {
                    entity.set_speed(speed);
                }
            } else {
                if self.wall_climber {
                    self.wall_climber_direct = true;
                    self.is_idle.store(false, Ordering::Relaxed);
                } else {
                    self.is_idle.store(true, Ordering::Relaxed);
                }
                self.current_path = None;
                entity.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
            }
        }

        self.current_goal = Some(goal);
    }

    fn wall_climber_target_reached(&self, entity: &LivingEntity, target: BlockPos) -> bool {
        let position = entity.entity.pos.load();
        let width_sq = f64::from(self.mob_width) * f64::from(self.mob_width);
        let center_distance = |y: i32| {
            let dx = position.x - (f64::from(target.0.x) + 0.5);
            let dy = position.y - (f64::from(y) + 0.5);
            let dz = position.z - (f64::from(target.0.z) + 0.5);
            dx.mul_add(dx, dy.mul_add(dy, dz * dz)) < width_sq
        };

        center_distance(target.0.y)
            || (position.y > f64::from(target.0.y) && center_distance(position.y.floor() as i32))
    }

    async fn can_move_directly(
        world: &std::sync::Arc<crate::world::World>,
        start: Vector3<f64>,
        stop: Vector3<f64>,
    ) -> bool {
        world
            .raycast_collision(start, stop, async |_, _| true)
            .await
            .is_none()
    }

    fn can_cut_corner(path_type: PathType, frog: bool) -> bool {
        !(matches!(
            path_type,
            PathType::DangerFire | PathType::DangerOther | PathType::WalkableDoor
        ) || frog && path_type == PathType::WaterBorder)
    }

    #[must_use]
    pub const fn get_current_path(&self) -> Option<&Path> {
        self.current_path.as_ref()
    }

    pub fn is_idle(&self) -> bool {
        self.is_idle.load(Ordering::Relaxed)
    }

    pub fn close_to_next_pos(&self, pos: Vector3<f64>) -> bool {
        self.current_path.as_ref().is_some_and(|path| {
            let target = path.get_target();
            let dx = pos.x - f64::from(target.x);
            let dy = pos.y - f64::from(target.y);
            let dz = pos.z - f64::from(target.z);
            dx.mul_add(dx, dy.mul_add(dy, dz * dz)) < 4.0
        })
    }

    /// Returns the next path waypoint in the form consumed by vanilla's
    /// `PathNavigation.tick`: navigation computes the path, then hands this point to the
    /// mob's `MoveControl`. Keeping this handoff separate lets flying controls receive the
    /// waypoint without making the async navigator hold a controller lock.
    #[must_use]
    pub fn next_movement_target(&self) -> Option<(Vector3<f64>, f64)> {
        let goal = self.current_goal.as_ref()?;
        if self.wall_climber_direct {
            let target = self.wall_climber_target?;
            return Some((
                Vector3::new(
                    f64::from(target.0.x),
                    f64::from(target.0.y),
                    f64::from(target.0.z),
                ),
                goal.speed,
            ));
        }
        let path = self.current_path.as_ref()?;
        let (x, y, z) = path.get_next_entity_pos(self.mob_width)?;
        Some((
            Vector3::new(f64::from(x), f64::from(y), f64::from(z)),
            goal.speed,
        ))
    }
}
