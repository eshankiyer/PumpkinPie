use super::EnderDragonPhase;
use crate::entity::EntityBase;
use crate::entity::{
    Entity,
    boss::ender_dragon::{EnderDragonEntity, Vector3Ext, find_path},
    projectile::dragon_fireball::DragonFireballEntity,
};
use futures::future::BoxFuture;
use pumpkin_data::entity::EntityType;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use uuid::Uuid;

/// `this.dragon.head` is `new EnderDragonPart(this, "head", 1.0F, 1.0F)` (EnderDragon.java:93),
/// so `head.getY(0.5) + 0.5` is `head.y + 1.0`. The parts in this codebase all carry the
/// dragon's own dimensions, so the vanilla head height has to be spelled out here.
const HEAD_SPAWN_Y_OFFSET: f64 = 1.0;
/// `viewVector.x * 1.0` / `viewVector.z * 1.0` (DragonStrafePlayerPhase.java:65-67).
const HEAD_SPAWN_BACKOFF: f64 = 1.0;

pub struct StrafingPhase;

impl super::Phase for StrafingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Strafing
    }

    fn begin<'a>(&'a self, dragon: &'a EnderDragonEntity) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            *dragon.fireball_charge.lock().await = 0;
            *dragon.target_location.lock().await = None;
        })
    }

    #[expect(clippy::too_many_lines)]
    fn tick<'a>(&'a self, dragon: &'a EnderDragonEntity) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let target_id = *dragon.target_player.lock().await;
            let world = dragon.mob_entity.living_entity.entity.world.load();
            let pos = dragon.mob_entity.living_entity.entity.pos.load();

            let player_target = if let Some(id) = target_id {
                world
                    .players
                    .load()
                    .iter()
                    .find(|p| p.gameprofile.id == id)
                    .cloned()
            } else {
                None
            };

            let Some(player) = player_target else {
                dragon.set_phase(EnderDragonPhase::Circling).await;
                return;
            };

            let player_pos = player.get_entity().pos.load();
            let mut path = dragon.path.lock().await;
            let mut target_location = dragon.target_location.lock().await;

            if path.is_empty() {
                let d2 = player_pos.x - pos.x;
                let d3 = player_pos.z - pos.z;
                let d4 = d2.hypot(d3);
                let d5 = (0.4 + d4 / 80.0 - 1.0).clamp(0.0, 10.0);
                *target_location =
                    Some(Vector3::new(player_pos.x, player_pos.y + d5, player_pos.z));
            }

            let d11 = target_location
                .map(|loc| pos.distance_squared(loc))
                .unwrap_or(0.0);
            if !(100.0..=22500.0).contains(&d11)
                || dragon
                    .mob_entity
                    .living_entity
                    .entity
                    .horizontal_collision
                    .load(Ordering::Relaxed)
            {
                if path.is_empty() {
                    drop(path);
                    let i = dragon.find_closest_node().await;
                    let j = dragon.find_closest_node_to(player_pos).await;

                    let mut path_lock = dragon.path.lock().await;
                    let nodes = dragon.nodes.lock().await;
                    *path_lock = find_path(&nodes, i, j, None);
                    drop(nodes);
                    path = path_lock;
                }

                if let Some(next_node_idx) = path.first().copied() {
                    path.remove(0);
                    let nodes = dragon.nodes.lock().await;
                    if let Some(node) = nodes[next_node_idx] {
                        let mut y_target = node.y + rand::random_range(0.0..20.0);
                        while y_target < node.y {
                            y_target = node.y + rand::random_range(0.0..20.0);
                        }
                        *target_location = Some(Vector3::new(node.x, y_target, node.z));
                    }
                }
            }
            drop(path);
            drop(target_location);

            if player_pos.distance_squared(pos) < 4096.0 {
                let mut charge = dragon.fireball_charge.lock().await;

                let aim_diff = player_pos - pos;
                let aim = if aim_diff.length_squared() > 1e-6 {
                    aim_diff.normalize()
                } else {
                    Vector3::new(0.0, 0.0, 0.0)
                };

                let yaw = dragon.mob_entity.living_entity.entity.yaw.load();
                let dir = Vector3::new(
                    (yaw * (std::f32::consts::PI / 180.0)).sin() as f64,
                    0.0,
                    -(yaw * (std::f32::consts::PI / 180.0)).cos() as f64,
                );

                let dir_norm = if dir.length_squared() > 1e-6 {
                    dir.normalize()
                } else {
                    Vector3::new(0.0, 0.0, 0.0)
                };

                let dot = dir_norm.dot(&aim) as f32;
                let angle_degs = dot.acos().to_degrees() + 0.5;

                *charge += 1;

                if *charge >= 5 && angle_degs < 10.0 {
                    *charge = 0;
                    drop(charge);

                    let dragon_entity = &dragon.mob_entity.living_entity.entity;

                    // `DragonStrafePlayerPhase.java:72-74`: the shoot sound is a level event,
                    // suppressed while the dragon is silent.
                    if !dragon_entity.is_silent() {
                        world.sync_world_event(
                            WorldEvent::SoundDragonFireball,
                            dragon_entity.block_pos.load(),
                            0,
                        );
                    }

                    // `DragonStrafePlayerPhase.java:62-71`: the fireball starts one block
                    // behind the head along the dragon's view vector and is aimed at the
                    // target's mid-height.
                    let head_pos = dragon
                        .parts
                        .first()
                        .map_or(pos, |head| head.entity.pos.load());
                    let view_vector = Vector3::rotation_vector(
                        f64::from(dragon_entity.pitch.load()),
                        f64::from(dragon_entity.yaw.load()),
                    );
                    let spawn_pos = Vector3::new(
                        head_pos.x - view_vector.x * HEAD_SPAWN_BACKOFF,
                        head_pos.y + HEAD_SPAWN_Y_OFFSET,
                        head_pos.z - view_vector.z * HEAD_SPAWN_BACKOFF,
                    );

                    let target_mid_y = player_pos.y
                        + f64::from(player.living_entity.entity.entity_dimension.load().height)
                            * 0.5;
                    let direction = Vector3::new(
                        player_pos.x - spawn_pos.x,
                        target_mid_y - spawn_pos.y,
                        player_pos.z - spawn_pos.z,
                    );

                    if direction.length_squared() > 1e-6 {
                        let base_entity = Entity::from_uuid(
                            Uuid::new_v4(),
                            world.clone(),
                            spawn_pos,
                            &EntityType::DRAGON_FIREBALL,
                        );
                        let fireball = DragonFireballEntity::new_shot(
                            base_entity,
                            dragon_entity.entity_id,
                            direction,
                        );
                        world.spawn_entity(Arc::new(fireball)).await;
                    }

                    dragon.path.lock().await.clear();
                    dragon.set_phase(EnderDragonPhase::Circling).await;
                }
            } else {
                let mut charge = dragon.fireball_charge.lock().await;
                if *charge > 0 {
                    *charge -= 1;
                }
            }
        })
    }
}
