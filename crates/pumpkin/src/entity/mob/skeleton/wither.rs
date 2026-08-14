use std::sync::Arc;

use pumpkin_data::entity::EntityType;

use crate::entity::{
    Entity, NBTStorage,
    ai::goal::active_target::ActiveTargetGoal,
    mob::{Mob, MobEntity, skeleton::SkeletonEntityBase},
};

pub struct WitherSkeletonEntity {
    entity: Arc<SkeletonEntityBase>,
}

impl WitherSkeletonEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = SkeletonEntityBase::new(entity);
        {
            let mut target_selector = entity.mob_entity.target_selector.lock().unwrap();
            // `WitherSkeleton#registerGoals` adds one nearest-target goal for AbstractPiglin
            // before the inherited selector goals. Its concrete subclasses are Piglin and
            // PiglinBrute; ZombifiedPiglin extends Zombie and is not included.
            target_selector.add_goal_first(
                3,
                ActiveTargetGoal::with_default_types(
                    &entity.mob_entity,
                    &[&EntityType::PIGLIN, &EntityType::PIGLIN_BRUTE],
                    true,
                ),
            );
        }
        let skeleton = Self { entity };
        Arc::new(skeleton)
    }
}

impl NBTStorage for WitherSkeletonEntity {}

impl Mob for WitherSkeletonEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }
}
