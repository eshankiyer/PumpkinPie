use std::sync::Arc;

use pumpkin_data::{attributes::Attributes, effect::StatusEffect, potion::Effect};
use pumpkin_util::Difficulty;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage,
    mob::{Mob, MobEntity, spider::SpiderEntity},
};

pub struct CaveSpiderEntity {
    pub spider: Arc<SpiderEntity>,
}

const fn poison_duration(difficulty: Difficulty) -> Option<i32> {
    match difficulty {
        Difficulty::Normal => Some(7 * 20),
        Difficulty::Hard => Some(15 * 20),
        Difficulty::Peaceful | Difficulty::Easy => None,
    }
}

impl CaveSpiderEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let spider = SpiderEntity::new(entity);
        Arc::new(Self { spider })
    }
}

impl NBTStorage for CaveSpiderEntity {}

impl Mob for CaveSpiderEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        self.spider.get_mob_entity()
    }

    /// Vanilla `CaveSpider.getVehicleAttachmentPoint` (`CaveSpider.java:59-60`) lowers a
    /// Cave Spider passenger's seat offset for vehicles no wider than the spider.
    fn get_vehicle_attachment_point(&self, vehicle: &Entity) -> Option<Vector3<f64>> {
        cave_spider_attachment_point(
            vehicle.width(),
            self.get_entity().width(),
            self.get_mob_entity()
                .living_entity
                .get_attribute_value(&Attributes::SCALE),
        )
    }

    fn on_successful_attack<'a>(&'a self, target: &'a dyn EntityBase) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let duration =
                poison_duration(self.get_entity().world.load().level_info.load().difficulty);

            if let (Some(duration), Some(living)) = (duration, target.get_living_entity()) {
                living
                    .add_effect(Effect {
                        effect_type: &StatusEffect::POISON,
                        duration,
                        amplifier: 0,
                        ambient: false,
                        show_particles: true,
                        show_icon: true,
                        blend: false,
                    })
                    .await;
            }
        })
    }
}

/// Mirrors `CaveSpider.getVehicleAttachmentPoint` (`CaveSpider.java:59-60`).
fn cave_spider_attachment_point(
    vehicle_width: f32,
    spider_width: f32,
    scale: f64,
) -> Option<Vector3<f64>> {
    (vehicle_width <= spider_width).then(|| Vector3::new(0.0, 0.21875 * scale, 0.0))
}

#[cfg(test)]
mod tests {
    use super::{cave_spider_attachment_point, poison_duration};
    use pumpkin_util::Difficulty;
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn cave_spider_poison_matches_vanilla_difficulty_durations() {
        assert_eq!(poison_duration(Difficulty::Peaceful), None);
        assert_eq!(poison_duration(Difficulty::Easy), None);
        assert_eq!(poison_duration(Difficulty::Normal), Some(140));
        assert_eq!(poison_duration(Difficulty::Hard), Some(300));
    }

    #[test]
    fn cave_spider_attachment_point_uses_vanilla_width_gate_and_offset() {
        assert_eq!(
            cave_spider_attachment_point(0.7, 0.7, 1.0),
            Some(Vector3::new(0.0, 0.21875, 0.0))
        );
        assert_eq!(cave_spider_attachment_point(0.8, 0.7, 1.0), None);
        assert_eq!(
            cave_spider_attachment_point(0.7, 0.7, 2.0),
            Some(Vector3::new(0.0, 0.4375, 0.0))
        );
    }
}
