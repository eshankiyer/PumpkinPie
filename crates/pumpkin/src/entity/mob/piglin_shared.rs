use crate::entity::player::Player;
use crate::entity::{EntityBase, mob::Mob};
use pumpkin_data::entity::EntityType;
use std::sync::Arc;
use std::sync::atomic::Ordering;

const ALERT_RADIUS: f64 = 16.0;

/// Sets `mob` to retaliate against `source`, then alerts nearby adult piglins that
/// don't already have a target to retaliate too.
///
/// Goal-based approximation of `PiglinAi.maybeRetaliate` + `broadcastAngerTarget`
/// (PiglinAi.java:556-618): vanilla propagates via a `Brain` memory broadcast to
/// `NEARBY_ADULT_PIGLINS`, which Pumpkin has no equivalent of, so this does a direct
/// 16-block world query instead (same pattern `RevengeGoal::start` already uses for
/// same-type alerting). Shared by `PiglinEntity` and `PiglinBruteEntity::on_damage`,
/// matching vanilla's shared `PiglinAi.maybeRetaliate` call from both `PiglinAi.wasHurtBy`
/// and `PiglinBruteAi.wasHurtBy`.
pub async fn retaliate_and_alert_piglins(mob: &dyn Mob, source: &dyn EntityBase) {
    let mob_entity = mob.get_mob_entity();
    let entity = &mob_entity.living_entity.entity;
    let world = entity.world.load();

    let source_id = source.get_entity().entity_id;
    let Some(source_arc) = world.get_entity_by_id(source_id) else {
        return;
    };

    mob.set_mob_target(Some(source_arc.clone())).await;

    let position = entity.pos.load();
    for nearby in world
        .get_nearby_entities(position, ALERT_RADIUS)
        .into_values()
    {
        if nearby.get_entity().entity_id == entity.entity_id
            || nearby.get_entity().entity_type.id != EntityType::PIGLIN.id
        {
            continue;
        }
        let Some(nearby_mob) = nearby.get_mob() else {
            continue;
        };
        if nearby_mob.get_mob_entity().target.lock().await.is_some() {
            continue;
        }
        nearby_mob.set_mob_target(Some(source_arc.clone())).await;
    }
}

/// `PiglinAi.angerNearbyPiglins` (`PiglinAi.java:536-545`) for guarded-block
/// interactions.
///
/// The goal-based server model has no Brain visibility memories, so it
/// performs the same 16-block query directly and uses each piglin's sensing raycast
/// for the `onlyIfTheySeeThePlayer` filter.
pub async fn anger_nearby_piglins(world: &Arc<crate::world::World>, player: &Arc<Player>) {
    let player_entity = player.get_entity();
    let search_box = player_entity.bounding_box.load().expand(16.0, 16.0, 16.0);
    let universal_anger = world.level_info.load().game_rules.universal_anger;

    for nearby in world.get_entities_at_box(&search_box) {
        if nearby.get_entity().entity_type.id != EntityType::PIGLIN.id
            || nearby.get_entity().age.load(Ordering::Relaxed) < 0
        {
            continue;
        }

        let Some(nearby_mob) = nearby.get_mob() else {
            continue;
        };
        if nearby_mob.get_mob_entity().target.lock().await.is_some()
            || !nearby_mob
                .get_mob_entity()
                .has_line_of_sight(player.as_ref())
                .await
        {
            continue;
        }

        let target: Arc<dyn EntityBase> = if universal_anger {
            let piglin_position = nearby.get_entity().pos.load();
            let mut visible_target = None;
            let mut closest_distance = f64::MAX;
            for candidate in world.get_nearby_players(piglin_position, 16.0) {
                let candidate_entity = candidate.get_entity();
                if candidate_entity.is_spectator()
                    || !candidate_entity.is_alive()
                    || candidate_entity.get_living_entity().is_some_and(|living| {
                        living.not_targetable_as_enemy.load(Ordering::Relaxed)
                    })
                    || super::piglin::is_wearing_safe_armor(&candidate.living_entity).await
                    || !nearby_mob
                        .get_mob_entity()
                        .has_line_of_sight(candidate.as_ref())
                        .await
                {
                    continue;
                }
                let distance = candidate_entity
                    .pos
                    .load()
                    .squared_distance_to_vec(&piglin_position);
                if distance < closest_distance {
                    closest_distance = distance;
                    visible_target = Some(candidate);
                }
            }
            visible_target.map_or_else(
                || player.clone() as Arc<dyn EntityBase>,
                |candidate| candidate,
            )
        } else {
            player.clone() as Arc<dyn EntityBase>
        };

        nearby_mob.set_mob_target(Some(target)).await;
    }
}
