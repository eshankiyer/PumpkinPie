use super::EnderDragonPhase;
use crate::entity::boss::ender_dragon::{DEATH_TIMER_MAX, EnderDragonEntity};
use crate::entity::experience_orb::ExperienceOrbEntity;
use futures::future::BoxFuture;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::math::vector3::Vector3;

pub struct DyingPhase;

/// Vanilla `EnderDragon.tickDeath` applies this self-motion every tick
/// (`EnderDragon.java:538-544`).
fn death_animation_position(position: Vector3<f64>) -> Vector3<f64> {
    position + Vector3::new(0.0, 0.1, 0.0)
}

/// Vanilla `EnderDragon.tickDeath` awards periodic death XP only when the
/// `mob_drops` rule is enabled and the timer is on a five-tick boundary
/// (`EnderDragon.java:528-531`).
const fn should_drop_death_experience(mob_drops: bool, death_time: i32) -> bool {
    mob_drops && death_time > 150 && death_time % 5 == 0
}

impl super::Phase for DyingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Dying
    }

    fn begin<'a>(&'a self, dragon: &'a EnderDragonEntity) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            *dragon.target_location.lock().await = None;
        })
    }

    fn tick<'a>(&'a self, dragon: &'a EnderDragonEntity) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut t = dragon.dragon_death_time.lock().await;
            *t += 1;

            let entity = &dragon.mob_entity.living_entity.entity;
            let world = entity.world.load();

            if *t == 1 {
                world.play_sound(
                    Sound::EntityEnderDragonDeath,
                    SoundCategory::Hostile,
                    &entity.pos.load(),
                );
            }

            if *t >= 180 && *t <= 200 {
                let xo = (rand::random::<f32>() - 0.5) * 8.0;
                let yo = (rand::random::<f32>() - 0.5) * 4.0;
                let zo = (rand::random::<f32>() - 0.5) * 8.0;
                let pos = entity.pos.load();
                world.spawn_particle(
                    Vector3::new(
                        pos.x + xo as f64,
                        pos.y + 2.0 + yo as f64,
                        pos.z + zo as f64,
                    ),
                    Vector3::new(0.0, 0.0, 0.0),
                    0.0,
                    1,
                    Particle::ExplosionEmitter,
                );
            }

            let xp_count = if let Some(ref fight_mutex) = world.dragon_fight
                && !fight_mutex.lock().await.has_previously_killed_dragon()
            {
                12000
            } else {
                500
            };

            let mob_drops = world.level_info.load().game_rules.mob_drops;
            if should_drop_death_experience(mob_drops, *t) {
                ExperienceOrbEntity::spawn(
                    &world,
                    entity.pos.load(),
                    (xp_count as f32 * 0.08) as u32,
                )
                .await;
            }

            // Vanilla moves the dragon and every part directly during `tickDeath`; clear the
            // shared living-tick velocity so Pumpkin does not apply this move a second time on
            // the next tick (`EnderDragon.java:538-544`).
            entity.set_pos(death_animation_position(entity.pos.load()));
            entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
            entity.send_pos_rot();
            for part in &dragon.parts {
                part.entity
                    .set_pos(death_animation_position(part.entity.pos.load()));
                part.entity.send_pos_rot();
            }

            if *t >= DEATH_TIMER_MAX {
                // Vanilla gates the final XP award on `mob_drops` too
                // (`EnderDragon.java:546-549`).
                if mob_drops {
                    ExperienceOrbEntity::spawn(
                        &world,
                        entity.pos.load(),
                        (xp_count as f32 * 0.2) as u32,
                    )
                    .await;
                }

                if let Some(ref fight_mutex) = world.dragon_fight {
                    fight_mutex
                        .lock()
                        .await
                        .set_dragon_killed(&world, entity.entity_uuid)
                        .await;
                }
                // Vanilla `EnderDragon.tickDeath` emits `ENTITY_DIE` before removing the
                // dragon (`EnderDragon.java:546-557`).
                crate::world::game_event::emit_game_event(
                    &world,
                    pumpkin_data::game_event::GameEvent::EntityDie,
                    entity.pos.load(),
                    crate::world::game_event::GameEventContext::none(),
                )
                .await;
                for part in &dragon.parts {
                    part.entity.remove().await;
                }
                entity.remove().await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{death_animation_position, should_drop_death_experience};
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn death_animation_moves_up_by_one_tenth() {
        assert_eq!(
            death_animation_position(Vector3::new(2.0, 3.0, 4.0)),
            Vector3::new(2.0, 3.1, 4.0)
        );
    }

    #[test]
    fn death_experience_obeys_rule_and_cadence() {
        assert!(!should_drop_death_experience(false, 155));
        assert!(!should_drop_death_experience(true, 150));
        assert!(!should_drop_death_experience(true, 152));
        assert!(should_drop_death_experience(true, 155));
    }
}
