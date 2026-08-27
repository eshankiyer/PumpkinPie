use std::sync::Arc;

use crate::block::entities::daylight_detector::DaylightDetectorBlockEntity;
use pumpkin_data::{Block, BlockStateId, block_properties::BlockProperties, game_event::GameEvent};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::block::{
    BlockActionResult, BlockBehaviour, BlockFuture, BrokenArgs, EmitsRedstonePowerArgs,
    GetRedstonePowerArgs, NormalUseArgs, PlacedArgs,
};
use crate::world::World;
use crate::world::game_event::{GameEventContext, emit_game_event};

type DaylightDetectorProperties = pumpkin_data::block_properties::DaylightDetectorLikeProperties;

#[pumpkin_block("minecraft:daylight_detector")]
pub struct DaylightDetectorBlock;

impl BlockBehaviour for DaylightDetectorBlock {
    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world
                .add_block_entity(Arc::new(DaylightDetectorBlockEntity::new(*args.position)));
        })
    }

    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world.remove_block_entity(args.position);
        })
    }

    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async {
            let player_abilities = args.player.abilities.lock();
            if !player_abilities.await.allow_modify_world {
                return BlockActionResult::Pass;
            }

            let state = args.world.get_block_state(args.position);
            let props = DaylightDetectorProperties::from_state_id(state.id, args.block);

            let new_state = self
                .update_inverted(props, args.world, args.position, args.block)
                .await;

            // DaylightDetectorBlock.useWithoutItem, lines 80-84: notify vibration/game-event
            // listeners about the inverted state before recalculating its power.
            emit_game_event(
                args.world,
                GameEvent::BlockChange,
                pumpkin_util::math::vector3::Vector3::new(
                    f64::from(args.position.0.x) + 0.5,
                    f64::from(args.position.0.y) + 0.5,
                    f64::from(args.position.0.z) + 0.5,
                ),
                GameEventContext::of_entity_with_block_state(
                    args.player.clone() as Arc<dyn crate::entity::EntityBase>,
                    new_state,
                ),
            )
            .await;

            DaylightDetectorBlockEntity::update_power(args.world, args.position).await;

            BlockActionResult::Success
        })
    }

    fn get_weak_redstone_power<'a>(
        &'a self,
        args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(async move {
            let props = DaylightDetectorProperties::from_state_id(args.state.id, args.block);

            props.power
        })
    }

    fn emits_redstone_power<'a>(
        &'a self,
        _args: EmitsRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, bool> {
        Box::pin(async move { true })
    }
}

impl DaylightDetectorBlock {
    async fn update_inverted(
        &self,
        props: DaylightDetectorProperties,
        world: &Arc<World>,
        block_pos: &BlockPos,
        block: &Block,
    ) -> BlockStateId {
        let mut props = props;
        props.inverted = !props.inverted;

        let state = props.to_state_id(block);

        world
            .set_block_state(block_pos, state, BlockFlags::NOTIFY_LISTENERS)
            .await;

        state
    }
}
