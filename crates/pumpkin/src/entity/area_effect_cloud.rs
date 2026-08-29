use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::{
    entity::{Entity, EntityBase},
    server::Server,
};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

type EffectEntry = (&'static StatusEffect, i32, u8, bool, bool, bool);

struct ParticleMeta<'a> {
    particle_id: pumpkin_protocol::codec::var_int::VarInt,
    data: &'a [u8],
}

const TIME_BETWEEN_APPLICATIONS: i32 = 5;
const MINIMAL_RADIUS: f32 = 0.5;
const MAX_RADIUS: f32 = 32.0;
const CLOUD_HEIGHT: f64 = 0.5;
const INFINITE_DURATION: i32 = -1;

const fn application_scale(_distance: f64, _radius: f64) -> f32 {
    1.0
}

fn can_reapply(reapplication_map: &HashMap<i32, i32>, entity_id: i32) -> bool {
    !reapplication_map.contains_key(&entity_id)
}

impl pumpkin_protocol::java::client::play::MetadataSerializer for ParticleMeta<'_> {
    fn write_metadata(
        &self,
        writer: &mut impl std::io::Write,
        _version: &pumpkin_util::version::JavaMinecraftVersion,
    ) -> Result<(), pumpkin_protocol::ser::WritingError> {
        use pumpkin_protocol::ser::NetworkWriteExt;
        writer.write_var_int(&self.particle_id)?;
        writer.write_slice(self.data)
    }
}

pub struct AreaEffectCloudEntity {
    pub entity: Entity,
    pub item_stack: Mutex<ItemStack>,
    pub effects: Mutex<Vec<EffectEntry>>,
    pub radius: Mutex<f32>,
    pub duration: Mutex<i32>,
    pub age: Mutex<i32>,
    pub reapplication_delay: Mutex<i32>,
    pub reapplication_map: Mutex<HashMap<i32, i32>>,
    pub radius_on_tick: Mutex<f32>,
    pub radius_on_use: Mutex<f32>,
    pub duration_on_use: Mutex<i32>,
    pub wait_time: Mutex<i32>,
}

impl AreaEffectCloudEntity {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(entity: Entity) -> Arc<dyn EntityBase> {
        entity
            .no_physics
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Arc::new(Self {
            entity,
            item_stack: Mutex::new(ItemStack::new(0, &pumpkin_data::item::Item::GLASS_BOTTLE)),
            effects: Mutex::new(Vec::new()),
            radius: Mutex::new(3.0),
            duration: Mutex::new(INFINITE_DURATION),
            age: Mutex::new(0),
            reapplication_delay: Mutex::new(20),
            reapplication_map: Mutex::new(HashMap::new()),
            radius_on_tick: Mutex::new(0.0),
            radius_on_use: Mutex::new(0.0),
            duration_on_use: Mutex::new(0),
            wait_time: Mutex::new(20),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        entity: Entity,
        item_stack: ItemStack,
        effects_in: Vec<EffectEntry>,
        duration_in: i32,
        radius_in: f32,
        reapplication_delay_in: i32,
        wait_time_in: i32,
        radius_on_use_in: f32,
        duration_on_use_in: i32,
        radius_per_tick_in: f32,
    ) -> Arc<dyn EntityBase> {
        entity
            .no_physics
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Arc::new(Self {
            entity,
            item_stack: Mutex::new(item_stack),
            effects: Mutex::new(effects_in),
            radius: Mutex::new(radius_in),
            duration: Mutex::new(duration_in),
            age: Mutex::new(0),
            reapplication_delay: Mutex::new(reapplication_delay_in),
            reapplication_map: Mutex::new(HashMap::new()),
            radius_on_tick: Mutex::new(radius_per_tick_in),
            radius_on_use: Mutex::new(radius_on_use_in),
            duration_on_use: Mutex::new(duration_on_use_in),
            wait_time: Mutex::new(wait_time_in),
        })
    }

    fn set_radius(&self, radius: f32) -> f32 {
        let clamped = radius.clamp(0.0, MAX_RADIUS);
        *self
            .radius
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = clamped;
        self.entity.send_meta_data(
            &[pumpkin_protocol::java::client::play::Metadata::new(
                pumpkin_data::tracked_data::area_effect_cloud::RADIUS,
                clamped,
            )],
            None,
        );
        clamped
    }
}

impl EntityBase for AreaEffectCloudEntity {
    fn init_data_tracker(&self) {
        let radius = *self
            .radius
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stack = self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let effects = self
            .effects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        let mut color: i32 = (0xFFi32 << 24) | 0x385dc6;
        if let Some(pc) =
            stack.get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
        {
            if let Some(c) = pc.custom_color {
                color = c | (0xFFi32 << 24);
            } else if !effects.is_empty() {
                let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
                for (effect, _, _, _, _, _) in &effects {
                    r += f32::from(((effect.color >> 16) & 0xFF) as u8);
                    g += f32::from(((effect.color >> 8) & 0xFF) as u8);
                    b += f32::from((effect.color & 0xFF) as u8);
                }
                let count = effects.len() as f32;
                color = (0xFFi32 << 24)
                    | (((r / count) as i32) << 16)
                    | (((g / count) as i32) << 8)
                    | (b / count) as i32;
            }
        }

        let data_bytes = color.to_be_bytes();
        let meta = ParticleMeta {
            particle_id: pumpkin_protocol::codec::var_int::VarInt(
                pumpkin_data::particle::Particle::EntityEffect as i32,
            ),
            data: &data_bytes,
        };
        self.entity.send_meta_data(
            &[pumpkin_protocol::java::client::play::Metadata::new(
                pumpkin_data::tracked_data::area_effect_cloud::PARTICLE,
                &meta,
            )],
            None,
        );
        self.entity.send_meta_data(
            &[pumpkin_protocol::java::client::play::Metadata::new(
                pumpkin_data::tracked_data::area_effect_cloud::RADIUS,
                radius,
            )],
            None,
        );
        let wait_time = *self
            .wait_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.entity.send_meta_data(
            &[pumpkin_protocol::java::client::play::Metadata::new(
                pumpkin_data::tracked_data::area_effect_cloud::WAITING,
                0 < wait_time,
            )],
            None,
        );
    }

    #[allow(clippy::too_many_lines)]
    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {
        let age = {
            let mut age = self
                .age
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *age += 1;
            *age
        };
        let wait_time = *self
            .wait_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let duration = *self
            .duration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // AreaEffectCloud.java:191 -- duration starts after the wait period; -1 is infinite.
        if duration != INFINITE_DURATION && age - wait_time >= duration {
            self.entity.remove();
            return;
        }
        if age == wait_time && wait_time > 0 {
            self.entity.send_meta_data(
                &[pumpkin_protocol::java::client::play::Metadata::new(
                    pumpkin_data::tracked_data::area_effect_cloud::WAITING,
                    false,
                )],
                None,
            );
        }
        if age < wait_time || !age.is_multiple_of(TIME_BETWEEN_APPLICATIONS) {
            return;
        }

        // AreaEffectCloud.java:201-210 -- radius changes are optional and have a minimum.
        let mut radius = *self
            .radius
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let radius_per_tick = *self
            .radius_on_tick
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if radius_per_tick != 0.0 {
            radius += radius_per_tick;
            if radius < MINIMAL_RADIUS {
                self.entity.remove();
                return;
            }
            radius = self.set_radius(radius);
        }

        // AreaEffectCloud.java:213 -- purge absolute reapplication expiry ticks before scanning.
        self.reapplication_map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, expires_at| age < *expires_at);
        let effects = self
            .effects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if effects.is_empty() {
            self.reapplication_map
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
            return;
        }

        // AreaEffectCloud.java:357 -- the cloud's box is radius*2 wide, 0.5 high, at its feet.
        let pos = self.entity.pos.load();
        let r = f64::from(radius);
        let aabb = BoundingBox::new(
            Vector3::new(pos.x - r, pos.y, pos.z - r),
            Vector3::new(pos.x + r, pos.y + CLOUD_HEIGHT, pos.z + r),
        );
        let world = self.entity.world.load();
        let mut candidates = world.get_entities_at_box(&aabb);
        for player in world.get_players_at_box(&aabb) {
            candidates.push(player as Arc<dyn EntityBase>);
        }

        for candidate in candidates {
            let entity = candidate.get_entity();
            if entity.entity_id == self.entity.entity_id
                || *entity.entity_type == pumpkin_data::entity::EntityType::AREA_EFFECT_CLOUD
            {
                continue;
            }
            if !can_reapply(
                &self
                    .reapplication_map
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                entity.entity_id,
            ) {
                continue;
            }

            // AreaEffectCloud.java:222 -- skip victims that no effect can affect.
            let Some(living) = candidate.get_living_entity() else {
                continue;
            };
            if !living.is_affected_by_potions()
                || !effects
                    .iter()
                    .any(|(effect_type, ..)| living.can_be_affected(effect_type))
            {
                continue;
            }

            // AreaEffectCloud.java:223-226 -- containment is horizontal only.
            let target_pos = entity.pos.load();
            let dx = target_pos.x - pos.x;
            let dz = target_pos.z - pos.z;
            let distance = dx.hypot(dz);
            if distance > r {
                continue;
            }
            let scale = application_scale(distance, r);
            crate::item::potion::PotionContents::apply_effects_to(
                living,
                effects.clone(),
                scale,
                crate::item::potion::PotionApplicationSource::AreaEffectCloud,
            );

            let delay = *self
                .reapplication_delay
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.reapplication_map
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(entity.entity_id, age + delay);

            let radius_on_use = *self
                .radius_on_use
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if radius_on_use != 0.0 {
                radius += radius_on_use;
                if radius < MINIMAL_RADIUS {
                    self.entity.remove();
                    return;
                }
                radius = self.set_radius(radius);
            }

            let duration_on_use = *self
                .duration_on_use
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if duration_on_use != 0 {
                let mut duration = self
                    .duration
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *duration != INFINITE_DURATION {
                    *duration += duration_on_use;
                    if *duration <= 0 {
                        drop(duration);
                        self.entity.remove();
                        return;
                    }
                }
            }
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    /// Vanilla `AreaEffectCloud.getPistonPushReaction` (`AreaEffectCloud.java:351-353`) returns
    /// `IGNORE`; the piston path consults this hook before moving an entity.
    fn can_be_pushed_by_piston(&self) -> bool {
        false
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{application_scale, can_reapply};
    use std::collections::HashMap;

    #[test]
    fn cloud_effect_scale_is_flat() {
        assert_eq!(application_scale(0.0, 3.0), 1.0);
        assert_eq!(application_scale(2.99, 3.0), 1.0);
    }

    #[test]
    fn reapplication_is_controlled_by_cooldown_map() {
        let mut cooldowns = HashMap::new();
        assert!(can_reapply(&cooldowns, 42));
        cooldowns.insert(42, 20);
        assert!(!can_reapply(&cooldowns, 42));
        cooldowns.remove(&42);
        assert!(can_reapply(&cooldowns, 42));
    }
}
