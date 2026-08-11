use rand::RngExt;

use super::drowned_util::is_bright_outside;
use super::{Controls, Goal, GoalFuture};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::mob::Mob;
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

/// Vanilla `MoveThroughVillageGoal` (`MoveThroughVillageGoal.java`).
///
/// Zombies use this at night to walk toward an unvisited, occupied village POI. The
/// pathfinder owns the search implementation in Pumpkin, while this goal preserves vanilla's
/// candidate selection, partial fallback, and wooden-door path stop.
pub struct MoveThroughVillageGoal {
    speed: f64,
    only_at_night: bool,
    distance_to_poi: f64,
    poi_pos: Option<BlockPos>,
    path: Option<Path>,
    path_destination: Option<Vector3<f64>>,
    visited: Vec<BlockPos>,
}

impl MoveThroughVillageGoal {
    #[must_use]
    pub const fn new(speed: f64, only_at_night: bool, distance_to_poi: i32) -> Self {
        Self {
            speed,
            only_at_night,
            distance_to_poi: distance_to_poi as f64,
            poi_pos: None,
            path: None,
            path_destination: None,
            visited: Vec::new(),
        }
    }

    fn has_not_visited(&self, pos: &BlockPos) -> bool {
        !self.visited.iter().any(|visited| visited == pos)
    }

    fn is_close_to_poi(mob: &dyn Mob, poi: BlockPos, extra: f64) -> bool {
        poi.to_centered_f64()
            .squared_distance_to_vec(&mob.get_entity().pos.load())
            < extra * extra
    }

    fn find_partial_target(
        mob: &dyn Mob,
        world: &std::sync::Arc<crate::world::World>,
        poi: BlockPos,
    ) -> Option<Vector3<f64>> {
        let entity = mob.get_entity();
        let origin = entity.pos.load();
        let target = poi.to_centered_f64();
        let direction = target - origin;
        let angle = direction.z.atan2(direction.x) - std::f64::consts::FRAC_PI_2;
        let mut random = mob.get_random();
        let mut best_weight = f64::NEG_INFINITY;
        let mut best = None;
        for _ in 0..10 {
            let radians =
                angle + (2.0 * random.random::<f64>() - 1.0) * std::f64::consts::FRAC_PI_2;
            let distance = random.random::<f64>().sqrt() * 10.0 * std::f64::consts::SQRT_2;
            let x = -distance * radians.sin();
            let z = distance * radians.cos();
            if x.abs() > 10.0 || z.abs() > 10.0 {
                continue;
            }

            let candidate = BlockPos::floored(
                origin.x + x,
                origin.y + f64::from(random.random_range(-7..=7)),
                origin.z + z,
            );
            if candidate.0.y < world.dimension.min_y
                || candidate.0.y >= world.dimension.min_y + world.dimension.height
            {
                continue;
            }
            let valid = {
                let navigator = mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                navigator.is_stable_destination(world, &candidate)
                    && !navigator.has_pathfinding_malus(world, &candidate)
            };
            if !valid {
                continue;
            }

            let weight = mob.get_walk_target_value(&candidate);
            if weight > best_weight {
                best_weight = weight;
                best = Some(Vector3::new(
                    f64::from(candidate.0.x) + 0.5,
                    f64::from(candidate.0.y),
                    f64::from(candidate.0.z) + 0.5,
                ));
            }
        }
        best
    }

    fn first_wooden_door_node(world: &crate::world::World, path: &Path) -> Option<Vector3<i32>> {
        for index in 0..path.get_node_count() {
            let Some(node_pos) = path.get_node_pos(index) else {
                continue;
            };
            let door_pos = BlockPos::new(node_pos.x, node_pos.y + 1, node_pos.z);
            let state = world.get_block_state(&door_pos);
            if pumpkin_data::Block::from_state_id(state.id)
                .has_tag(&pumpkin_data::tag::Block::MINECRAFT_WOODEN_DOORS)
            {
                return Some(node_pos);
            }
        }
        None
    }
}

impl Goal for MoveThroughVillageGoal {
    #[allow(clippy::too_many_lines)]
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            self.poi_pos = None;
            self.path = None;
            self.path_destination = None;
            if self.visited.len() > 15 {
                self.visited.remove(0);
            }

            let entity = mob.get_entity();
            let world = entity.world.load_full();
            if self.only_at_night && is_bright_outside(&world) {
                return false;
            }

            let origin = entity.block_pos.load();
            if !world.is_close_to_village(origin, 6).await {
                return false;
            }

            // Vanilla samples ten valid land positions within (15, 7), then searches for an
            // occupied village POI within ten blocks of each sample. Keep the same candidate
            // bounds and reject targets that the ground navigator cannot reach before starting
            // the goal.
            let min_y = world.dimension.min_y;
            let max_y = min_y + world.dimension.height - 1;
            let mut best_land_pos: Option<(f64, BlockPos)> = None;
            for _ in 0..10 {
                let mut land_pos = {
                    let mut random = mob.get_random();
                    BlockPos::new(
                        origin.0.x + random.random_range(-15..=15),
                        origin.0.y + random.random_range(-7..=7),
                        origin.0.z + random.random_range(-15..=15),
                    )
                };
                if land_pos.0.y < min_y || land_pos.0.y > max_y {
                    continue;
                }

                let valid_start = {
                    let navigator = mob
                        .get_mob_entity()
                        .navigator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    navigator.is_stable_destination(&world, &land_pos)
                };
                if !valid_start {
                    continue;
                }

                while land_pos.0.y <= max_y && world.get_block_state(&land_pos).is_solid() {
                    land_pos = land_pos.up();
                }
                if land_pos.0.y > max_y
                    || world
                        .get_fluid(&land_pos)
                        .has_tag(&pumpkin_data::tag::Fluid::MINECRAFT_WATER)
                    || !world.is_close_to_village(land_pos, 1).await
                {
                    continue;
                }

                let has_malus = {
                    let navigator = mob
                        .get_mob_entity()
                        .navigator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    navigator.has_pathfinding_malus(&world, &land_pos)
                };
                if has_malus {
                    continue;
                }

                let Some(candidate) = world
                    .village_poi_positions_in_range(land_pos, 10)
                    .await
                    .into_iter()
                    .find(|pos| self.has_not_visited(pos))
                else {
                    continue;
                };
                let distance = candidate.0.squared_distance_to_vec(&origin.0) as f64;
                if best_land_pos.is_none_or(|(best_distance, _)| distance < best_distance) {
                    best_land_pos = Some((distance, land_pos));
                }
            }

            let Some((_, land_pos)) = best_land_pos else {
                return false;
            };
            let Some(poi_pos) = world
                .village_poi_positions_in_range(land_pos, 10)
                .await
                .into_iter()
                .find(|pos| self.has_not_visited(pos))
            else {
                return false;
            };
            let mut path_destination = poi_pos.to_centered_f64();
            let can_open_doors = mob.can_break_doors();
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_can_open_doors(can_open_doors);
            let probe = {
                let navigator = mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                navigator.path_probe()
            };
            let mut probe = probe;
            // `MoveThroughVillageGoal.canUse` temporarily leaves the real navigation in the
            // zombie's door capability while it probes the village path, then vanilla restores
            // `canOpenDoors` unconditionally before returning.
            let restore_navigation = || {
                mob.get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_can_open_doors(true);
            };
            let mut path = probe
                .compute_path(&mob.get_mob_entity().living_entity, path_destination)
                .await;
            if path.is_none()
                && let Some(partial_target) = Self::find_partial_target(mob, &world, poi_pos)
            {
                path_destination = partial_target;
                path = probe
                    .compute_path(&mob.get_mob_entity().living_entity, partial_target)
                    .await;
            }
            let Some(mut path) = path else {
                restore_navigation();
                return false;
            };
            if let Some(node_pos) = Self::first_wooden_door_node(&world, &path) {
                path_destination = Vector3::new(
                    f64::from(node_pos.x) + 0.5,
                    f64::from(node_pos.y),
                    f64::from(node_pos.z) + 0.5,
                );
                restore_navigation();
                let mut door_probe = {
                    let navigator = mob
                        .get_mob_entity()
                        .navigator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    navigator.path_probe()
                };
                let Some(door_path) = door_probe
                    .compute_path(&mob.get_mob_entity().living_entity, path_destination)
                    .await
                else {
                    restore_navigation();
                    return false;
                };
                path = door_path;
            }
            restore_navigation();
            if !path.is_valid() {
                return false;
            }
            self.poi_pos = Some(poi_pos);
            self.path = Some(path);
            self.path_destination = Some(path_destination);
            true
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(poi_pos) = self.poi_pos else {
                return false;
            };
            let navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            !navigator.is_idle()
                && !Self::is_close_to_poi(
                    mob,
                    poi_pos,
                    f64::from(mob.get_entity().width()) + self.distance_to_poi,
                )
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(poi_pos) = self.poi_pos else {
                return;
            };
            let entity = mob.get_entity();
            let current = entity.pos.load();
            let destination = self
                .path_destination
                .unwrap_or_else(|| poi_pos.to_centered_f64());
            let mut navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(path) = self.path.take() {
                navigator.set_progress_with_path(
                    NavigatorGoal::new(current, destination, self.speed),
                    path,
                );
            } else {
                navigator.set_progress(NavigatorGoal::new(current, destination, self.speed));
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(poi_pos) = self.poi_pos.take() else {
                return;
            };
            self.path_destination = None;
            let navigator_idle = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle();
            if navigator_idle || Self::is_close_to_poi(mob, poi_pos, self.distance_to_poi) {
                self.visited.push(poi_pos);
            }
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
