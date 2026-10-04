use std::sync::Arc;

use pumpkin_data::{
    Block, BlockDirection, BlockState, BlockStateId,
    game_event::GameEvent,
    sound::{Sound, SoundCategory},
};
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos, vector3::Vector3};
use pumpkin_world::{tick::TickPriority, world::BlockFlags};

use crate::{
    block::{OnEntityCollisionArgs, OnScheduledTickArgs, OnStateReplacedArgs},
    entity::EntityBase,
    world::{
        World,
        game_event::{GameEventContext, emit_game_event},
    },
};

pub mod plate;
pub mod weighted;

#[cfg(test)]
mod tests;

// Vanilla pressure plates detect entities in a centered 14x4x14-pixel volume.
const PRESSURE_PLATE_DETECTION_BOX: BoundingBox = BoundingBox::new_array(
    [1.0 / 16.0, 0.0, 1.0 / 16.0],
    [15.0 / 16.0, 4.0 / 16.0, 15.0 / 16.0],
);

fn detection_box_at(pos: &BlockPos) -> BoundingBox {
    PRESSURE_PLATE_DETECTION_BOX.at_pos(*pos)
}

/// Vanilla `BasePressurePlateBlock.checkPressed` plays the block-set type's
/// `pressurePlateClickOn`/`pressurePlateClickOff` (`BlockSetType.java:22-23`). Gold and iron
/// (the weighted plates) use the metal clicks, stone and polished blackstone the stone clicks;
/// cherry, bamboo and crimson/warped (nether wood) register their own, and every plain wood set
/// type uses the single-arg constructor's wooden clicks (`BlockSetType.java:200-216`).
fn pressure_plate_click_sound(block: &Block, pressed: bool) -> Sound {
    let (on, off) = match block.name {
        "light_weighted_pressure_plate" | "heavy_weighted_pressure_plate" => (
            Sound::BlockMetalPressurePlateClickOn,
            Sound::BlockMetalPressurePlateClickOff,
        ),
        "stone_pressure_plate" | "polished_blackstone_pressure_plate" => (
            Sound::BlockStonePressurePlateClickOn,
            Sound::BlockStonePressurePlateClickOff,
        ),
        "cherry_pressure_plate" => (
            Sound::BlockCherryWoodPressurePlateClickOn,
            Sound::BlockCherryWoodPressurePlateClickOff,
        ),
        "bamboo_pressure_plate" => (
            Sound::BlockBambooWoodPressurePlateClickOn,
            Sound::BlockBambooWoodPressurePlateClickOff,
        ),
        "crimson_pressure_plate" | "warped_pressure_plate" => (
            Sound::BlockNetherWoodPressurePlateClickOn,
            Sound::BlockNetherWoodPressurePlateClickOff,
        ),
        _ => (
            Sound::BlockWoodenPressurePlateClickOn,
            Sound::BlockWoodenPressurePlateClickOff,
        ),
    };
    if pressed { on } else { off }
}

pub(crate) trait PressurePlate {
    async fn on_entity_collision_pp(&self, args: OnEntityCollisionArgs<'_>) {
        let output = self.get_redstone_output(args.block, args.state.id);
        if output == 0 {
            self.update_plate_state(
                args.world,
                args.position,
                args.block,
                args.state,
                output,
                Some(args.entity),
            )
            .await;
        }
    }

    async fn on_scheduled_tick_pp(&self, args: OnScheduledTickArgs<'_>) {
        let state = args.world.get_block_state(args.position);
        let output = self.get_redstone_output(args.block, state.id);
        if output > 0 {
            self.update_plate_state(args.world, args.position, args.block, state, output, None)
                .await;
        }
    }

    async fn on_state_replaced_pp(&self, args: OnStateReplacedArgs<'_>) {
        if !args.moved && self.get_redstone_output(args.block, args.old_state_id) > 0 {
            args.world.update_neighbors(args.position, None).await;
            args.world
                .update_neighbors(&args.position.down(), None)
                .await;
        }
    }

    async fn update_plate_state(
        &self,
        world: &Arc<World>,
        pos: &BlockPos,
        block: &Block,
        state: &BlockState,
        output: u8,
        source: Option<&dyn EntityBase>,
    ) {
        let calc_output = self.calculate_redstone_output(world, block, pos).await;
        // Vanilla takes `isPressed` from the same signal it writes, so a plugin that rewrites
        // the current also decides the click, game event and rescheduling below.
        let new_output = if calc_output == output {
            calc_output
        } else {
            let next_output = if let Some(server) = world.server.upgrade() {
                let mut event = crate::plugin::block::block_redstone::BlockRedstoneEvent::new(
                    world.clone(),
                    state.id,
                    *pos,
                    i32::from(output),
                    i32::from(calc_output),
                );
                server.plugin_manager.fire(&server, &mut event).await;
                if event.cancelled {
                    return;
                }
                event.new_current.clamp(0, 15) as u8
            } else {
                calc_output
            };
            let state = self.set_redstone_output(block, state, next_output);
            world
                .set_block_state(pos, state, BlockFlags::NOTIFY_LISTENERS)
                .await;
            world.update_neighbors(pos, None).await;
            world.update_neighbors(&pos.down(), None).await;
            next_output
        };
        let has_output = new_output > 0;
        // Vanilla `checkPressed` (BasePressurePlateBlock.java:108-114): the click sound and
        // BLOCK_ACTIVATE/BLOCK_DEACTIVATE fire only when the pressed boolean flips, so a
        // weighted plate going 5 -> 10 stays silent. The source is the colliding entity on
        // the collision path and none on the scheduled-tick path.
        let was_pressed = output > 0;
        if was_pressed != has_output {
            world.play_block_sound(
                pressure_plate_click_sound(block, has_output),
                SoundCategory::Blocks,
                *pos,
            );
            // Resolve the Arc only here: the collision path runs every tick an entity rests
            // on an unpowered plate, and `get_entity_by_id` is a scan.
            let context = source
                .and_then(|entity| world.get_entity_by_id(entity.get_entity().entity_id))
                .map_or_else(GameEventContext::none, GameEventContext::of_entity);
            emit_game_event(
                world,
                if has_output {
                    GameEvent::BlockActivate
                } else {
                    GameEvent::BlockDeactivate
                },
                Vector3::new(
                    f64::from(pos.0.x) + 0.5,
                    f64::from(pos.0.y) + 0.5,
                    f64::from(pos.0.z) + 0.5,
                ),
                context,
            )
            .await;
        }
        if has_output {
            world.schedule_block_tick(block, *pos, self.tick_rate(), TickPriority::Normal);
        }
    }

    fn can_pressure_plate_place_at(world: &World, block_pos: &BlockPos) -> bool {
        let floor = world.get_block_state(&block_pos.down());
        floor.is_side_solid(BlockDirection::Up)
    }

    fn get_redstone_output(&self, block: &Block, state: BlockStateId) -> u8;

    fn set_redstone_output(&self, block: &Block, state: &BlockState, output: u8) -> BlockStateId;

    async fn calculate_redstone_output(&self, world: &World, block: &Block, pos: &BlockPos) -> u8;

    fn tick_rate(&self) -> u8 {
        20
    }
}
