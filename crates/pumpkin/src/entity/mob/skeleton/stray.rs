use std::sync::Arc;

use pumpkin_data::sound::Sound;

use crate::entity::{
    Entity, EntityBaseFuture, NBTStorage,
    mob::{Mob, MobEntity, skeleton::SkeletonEntityBase},
};

pub struct StraySkeletonEntity {
    entity: Arc<SkeletonEntityBase>,
}

impl StraySkeletonEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = SkeletonEntityBase::new(entity);
        let stray = Self { entity };
        Arc::new(stray)
    }
}

impl NBTStorage for StraySkeletonEntity {}

impl Mob for StraySkeletonEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    /// `Stray.getStepSound` returns the stray step event.
    /// Vanilla: `Stray.java:55-57` (emitted by `AbstractSkeleton.playStepSound`,
    /// `AbstractSkeleton.java:94-98`).
    fn get_step_sound(&self) -> Option<Sound> {
        Some(Sound::EntityStrayStep)
    }

    fn pre_ai_tick(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move { self.entity.reassess_weapon_goal(self).await })
    }
}
