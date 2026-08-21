//! Port of `behavior/GoAndGiveItemsToTarget.java`, together with the
//! `BehaviorUtils.throwItem` helper it calls (`behavior/BehaviorUtils.java:94-105`).

use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::Entity;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::behavior::{
    Behavior, MemoryKeyId, MemoryStatus, TimedBehavior, TimedBehaviorControl,
};
use crate::entity::ai::brain::memory::{
    ItemPickupCooldownTicksMemory, LookTargetMemory, PositionTracker, WalkTarget, WalkTargetMemory,
};
use crate::entity::item::ItemEntity;
use crate::entity::mob::Mob;

/// `GoAndGiveItemsToTarget.CLOSE_ENOUGH_DISTANCE_TO_TARGET` (`:16`).
const CLOSE_ENOUGH_DISTANCE_TO_TARGET: f64 = 3.0;
/// `GoAndGiveItemsToTarget.ITEM_PICKUP_COOLDOWN_AFTER_THROWING` (`:17`).
const ITEM_PICKUP_COOLDOWN_AFTER_THROWING: i32 = 60;
/// `this.throwVelocity = new Vec3(0.2F, 0.3F, 0.2F)` (`:43`).
const THROW_VELOCITY: Vector3<f64> = Vector3::new(0.2, 0.3, 0.2);
/// The `handYDistanceFromEye` this behavior passes to `BehaviorUtils.throwItem` (`:76`).
const HAND_Y_DISTANCE_FROM_EYE: f64 = 0.2;
/// `ItemEntity.setDefaultPickUpDelay()` is 10 ticks.
const DEFAULT_PICKUP_DELAY: u8 = 10;

pub type TargetPositionGetter = fn(&dyn Mob, &Brain) -> Option<PositionTracker>;
/// `GoAndGiveItemsToTarget.ItemThrower<E>` (`:88-91`).
///
/// The trailing `i64` is the brain tick's `game_time`; vanilla reads it back off the level
/// inside the callback (`AllayAi.java:160`), which is not reachable synchronously here.
pub type ItemThrower = fn(&dyn Mob, &ItemStack, BlockPos, i64);

pub struct GoAndGiveItemsToTarget {
    target_position_getter: TargetPositionGetter,
    speed_modifier: f32,
    item_thrower: ItemThrower,
}

impl GoAndGiveItemsToTarget {
    /// `new GoAndGiveItemsToTarget(targetPositionGetter, speedModifier, timeoutDuration,
    /// itemThrower)` (`:22-45`). `Behavior(entryCondition, duration)` sets min and max
    /// duration to the same value (`behavior/Behavior.java:23-25`).
    // Returns a boxed trait object, not Self by name -- constructor pattern for this behavior/sensor family.
    #[allow(clippy::new_ret_no_self)]
    #[must_use]
    pub fn new(
        target_position_getter: TargetPositionGetter,
        speed_modifier: f32,
        timeout_duration: i32,
        item_thrower: ItemThrower,
    ) -> Box<dyn Behavior> {
        Box::new(TimedBehaviorControl::with_duration(
            Self {
                target_position_getter,
                speed_modifier,
                item_thrower,
            },
            vec![
                (MemoryKeyId::LookTarget, MemoryStatus::Registered),
                (MemoryKeyId::WalkTarget, MemoryStatus::Registered),
                (
                    MemoryKeyId::ItemPickupCooldownTicks,
                    MemoryStatus::Registered,
                ),
            ],
            timeout_duration,
            timeout_duration,
        ))
    }

    /// `canThrowItemToTarget` (`:80-86`).
    fn can_throw_item_to_target(&self, mob: &dyn Mob, brain: &Brain) -> bool {
        if mob.carried_inventory_is_empty() {
            return false;
        }
        (self.target_position_getter)(mob, brain).is_some()
    }
}

impl TimedBehavior for GoAndGiveItemsToTarget {
    fn debug_name(&self) -> &'static str {
        "GoAndGiveItemsToTarget"
    }

    /// `checkExtraStartConditions` (`:47-50`).
    fn check_extra_start_conditions(&mut self, mob: &dyn Mob, brain: &Brain) -> bool {
        self.can_throw_item_to_target(mob, brain)
    }

    /// `canStillUse` (`:52-55`).
    fn can_still_use(&mut self, mob: &dyn Mob, brain: &Brain, _game_time: i64) -> bool {
        self.can_throw_item_to_target(mob, brain)
    }

    /// `start` (`:57-62`), which is `BehaviorUtils.setWalkAndLookTargetMemories(body, tracker,
    /// speedModifier, 3)` (`BehaviorUtils.java:75-85`).
    fn start(&mut self, mob: &dyn Mob, brain: &Brain, _game_time: i64) {
        let Some(target) = (self.target_position_getter)(mob, brain) else {
            return;
        };
        brain.set::<LookTargetMemory>(target.clone());
        brain.set::<WalkTargetMemory>(WalkTarget::new(
            target,
            self.speed_modifier,
            CLOSE_ENOUGH_DISTANCE_TO_TARGET as i32,
        ));
    }

    /// `tick` (`:64-78`): once inside three blocks of the deposit position, pull one item out
    /// of the carrier's inventory, throw it just above the target, run the thrower callback
    /// and start the post-throw pickup cooldown.
    fn tick(&mut self, mob: &dyn Mob, brain: &Brain, game_time: i64) {
        let Some(target) = (self.target_position_getter)(mob, brain) else {
            return;
        };
        let (Some(deposit_pos), Some(deposit_block_pos)) =
            (target.current_position(), target.current_block_position())
        else {
            return;
        };

        let entity = &mob.get_mob_entity().living_entity.entity;
        if (deposit_pos - entity.get_eye_pos()).length() >= CLOSE_ENOUGH_DISTANCE_TO_TARGET {
            return;
        }

        let item = mob.remove_one_carried_item();
        if item.is_empty() {
            return;
        }

        throw_item(
            mob,
            item.clone(),
            deposit_pos + Vector3::new(0.0, 1.0, 0.0),
            THROW_VELOCITY,
            HAND_Y_DISTANCE_FROM_EYE,
        );
        (self.item_thrower)(mob, &item, deposit_block_pos, game_time);
        brain.set::<ItemPickupCooldownTicksMemory>(ITEM_PICKUP_COOLDOWN_AFTER_THROWING);
    }
}

/// `BehaviorUtils.throwItem(thrower, item, targetPos, throwVelocity, handYDistanceFromEye)`
/// (`BehaviorUtils.java:94-105`).
///
/// DEVIATION: `itemEntity.setThrower(thrower)` is not ported -- Pumpkin's `ItemEntity` has no
/// thrower field, so the "you threw this" pickup attribution vanilla records is lost. The
/// actual world insert is deferred onto the runtime because `World::spawn_entity` is async
/// while the `Behavior` trait is deliberately synchronous (see `behavior/mod.rs`).
fn throw_item(
    thrower: &dyn Mob,
    item: ItemStack,
    target_pos: Vector3<f64>,
    throw_velocity: Vector3<f64>,
    hand_y_distance_from_eye: f64,
) {
    let entity = &thrower.get_mob_entity().living_entity.entity;
    let pos = entity.pos.load();
    let spawn_pos = Vector3::new(pos.x, entity.get_eye_y() - hand_y_distance_from_eye, pos.z);

    let direction = target_pos - pos;
    let length = direction.length();
    let velocity = if length == 0.0 {
        Vector3::new(0.0, 0.0, 0.0)
    } else {
        Vector3::new(
            direction.x / length * throw_velocity.x,
            direction.y / length * throw_velocity.y,
            direction.z / length * throw_velocity.z,
        )
    };

    let world = entity.world.load_full();
    let item_entity = Arc::new(ItemEntity::new_with_velocity(
        Entity::new(world.clone(), spawn_pos, &EntityType::ITEM),
        item,
        velocity,
        DEFAULT_PICKUP_DELAY,
    ));
    tokio::spawn(async move {
        world.spawn_entity(item_entity).await;
    });
}
