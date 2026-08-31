use crate::block::blocks::copper_weathering;
use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockFuture, ExplodeArgs, NormalUseArgs, OnNeighborUpdateArgs, OnPlaceArgs,
    RandomTickArgs,
};
use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::world::World;
use crate::world::game_event::{GameEventContext, emit_game_event};
use pumpkin_data::block_properties::{BlockProperties, Half, HorizontalFacing};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, BlockStateId, tag};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::{tick::TickPriority, world::BlockFlags};
use std::sync::Arc;

type TrapDoorProperties = pumpkin_data::block_properties::OakTrapdoorLikeProperties;

async fn toggle_trapdoor(player: Option<&Arc<Player>>, world: &Arc<World>, block_pos: &BlockPos) {
    let (block, block_state) = world.get_block_and_state_id(block_pos);
    let mut trapdoor_props = TrapDoorProperties::from_state_id(block_state, block);
    trapdoor_props.open = !trapdoor_props.open;

    if let Some(player) = player {
        world.play_block_sound_expect(
            player,
            get_sound(block, trapdoor_props.open),
            SoundCategory::Blocks,
            *block_pos,
        );
    } else {
        world.play_block_sound(
            get_sound(block, trapdoor_props.open),
            SoundCategory::Blocks,
            *block_pos,
        );
    }

    // TrapDoorBlock.java:122 (`playSound`, called from the click-triggered `toggle`): fires
    // BLOCK_OPEN/BLOCK_CLOSE with the player as source entity.
    emit_game_event(
        world,
        if trapdoor_props.open {
            GameEvent::BlockOpen
        } else {
            GameEvent::BlockClose
        },
        block_pos.to_centered_f64(),
        player.map_or(GameEventContext::none(), |player| {
            GameEventContext::of_entity(player.clone())
        }),
    )
    .await;

    world
        .set_block_state(
            block_pos,
            trapdoor_props.to_state_id(block),
            BlockFlags::NOTIFY_LISTENERS,
        )
        .await;

    // `TrapDoorBlock.toggle` (`TrapDoorBlock.java:108-114`) schedules the water fluid tick
    // after changing a waterlogged trapdoor, so flowing water observes the new state.
    if trapdoor_props.waterlogged {
        world.schedule_fluid_tick(&Fluid::WATER, *block_pos, 5, TickPriority::Normal);
    }
}

fn can_open_trapdoor(block: &Block) -> bool {
    if block == &Block::IRON_TRAPDOOR {
        return false;
    }
    true
}

// `TrapDoorBlock.getStateForPlacement` (`TrapDoorBlock.java:150-155`) selects the clicked
// horizontal face, otherwise the opposite of the player's horizontal direction.
fn placement_facing(
    direction: BlockDirection,
    player_facing: HorizontalFacing,
) -> HorizontalFacing {
    direction
        .to_horizontal_facing()
        .unwrap_or(player_facing.opposite())
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
    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            if !can_open_trapdoor(args.block) {
                return BlockActionResult::Pass;
            }

            toggle_trapdoor(Some(args.player), args.world, args.position).await;

            BlockActionResult::Success
        })
    }

    /// `TrapDoorBlock.onExplosionHit` (`TrapDoorBlock.java:98-106`) toggles an unpowered
    /// hand-openable trapdoor when a trigger-block explosion, such as a wind charge, reaches it.
    fn explode<'a>(&'a self, args: ExplodeArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !args.can_trigger_blocks {
                return;
            }

            let state = args.world.get_block_state(args.position);
            let props = TrapDoorProperties::from_state_id(state.id, args.block);
            if props.powered || !can_open_trapdoor(args.block) {
                return;
            }

            toggle_trapdoor(None, args.world, args.position).await;
        })
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut trapdoor_props = TrapDoorProperties::default(args.block);
            trapdoor_props.waterlogged = args.replacing.water_source();

            let powered = block_receives_redstone_power(args.world, args.position).await;

            let player_facing = args.player.get_entity().get_horizontal_facing();

            // `TrapDoorBlock.getStateForPlacement` (`TrapDoorBlock.java:146-161`) uses the
            // clicked horizontal face, or the opposite of the player's horizontal direction
            // for a floor/ceiling placement.
            let facing = placement_facing(args.direction, player_facing);

            trapdoor_props.facing = facing;

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
        })
    }

    fn on_neighbor_update<'a>(&'a self, args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let block_state = args.world.get_block_state(args.position);
            let mut trapdoor_props = TrapDoorProperties::from_state_id(block_state.id, args.block);
            let powered = block_receives_redstone_power(args.world, args.position).await;

            if powered != trapdoor_props.powered {
                trapdoor_props.powered = !trapdoor_props.powered;

                if powered != trapdoor_props.open {
                    trapdoor_props.open = trapdoor_props.powered;

                    args.world.play_block_sound(
                        get_sound(args.block, powered),
                        SoundCategory::Blocks,
                        *args.position,
                    );

                    // TrapDoorBlock.java's redstone-triggered path also routes through
                    // `playSound`, so this fires BLOCK_OPEN/BLOCK_CLOSE with no source entity.
                    emit_game_event(
                        args.world,
                        if powered {
                            GameEvent::BlockOpen
                        } else {
                            GameEvent::BlockClose
                        },
                        args.position.to_centered_f64(),
                        GameEventContext::none(),
                    )
                    .await;
                }
            }

            args.world
                .set_block_state(
                    args.position,
                    trapdoor_props.to_state_id(args.block),
                    BlockFlags::NOTIFY_LISTENERS,
                )
                .await;
        })
    }

    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // No tag gate needed: the oxidation_stages table below only contains the
            // copper trapdoor family, so this is a no-op for every other trapdoor type.

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
            )
            .await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::placement_facing;
    use pumpkin_data::BlockDirection;
    use pumpkin_data::block_properties::HorizontalFacing;

    #[test]
    fn vertical_trapdoor_placement_faces_away_from_player() {
        // `TrapDoorBlock.getStateForPlacement` (`TrapDoorBlock.java:150-155`) uses the
        // opposite player direction for non-horizontal clicked faces.
        assert_eq!(
            placement_facing(BlockDirection::Up, HorizontalFacing::North),
            HorizontalFacing::South
        );
        assert_eq!(
            placement_facing(BlockDirection::East, HorizontalFacing::North),
            HorizontalFacing::East
        );
    }
}
