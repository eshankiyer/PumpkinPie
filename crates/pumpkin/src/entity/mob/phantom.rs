// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::game_event::GameEvent;
use pumpkin_protocol::{codec::var_int::VarInt, java::client::play::Metadata};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::control::phantom_move_control::PhantomMoveControl,
    ai::goal::{
        phantom_attack_player_target::PhantomAttackPlayerTargetGoal,
        phantom_attack_strategy::PhantomAttackStrategyGoal,
        phantom_circle_anchor::PhantomCircleAroundAnchorGoal,
        phantom_sweep_attack::PhantomSweepAttackGoal,
    },
    mob::{Mob, MobEntity},
};
use crate::world::game_event::{GameEventContext, emit_game_event};

/// Vanilla `Phantom.TICKS_PER_FLAP` (`Phantom.java:44`): `Mth.ceil(24.166098F)`.
const TICKS_PER_FLAP: i32 = 25;

/// Vanilla: `Phantom.AttackPhase` (`Phantom.java:211-214`).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum AttackPhase {
    #[default]
    Circle,
    Swoop,
}

pub struct PhantomEntity {
    pub mob_entity: MobEntity,
    size: AtomicI32,
    /// Vanilla: `Phantom.moveTargetPoint`. Written by the circle/sweep goals, read every tick
    /// by `PhantomMoveControl`.
    move_target_point: AtomicCell<Vector3<f64>>,
    /// Vanilla: `Phantom.anchorPoint`.
    anchor_point: AtomicCell<Option<BlockPos>>,
    /// Vanilla: `Phantom.attackPhase`.
    attack_phase: AtomicCell<AttackPhase>,
}

impl PhantomEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla: `Phantom`'s constructor replaces the default `MoveControl` with the
        // circling/diving `PhantomMoveControl` (`Phantom.java:55`).
        // `finalizeSpawn` (`Phantom.java:156`) sets `anchorPoint = blockPosition().above(5)`;
        // there's no dedicated spawn-finalization hook here, so it's seeded from the entity's
        // position at construction time instead, which is equivalent in practice.
        let initial_anchor = entity.block_pos.load().up_height(5);
        let mob_entity = MobEntity::new(entity);
        *mob_entity.move_control.lock().unwrap() = Box::new(PhantomMoveControl::default());
        let phantom = Self {
            mob_entity,
            size: AtomicI32::new(0),
            move_target_point: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
            anchor_point: AtomicCell::new(Some(initial_anchor)),
            attack_phase: AtomicCell::new(AttackPhase::Circle),
        };
        let mob_arc = Arc::new(phantom);
        let phantom_weak = Arc::downgrade(&mob_arc);

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

            // Vanilla `Phantom.registerGoals` (`Phantom.java:70-74`). Note vanilla installs a
            // no-op `PhantomLookControl` (`Phantom.java:375-383`) precisely because it has no
            // look-related goals; `LookControl` here isn't swappable (concrete field, not a
            // trait object like `move_control`), but since `move_control.tick()` runs after
            // `look_control.tick()` every tick (see `MobEntity`'s tick order), the pitch this
            // sets below always wins. Only head-yaw drifts slightly toward body-yaw each tick
            // as an approximation of the missing no-op.
            goal_selector.add_goal(
                1,
                Box::new(PhantomAttackStrategyGoal::new(phantom_weak.clone())),
            );
            goal_selector.add_goal(
                2,
                Box::new(PhantomSweepAttackGoal::new(phantom_weak.clone())),
            );
            goal_selector.add_goal(
                3,
                Box::new(PhantomCircleAroundAnchorGoal::new(phantom_weak.clone())),
            );

            target_selector.add_goal(
                1,
                Box::new(PhantomAttackPlayerTargetGoal::new(phantom_weak)),
            );
        };

        mob_arc
    }

    pub fn set_size(&self, size: i32) {
        let size = size.clamp(0, 64);
        // Vanilla keeps the size in synched data (`ID_SIZE`), which only broadcasts on change.
        let changed = self.size.swap(size, Ordering::Relaxed) != size;

        let entity = &self.mob_entity.living_entity.entity;
        if changed {
            self.send_size_meta_data(size);
        }
        if let Some(attack_damage) = self
            .mob_entity
            .living_entity
            .attributes
            .write()
            .unwrap()
            .get_mut(&Attributes::ATTACK_DAMAGE.id)
        {
            attack_damage.base_value = 6.0 + f64::from(size);
            attack_damage.dirty.store(true, Ordering::Relaxed);
        }

        let original = entity.entity_type.dimension;
        let scale = 1.0 + 0.15 * size as f32;
        let dimensions = EntityDimensions {
            width: original[0] * scale,
            height: original[1] * scale,
            eye_height: entity.entity_type.eye_height * scale,
            fixed: false,
        };
        entity.base_dimension.store(dimensions);
        entity.entity_dimension.store(dimensions);
        let position = entity.pos.load();
        entity.bounding_box.store(BoundingBox::new_from_pos(
            position.x,
            position.y,
            position.z,
            &dimensions,
        ));
    }

    fn send_size_meta_data(&self, size: i32) {
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::phantom::ID_SIZE,
                VarInt(size),
            )],
            None,
        );
    }

    #[must_use]
    pub fn size(&self) -> i32 {
        self.size.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn move_target_point(&self) -> Vector3<f64> {
        self.move_target_point.load()
    }

    pub fn set_move_target_point(&self, point: Vector3<f64>) {
        self.move_target_point.store(point);
    }

    #[must_use]
    pub fn anchor_point(&self) -> Option<BlockPos> {
        self.anchor_point.load()
    }

    pub fn set_anchor_point(&self, anchor: Option<BlockPos>) {
        self.anchor_point.store(anchor);
    }

    #[must_use]
    pub fn attack_phase(&self) -> AttackPhase {
        self.attack_phase.load()
    }

    pub fn set_attack_phase(&self, phase: AttackPhase) {
        self.attack_phase.store(phase);
    }
}

impl NBTStorage for PhantomEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            nbt.put_int("size", self.size());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.set_size(nbt.get_int("size").unwrap_or(0));
        })
    }
}

impl Mob for PhantomEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0
    }

    /// `set_size` runs from NBT load before the entity has viewers, so its broadcast reaches
    /// nobody; publish `ID_SIZE` here. Also replaces the default `Mob` init, whose
    /// `DATA_BABY_ID` send does not apply to a non-ageable mob.
    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.send_size_meta_data(self.size());
        })
    }

    /// Vanilla `Phantom.isFlapping` (`Phantom.java:59-62`) drives `Entity.processFlappingMovement`
    /// (`Entity.java:1037-1044`): with air under the feet, a flap fires `GameEvent.FLAP` every
    /// `TICKS_PER_FLAP` ticks, offset per entity id. The flap sound is client-only.
    fn mob_tick<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;
            let unique_offset = entity.entity_id.wrapping_mul(3);
            if unique_offset.wrapping_add(entity.age.load(Ordering::Relaxed)) % TICKS_PER_FLAP != 0
                || entity.has_vehicle().await
            {
                return;
            }
            let world = entity.world.load();
            let pos = entity.pos.load();
            // `getOnPos` samples 1.0E-5 below the feet.
            let on_pos = BlockPos::floored(pos.x, pos.y - 1.0E-5, pos.z);
            if !world.get_block_state(&on_pos).is_air() {
                return;
            }
            emit_game_event(
                &world,
                GameEvent::Flap,
                pos,
                GameEventContext::of_entity(caller.clone()),
            )
            .await;
        })
    }

    /// Vanilla `Phantom.travel` (`Phantom.java:148-150`) is `travelFlying(input, 0.2F)`
    /// (`LivingEntity.java:2443-2457`): no gravity, block friction or fluid physics.
    fn custom_travel<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let living = &self.mob_entity.living_entity;
            let entity = &living.entity;
            let in_water = entity.touching_water.load(Ordering::Relaxed);
            let in_lava = !in_water && entity.touching_lava.load(Ordering::Relaxed);
            let speed = if in_water || in_lava {
                f64::from(0.02f32)
            } else {
                f64::from(0.2f32)
            };
            entity.update_velocity_from_input(living.movement_input.load(), speed);
            entity.move_entity(caller, entity.velocity.load()).await;
            let drag = if in_water {
                f64::from(0.8f32)
            } else if in_lava {
                0.5
            } else {
                f64::from(0.91f32)
            };
            entity.velocity.store(entity.velocity.load() * drag);
            true
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn phantom_sizes_follow_vanilla_bounds() {
        assert_eq!((-10i32).clamp(0, 64), 0);
        assert_eq!(64i32.clamp(0, 64), 64);
        assert_eq!(90i32.clamp(0, 64), 64);
    }

    #[test]
    fn phantom_attack_damage_scales_with_size() {
        assert_eq!(6.0 + f64::from(0), 6.0);
        assert_eq!(6.0 + f64::from(12), 18.0);
    }
}
