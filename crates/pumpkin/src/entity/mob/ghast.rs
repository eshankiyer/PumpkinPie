// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Weak};

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage,
    ai::control::ghast_move_control::GhastMoveControl,
    ai::goal::{
        Controls, Goal, GoalFuture, ghast_random_float::GhastRandomFloatAroundGoal,
        ghast_shoot_fireball::GhastShootFireballGoal, ghast_target::GhastNearestPlayerTargetGoal,
    },
    mob::{Mob, MobEntity},
};

pub struct GhastEntity {
    pub mob_entity: MobEntity,
    pub is_charging: AtomicBool,
    pub explosion_power: AtomicU8,
}

impl GhastEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        // Vanilla: `Ghast`'s constructor replaces the default `MoveControl` with
        // `Ghast.GhastMoveControl` (Ghast.java:52).
        *mob_entity.move_control.lock().unwrap() = Box::new(GhastMoveControl::default());
        let ghast = Self {
            mob_entity,
            is_charging: AtomicBool::new(false),
            explosion_power: AtomicU8::new(1),
        };

        let mob_arc = Arc::new(ghast);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let ghast_weak = Arc::downgrade(&mob_arc);

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Vanilla: Ghast.java:57-59.
            goal_selector.add_goal(5, Box::new(GhastRandomFloatAroundGoal::new()));
            goal_selector.add_goal(7, Box::new(GhastLookGoal::new(mob_weak.clone())));
            goal_selector.add_goal(7, Box::new(GhastShootFireballGoal::new(ghast_weak)));

            // Vanilla: Ghast.java:60-61.
            target_selector.add_goal(1, GhastNearestPlayerTargetGoal::new(&mob_arc.mob_entity));
        };

        mob_arc
    }

    pub fn set_charging(&self, charging: bool) {
        // You would also sync this to the client via EntityMetadata here
        self.is_charging.store(charging, Ordering::Relaxed);
    }

    pub fn is_charging(&self) -> bool {
        self.is_charging.load(Ordering::Relaxed)
    }

    pub fn explosion_power(&self) -> u8 {
        self.explosion_power.load(Ordering::Relaxed)
    }
}

impl NBTStorage for GhastEntity {}

impl Mob for GhastEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0 // Ghasts fly, no gravity applied in standard travel
    }

    /// Vanilla `Ghast.travel` (`Ghast.java:93-96`) is `travelFlying(input, 0.02F)`
    /// (`LivingEntity.java:2443-2457`): 0.02 acceleration in every medium, no gravity or block
    /// friction, then 0.8 (water), 0.5 (lava) or 0.91 drag on all axes.
    fn custom_travel<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let living = &self.mob_entity.living_entity;
            let entity = &living.entity;
            entity.update_velocity_from_input(living.movement_input.load(), f64::from(0.02f32));
            entity.move_entity(caller, entity.velocity.load()).await;
            let drag = if entity.touching_water.load(Ordering::Relaxed) {
                f64::from(0.8f32)
            } else if entity.touching_lava.load(Ordering::Relaxed) {
                0.5
            } else {
                f64::from(0.91f32)
            };
            entity.velocity.store(entity.velocity.load() * drag);
            true
        })
    }
}

#[expect(dead_code)]
pub struct GhastLookGoal {
    goal_control: Controls,
    mob_weak: Weak<dyn Mob>,
}

impl GhastLookGoal {
    #[must_use]
    pub fn new(mob_weak: Weak<dyn Mob>) -> Self {
        Self {
            goal_control: Controls::LOOK,
            mob_weak,
        }
    }
}

/// Vanilla `Ghast.faceMovementDirection` (`Ghast.java:186-202`).
///
/// Instantly turns the body (and yaw) toward the target when it is within 64 blocks, or toward
/// the movement direction when there is no target. It does not go through `LookControl`.
pub async fn face_movement_direction(mob: &dyn Mob) {
    let mob_entity = mob.get_mob_entity();
    let entity = &mob_entity.living_entity.entity;
    let target = mob_entity.target.lock().await.clone();
    let yaw = if let Some(target) = target {
        let pos = entity.pos.load();
        let target_pos = target.get_entity().pos.load();
        if pos.squared_distance_to_vec(&target_pos) >= 4096.0 {
            return;
        }
        let (dx, dz) = (target_pos.x - pos.x, target_pos.z - pos.z);
        (-f64::atan2(dx, dz) as f32) * (180.0 / std::f32::consts::PI)
    } else {
        let velocity = entity.velocity.load();
        (-f64::atan2(velocity.x, velocity.z) as f32) * (180.0 / std::f32::consts::PI)
    };
    entity.yaw.store(yaw);
    entity.body_yaw.store(yaw);
}

impl Goal for GhastLookGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            face_movement_direction(mob).await;
        })
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}
