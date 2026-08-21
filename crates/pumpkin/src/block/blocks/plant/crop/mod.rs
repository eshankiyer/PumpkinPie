use std::sync::Arc;

use pumpkin_data::{
    Block,
    BlockDirection::{East, North, South, West},
    BlockStateId,
    block_properties::{BlockProperties, FarmlandLikeProperties, WheatLikeProperties},
};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::RngExt;

use crate::{
    block::blocks::plant::PlantBlockBase, plugin::api::events::block::block_grow::BlockGrowEvent,
    world::World,
};

type CropProperties = WheatLikeProperties;
type FarmlandProperties = FarmlandLikeProperties;

pub mod beetroot;
pub mod carrot;
pub mod gourds;
pub mod nether_wart;
pub mod pitcher_crop;
pub mod potatoes;
pub mod sweet_berry_bush;
pub mod torch_flower;
pub mod wheat;

trait CropBlockBase: PlantBlockBase {
    fn can_plant_on_top(&self, block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        let block = block_accessor.get_block(pos);
        block == &Block::FARMLAND
    }

    /// `CropBlock.canSurvive` (`CropBlock.java:145-147`):
    /// `hasSufficientLight(level, pos) && super.canSurvive(...)`.
    ///
    /// Wired in by each crop block's `PlantBlockBase::can_place_at` override, so both placement
    /// and the neighbour-update drop path go through it.
    fn crop_can_survive(&self, block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        has_sufficient_light(block_accessor, pos)
            && <Self as CropBlockBase>::can_plant_on_top(self, block_accessor, &pos.down())
    }

    fn max_age(&self) -> i32 {
        7
    }

    fn get_age(&self, state: BlockStateId, block: &Block) -> i32 {
        let props = CropProperties::from_state_id(state, block);
        i32::from(props.age)
    }

    fn state_with_age(&self, block: &Block, state: BlockStateId, age: i32) -> BlockStateId {
        let mut props = CropProperties::from_state_id(state, block);
        props.age = age as u8;
        props.to_state_id(block)
    }

    fn bonemeal_age_increase(&self) -> i32 {
        rand::rng().random_range(2..=5)
    }

    fn is_valid_bonemeal_target(&self, world: &World, pos: &BlockPos) -> bool {
        let (block, state) = world.get_block_and_state_id(pos);
        self.get_age(state, block) < self.max_age()
    }

    async fn perform_bonemeal(&self, world: &Arc<World>, pos: &BlockPos) {
        let (block, state) = world.get_block_and_state_id(pos);
        let age = self.get_age(state, block);
        let new_age = (age + self.bonemeal_age_increase()).min(self.max_age());
        world
            .set_block_state(
                pos,
                self.state_with_age(block, state, new_age),
                BlockFlags::NOTIFY_LISTENERS,
            )
            .await;
    }

    async fn random_tick(&self, world: &Arc<World>, pos: &BlockPos) {
        if world.get_raw_brightness(pos, 0) < MIN_GROWTH_LIGHT {
            return;
        }
        let (block, state) = world.get_block_and_state_id(pos);
        let age = self.get_age(state, block);
        if age < self.max_age() {
            let f = get_available_moisture(world, pos, block).await;
            if rand::rng().random_range(0..=(25.0 / f).floor() as i64) == 0 {
                let mut new_state_id = self.state_with_age(block, state, age + 1);
                if let Some(server) = world.server.upgrade() {
                    let mut event = BlockGrowEvent::new(
                        world.clone(),
                        block,
                        state,
                        Block::from_state_id(new_state_id),
                        new_state_id,
                        *pos,
                    );
                    server.plugin_manager.fire(&server, &mut event).await;
                    if event.cancelled {
                        return;
                    }
                    new_state_id = event.new_state_id;
                }
                world
                    .set_block_state(pos, new_state_id, BlockFlags::NOTIFY_NEIGHBORS)
                    .await;
            }
        }
    }
}

/// `CropBlock.hasSufficientLight` (`CropBlock.java:149-151`): raw brightness at least
/// [`MIN_SURVIVE_LIGHT`].
///
/// An accessor with no light engine behind it (worldgen) reports `None`; the gate is then
/// skipped, matching the previous unconditional behaviour rather than uprooting crops during
/// chunk generation.
pub fn has_sufficient_light(block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
    block_accessor
        .get_raw_brightness(pos, 0)
        .is_none_or(|light| light >= MIN_SURVIVE_LIGHT)
}

/// Raw brightness a crop needs to *stay planted*, `CropBlock.hasSufficientLight`
/// (`CropBlock.java:149-151`). One less than [`MIN_GROWTH_LIGHT`]: a crop at exactly 8 survives
/// but never advances.
pub const MIN_SURVIVE_LIGHT: u8 = 8;

/// Raw brightness a crop or stem needs to advance a growth stage.
///
/// Vanilla checks `getRawBrightness(pos, 0)`, so the sky light is not reduced
/// by the time of day and crops keep growing at night.
pub const MIN_GROWTH_LIGHT: u8 = 9;

pub async fn get_available_moisture(world: &Arc<World>, pos: &BlockPos, block: &Block) -> f32 {
    let mut moisture = 1.0;
    let down_pos = pos.down();

    for dx in -1..=1 {
        for dz in -1..=1 {
            let mut local_moisture = 0.0;

            let (block, block_state) =
                world.get_block_and_state_id(&down_pos.offset(Vector3 { x: dx, y: 0, z: dz }));
            if block == &Block::FARMLAND {
                local_moisture = 1.0;
                let props = FarmlandProperties::from_state_id(block_state, block);
                if props.moisture != 0 {
                    local_moisture = 3.0;
                }
            }

            if dx != 0 || dz != 0 {
                local_moisture /= 4.0;
            }

            moisture += local_moisture;
        }
    }

    let north = pos.offset(North.to_offset());
    let south = pos.offset(South.to_offset());
    let west = pos.offset(West.to_offset());
    let east = pos.offset(East.to_offset());
    let horizontal = world.get_block(&west) == block || world.get_block(&east) == block;
    let vertical = world.get_block(&north) == block || world.get_block(&south) == block;
    if (horizontal && vertical)
        || world.get_block(&west.offset(North.to_offset())) == block
        || world.get_block(&east.offset(North.to_offset())) == block
        || world.get_block(&east.offset(South.to_offset())) == block
        || world.get_block(&west.offset(South.to_offset())) == block
    {
        moisture /= 2.0;
    }

    moisture
}
