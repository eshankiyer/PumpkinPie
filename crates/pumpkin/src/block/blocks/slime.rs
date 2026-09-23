use pumpkin_macros::pumpkin_block;

use crate::block::{
    BlockBehaviour, OnEntityStepArgs, OnLandedUponArgs, UpdateEntityMovementAfterFallOnArgs,
    bounce_entity_after_fall,
};

#[pumpkin_block("minecraft:slime_block")]
pub struct SlimeBlock;

impl BlockBehaviour for SlimeBlock {
    fn on_landed_upon(&self, args: OnLandedUponArgs<'_>) {
        // `SlimeBlock.fallOn` (`SlimeBlock.java:23-27`) suppresses the zero-damage
        // fall callback while the entity is stepping carefully.
        if !args.entity.get_entity().is_sneaking()
            && let Some(living) = args.entity.get_living_entity()
        {
            living.handle_fall_damage(args.entity, args.fall_distance, 0.0);
        }
    }

    /// `SlimeBlock.stepOn` (`SlimeBlock.java:30-37`): low vertical movement is damped
    /// horizontally unless the entity is stepping carefully.
    fn on_entity_step(&self, args: OnEntityStepArgs<'_>) {
        let entity = args.entity.get_entity();
        let velocity = entity.velocity.load();
        let abs_delta_y = velocity.y.abs();
        if abs_delta_y < 0.1 && !entity.is_sneaking() {
            let scale = 0.4 + abs_delta_y * 0.2;
            entity.set_velocity(velocity.multiply(scale, 1.0, scale));
        }
    }

    fn update_entity_movement_after_fall_on(&self, args: UpdateEntityMovementAfterFallOnArgs<'_>) {
        bounce_entity_after_fall(args.entity, 1.0)
    }
}
