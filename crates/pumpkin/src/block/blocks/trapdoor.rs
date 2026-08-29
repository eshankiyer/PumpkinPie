use crate::block::blocks::copper_weathering;
use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::registry::BlockActionResult;
use crate::block::{BlockBehaviour, ExplodeArgs, NormalUseArgs, OnNeighborUpdateArgs, OnPlaceArgs, RandomTickArgs};
use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::world::World;
use pumpkin_data::BlockDirection;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, Half};
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, tag};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;

type TrapDoorProperties = pumpkin_data::block_properties::OakTrapdoorLikeProperties;

fn toggle_trapdoor(player: Option<&Arc<Player>>, world: &Arc<World>, block_pos: &BlockPos) {
    let (block, block_state) = world.get_block_and_state_id(block_pos);
    let mut trapdoor_props = TrapDoorProperties::from_state_id(block_state, block);
    trapdoor_props.open = !trapdoor_props.open;

    if let Some(player) = player {
        world.play_block_sound_expect(player, get_sound(block, trapdoor_props.open), SoundCategory::Blocks, *block_pos);
    } else {
        world.play_block_sound(get_sound(block, trapdoor_props.open), SoundCategory::Blocks, *block_pos);
    }
    world.emit_game_event(
        if trapdoor_props.open { "block_open" } else { "block_close" },
        block_pos.to_centered_f64(),
    );
    world.set_block_state(
        block_pos,
        trapdoor_props.to_state_id(block),
        BlockFlags::NOTIFY_LISTENERS,
    );
}

fn can_open_trapdoor(block: &Block) -> bool {
    block != &Block::IRON_TRAPDOOR
}

fn get_sound(block: &Block, open: bool) -> Sound {
    if open {
        if block.has_tag(&tag::Block::MINECRAFT_WOODEN_TRAPDOORS) {
            Sound::BlockWoodenTrapdoorOpen
        } else if block == &Block::IRON_TRAPDOOR {
            Sound::BlockIronTrapdoorOpen
        } else {
            Sound::BlockCopperTrapdoorOpen
        }
    } else if block.has_tag(&tag::Block::MINECRAFT_WOODEN_TRAPDOORS) {
        Sound::BlockWoodenTrapdoorClose
    } else if block == &Block::IRON_TRAPDOOR {
        Sound::BlockIronTrapdoorClose
    } else {
        Sound::BlockCopperTrapdoorClose
    }
}

#[pumpkin_block_from_tag("minecraft:trapdoors")]
pub struct TrapDoorBlock;

impl BlockBehaviour for TrapDoorBlock {
    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        if !can_open_trapdoor(args.block) {
            return BlockActionResult::Pass;
        }
        toggle_trapdoor(Some(args.player), args.world, args.position);
        BlockActionResult::Success
    }

    /// `TrapDoorBlock.onExplosionHit` (`TrapDoorBlock.java:98-106`) toggles an unpowered
    /// hand-openable trapdoor when a trigger-block explosion reaches it.
    fn explode(&self, args: ExplodeArgs<'_>) {
        if !args.can_trigger_blocks {
            return;
        }
        let state = args.world.get_block_state(args.position);
        let props = TrapDoorProperties::from_state_id(state.id, args.block);
        if props.powered || !can_open_trapdoor(args.block) {
            return;
        }
        toggle_trapdoor(None, args.world, args.position);
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut trapdoor_props = TrapDoorProperties::default(args.block);
        trapdoor_props.waterlogged = args.replacing.water_source();
        let powered = block_receives_redstone_power(args.world, args.position);
        let player_facing = args.player.get_entity().get_horizontal_facing();
        trapdoor_props.facing = args.direction.to_horizontal_facing().unwrap_or(player_facing);
        trapdoor_props.half = match args.direction {
            BlockDirection::Up => Half::Top,
            BlockDirection::Down => Half::Bottom,
            _ => match args.use_item_on.cursor_pos.y {
                0.0..0.5 => Half::Bottom,
                _ => Half::Top,
            },
        };
        trapdoor_props.powered = powered;
        trapdoor_props.open = powered;
        trapdoor_props.to_state_id(args.block)
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        let block_state = args.world.get_block_state(args.position);
        let mut trapdoor_props = TrapDoorProperties::from_state_id(block_state.id, args.block);
        let powered = block_receives_redstone_power(args.world, args.position);
        if powered != trapdoor_props.powered {
            trapdoor_props.powered = !trapdoor_props.powered;
            if powered != trapdoor_props.open {
                trapdoor_props.open = trapdoor_props.powered;
                args.world.play_block_sound(get_sound(args.block, powered), SoundCategory::Blocks, *args.position);
                args.world.emit_game_event(
                    if powered { "block_open" } else { "block_close" },
                    args.position.to_centered_f64(),
                );
            }
        }
        args.world.set_block_state(
            args.position,
            trapdoor_props.to_state_id(args.block),
            BlockFlags::NOTIFY_LISTENERS,
        );
    }

    fn random_tick(&self, args: RandomTickArgs<'_>) {
        let current_state_id = args.world.get_block_state_id(args.position);
        let trapdoor_props = TrapDoorProperties::from_state_id(current_state_id, args.block);
        let oxidation_stages = [
            &Block::COPPER_TRAPDOOR,
            &Block::EXPOSED_COPPER_TRAPDOOR,
            &Block::WEATHERED_COPPER_TRAPDOOR,
            &Block::OXIDIZED_COPPER_TRAPDOOR,
        ];
        copper_weathering::try_oxidize_copper(
            args.world,
            args.position,
            args.block,
            &oxidation_stages,
            |next_block| {
                let mut new_props = TrapDoorProperties::default(next_block);
                new_props.facing = trapdoor_props.facing;
                new_props.half = trapdoor_props.half;
                new_props.open = trapdoor_props.open;
                new_props.powered = trapdoor_props.powered;
                new_props.waterlogged = trapdoor_props.waterlogged;
                new_props.to_state_id(next_block)
            },
        );
    }
}
