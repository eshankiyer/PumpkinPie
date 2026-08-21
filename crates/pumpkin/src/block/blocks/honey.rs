use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::sound::Sound;
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::vector3::Vector3;
use rand::{RngExt, rng};

use crate::block::{BlockBehaviour, BlockFuture, OnEntityCollisionArgs, OnLandedUponArgs};
use crate::entity::{Entity, EntityBase};

/// `HoneyBlock.SLIDE_STARTS_WHEN_VERTICAL_SPEED_IS_AT_LEAST` (`HoneyBlock.java:27`).
const SLIDE_STARTS_WHEN_VERTICAL_SPEED_IS_AT_LEAST: f64 = 0.13;
/// `HoneyBlock.MIN_FALL_SPEED_TO_BE_CONSIDERED_SLIDING` (`HoneyBlock.java:28`).
const MIN_FALL_SPEED_TO_BE_CONSIDERED_SLIDING: f64 = 0.08;
/// `HoneyBlock.THROTTLE_SLIDE_SPEED_TO` (`HoneyBlock.java:29`).
const THROTTLE_SLIDE_SPEED_TO: f64 = 0.05;

#[pumpkin_block("minecraft:honey_block")]
pub struct HoneyBlock;

/// `HoneyBlock.getOldDeltaY` (`HoneyBlock.java:76-78`): the stored velocity is already
/// post-gravity/post-drag, so undo both to recover the pre-tick vertical speed.
fn get_old_delta_y(delta_y: f64) -> f64 {
    delta_y / f64::from(0.98f32) + MIN_FALL_SPEED_TO_BE_CONSIDERED_SLIDING
}

/// `HoneyBlock.getNewDeltaY` (`HoneyBlock.java:80-82`).
fn get_new_delta_y(delta_y: f64) -> f64 {
    (delta_y - MIN_FALL_SPEED_TO_BE_CONSIDERED_SLIDING) * f64::from(0.98f32)
}

/// `HoneyBlock.doesEntityDoHoneyBlockSlideEffects` (`HoneyBlock.java:42-44`).
fn does_entity_do_slide_effects(entity: &dyn EntityBase) -> bool {
    if entity.get_living_entity().is_some() {
        return true;
    }
    let entity_type = entity.get_entity().entity_type;
    entity_type == &EntityType::TNT
        || entity_type.has_tag(&tag::EntityType::C_MINECARTS)
        || entity_type.has_tag(&tag::EntityType::C_BOATS)
}

/// `HoneyBlock.isSlidingDown` (`HoneyBlock.java:84-101`).
fn is_sliding_down(pos: &pumpkin_util::math::position::BlockPos, entity: &Entity) -> bool {
    if entity.on_ground.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }

    let entity_pos = entity.pos.load();
    if entity_pos.y > f64::from(pos.0.y) + 0.9375 - 1.0e-7 {
        return false;
    }

    if get_old_delta_y(entity.velocity.load().y) >= -MIN_FALL_SPEED_TO_BE_CONSIDERED_SLIDING {
        return false;
    }

    let dx = (f64::from(pos.0.x) + 0.5 - entity_pos.x).abs();
    let dz = (f64::from(pos.0.z) + 0.5 - entity_pos.z).abs();
    let overlap_distance = 0.4375 + f64::from(entity.entity_dimension.load().width) / 2.0;
    dx + 1.0e-7 > overlap_distance || dz + 1.0e-7 > overlap_distance
}

/// `HoneyBlock.doSlideMovement` (`HoneyBlock.java:109-119`).
fn do_slide_movement(entity: &dyn EntityBase) {
    let base = entity.get_entity();
    let delta = base.velocity.load();
    let old_delta_y = get_old_delta_y(delta.y);

    let new_velocity = if old_delta_y < -SLIDE_STARTS_WHEN_VERTICAL_SPEED_IS_AT_LEAST {
        let horizontal_reduction_factor = -THROTTLE_SLIDE_SPEED_TO / old_delta_y;
        Vector3::new(
            delta.x * horizontal_reduction_factor,
            get_new_delta_y(-THROTTLE_SLIDE_SPEED_TO),
            delta.z * horizontal_reduction_factor,
        )
    } else {
        Vector3::new(delta.x, get_new_delta_y(-THROTTLE_SLIDE_SPEED_TO), delta.z)
    };

    base.set_velocity(new_velocity);

    // `Entity.resetFallDistance`
    if let Some(living) = entity.get_living_entity() {
        living.fall_distance.store(0.0);
    }
}

impl BlockBehaviour for HoneyBlock {
    /// `HoneyBlock.fallOn` (`HoneyBlock.java:51-61`): the slide sound always plays, and fall
    /// damage is scaled by 0.2 rather than the vanilla default of 1.0.
    fn on_landed_upon<'a>(&'a self, args: OnLandedUponArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = args.entity.get_entity();
            entity.play_sound(Sound::BlockHoneyBlockSlide);
            args.world
                .send_entity_status(entity, EntityStatus::HoneyJump, None);

            if let Some(living) = args.entity.get_living_entity() {
                living
                    .handle_fall_damage(args.entity, args.fall_distance, 0.2)
                    .await;
            }
        })
    }

    /// `HoneyBlock.entityInside` (`HoneyBlock.java:63-74`).
    fn on_entity_collision<'a>(&'a self, args: OnEntityCollisionArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = args.entity.get_entity();
            if !is_sliding_down(args.position, entity) {
                return;
            }

            // `maybeDoSlideAchievement` (`HoneyBlock.java:103-107`) is an advancement trigger and
            // has no analogue here.
            do_slide_movement(args.entity);

            // `maybeDoSlideEffects` (`HoneyBlock.java:121-132`).
            if does_entity_do_slide_effects(args.entity) {
                if rng().random_range(0..5) == 0 {
                    entity.play_sound(Sound::BlockHoneyBlockSlide);
                }
                if rng().random_range(0..5) == 0 {
                    args.world
                        .send_entity_status(entity, EntityStatus::HoneySlide, None);
                }
            }
        })
    }
}
