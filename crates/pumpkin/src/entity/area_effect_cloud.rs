use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    entity::{Entity, EntityBase, EntityBaseFuture, NBTStorage},
    server::Server,
};
use pumpkin_data::effect::StatusEffect;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

type EffectEntry = (&'static StatusEffect, i32, u8, bool, bool, bool);
use pumpkin_data::item_stack::ItemStack;
use tokio::sync::Mutex;

struct ParticleMeta<'a> {
    particle_id: pumpkin_protocol::codec::var_int::VarInt,
    data: &'a [u8],
}

// AreaEffectCloud.java:101-115 stores either a custom particle option or the default colored
// entity-effect option in the tracked particle field.
pub type CustomParticle = (pumpkin_protocol::codec::var_int::VarInt, Vec<u8>);

/// `AreaEffectCloud.TIME_BETWEEN_APPLICATIONS`
const TIME_BETWEEN_APPLICATIONS: i32 = 5;
/// `AreaEffectCloud.MINIMAL_RADIUS`
const MINIMAL_RADIUS: f32 = 0.5;
/// `AreaEffectCloud.MAX_RADIUS`
const MAX_RADIUS: f32 = 32.0;
/// `AreaEffectCloud.HEIGHT`
const CLOUD_HEIGHT: f64 = 0.5;
/// `AreaEffectCloud.INFINITE_DURATION`
const INFINITE_DURATION: i32 = -1;

const fn application_scale(_distance: f64, _radius: f64) -> f32 {
    1.0
}

fn can_reapply(reapplication_map: &HashMap<i32, i32>, entity_id: i32) -> bool {
    !reapplication_map.contains_key(&entity_id)
}

// AreaEffectCloud.java:375-378 applies the source stack's potion-duration-scale component.
fn potion_duration_scale_from_stack(item_stack: &ItemStack) -> f32 {
    item_stack
        .get_data_component::<pumpkin_data::data_component_impl::PotionDurationScaleImpl>()
        .map_or(1.0, |component| component.scale)
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

/// The effect cloud entity that is spawned where a lingering potion lands.
pub struct AreaEffectCloudEntity {
    pub entity: Entity,
    /// Stored potion item stack (may be empty) to read effects from.
    pub item_stack: Mutex<ItemStack>,
    /// Active potion effects as tuples: (`StatusEffect`, `duration_ticks`, `amplifier`, `ambient`, `show_particles`, `show_icon`)
    pub effects: Mutex<Vec<EffectEntry>>,
    pub radius: Mutex<f32>,
    pub duration: Mutex<i32>,
    pub age: Mutex<i32>,
    /// ticks between reapplications to the same entity
    pub reapplication_delay: Mutex<i32>,
    /// map of `entity_id` -> ticks remaining until that entity can be affected again
    pub reapplication_map: Mutex<HashMap<i32, i32>>,
    /// linear radius change per tick
    pub radius_on_tick: Mutex<f32>,
    /// radius change when an entity is affected
    pub radius_on_use: Mutex<f32>,
    /// duration change (ticks) when an entity is affected
    pub duration_on_use: Mutex<i32>,
    /// ticks to wait before the cloud becomes active and applies effects (grace period)
    pub wait_time: Mutex<i32>,
    /// AreaEffectCloud.java:38,106-108 — PotionContents duration multiplier applied while
    /// copying effects to victims.
    pub potion_duration_scale: Mutex<f32>,
    /// AreaEffectCloud.java:32,101-115 — optional particle option replacing the default colored
    /// entity-effect particle.
    pub custom_particle: Mutex<Option<CustomParticle>>,
}

impl AreaEffectCloudEntity {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(entity: Entity) -> Arc<dyn EntityBase> {
        let cloud = Self {
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
            potion_duration_scale: Mutex::new(1.0),
            custom_particle: Mutex::new(None),
        };

        Arc::new(cloud)
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
        // AreaEffectCloud.java:374-378 applies potion contents and potion-duration-scale
        // components from the source item stack.
        let potion_duration_scale = potion_duration_scale_from_stack(&item_stack);
        Self::create_with_options(
            entity,
            item_stack,
            effects_in,
            duration_in,
            radius_in,
            reapplication_delay_in,
            wait_time_in,
            radius_on_use_in,
            duration_on_use_in,
            radius_per_tick_in,
            None,
            potion_duration_scale,
        )
    }

    #[allow(clippy::too_many_arguments)]
    /// AreaEffectCloud.java:101-115 and DragonFireball.java:40-45 apply a custom particle and
    /// keep the potion duration scale as entity state when a cloud is configured.
    pub fn create_with_options(
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
        custom_particle: Option<CustomParticle>,
        potion_duration_scale: f32,
    ) -> Arc<dyn EntityBase> {
        let cloud = Self {
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
            potion_duration_scale: Mutex::new(potion_duration_scale),
            custom_particle: Mutex::new(custom_particle),
        };

        Arc::new(cloud)
    }

    /// Stores the radius clamped to `[0, MAX_RADIUS]` and syncs it to clients.
    /// Returns the clamped value.
    async fn set_radius(&self, radius: f32) -> f32 {
        let clamped = radius.clamp(0.0, MAX_RADIUS);
        *self.radius.lock().await = clamped;
        self.entity.send_meta_data(
            &[pumpkin_protocol::java::client::play::Metadata::new(
                pumpkin_data::tracked_data::area_effect_cloud::RADIUS,
                clamped,
            )],
            None,
        );
        clamped
    }

    /// AreaEffectCloud.java:96-108 stores potion contents and refreshes the derived particle.
    pub async fn set_potion_contents(&self, item_stack: ItemStack) {
        let effects = crate::item::potion::PotionContents::read_potion_effects(&item_stack);
        let duration_scale = potion_duration_scale_from_stack(&item_stack);
        *self.item_stack.lock().await = item_stack;
        *self.effects.lock().await = effects;
        *self.potion_duration_scale.lock().await = duration_scale;
        self.send_particle_metadata().await;
    }

    /// AreaEffectCloud.java:101-115 replaces the tracked particle option immediately.
    pub async fn set_custom_particle(&self, particle: CustomParticle) {
        *self.custom_particle.lock().await = Some(particle);
        self.send_particle_metadata().await;
    }

    /// AreaEffectCloud.java:106-108 stores the multiplier used by `PotionContents.forEachEffect`.
    pub async fn set_potion_duration_scale(&self, scale: f32) {
        *self.potion_duration_scale.lock().await = scale;
    }

    /// AreaEffectCloud.java:96-115 updates the tracked particle whenever potion or custom
    /// particle contents change.
    async fn send_particle_metadata(&self) {
        let custom_particle = self.custom_particle.lock().await.clone();
        let stack = self.item_stack.lock().await.clone();
        let effects = self.effects.lock().await.clone();
        let mut color = crate::item::potion::PotionContents::get_color_or(&effects, -13_083_194);
        if let Some(pc) =
            stack.get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
            && let Some(custom_color) = pc.custom_color
        {
            color = custom_color | (0xFFi32 << 24);
        }
        let color_bytes = color.to_be_bytes();
        let (particle_id, particle_data) = custom_particle.as_ref().map_or_else(
            || {
                (
                    pumpkin_protocol::codec::var_int::VarInt(
                        pumpkin_data::particle::Particle::EntityEffect as i32,
                    ),
                    color_bytes.as_slice(),
                )
            },
            |(particle_id, particle_data)| (*particle_id, particle_data.as_slice()),
        );
        let meta = ParticleMeta {
            particle_id,
            data: particle_data,
        };
        self.entity.send_meta_data(
            &[pumpkin_protocol::java::client::play::Metadata::new(
                pumpkin_data::tracked_data::area_effect_cloud::PARTICLE,
                &meta,
            )],
            None,
        );
    }
}

impl NBTStorage for AreaEffectCloudEntity {}

impl EntityBase for AreaEffectCloudEntity {
    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            // Send initial radius and particle (color) so clients render correctly
            let radius = *self.radius.lock().await;

            // AreaEffectCloud.java:110-116 uses the opaque PotionContents color for its default
            // particle; custom particles take precedence as in AreaEffectCloud.java:101-103.
            self.send_particle_metadata().await;

            self.entity.send_meta_data(
                &[pumpkin_protocol::java::client::play::Metadata::new(
                    pumpkin_data::tracked_data::area_effect_cloud::RADIUS,
                    radius,
                )],
                None,
            );

            // Initial waiting flag
            let wait_time = *self.wait_time.lock().await;
            let is_waiting = 0 < wait_time;
            self.entity.send_meta_data(
                &[pumpkin_protocol::java::client::play::Metadata::new(
                    pumpkin_data::tracked_data::area_effect_cloud::WAITING,
                    is_waiting,
                )],
                None,
            );
        })
    }

    #[allow(clippy::too_many_lines)]
    #[allow(clippy::semicolon_outside_block)]
    fn tick<'a>(
        &'a self,
        _caller: &'a Arc<dyn EntityBase>,
        _server: &'a Server,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let age = {
                let mut age = self.age.lock().await;
                *age += 1;
                *age
            };
            let wait_time = *self.wait_time.lock().await;

            // AreaEffectCloud.java:191 -- the lifetime is measured from the end of the wait
            // period, and a duration of -1 means the cloud never expires on its own.
            let duration = *self.duration.lock().await;
            if duration != INFINITE_DURATION && age - wait_time >= duration {
                self.entity.remove().await;
                return;
            }

            // When the waiting period ends, notify clients so they render full particles
            if age == wait_time && wait_time > 0 {
                self.entity.send_meta_data(
                    &[pumpkin_protocol::java::client::play::Metadata::new(
                        pumpkin_data::tracked_data::area_effect_cloud::WAITING,
                        false,
                    )],
                    None,
                );
            }

            if age < wait_time {
                // Respect waiting/grace period
                return;
            }

            // AreaEffectCloud.java:201-210 -- the radius is only touched when a per-tick delta
            // is configured, and the cloud dies once it shrinks below the minimal radius.
            let mut radius = *self.radius.lock().await;
            let radius_per_tick = *self.radius_on_tick.lock().await;
            if radius_per_tick != 0.0 {
                radius += radius_per_tick;
                if radius < MINIMAL_RADIUS {
                    self.entity.remove().await;
                    return;
                }
                radius = self.set_radius(radius).await;
            }

            // AreaEffectCloud.java:212 -- effects are only applied every 5 ticks.
            if age % TIME_BETWEEN_APPLICATIONS != 0 {
                return;
            }

            // AreaEffectCloud.java:213 -- victims hold an absolute expiry tick, purged before
            // the scan.
            self.reapplication_map
                .lock()
                .await
                .retain(|_, expires_at| age < *expires_at);

            let effects = self.effects.lock().await.clone();
            // AreaEffectCloud.java:217-233 scales each copied effect with the cloud's potion
            // duration scale; the distance factor is intentionally flat for clouds.
            let potion_duration_scale =
                *self.potion_duration_scale.lock().await * application_scale(0.0, radius.into());
            if effects.is_empty() {
                self.reapplication_map.lock().await.clear();
                return;
            }

            // AreaEffectCloud.java:357 -- the cloud's bounding box is `radius * 2` wide and
            // 0.5 tall, anchored at its feet, not a cube extending `radius` on every axis.
            let pos = self.entity.pos.load();
            let r = f64::from(radius);
            let min = Vector3::new(pos.x - r, pos.y, pos.z - r);
            let max = Vector3::new(pos.x + r, pos.y + CLOUD_HEIGHT, pos.z + r);
            let aabb = BoundingBox::new(min, max);
            let world = self.entity.world.load();

            let mut candidates = world.get_entities_at_box(&aabb);
            let players = world.get_players_at_box(&aabb);
            for p in players {
                candidates.push(p.clone() as Arc<dyn EntityBase>);
            }

            for cand in candidates {
                let cand_clone = cand.clone();

                // Skip self and other `AreaEffectCloud` entities
                if cand_clone.get_entity().entity_id == self.get_entity().entity_id {
                    continue;
                }
                if *cand_clone.get_entity().entity_type
                    == pumpkin_data::entity::EntityType::AREA_EFFECT_CLOUD
                {
                    continue;
                }

                // Determine candidate id early
                let ent_id = cand_clone.get_entity().entity_id;

                {
                    let reapplication_map = self.reapplication_map.lock().await;
                    if !can_reapply(&reapplication_map, ent_id) {
                        continue;
                    }
                }

                // AreaEffectCloud.java:222 -- skip victims that none of the effects can affect.
                let affectable = cand_clone.get_living_entity().is_some_and(|living| {
                    living.is_affected_by_potions()
                        && effects
                            .iter()
                            .any(|(effect_type, ..)| living.can_be_affected(effect_type))
                });
                if !affectable {
                    continue;
                }

                // AreaEffectCloud.java:223-226 -- the containment test is horizontal only.
                let radius_f = f64::from(radius);
                let pos_e = cand_clone.get_entity().pos.load();
                let dx = pos_e.x - pos.x;
                let dz = pos_e.z - pos.z;
                let dist_sq = dx * dx + dz * dz;
                if dist_sq > radius_f * radius_f {
                    continue;
                }
                // Apply effects inside a spawned task
                let cand_for_spawn = cand_clone.clone();
                let effs_for_spawn = effects.clone();
                tokio::spawn(async move {
                    if let Some(living) = cand_for_spawn.get_living_entity() {
                        crate::item::potion::PotionContents::apply_effects_to(
                            living,
                            effs_for_spawn,
                            potion_duration_scale,
                            crate::item::potion::PotionApplicationSource::AreaEffectCloud,
                        )
                        .await;
                    }
                });

                // Set reapplication delay for this entity
                let delay = *self.reapplication_delay.lock().await;
                self.reapplication_map
                    .lock()
                    .await
                    .insert(ent_id, age + delay);

                // Apply radius-on-use (shrink)
                let radius_on_use = *self.radius_on_use.lock().await;
                if radius_on_use != 0.0 {
                    radius += radius_on_use;
                    if radius < MINIMAL_RADIUS {
                        self.entity.remove().await;
                        return;
                    }
                    radius = self.set_radius(radius).await;
                }

                // Apply duration-on-use (shorten lifespan)
                let duration_on_use = *self.duration_on_use.lock().await;
                if duration_on_use != 0 {
                    let mut duration_lock = self.duration.lock().await;
                    if *duration_lock != INFINITE_DURATION {
                        *duration_lock += duration_on_use;
                        if *duration_lock <= 0 {
                            drop(duration_lock);
                            self.entity.remove().await;
                            return;
                        }
                    }
                }
            }
        })
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    /// Vanilla `AreaEffectCloud.getPistonPushReaction` (`AreaEffectCloud.java:351-353`)
    /// returns `IGNORE`; the piston entity path consults this hook before moving an entity.
    fn can_be_pushed_by_piston(&self) -> bool {
        false
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{application_scale, can_reapply, potion_duration_scale_from_stack};
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
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

    #[test]
    fn potion_duration_scale_reads_the_item_component() {
        // AreaEffectCloud.java:375-378 applies the stack component instead of assuming 1.0.
        assert_eq!(
            potion_duration_scale_from_stack(&ItemStack::new(1, &Item::TIPPED_ARROW)),
            0.125
        );
        assert_eq!(
            potion_duration_scale_from_stack(&ItemStack::new(1, &Item::DRAGON_BREATH)),
            1.0
        );
    }
}
