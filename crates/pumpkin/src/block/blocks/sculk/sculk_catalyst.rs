//! `SculkCatalystBlock` (`world/level/block/SculkCatalystBlock.java`) plus
//! `SculkCatalystBlockEntity.CatalystListener`
//! (`world/level/block/entity/SculkCatalystBlockEntity.java:57-123`).

use std::sync::Arc;

use crate::block::entities::sculk_catalyst::SculkCatalystBlockEntity;
use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, BrokenArgs, OnPlaceArgs, OnScheduledTickArgs,
    PlacedArgs,
};
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::world::World;
use crate::world::game_event::{
    GameEventContext, GameEventFuture, GameEventListener, PositionSource,
};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{
    BlockId, BlockStateId,
    block_properties::{BlockProperties, SculkCatalystLikeProperties},
    particle::Particle,
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

/// `CatalystListener.PULSE_TICKS` (line 58) — the bloom lasts 8 ticks.
const PULSE_TICKS: u8 = 8;
/// `CatalystListener.getListenerRadius()` (lines 74-77).
const LISTENER_RADIUS: i32 = 8;

pub struct SculkCatalystBlock;

impl BlockMetadata for SculkCatalystBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::SCULK_CATALYST].into()
    }
}

impl BlockBehaviour for SculkCatalystBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = SculkCatalystLikeProperties::default(args.block);
            props.bloom = false;
            props.to_state_id(args.block)
        })
    }

    /// The block entity itself is created by the generic `on_placed` path in
    /// `block/registry.rs` (`create_block_entity`); what vanilla gets from constructing
    /// `CatalystListener` alongside it has to be done explicitly here.
    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world
                .register_game_event_listener(Arc::new(CatalystListener {
                    pos: *args.position,
                }))
                .await;
        })
    }

    /// `SculkCatalystBlock.tick` (lines 43-48): clear `PULSE` once the bloom expires.
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let mut props = SculkCatalystLikeProperties::from_state_id(state.id, args.block);
            if props.bloom {
                props.bloom = false;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
            }
        })
    }

    /// `SculkCatalystBlock.spawnAfterBreak` (`SculkCatalystBlock.java:61-66`) awards 5
    /// experience when the break is allowed to drop experience and the tool lacks Silk Touch.
    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.player.gamemode.load() == pumpkin_util::GameMode::Creative
                || !args.world.level_info.load().game_rules.block_drops
            {
                return;
            }

            let tool = args.player.inventory().held_item().await;
            if tool.get_enchantment_level(&pumpkin_data::Enchantment::SILK_TOUCH) > 0 {
                return;
            }

            ExperienceOrbEntity::spawn(args.world, args.position.to_centered_f64(), 5).await;
        })
    }
}

/// `SculkCatalystBlockEntity.CatalystListener` (lines 57-123).
///
/// Holds only the position, matching the `SculkSensorListener` precedent in this
/// codebase: the spreader lives on the block entity and is reached through
/// `get_block_entity`, so the listener never co-owns it.
pub struct CatalystListener {
    pub pos: BlockPos,
}

impl GameEventListener for CatalystListener {
    fn listener_source(&self) -> PositionSource {
        PositionSource::Block(self.pos)
    }

    fn listener_radius(&self) -> i32 {
        LISTENER_RADIUS
    }

    /// `handleGameEvent` (lines 84-103).
    ///
    /// The catalyst consumes the experience rather than sharing it: vanilla calls
    /// `mob.skipDropExperience()` (`SculkCatalystBlockEntity.java:80`), which sets the
    /// flag that `shouldDropExperience()` gates the orb on in the death path
    /// (`LivingEntity.java:278,1527,1680`). Without that the death would pay out twice,
    /// once as an orb and once as sculk charge.
    fn handle_game_event<'a>(
        &'a self,
        world: &'a Arc<World>,
        event: &'a GameEvent,
        context: &'a GameEventContext,
        source_position: Vector3<f64>,
    ) -> GameEventFuture<'a> {
        Box::pin(async move {
            if !matches!(event, GameEvent::EntityDie) {
                return false;
            }
            let Some(source_entity) = context.source_entity.as_ref() else {
                return false;
            };
            let Some(living) = source_entity.get_living_entity() else {
                return false;
            };

            let experience_would_drop = source_entity.get_experience_reward(None);
            if experience_would_drop > 0 {
                // Claim the drop before the death path reaches its orb spawn.
                living
                    .skip_drop_experience
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                // `BlockPos.containing(sourcePosition.relative(Direction.UP, 0.5))`.
                let cursor_pos = BlockPos::floored(
                    source_position.x,
                    source_position.y + 0.5,
                    source_position.z,
                );
                if let Some(block_entity) = world.get_block_entity(&self.pos)
                    && let Some(catalyst) = block_entity
                        .as_any()
                        .downcast_ref::<SculkCatalystBlockEntity>()
                {
                    #[allow(clippy::cast_possible_wrap)]
                    catalyst.spreader.lock().await.add_cursors(
                        cursor_pos,
                        experience_would_drop.min(i32::MAX as u32) as i32,
                    );
                }
            }

            bloom(world, self.pos).await;
            true
        })
    }
}

/// `CatalystListener.bloom` (lines 110-115).
async fn bloom(world: &Arc<World>, pos: BlockPos) {
    let block = world.get_block(&pos);
    if block.id != BlockId::SCULK_CATALYST {
        return;
    }
    let state = world.get_block_state(&pos);
    let mut props = SculkCatalystLikeProperties::from_state_id(state.id, block);
    props.bloom = true;
    world
        .set_block_state(&pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
        .await;
    world.schedule_block_tick(block, pos, PULSE_TICKS, TickPriority::Normal);

    world.spawn_particle(
        Vector3::new(
            f64::from(pos.0.x) + 0.5,
            f64::from(pos.0.y) + 1.15,
            f64::from(pos.0.z) + 0.5,
        ),
        Vector3::new(0.2, 0.0, 0.2),
        0.0,
        2,
        Particle::SculkSoul,
    );

    // `playSound(..., 2.0F, 0.6F + random.nextFloat() * 0.4F)`.
    let pitch = 0.6 + rand::random::<f32>() * 0.4;
    world.play_sound_raw(
        Sound::BlockSculkCatalystBloom as u16,
        SoundCategory::Blocks,
        &Vector3::new(
            f64::from(pos.0.x) + 0.5,
            f64::from(pos.0.y) + 0.5,
            f64::from(pos.0.z) + 0.5,
        ),
        2.0,
        pitch,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_radius_matches_vanilla() {
        let listener = CatalystListener {
            pos: BlockPos::new(0, 0, 0),
        };
        assert_eq!(listener.listener_radius(), 8);
        assert!(matches!(
            listener.listener_source(),
            PositionSource::Block(pos) if pos == BlockPos::new(0, 0, 0)
        ));
    }

    #[test]
    fn cursor_position_is_half_a_block_above_the_death_position() {
        // `sourcePosition.relative(Direction.UP, 0.5)` then `BlockPos.containing`.
        let source = Vector3::new(3.2, 64.7, -8.1);
        let cursor_pos = BlockPos::floored(source.x, source.y + 0.5, source.z);
        assert_eq!(cursor_pos, BlockPos::new(3, 65, -9));
    }

    #[test]
    fn pulse_ticks_matches_vanilla() {
        assert_eq!(PULSE_TICKS, 8);
    }
}
