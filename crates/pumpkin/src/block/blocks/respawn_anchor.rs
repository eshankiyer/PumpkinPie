use pumpkin_data::item::Item;
use pumpkin_data::translation;
use pumpkin_data::{
    Block, BlockDirection,
    block_properties::{BlockProperties, RespawnAnchorLikeProperties},
    dimension::Dimension,
    fluid::Fluid,
    sound::{Sound, SoundCategory},
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;

use crate::block::registry::BlockActionResult;
use crate::block::{BlockBehaviour, GetComparatorOutputArgs, NormalUseArgs, UseWithItemArgs};
use crate::world::World;
use crate::world::explosion::{DefaultExplosionDamageCalculator, Explosion, ExplosionDamageCalculator};

/// `RespawnAnchorBlock.isWaterThatWouldFlow` (`RespawnAnchorBlock.java:140-156`): water that
/// would flow into the anchor's former position during the explosion.
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

/// `RespawnAnchorBlock.explode`'s custom explosion resistance
/// (`RespawnAnchorBlock.java:163-172`).
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
            Some(Block::WATER.blast_resistance)
        } else {
            DefaultExplosionDamageCalculator
                .get_block_explosion_resistance(explosion, world, pos, block, fluid)
        }
    }
}

#[pumpkin_block("minecraft:respawn_anchor")]
pub struct RespawnAnchorBlock;

impl RespawnAnchorBlock {
    const MAX_CHARGES: u8 = 4;

    fn works_here(world: &World) -> bool {
        world.dimension == Dimension::THE_NETHER
    }

    const fn charges_to_comparator_output(charges: u8) -> u8 {
        charges * 15 / Self::MAX_CHARGES
    }
}

impl BlockBehaviour for RespawnAnchorBlock {
    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        // The off-hand retry in vanilla is part of `useItemOn` (`RespawnAnchorBlock.java:92-96`).
        let state_id = args.world.get_block_state_id(args.position);
        let mut props = RespawnAnchorLikeProperties::from_state_id(state_id, args.block);
        if args.item_stack.item.id != Item::GLOWSTONE.id || props.charges >= Self::MAX_CHARGES {
            return BlockActionResult::PassToDefaultBlockAction;
        }

        args.item_stack
            .decrement_unless_creative(args.player.gamemode.load(), 1);
        props.charges += 1;
        args.world.set_block_state(
            args.position,
            props.to_state_id(args.block),
            BlockFlags::NOTIFY_ALL,
        );
        args.world.play_sound(
            Sound::BlockRespawnAnchorCharge,
            SoundCategory::Blocks,
            &args.position.to_centered_f64(),
        );
        args.world
            .emit_game_event("block_change", args.position.to_centered_f64());
        BlockActionResult::Success
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        if !Self::works_here(args.world) {
            // Vanilla explosion behavior is `RespawnAnchorBlock.explode`
            // (`RespawnAnchorBlock.java:159-176`), with the custom resistance below
            // (`RespawnAnchorBlock.java:163-172`).
            args.world
                .break_block(args.position, None, BlockFlags::SKIP_DROPS);
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
            args.world.explode_with_calculator(
                args.position.to_centered_f64(),
                5.0,
                crate::world::ExplosionInteraction::Block,
                Some(Arc::new(RespawnAnchorExplosionCalculator {
                    anchor_pos: *args.position,
                    in_water,
                })),
            );
            return BlockActionResult::SuccessServer;
        }

        let state_id = args.world.get_block_state_id(args.position);
        let mut props = RespawnAnchorLikeProperties::from_state_id(state_id, args.block);
        if props.charges == 0 {
            args.player.send_system_message(&pumpkin_macros::translate_cross!(
                translation::java::BLOCK_MINECRAFT_BED_NO_SLEEP,
                translation::bedrock::TILE_BED_NOSLEEP
            ));
            return BlockActionResult::SuccessServer;
        }

        if args.player.set_respawn_point(
            args.world.dimension.clone(),
            *args.position,
            args.player.get_entity().yaw.load(),
            args.player.get_entity().pitch.load(),
            false,
        ) {
            props.charges -= 1;
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_ALL,
            );
            args.world.play_sound(
                Sound::BlockRespawnAnchorSetSpawn,
                SoundCategory::Blocks,
                &args.position.to_centered_f64(),
            );
            args.player.send_system_message(&pumpkin_macros::translate_cross!(
                translation::java::BLOCK_MINECRAFT_SET_SPAWN,
                translation::bedrock::TILE_BED_RESPAWNSET
            ));
        }
        BlockActionResult::SuccessServer
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        let props = RespawnAnchorLikeProperties::from_state_id(args.state.id, args.block);
        Some(Self::charges_to_comparator_output(props.charges))
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
