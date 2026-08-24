use std::sync::Arc;

use pumpkin_data::Block;
use pumpkin_data::BlockDirection;
use pumpkin_data::BlockStateId;
use pumpkin_data::HorizontalFacingExt;
use pumpkin_data::block_properties::AttachFace;
use pumpkin_data::block_properties::BlockProperties;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

type ButtonLikeProperties = pumpkin_data::block_properties::LeverLikeProperties;

use crate::block::BlockFuture;
use crate::block::CanPlaceAtArgs;
use crate::block::EmitsRedstonePowerArgs;
use crate::block::ExplodeArgs;
use crate::block::GetRedstonePowerArgs;
use crate::block::GetStateForNeighborUpdateArgs;
use crate::block::OnPlaceArgs;
use crate::block::OnScheduledTickArgs;
use crate::block::OnStateReplacedArgs;
use crate::block::blocks::abstract_wall_mounting::WallMountedBlock;
use crate::block::blocks::redstone::lever::LeverLikePropertiesExt;
use crate::block::registry::BlockActionResult;
use crate::block::{BlockBehaviour, NormalUseArgs};
use crate::entity::player::Player;
use crate::world::World;
use crate::world::game_event::{GameEventContext, emit_game_event};

/// Vanilla `ButtonBlock.getSound` (`ButtonBlock.java:103-105`) resolves through the block-set
/// type's `buttonClickOn`/`buttonClickOff` (`BlockSetType.java:24-25`). Both stone-family
/// buttons - `stone_button` and `polished_blackstone_button` - register with
/// `BlockSetType.STONE` (`Blocks.java:1929,4937`) and so share the stone click sounds; every
/// plain wood button (oak, spruce, birch, jungle, acacia, dark oak, pale oak, mangrove) uses
/// its set type's single-arg constructor default, which is the wooden click sounds
/// (`BlockSetType.java:200-217`); bamboo, cherry, crimson and warped each register their own
/// distinct sounds.
fn button_click_sound(block: &Block, pressed: bool) -> Sound {
    let (on, off) = match block.name {
        "bamboo_button" => (
            Sound::BlockBambooWoodButtonClickOn,
            Sound::BlockBambooWoodButtonClickOff,
        ),
        "cherry_button" => (
            Sound::BlockCherryWoodButtonClickOn,
            Sound::BlockCherryWoodButtonClickOff,
        ),
        "crimson_button" | "warped_button" => (
            Sound::BlockNetherWoodButtonClickOn,
            Sound::BlockNetherWoodButtonClickOff,
        ),
        "stone_button" | "polished_blackstone_button" => (
            Sound::BlockStoneButtonClickOn,
            Sound::BlockStoneButtonClickOff,
        ),
        _ => (
            Sound::BlockWoodenButtonClickOn,
            Sound::BlockWoodenButtonClickOff,
        ),
    };
    if pressed { on } else { off }
}

async fn click_button(
    world: &Arc<World>,
    block_pos: &BlockPos,
    block: &Block,
    player: Option<&Arc<Player>>,
) {
    let (_, state) = world.get_block_and_state_id(block_pos);

    let mut button_props = ButtonLikeProperties::from_state_id(state, block);
    if !button_props.powered {
        button_props.powered = true;
        world
            .set_block_state(
                block_pos,
                button_props.to_state_id(block),
                BlockFlags::NOTIFY_ALL,
            )
            .await;
        // Vanilla `ticksToStayPressed`: 20 for the stone button (`ButtonBlock.java`
        // registration in `Blocks.java`), 30 for every other set.
        let delay = if *block == Block::STONE_BUTTON {
            20
        } else {
            30
        };
        world.schedule_block_tick(block, *block_pos, delay, TickPriority::Normal);
        ButtonBlock::update_neighbors(world, block_pos, &button_props).await;

        // Vanilla `press` (ButtonBlock.java:94-100): click-on sound and a BLOCK_ACTIVATE
        // game event whose source is the pressing player (null for explosions,
        // `ButtonBlock.java:88-90`). `playSound(pressed ? player : null, ...)` excludes the
        // pressing player's own client, which already renders the click via local
        // prediction.
        if let Some(player) = player {
            world.play_block_sound_expect(
                player,
                button_click_sound(block, true),
                SoundCategory::Blocks,
                *block_pos,
            );
        } else {
            world.play_block_sound(
                button_click_sound(block, true),
                SoundCategory::Blocks,
                *block_pos,
            );
        }
        emit_game_event(
            world,
            GameEvent::BlockActivate,
            Vector3::new(
                f64::from(block_pos.0.x) + 0.5,
                f64::from(block_pos.0.y) + 0.5,
                f64::from(block_pos.0.z) + 0.5,
            ),
            player.map_or_else(GameEventContext::none, |player| {
                GameEventContext::of_entity(player.clone())
            }),
        )
        .await;
    }
}

#[pumpkin_block_from_tag("minecraft:buttons")]
pub struct ButtonBlock;

impl BlockBehaviour for ButtonBlock {
    /// Vanilla `useWithoutItem` (ButtonBlock.java:76-83): an already-pressed button only
    /// consumes the interaction; otherwise it presses and succeeds.
    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let props = ButtonLikeProperties::from_state_id(state.id, args.block);
            if props.powered {
                return BlockActionResult::Consume;
            }

            click_button(args.world, args.position, args.block, Some(args.player)).await;

            BlockActionResult::Success
        })
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let mut props = ButtonLikeProperties::from_state_id(state.id, args.block);
            props.powered = false;
            args.world
                .set_block_state(
                    args.position,
                    props.to_state_id(args.block),
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
            Self::update_neighbors(args.world, args.position, &props).await;

            // Vanilla `checkPressed` (ButtonBlock.java:170-175): releasing plays the
            // click-off sound with no source and fires BLOCK_DEACTIVATE. Not ported:
            // `checkPressed` re-checks for an arrow lodged in an arrow-activatable button
            // (`type.canButtonBeActivatedByArrows()`) before unpressing, and arrows never
            // press a button here at all (`entityInside`, ButtonBlock.java:156-160) - this
            // always unpresses unconditionally, matching every button except one an arrow
            // is still sitting in.
            args.world.play_sound(
                button_click_sound(args.block, false),
                SoundCategory::Blocks,
                &Vector3::new(
                    f64::from(args.position.0.x) + 0.5,
                    f64::from(args.position.0.y) + 0.5,
                    f64::from(args.position.0.z) + 0.5,
                ),
            );
            emit_game_event(
                args.world,
                GameEvent::BlockDeactivate,
                Vector3::new(
                    f64::from(args.position.0.x) + 0.5,
                    f64::from(args.position.0.y) + 0.5,
                    f64::from(args.position.0.z) + 0.5,
                ),
                GameEventContext::none(),
            )
            .await;
        })
    }

    /// Vanilla `onExplosionHit` (ButtonBlock.java:85-92): a blast that can trigger blocks
    /// presses the button if it is not already pressed (`press` with a null player).
    fn explode<'a>(&'a self, args: ExplodeArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.can_trigger_blocks {
                click_button(args.world, args.position, args.block, None).await;
            }
        })
    }

    fn emits_redstone_power<'a>(
        &'a self,
        _args: EmitsRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, bool> {
        Box::pin(async move { true })
    }

    fn get_weak_redstone_power<'a>(
        &'a self,
        args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(async move {
            let button_props = ButtonLikeProperties::from_state_id(args.state.id, args.block);
            if button_props.powered { 15 } else { 0 }
        })
    }

    fn get_strong_redstone_power<'a>(
        &'a self,
        args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(async move {
            let button_props = ButtonLikeProperties::from_state_id(args.state.id, args.block);
            if button_props.powered && button_props.get_direction() == args.direction {
                15
            } else {
                0
            }
        })
    }

    fn on_state_replaced<'a>(&'a self, args: OnStateReplacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !args.moved {
                let button_props =
                    ButtonLikeProperties::from_state_id(args.old_state_id, args.block);
                if button_props.powered {
                    Self::update_neighbors(args.world, args.position, &button_props).await;
                }
            }
        })
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props =
                ButtonLikeProperties::from_state_id(args.block.default_state.id, args.block);
            (props.face, props.facing) =
                WallMountedBlock::get_placement_face(self, args.player, args.direction);

            props.to_state_id(args.block)
        })
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        // Use the provided direction, or fallback to the current state's direction if missing
        let direction = args
            .direction
            .unwrap_or_else(|| self.get_direction(args.state.id, args.block));

        WallMountedBlock::can_place_at(self, args.block_accessor, args.position, direction)
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move { WallMountedBlock::get_state_for_neighbor_update(self, args).await })
    }
}

impl WallMountedBlock for ButtonBlock {
    fn get_direction(&self, state_id: BlockStateId, block: &Block) -> BlockDirection {
        let props = ButtonLikeProperties::from_state_id(state_id, block);
        match props.face {
            AttachFace::Floor => BlockDirection::Up,
            AttachFace::Ceiling => BlockDirection::Down,
            AttachFace::Wall => props.facing.to_block_direction(),
        }
    }
}

impl ButtonBlock {
    async fn update_neighbors(
        world: &Arc<World>,
        block_pos: &BlockPos,
        props: &ButtonLikeProperties,
    ) {
        let direction = props.get_direction().opposite();
        world.update_neighbors(block_pos, None).await;
        world
            .update_neighbors(&block_pos.offset(direction.to_offset()), None)
            .await;
    }
}
