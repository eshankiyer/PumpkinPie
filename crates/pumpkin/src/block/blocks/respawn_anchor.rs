use pumpkin_data::item::Item;
use pumpkin_data::translation;
use pumpkin_data::{
    Block, BlockDirection,
    block_properties::{BlockProperties, RespawnAnchorLikeProperties},
    dimension::Dimension,
    fluid::Fluid,
    game_event::GameEvent,
    sound::{Sound, SoundCategory},
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;

use crate::block::{
    BlockBehaviour, BlockFuture, GetComparatorOutputArgs, NormalUseArgs, UseWithItemArgs,
    registry::BlockActionResult,
};
use crate::entity::EntityBase;
use crate::world::World;
use crate::world::explosion::{
    DefaultExplosionDamageCalculator, Explosion, ExplosionDamageCalculator,
};
use crate::world::game_event::{GameEventContext, emit_game_event};

/// `RespawnAnchorBlock.isWaterThatWouldFlow` (`RespawnAnchorBlock.java:140-156`): a water
/// source, or flowing water with enough amount left (`getAmount() == FluidState.level`,
/// 8 = a full/source column - see `grass_block.rs`'s `FULL_FLUID_AMOUNT`) whose column
/// isn't just draining into empty space below.
fn is_water_that_would_flow(world: &World, pos: BlockPos) -> bool {
    let (fluid, state) = world.get_fluid_and_fluid_state(&pos);
    if !fluid.matches_type(&Fluid::WATER) {
        return false;
    }
    if state.is_source {
        return true;
    }
    if state.level < 2 {
        return false;
    }
    let (below_fluid, _) = world.get_fluid_and_fluid_state(&pos.down());
    below_fluid.matches_type(&Fluid::WATER)
}

/// `RespawnAnchorBlock.explode`'s custom `ExplosionDamageCalculator`
/// (`RespawnAnchorBlock.java:163-172`): the anchor's own former position reports water's
/// explosion resistance instead of the surrounding terrain's, when water would flow in.
struct RespawnAnchorExplosionCalculator {
    anchor_pos: BlockPos,
    in_water: bool,
}

impl ExplosionDamageCalculator for RespawnAnchorExplosionCalculator {
    fn get_block_explosion_resistance(
        &self,
        explosion: &Explosion,
        world: &World,
        pos: &BlockPos,
        block: &Block,
        fluid: &pumpkin_data::fluid::FluidState,
    ) -> Option<f32> {
        if *pos == self.anchor_pos && self.in_water {
            // `Blocks.WATER.getExplosionResistance()`.
            Some(Block::WATER.blast_resistance)
        } else {
            DefaultExplosionDamageCalculator
                .get_block_explosion_resistance(explosion, world, pos, block, fluid)
        }
    }
}

/// `net.minecraft.world.level.block.RespawnAnchorBlock`: charges with glowstone, sets the
/// player's respawn point on an empty-hand use, and explodes instead outside the Nether.
#[pumpkin_block("minecraft:respawn_anchor")]
pub struct RespawnAnchorBlock;

impl RespawnAnchorBlock {
    const MAX_CHARGES: u8 = 4;

    /// Mirrors `RespawnAnchorBlock.canSetSpawn`, which vanilla implements by reading the
    /// `minecraft:gameplay/respawn_anchor_works` dimension attribute (true only in the Nether by
    /// default). This codebase's generated `Dimension` data has no per-dimension attribute table
    /// (confirmed: no `respawn_anchor_works`/`EnvironmentAttribute`-equivalent field anywhere in
    /// `pumpkin-data`), so this is a direct dimension comparison rather than an attribute read.
    /// It matches vanilla's default behavior but, unlike vanilla, cannot be changed by a datapack.
    fn works_here(world: &crate::world::World) -> bool {
        world.dimension == Dimension::THE_NETHER
    }

    /// Mirrors `RespawnAnchorBlock.getAnalogOutputSignal`: `charge * 15 / MAX_CHARGES`.
    const fn charges_to_comparator_output(charges: u8) -> u8 {
        charges * 15 / Self::MAX_CHARGES
    }
}

impl BlockBehaviour for RespawnAnchorBlock {
    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let mut props = RespawnAnchorLikeProperties::from_state_id(state_id, args.block);

            if args.item_stack.item.id != Item::GLOWSTONE.id || props.charges >= Self::MAX_CHARGES {
                // Vanilla additionally checks the off-hand item here (`useItemOn`,
                // `RespawnAnchorBlock.java:92-96`): if the main hand isn't usable but the
                // off-hand holds glowstone and the anchor is chargeable, it returns `PASS` so
                // the interaction is retried with the off-hand item instead of falling through
                // to the empty-hand action. This codebase's packet dispatch
                // (`call_use_item_on` in `pumpkin/src/net/java/play.rs`) only ever processes the
                // single hand named by the incoming packet and has no generic same-click
                // off-hand retry, so this case is a known divergence rather than something
                // fixable locally in this block.
                return BlockActionResult::PassToDefaultBlockAction;
            }

            if args.player.gamemode.load() != GameMode::Creative {
                args.item_stack.decrement(1);
            }

            props.charges += 1;
            args.world
                .set_block_state(
                    args.position,
                    props.to_state_id(args.block),
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
            args.world.play_sound(
                Sound::BlockRespawnAnchorCharge,
                SoundCategory::Blocks,
                &args.position.to_centered_f64(),
            );
            emit_game_event(
                args.world,
                GameEvent::BlockChange,
                args.position.to_centered_f64(),
                GameEventContext::of_entity(args.player.clone() as Arc<dyn EntityBase>),
            )
            .await;

            BlockActionResult::Success
        })
    }

    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            if !Self::works_here(args.world) {
                args.world
                    .break_block(args.position, None, BlockFlags::SKIP_DROPS)
                    .await;
                // Vanilla `RespawnAnchorBlock.explode` (`RespawnAnchorBlock.java:159-176`):
                // the anchor's own former position reports water's explosion resistance
                // when the anchor block was in/adjacent to flowing water, softening the
                // blast there. The damage-source attribution
                // (`damageSources().badRespawnPointExplosion`) has no equivalent yet --
                // entities hurt by this blast are attributed the generic explosion damage
                // type instead of a respawn-anchor-specific one; narrow, admin/edge-case
                // divergence, left as-is.
                let mut in_water = false;
                for direction in [
                    BlockDirection::North,
                    BlockDirection::South,
                    BlockDirection::East,
                    BlockDirection::West,
                ] {
                    if is_water_that_would_flow(
                        args.world,
                        args.position.offset(direction.to_offset()),
                    ) {
                        in_water = true;
                        break;
                    }
                }
                if !in_water {
                    let (above_fluid, _) =
                        args.world.get_fluid_and_fluid_state(&args.position.up());
                    in_water = above_fluid.matches_type(&Fluid::WATER);
                }

                args.world
                    .explode_with_fire_and_calculator(
                        args.position.to_centered_f64(),
                        5.0,
                        crate::world::ExplosionInteraction::Block,
                        Arc::new(RespawnAnchorExplosionCalculator {
                            anchor_pos: *args.position,
                            in_water,
                        }),
                    )
                    .await;
                return BlockActionResult::SuccessServer;
            }

            let state_id = args.world.get_block_state_id(args.position);
            let mut props = RespawnAnchorLikeProperties::from_state_id(state_id, args.block);
            if props.charges == 0 {
                args.player
                    .send_system_message(&pumpkin_macros::translate_cross!(
                        translation::java::BLOCK_MINECRAFT_BED_NO_SLEEP,
                        translation::bedrock::TILE_BED_NOSLEEP
                    ))
                    .await;
                return BlockActionResult::SuccessServer;
            }

            let changed = args
                .player
                .set_respawn_point(
                    args.world.dimension.clone(),
                    *args.position,
                    args.player.get_entity().yaw.load(),
                    args.player.get_entity().pitch.load(),
                    false,
                )
                .await;
            if changed {
                props.charges -= 1;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
                args.world.play_sound(
                    Sound::BlockRespawnAnchorSetSpawn,
                    SoundCategory::Blocks,
                    &args.position.to_centered_f64(),
                );
                args.player
                    .send_system_message(&pumpkin_macros::translate_cross!(
                        translation::java::BLOCK_MINECRAFT_SET_SPAWN,
                        translation::bedrock::TILE_BED_RESPAWNSET
                    ))
                    .await;
            }
            BlockActionResult::SuccessServer
        })
    }

    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            let props = RespawnAnchorLikeProperties::from_state_id(args.state.id, args.block);
            Some(Self::charges_to_comparator_output(props.charges))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparator_output_matches_vanilla_scaling() {
        assert_eq!(RespawnAnchorBlock::charges_to_comparator_output(0), 0);
        assert_eq!(RespawnAnchorBlock::charges_to_comparator_output(1), 3);
        assert_eq!(RespawnAnchorBlock::charges_to_comparator_output(2), 7);
        assert_eq!(RespawnAnchorBlock::charges_to_comparator_output(3), 11);
        assert_eq!(RespawnAnchorBlock::charges_to_comparator_output(4), 15);
    }
}
