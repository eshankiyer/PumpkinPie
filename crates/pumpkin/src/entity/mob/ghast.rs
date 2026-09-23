// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::position::BlockPos;

use crate::entity::{
    Entity, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::control::ghast_move_control::GhastMoveControl,
    ai::goal::{
        Controls, Goal, GoalFuture, ghast_random_float::GhastRandomFloatAroundGoal,
        ghast_shoot_fireball::GhastShootFireballGoal, ghast_target::GhastNearestPlayerTargetGoal,
    },
    mob::{Mob, MobEntity},
};
use crate::world::World;

pub struct GhastEntity {
    pub mob_entity: MobEntity,
    pub is_charging: AtomicBool,
    pub explosion_power: AtomicU8,
}

impl GhastEntity {
    /// `Ghast.explosionPower` default (`Ghast.java`, `ExplosionPower` NBT default 1).
    pub const DEFAULT_EXPLOSION_POWER: u8 = 1;
    /// `Ghast.xpReward`.
    pub const XP_REWARD: u32 = 5;
    /// `Attributes.FLYING_SPEED` for the ghast.
    pub const FLYING_SPEED: f64 = 0.06;

    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        // Vanilla: `Ghast`'s constructor replaces the default `MoveControl` with
        // `Ghast.GhastMoveControl` (Ghast.java:52).
        *mob_entity.move_control.lock().unwrap() = Box::new(GhastMoveControl::default());
        let ghast = Self {
            mob_entity,
            is_charging: AtomicBool::new(false),
            explosion_power: AtomicU8::new(Self::DEFAULT_EXPLOSION_POWER),
        };

        let mob_arc = Arc::new(ghast);
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
            goal_selector.add_goal(7, Box::new(GhastLookGoal::new()));
            goal_selector.add_goal(7, Box::new(GhastShootFireballGoal::new(ghast_weak)));

            // Vanilla: Ghast.java:60-61.
            target_selector.add_goal(1, GhastNearestPlayerTargetGoal::new(&mob_arc.mob_entity));
        };

        mob_arc
    }

    /// `Ghast.setCharging`: updates the synced `DATA_IS_CHARGING`.
    pub fn set_charging(&self, charging: bool) {
        self.is_charging.store(charging, Ordering::Relaxed);
        let entity = &self.mob_entity.living_entity.entity;
        entity.send_meta_data(
            &[Metadata::new(
                tracked_data::ghast::DATA_IS_CHARGING,
                charging,
            )],
            None,
        );
    }

    #[must_use]
    pub fn is_charging(&self) -> bool {
        self.is_charging.load(Ordering::Relaxed)
    }

    /// `Ghast.getExplosionPower`.
    #[must_use]
    pub fn explosion_power(&self) -> u8 {
        self.explosion_power.load(Ordering::Relaxed)
    }

    /// Upstream-named alias of [`Self::explosion_power`].
    #[must_use]
    pub fn get_explosion_power(&self) -> u8 {
        self.explosion_power()
    }

    pub fn set_explosion_power(&self, power: u8) {
        self.explosion_power.store(power, Ordering::Relaxed);
    }

    /// `Ghast.checkGhastSpawnRules` (`Ghast.java:149-153`): not in Peaceful, then a 1-in-20
    /// roll. The trailing `checkMobSpawnRules` spawn-block test is left to the spawn
    /// placement check, like the other Pumpkin spawn-rule predicates.
    #[must_use]
    pub fn check_ghast_spawn_rules(world: &World, _pos: &BlockPos) -> bool {
        if world.level_info.load().difficulty == pumpkin_util::Difficulty::Peaceful {
            return false;
        }
        rand::random_range(0..20) == 0
    }
}

impl NBTStorage for GhastEntity {
    /// `Ghast.addAdditionalSaveData` (`Ghast.java:160-164`).
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            nbt.put_byte("ExplosionPower", self.explosion_power() as i8);
        })
    }

    /// `Ghast.readAdditionalSaveData` (`Ghast.java:166-170`): `getByteOr("ExplosionPower", 1)`.
    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            let power = nbt
                .get_byte("ExplosionPower")
                .map_or(Self::DEFAULT_EXPLOSION_POWER, |power| power as u8);
            self.set_explosion_power(power);
        })
    }
}

impl Mob for GhastEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0 // Ghasts fly, no gravity applied in standard travel
    }

    /// `Ghast.travel` -> `travelFlying(input, 0.02F)`: airborne velocity is scaled by the same
    /// `0.91F` on every axis.
    fn get_mob_y_velocity_drag(&self) -> Option<f64> {
        Some(0.91)
    }

    /// `Ghast.defineSynchedData`: `DATA_IS_CHARGING`.
    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            if self.is_charging() {
                entity.send_meta_data(
                    &[Metadata::new(tracked_data::ghast::DATA_IS_CHARGING, true)],
                    None,
                );
            }
        })
    }

    /// `Ghast.hurtServer` (`Ghast.java:101-108`) turns a reflected large fireball into 1000
    /// damage. Only the damage type reaches this hook, so every `fireball` hit counts as
    /// reflected; a ghast is only hit by one after a player has deflected it.
    fn modify_incoming_damage(&self, amount: f32, damage_type: DamageType) -> f32 {
        if damage_type.id == DamageType::FIREBALL.id {
            1000.0
        } else {
            amount
        }
    }

    fn get_base_experience_reward(&self) -> u32 {
        Self::XP_REWARD
    }
}

/// Vanilla: `Ghast.GhastLookGoal` (`Ghast.java:204-226`), which runs
/// `Ghast.faceMovementDirection` every tick.
pub struct GhastLookGoal {
    goal_control: Controls,
}

impl Default for GhastLookGoal {
    fn default() -> Self {
        Self {
            goal_control: Controls::LOOK,
        }
    }
}

impl GhastLookGoal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Goal for GhastLookGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    /// `Ghast.faceMovementDirection` (`Ghast.java:187-202`): faces the target within 64 blocks,
    /// or the movement direction without one, setting `yRot` and `yBodyRot` directly.
    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let mob_entity = mob.get_mob_entity();
            let entity = &mob_entity.living_entity.entity;
            let target_opt = mob_entity.target.lock().await.clone();

            let yaw = if let Some(target) = target_opt {
                let mob_pos = entity.pos.load();
                let target_pos = target.get_entity().pos.load();
                if target_pos.squared_distance_to_vec(&mob_pos) >= 4096.0 {
                    return;
                }
                -(f64::atan2(target_pos.x - mob_pos.x, target_pos.z - mob_pos.z) as f32)
                    .to_degrees()
            } else {
                let velocity = entity.velocity.load();
                -(f64::atan2(velocity.x, velocity.z) as f32).to_degrees()
            };
            entity.yaw.store(yaw);
            entity.body_yaw.store(yaw);
        })
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}
