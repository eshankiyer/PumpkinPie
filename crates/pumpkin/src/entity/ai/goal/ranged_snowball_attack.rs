// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::{Controls, Goal, GoalFuture};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::projectile::snowball::SnowballEntity;
use crate::entity::{Entity, EntityBase};

pub struct RangedSnowballAttackGoal {
    target: Option<Arc<dyn EntityBase>>,
    attack_time: i32,
    see_time: i32,
    attack_interval: i32,
    speed: f64,
    range: f64,
}

impl RangedSnowballAttackGoal {
    #[must_use]
    pub const fn new(interval: i32, range: f64) -> Self {
        Self {
            target: None,
            attack_time: -1,
            see_time: 0,
            attack_interval: interval,
            speed: 1.25,
            range,
        }
    }

    async fn has_line_of_sight(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
        mob.get_mob_entity().has_line_of_sight(target).await
    }

    fn projectile_velocity(
        shooter: Vector3<f64>,
        target: Vector3<f64>,
        target_eye_height: f64,
    ) -> Vector3<f64> {
        let x = target.x - shooter.x;
        let z = target.z - shooter.z;
        let horizontal = x.hypot(z);
        let y = target.y + target_eye_height - 1.1 - shooter.y + horizontal * 0.2;
        Vector3::new(x, y, z).normalize() * 1.6
    }

    async fn shoot(&self, mob: &dyn Mob, target: &dyn EntityBase) {
        let shooter = mob.get_entity();
        let world = shooter.world.load_full();
        let position = shooter.pos.load();
        let projectile_entity = Entity::new(world.clone(), position, &EntityType::SNOWBALL);
        let projectile = SnowballEntity::new_shot(projectile_entity, shooter);
        let projectile_position = projectile.get_entity().pos.load();
        let velocity = Self::projectile_velocity(
            projectile_position,
            target.get_entity().pos.load(),
            target.get_entity().get_eye_height(),
        );
        projectile
            .thrown
            .set_velocity(velocity.x, velocity.y, velocity.z, 1.6, 12.0);
        world.spawn_entity(Arc::new(projectile)).await;
        // `Entity.playSound` (`Entity.java:1486-1490`): skipped when silent, category is
        // `getSoundSource()`, which `SnowGolem` leaves at the `NEUTRAL` default.
        if !shooter.is_silent() {
            let pitch = 0.4 / mob.get_random().random::<f32>().mul_add(0.4, 0.8);
            world.play_sound_fine(
                Sound::EntitySnowGolemShoot,
                SoundCategory::Neutral,
                &position,
                1.0,
                pitch,
            );
        }
    }

    /// `RangedAttackGoal.canUse`: caches the mob's target while it is alive.
    async fn refresh_target(&mut self, mob: &dyn Mob) -> bool {
        let target = mob.get_mob_entity().target.lock().await.clone();
        match target {
            Some(target) if target.get_entity().is_alive() => {
                self.target = Some(target);
                true
            }
            _ => false,
        }
    }
}

impl Goal for RangedSnowballAttackGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { self.refresh_target(mob).await })
    }

    /// `RangedAttackGoal.canContinueToUse` (`RangedAttackGoal.java:55-57`): the live target is
    /// still valid, or the cached one is alive and the mob is still navigating.
    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if self.refresh_target(mob).await {
                return true;
            }
            self.target
                .as_ref()
                .is_some_and(|target| target.get_entity().is_alive())
                && !mob.get_mob_entity().navigator.lock().unwrap().is_idle()
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {})
    }

    fn stop<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.target = None;
            self.attack_time = -1;
            self.see_time = 0;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(target) = self.target.clone() else {
                return;
            };
            let shooter = mob.get_entity();
            let shooter_pos = shooter.pos.load();
            let target_pos = target.get_entity().pos.load();
            let distance_squared = shooter_pos.squared_distance_to_vec(&target_pos);
            let has_line_of_sight = Self::has_line_of_sight(mob, target.as_ref()).await;
            if has_line_of_sight {
                self.see_time += 1;
            } else {
                self.see_time = 0;
            }

            // `RangedAttackGoal.tick` (`RangedAttackGoal.java:81-99`): range and sight only pick
            // between holding still and approaching; the countdown below always runs, so the
            // golem keeps throwing while it closes in on a visible target.
            if distance_squared > self.range * self.range || self.see_time < 5 {
                mob.get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap()
                    .set_progress(NavigatorGoal {
                        current_progress: shooter_pos,
                        destination: target_pos,
                        speed: self.speed,
                    });
            } else {
                mob.get_mob_entity().navigator.lock().unwrap().stop();
            }

            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap()
                .look_at_entity_with_range(&target, 30.0, 30.0);

            self.attack_time -= 1;
            if self.attack_time == 0 {
                if !has_line_of_sight {
                    return;
                }
                self.shoot(mob, target.as_ref()).await;
                self.attack_time = self.attack_interval;
            } else if self.attack_time < 0 {
                self.attack_time = self.attack_interval;
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
    use super::RangedSnowballAttackGoal;
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn snowball_velocity_uses_vanilla_vertical_lead() {
        let velocity = RangedSnowballAttackGoal::projectile_velocity(
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(3.0, 0.0, 4.0),
            1.8,
        );
        assert!(velocity.y > 0.0);
        assert!((velocity.length() - 1.6).abs() < 1.0e-9);
    }

    #[test]
    fn snowball_goal_uses_vanilla_interval_and_range() {
        let goal = RangedSnowballAttackGoal::new(20, 10.0);
        assert_eq!(goal.attack_interval, 20);
        assert_eq!(goal.speed, 1.25);
        assert_eq!(goal.range, 10.0);
    }
}
