//! `SculkShriekerBlock` (`world/level/block/SculkShriekerBlock.java`).
//!
//! The block half of the shrieker: state, the `stepOn` trigger, the scheduled tick that ends
//! a shriek and the vibration listener. Everything that reads or writes the warning level
//! lives on `SculkShriekerBlockEntity`.

use std::sync::Arc;

use crate::block::entities::sculk_shrieker::SculkShriekerBlockEntity;
use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, BrokenArgs, OnEntityStepArgs, OnPlaceArgs,
    OnScheduledTickArgs, OnStateReplacedArgs, PlacedArgs,
};
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::player::Player;
use crate::world::World;
use crate::world::game_event::{
    GameEventContext, GameEventFuture, GameEventListener, PositionSource,
};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::{
    BlockId, BlockStateId,
    block_properties::{BlockProperties, SculkShriekerLikeProperties},
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;

/// `SculkShriekerBlockEntity.VibrationUser.LISTENER_RADIUS` (line 176).
const LISTENER_RADIUS: i32 = 8;

pub struct SculkShriekerBlock;

impl BlockMetadata for SculkShriekerBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::SCULK_SHRIEKER].into()
    }
}

/// `GameEventTags.SHRIEKER_CAN_LISTEN`
/// (`data/minecraft/tags/game_event/shrieker_can_listen.json`): a single event, the sculk
/// sensor's tendril click. Matched directly here for the same reason `warden.rs` matches
/// `WARDEN_CAN_LISTEN` directly - `GameEvent` carries no `Taggable` impl.
const fn shrieker_can_listen(event: &GameEvent) -> bool {
    matches!(event, GameEvent::SculkSensorTendrilsClicking)
}

/// `SculkShriekerBlockEntity.VibrationUser` (lines 175-224).
pub struct ShriekerListener {
    pub pos: BlockPos,
}

impl GameEventListener for ShriekerListener {
    fn listener_source(&self) -> PositionSource {
        PositionSource::Block(self.pos)
    }

    fn listener_radius(&self) -> i32 {
        LISTENER_RADIUS
    }

    fn handle_game_event<'a>(
        &'a self,
        world: &'a Arc<World>,
        event: &'a GameEvent,
        context: &'a GameEventContext,
        _source_position: Vector3<f64>,
    ) -> GameEventFuture<'a> {
        Box::pin(async move {
            if !shrieker_can_listen(event) {
                return false;
            }
            let (block, state) = world.get_block_and_state(&self.pos);
            if block.id != BlockId::SCULK_SHRIEKER {
                return false;
            }
            // `canReceiveVibration` (lines 198-201).
            if SculkShriekerLikeProperties::from_state_id(state.id, block).shrieking {
                return false;
            }
            let Some(entity) = context.source_entity.as_ref() else {
                return false;
            };
            let Some(player) = try_get_player(world, entity.as_ref()).await else {
                return false;
            };

            let Some(block_entity) = world.get_block_entity(&self.pos) else {
                return false;
            };
            let Some(shrieker) = block_entity
                .as_any()
                .downcast_ref::<SculkShriekerBlockEntity>()
            else {
                return false;
            };
            shrieker.try_shriek(world, &player).await;
            true
        })
    }
}

/// `SculkShriekerBlockEntity.tryGetPlayer` (lines 88-98), reduced to the two cases reachable
/// from this codebase's game-event context: the player itself, or a vehicle it is steering.
/// Projectile and item-entity owners are not resolvable here - `GameEventContext` carries no
/// owner field (the same gap `warden.rs` documents for projectile anger scaling).
async fn try_get_player(
    world: &Arc<World>,
    entity: &dyn crate::entity::EntityBase,
) -> Option<Arc<Player>> {
    let base = entity.get_entity();
    if let Some(player) = world.get_player_by_uuid(base.entity_uuid) {
        return Some(player);
    }
    let passengers = base.passengers.lock().await;
    passengers
        .iter()
        .find_map(|passenger| world.get_player_by_uuid(passenger.get_entity().entity_uuid))
}

impl BlockBehaviour for SculkShriekerBlock {
    /// `SculkShriekerBlock.spawnAfterBreak` (`SculkShriekerBlock.java:128-133`): after the
    /// normal player-break path, a break eligible for experience emits five XP.
    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.player.gamemode.load() != pumpkin_util::GameMode::Creative
                && args.player.gamemode.load() != pumpkin_util::GameMode::Spectator
                && args.world.level_info.load().game_rules.block_drops
            {
                ExperienceOrbEntity::spawn(args.world, args.position.to_centered_f64(), 5).await;
            }
        })
    }

    /// `getStateForPlacement` (lines 117-120).
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = SculkShriekerLikeProperties::default(args.block);
            props.shrieking = false;
            props.waterlogged = args.replacing.water_source();
            props.to_state_id(args.block)
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world
                .register_game_event_listener(Arc::new(ShriekerListener {
                    pos: *args.position,
                }))
                .await;
        })
    }

    fn on_state_replaced<'a>(&'a self, args: OnStateReplacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world
                .unregister_game_event_listener_at(args.position)
                .await;
        })
    }

    /// `stepOn` (lines 59-69).
    fn on_entity_step<'a>(&'a self, args: OnEntityStepArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let Some(player) = try_get_player(args.world, args.entity).await else {
                return;
            };
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return;
            };
            let Some(shrieker) = block_entity
                .as_any()
                .downcast_ref::<SculkShriekerBlockEntity>()
            else {
                return;
            };
            shrieker.try_shriek(args.world, &player).await;
        })
    }

    /// `tick` (lines 71-77): the shriek ends and the shrieker responds.
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let mut props = SculkShriekerLikeProperties::from_state_id(state.id, args.block);
            if !props.shrieking {
                return;
            }
            props.shrieking = false;
            args.world
                .set_block_state(
                    args.position,
                    props.to_state_id(args.block),
                    BlockFlags::NOTIFY_ALL,
                )
                .await;

            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return;
            };
            let Some(shrieker) = block_entity
                .as_any()
                .downcast_ref::<SculkShriekerBlockEntity>()
            else {
                return;
            };
            shrieker.try_respond(args.world).await;
        })
    }
}
