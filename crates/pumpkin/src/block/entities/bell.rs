use crate::block::entities::BlockEntity;
use crate::world::World;
use crossbeam::atomic::AtomicCell;
use pumpkin_data::block_properties::HorizontalFacing;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use std::any::Any;
use std::pin::Pin;
use std::sync::Arc;

// BellBlockEntity.HEAR_BELL_RADIUS (BellBlockEntity.java).
const HEAR_BELL_RADIUS: f64 = 32.0;
// BellBlockEntity.HIGHLIGHT_RAIDERS_RADIUS / GLOW_DURATION (BellBlockEntity.java).
const HIGHLIGHT_RAIDERS_RADIUS: f64 = 48.0;
const GLOW_DURATION: i32 = 60;

pub struct BellBlockEntity {
    pub position: BlockPos,
    pub last_side_hit: AtomicCell<Option<HorizontalFacing>>,
    pub ring_ticks: AtomicCell<i32>,
    pub ringing: AtomicCell<bool>,
    resonating: AtomicCell<bool>,
    resonate_time: AtomicCell<i32>,
}

impl BellBlockEntity {
    pub const ID: &'static str = "minecraft:bell";
    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            last_side_hit: AtomicCell::new(None),
            ring_ticks: AtomicCell::new(0),
            resonate_time: AtomicCell::new(0),
            resonating: AtomicCell::new(false),
            ringing: AtomicCell::new(false),
        }
    }
    pub fn activate(&self, direction: HorizontalFacing) {
        self.last_side_hit.store(Some(direction));
        if self.ringing.load() {
            self.ring_ticks.store(0);
        } else {
            self.ringing.store(true);
        }
    }
    /// `BellBlockEntity.triggerEvent` for event type 1, run when the ring's block event is
    /// processed: restarts the shake and lets the bell resonate again.
    pub fn trigger_event(&self, data: u8) {
        self.resonate_time.store(0);
        self.last_side_hit.store(
            pumpkin_data::BlockDirection::from_index(data)
                .and_then(|direction| direction.to_horizontal_facing()),
        );
        self.ring_ticks.store(0);
        self.ringing.store(true);
    }
    /// `BellBlockEntity.makeRaidersGlow`: every living raider within
    /// `HIGHLIGHT_RAIDERS_RADIUS` of the bell centre glows for `GLOW_DURATION` ticks.
    ///
    /// Vanilla filters the entity snapshot taken when the bell was rung; this queries the
    /// entities around the bell at resonance end instead.
    async fn make_raiders_glow(&self, world: &World) {
        let center = self.position.to_centered_f64();
        for entity in world
            .get_nearby_entities(center, HIGHLIGHT_RAIDERS_RADIUS)
            .values()
        {
            let base = entity.get_entity();
            if !base.is_alive()
                || base.is_removed()
                || !base
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
                // `closerToCenterThan` is a strict comparison.
                || base.pos.load().squared_distance_to_vec(&center)
                    >= HIGHLIGHT_RAIDERS_RADIUS * HIGHLIGHT_RAIDERS_RADIUS
            {
                continue;
            }
            if let Some(living) = entity.get_living_entity() {
                living
                    .add_effect(pumpkin_data::potion::Effect {
                        effect_type: &pumpkin_data::effect::StatusEffect::GLOWING,
                        duration: GLOW_DURATION,
                        amplifier: 0,
                        ambient: false,
                        show_particles: true,
                        show_icon: true,
                        blend: false,
                    })
                    .await;
            }
        }
    }
    /// `BellBlockEntity.areRaidersNearby`: whether a living raider is within
    /// `HEAR_BELL_RADIUS` of the bell.
    pub fn raiders_hear_bell(&self, world: &World) -> bool {
        world
            .get_nearby_entities(self.position.to_centered_f64(), HEAR_BELL_RADIUS)
            .values()
            .any(|entity| {
                entity.get_entity().is_alive()
                    && entity
                        .get_entity()
                        .entity_type
                        .has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
            })
    }
}

impl BlockEntity for BellBlockEntity {
    fn write_nbt<'a>(
        &'a self,
        _nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {})
    }

    fn from_nbt(_nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        Self::new(position)
    }

    fn tick<'a>(&'a self, world: &'a Arc<World>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.ringing.load() {
                self.ring_ticks.fetch_add(1);
            }
            if self.ring_ticks.load() >= 50 {
                self.ringing.store(false);
                self.ring_ticks.store(0);
            }
            if self.ring_ticks.load() >= 5
                && self.resonate_time.load() == 0
                && self.raiders_hear_bell(world)
            {
                self.resonating.store(true);
                world.play_sound_fine(
                    Sound::BlockBellResonate,
                    SoundCategory::Blocks,
                    &self.position.to_centered_f64(),
                    1.0,
                    1.0,
                );
            }

            if self.resonating.load() {
                if self.resonate_time.load() < 40 {
                    self.resonate_time.fetch_add(1);
                } else {
                    self.make_raiders_glow(world).await;
                    self.resonating.store(false);
                }
            }
        })
    }

    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
