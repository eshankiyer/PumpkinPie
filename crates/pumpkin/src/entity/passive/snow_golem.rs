use std::sync::{
    Arc, Weak,
    atomic::{AtomicI8, Ordering},
};

use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase, NBTStorage,
    ai::goal::{
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        nearest_hostile_target::NearestHostileTargetGoal,
        ranged_snowball_attack::RangedSnowballAttackGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity, RangedAttackMob},
    projectile::snowball::SnowballEntity,
};

pub struct SnowGolemEntity {
    pub mob_entity: MobEntity,
    pumpkin_flags: AtomicI8,
}

impl SnowGolemEntity {
    /// Vanilla `SnowGolem.hasPumpkin` (`SnowGolem.java:164-166`).
    #[must_use]
    pub fn has_pumpkin(&self) -> bool {
        self.pumpkin_flags.load(Ordering::Relaxed) & 16 != 0
    }

    /// Vanilla `SnowGolem.setPumpkin` (`SnowGolem.java:168-175`).
    pub fn set_pumpkin(&self, pumpkin: bool) {
        let next = if pumpkin {
            self.pumpkin_flags.fetch_or(16, Ordering::Relaxed) | 16
        } else {
            self.pumpkin_flags.fetch_and(!16, Ordering::Relaxed) & !16
        };
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::snow_golem::PUMPKIN_ID,
                next,
            )],
            None,
        );
    }

    /// Vanilla `SnowGolem.getLeashOffset` (`SnowGolem.java:192-195`).
    #[must_use]
    pub fn get_leash_offset(&self) -> Vector3<f64> {
        let entity = &self.mob_entity.living_entity.entity;
        let dimensions = entity.entity_dimension.load();
        Vector3::new(
            0.0,
            entity.get_eye_height() * 0.75,
            f64::from(dimensions.width) * 0.4,
        )
    }

    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let snow_golem = Self {
            mob_entity,
            pumpkin_flags: AtomicI8::new(16),
        };
        let mob_arc = Arc::new(snow_golem);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

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

            // `SnowGolem.java`'s `registerGoals`: priorities 1 (attack), 2 (wander), 3 (look at
            // player), 4 (random look around).
            goal_selector.add_goal(1, Box::new(RangedSnowballAttackGoal::new(20, 10.0)));
            goal_selector.add_goal(
                2,
                Box::new(WanderAroundGoal::new_water_avoiding_with_probability(
                    1.0, 0.00001,
                )),
            );
            goal_selector.add_goal(
                3,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(4, Box::new(RandomLookAroundGoal::default()));

            // `SnowGolem.java`'s `targetSelector.addGoal(1, new NearestAttackableTargetGoal<>(
            // this, Mob.class, 10, true, false, (target, level) -> target instanceof Enemy))`
            // -- any hostile mob (including creepers), not just zombies.
            target_selector.add_goal(
                1,
                NearestHostileTargetGoal::new_for_snow_golem(&mob_arc.mob_entity),
            );
        };

        mob_arc
    }

    /// Vanilla `SnowGolem.performRangedAttack` (`SnowGolem.java:118-134`): aims at the
    /// target's eye height minus 1.1 with a horizontal-distance lead, shot at speed 1.6 with
    /// `rangedAttackUncertainty` 12.
    pub fn throw_snowball(&self, target: &Arc<dyn EntityBase>) {
        let entity = self.get_entity();
        let world = entity.world.load();

        let snowball_entity = Entity::new(world.clone(), entity.pos.load(), &EntityType::SNOWBALL);
        let snowball = SnowballEntity::new_shot(snowball_entity, entity);

        let mob_pos = entity.pos.load();
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();

        let dx = target_pos.x - mob_pos.x;
        let dy = target_pos.y + target_entity.get_eye_height() - 1.1;
        let dz = target_pos.z - mob_pos.z;
        let yo = dx.hypot(dz) * 0.2;
        let projectile_y = snowball.get_entity().pos.load().y;

        snowball
            .thrown
            .set_velocity(dx, dy + yo - projectile_y, dz, 1.6, 12.0);

        let snowball_arc: Arc<dyn EntityBase> = Arc::new(snowball);
        world.spawn_entity(snowball_arc);

        if !entity.silent.load(Ordering::Relaxed) {
            world.play_sound(
                Sound::EntitySnowGolemShoot,
                SoundCategory::Neutral,
                &mob_pos,
            );
        }
    }
}

impl NBTStorage for SnowGolemEntity {}

impl Mob for SnowGolemEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `SnowGolem.getAmbientSound` (`SnowGolem.java:178-180`), overriding
    /// `AbstractGolem`'s silent default.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntitySnowGolemAmbient)
    }

    /// `SnowGolem.isSensitiveToWater` (`SnowGolem.java:86-88`).
    fn mob_is_sensitive_to_water(&self) -> bool {
        true
    }
}

impl RangedAttackMob for SnowGolemEntity {
    fn perform_ranged_attack(&self, target: &Arc<dyn EntityBase>, _power: f32) {
        self.throw_snowball(target);
    }
}
