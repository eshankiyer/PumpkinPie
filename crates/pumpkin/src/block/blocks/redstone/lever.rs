use std::sync::Arc;

use crate::block::{CanPlaceAtArgs, EmitsRedstonePowerArgs, ExplodeArgs, GetRedstonePowerArgs,
    GetStateForNeighborUpdateArgs, OnPlaceArgs, OnStateReplacedArgs,
    blocks::abstract_wall_mounting::WallMountedBlock};
use pumpkin_data::{Block, BlockDirection, BlockStateId, HorizontalFacingExt,
    block_properties::{AttachFace, BlockProperties, LeverLikeProperties},
    game_event::GameEvent, sound::{Sound, SoundCategory}};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use crate::{block::{BlockBehaviour, NormalUseArgs, registry::BlockActionResult}, world::World,
    world::game_event::{GameEventContext, emit_game_event}};

fn toggle_lever(world: &Arc<World>, block_pos: &BlockPos) {
    let (block, state) = world.get_block_and_state_id(block_pos);
    let mut props = LeverLikeProperties::from_state_id(state, block);
    props.powered = !props.powered;
    world.set_block_state(block_pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
    LeverBlock::update_neighbors(world, block_pos, props);

    // `LeverBlock.java:72`, `LeverBlock.java:79-87`, and `ServerExplosion.java:297-302`: the
    // vanilla pull path plays the click and emits activate/deactivate with no source entity.
    world.play_sound_raw(
        Sound::BlockLeverClick as u16, SoundCategory::Blocks,
        &Vector3::new(f64::from(block_pos.0.x) + 0.5, f64::from(block_pos.0.y) + 0.5, f64::from(block_pos.0.z) + 0.5),
        0.3, if props.powered { 0.6 } else { 0.5 },
    );
    emit_game_event_blocking(world,
        if props.powered { GameEvent::BlockActivate } else { GameEvent::BlockDeactivate },
        Vector3::new(f64::from(block_pos.0.x) + 0.5, f64::from(block_pos.0.y) + 0.5, f64::from(block_pos.0.z) + 0.5));
}

#[pumpkin_block("minecraft:lever")]
pub struct LeverBlock;

impl BlockBehaviour for LeverBlock {
    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        toggle_lever(args.world, args.position);
        BlockActionResult::Consume
    }

    /// Vanilla `LeverBlock.java:79-87` (`onExplosionHit`): trigger-block blasts flip levers.
    fn explode(&self, args: ExplodeArgs<'_>) {
        if args.can_trigger_blocks { toggle_lever(args.world, args.position); }
    }

    fn emits_redstone_power(&self, _args: EmitsRedstonePowerArgs<'_>) -> bool { true }
    fn get_weak_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8 {
        let props = LeverLikeProperties::from_state_id(args.state.id, args.block);
        if props.powered { 15 } else { 0 }
    }
    fn get_strong_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8 {
        let props = LeverLikeProperties::from_state_id(args.state.id, args.block);
        if props.powered && props.get_direction() == args.direction { 15 } else { 0 }
    }
    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>) {
        let props = LeverLikeProperties::from_state_id(args.old_state_id, args.block);
        if !args.moved && props.powered { Self::update_neighbors(args.world, args.position, props); }
    }
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = LeverLikeProperties::default(args.block);
        (props.face, props.facing) = WallMountedBlock::get_placement_face(self, args.player, args.direction);
        props.to_state_id(args.block)
    }
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        let direction = args.direction.unwrap_or_else(|| self.get_direction(args.state.id, args.block));
        WallMountedBlock::can_place_at(self, args.block_accessor, args.position, direction)
    }
    fn get_state_for_neighbor_update(&self, args: GetStateForNeighborUpdateArgs<'_>) -> BlockStateId {
        WallMountedBlock::get_state_for_neighbor_update(self, args)
    }
}

impl WallMountedBlock for LeverBlock {
    fn get_direction(&self, state_id: BlockStateId, block: &Block) -> BlockDirection {
        let props = LeverLikeProperties::from_state_id(state_id, block);
        match props.face {
            AttachFace::Floor => BlockDirection::Up,
            AttachFace::Ceiling => BlockDirection::Down,
            AttachFace::Wall => props.facing.to_block_direction(),
        }
    }
}

impl LeverBlock {
    fn update_neighbors(world: &Arc<World>, block_pos: &BlockPos, props: LeverLikeProperties) {
        let direction = props.get_direction().opposite();
        world.update_neighbors(block_pos, None);
        world.update_neighbors(&block_pos.offset(direction.to_offset()), None);
    }
}

pub trait LeverLikePropertiesExt { fn get_direction(&self) -> BlockDirection; }
impl LeverLikePropertiesExt for LeverLikeProperties {
    fn get_direction(&self) -> BlockDirection {
        match self.face {
            AttachFace::Ceiling => BlockDirection::Down,
            AttachFace::Floor => BlockDirection::Up,
            AttachFace::Wall => self.facing.to_block_direction(),
        }
    }
}

fn emit_game_event_blocking(world: &Arc<World>, event: GameEvent, pos: Vector3<f64>) {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(emit_game_event(world, event, pos, GameEventContext::none()));
    });
}
