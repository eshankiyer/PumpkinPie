// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;

use pumpkin_data::{attributes::Attributes, entity::EntityType};
use rand::{RngExt, rng};
use uuid::Uuid;

use crate::entity::{
    EntityBase,
    ai::pathfinder::NavigatorGoal,
    experience_orb::ExperienceOrbEntity,
    mob::Mob,
    passive::equine::{
        MAX_HEALTH, MAX_JUMP_STRENGTH, MAX_MOVEMENT_SPEED, MIN_HEALTH, MIN_JUMP_STRENGTH,
        MIN_MOVEMENT_SPEED, apply_offspring_attribute,
    },
    r#type::from_type,
};

use super::{Controls, Goal, GoalFuture};

/// Donkey.java#getBreedOffspring / Horse.java#getBreedOffspring: Donkey + Donkey -> Donkey,
/// Horse + Horse -> Horse, Donkey + Horse (either order) -> Mule.
const fn horse_family_offspring(
    a: &'static EntityType,
    b: &'static EntityType,
) -> &'static EntityType {
    if a.id == EntityType::DONKEY.id && b.id == EntityType::DONKEY.id {
        &EntityType::DONKEY
    } else if a.id == EntityType::HORSE.id && b.id == EntityType::HORSE.id {
        &EntityType::HORSE
    } else {
        &EntityType::MULE
    }
}

pub struct HorseBreedGoal {
    speed: f64,
    compatible: &'static [&'static EntityType],
    mate: Option<Arc<dyn EntityBase>>,
    timer: i32,
}

impl HorseBreedGoal {
    #[must_use]
    pub fn new(speed: f64, compatible: &'static [&'static EntityType]) -> Box<Self> {
        Box::new(Self {
            speed,
            compatible,
            mate: None,
            timer: 0,
        })
    }

    async fn can_parent(mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let living = &mob.get_mob_entity().living_entity;
        !entity.has_passengers().await
            && !entity.has_vehicle().await
            && mob.get_mob_entity().is_tamed()
            && entity.age.load(std::sync::atomic::Ordering::Relaxed) >= 0
            && living.health.load() >= living.get_max_health()
            && mob.get_mob_entity().is_breeding_ready()
            && mob.get_mob_entity().is_in_love()
    }

    async fn find_mate(&self, mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        if !Self::can_parent(mob).await {
            return None;
        }

        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let world = entity.world.load();
        let my_uuid = entity.entity_uuid;

        let nearby = world.get_nearby_entities(pos, 8.0);
        let mut closest: Option<(f64, Arc<dyn EntityBase>)> = None;

        for candidate in nearby.values() {
            let c_entity = candidate.get_entity();
            if c_entity.entity_uuid == my_uuid {
                continue;
            }
            if !self
                .compatible
                .iter()
                .any(|t| t.id == c_entity.entity_type.id)
            {
                continue;
            }
            let Some(candidate_mob) = candidate.get_mob() else {
                continue;
            };
            if !Self::can_parent(candidate_mob).await || candidate.is_panicking() {
                continue;
            }

            let dist = pos.squared_distance_to_vec(&c_entity.pos.load());
            match &closest {
                Some((best_dist, _)) if dist >= *best_dist => {}
                _ => closest = Some((dist, candidate.clone())),
            }
        }

        closest.map(|(_, e)| e)
    }

    #[allow(clippy::if_not_else, clippy::too_many_lines)]
    async fn breed(mob: &dyn Mob, mate: &dyn EntityBase) {
        let mob_entity = mob.get_mob_entity();
        if !mob_entity.try_claim_love() {
            return;
        }
        if !mate.try_claim_love() {
            return;
        }

        let entity = mob.get_entity();
        let world = entity.world.load();

        mob_entity
            .breeding_cooldown
            .store(6000, std::sync::atomic::Ordering::Relaxed);

        let parent_pos = entity.pos.load();
        let baby = if entity.entity_type.id != mate.get_entity().entity_type.id {
            let baby = from_type(
                horse_family_offspring(entity.entity_type, mate.get_entity().entity_type),
                parent_pos,
                &world,
                Uuid::new_v4(),
            );
            if let Some(baby_mob) = baby.get_mob() {
                let mut random = rand::rng();
                let parent_a = mob.get_mob_entity();
                apply_offspring_attribute(
                    baby_mob,
                    &Attributes::MAX_HEALTH,
                    parent_a
                        .living_entity
                        .get_attribute_base(&Attributes::MAX_HEALTH),
                    mate.get_mob().map_or(MIN_HEALTH, |m| {
                        m.get_mob_entity()
                            .living_entity
                            .get_attribute_base(&Attributes::MAX_HEALTH)
                    }),
                    MIN_HEALTH,
                    MAX_HEALTH,
                    &mut random,
                );
                apply_offspring_attribute(
                    baby_mob,
                    &Attributes::JUMP_STRENGTH,
                    parent_a
                        .living_entity
                        .get_attribute_base(&Attributes::JUMP_STRENGTH),
                    mate.get_mob().map_or(MIN_JUMP_STRENGTH, |m| {
                        m.get_mob_entity()
                            .living_entity
                            .get_attribute_base(&Attributes::JUMP_STRENGTH)
                    }),
                    MIN_JUMP_STRENGTH,
                    MAX_JUMP_STRENGTH,
                    &mut random,
                );
                apply_offspring_attribute(
                    baby_mob,
                    &Attributes::MOVEMENT_SPEED,
                    parent_a
                        .living_entity
                        .get_attribute_base(&Attributes::MOVEMENT_SPEED),
                    mate.get_mob().map_or(MIN_MOVEMENT_SPEED, |m| {
                        m.get_mob_entity()
                            .living_entity
                            .get_attribute_base(&Attributes::MOVEMENT_SPEED)
                    }),
                    MIN_MOVEMENT_SPEED,
                    MAX_MOVEMENT_SPEED,
                    &mut random,
                );
            }
            Some(baby)
        } else {
            mob.create_offspring(mate, &world).await
        };

        let player = mob_entity
            .breeder
            .load()
            .and_then(|uuid| {
                world
                    .server
                    .upgrade()
                    .and_then(|server| server.get_player_by_uuid(uuid))
                    .or_else(|| world.get_player_by_uuid(uuid))
            })
            .or_else(|| {
                mate.get_mob()
                    .and_then(|mate_mob| mate_mob.get_mob_entity().breeder.load())
                    .and_then(|uuid| {
                        world
                            .server
                            .upgrade()
                            .and_then(|server| server.get_player_by_uuid(uuid))
                            .or_else(|| world.get_player_by_uuid(uuid))
                    })
            });
        if let Some(player) = player {
            player
                .increment_stat(
                    pumpkin_data::statistic::StatisticCategory::Custom,
                    pumpkin_data::statistic::CustomStatistic::AnimalsBred as i32,
                    1,
                )
                .await;

            player
                .trigger_advancement(
                    crate::entity::player::advancement::trigger::AdvancementTrigger::BredAnimal {
                        parent_type: format!("minecraft:{}", entity.entity_type.resource_name),
                    },
                )
                .await;
        }

        mob.get_entity().set_age(6000);
        mob.reset_love();
        mate.get_entity().set_age(6000);
        mate.reset_love();
        mate.set_breeding_cooldown(6000);
        world.send_entity_status(
            mob.get_entity(),
            pumpkin_data::entity::EntityStatus::InLoveHearts,
            Some(pumpkin_protocol::bedrock::server::actor_event::ActorEventType::InLoveHearts),
        );

        if world.level_info.load().game_rules.mob_drops {
            let xp = rng().random_range(1u32..=7);
            ExperienceOrbEntity::spawn(&world, parent_pos, xp).await;
        }

        let Some(baby) = baby else {
            return;
        };
        if let Some(baby_mob) = baby.get_mob() {
            baby_mob.set_persistence_required();
        }
        baby.get_entity().set_age(-24000);
        world.spawn_entity(baby).await;
    }
}

impl Goal for HorseBreedGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            if !Self::can_parent(mob).await {
                return false;
            }

            self.mate = self.find_mate(mob).await;
            self.mate.is_some()
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            let Some(mate) = &self.mate else {
                return false;
            };

            if !mate.get_entity().is_alive() || mate.is_panicking() {
                return false;
            }

            mate.is_in_love() && self.timer < 60
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.timer = 0;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.mate = None;
            self.timer = 0;
            let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
            navigator.stop();
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            let Some(mate) = &self.mate else {
                return;
            };

            let mob_entity = mob.get_mob_entity();
            let mate_pos = mate.get_entity().pos.load();

            {
                let mut look_control = mob_entity.look_control.lock().unwrap();
                look_control.look_at_entity(mob, mate);
            };

            let my_pos = mob.get_entity().pos.load();
            let dist_sq = my_pos.squared_distance_to_vec(&mate_pos);

            {
                let mut navigator = mob_entity.navigator.lock().unwrap();
                navigator.set_progress(NavigatorGoal::new(my_pos, mate_pos, self.speed));
            };

            self.timer += 1;

            if self.timer >= 60 && dist_sq < 9.0 {
                Self::breed(mob, mate.as_ref()).await;
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn donkey_and_donkey_breed_donkey() {
        let result = horse_family_offspring(&EntityType::DONKEY, &EntityType::DONKEY);
        assert_eq!(result.id, EntityType::DONKEY.id);
    }

    #[test]
    fn horse_and_horse_breed_horse() {
        let result = horse_family_offspring(&EntityType::HORSE, &EntityType::HORSE);
        assert_eq!(result.id, EntityType::HORSE.id);
    }

    #[test]
    fn donkey_and_horse_breed_mule() {
        assert_eq!(
            horse_family_offspring(&EntityType::DONKEY, &EntityType::HORSE).id,
            EntityType::MULE.id
        );
        assert_eq!(
            horse_family_offspring(&EntityType::HORSE, &EntityType::DONKEY).id,
            EntityType::MULE.id
        );
    }
}
