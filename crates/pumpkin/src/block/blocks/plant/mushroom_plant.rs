use std::sync::Arc;

use pumpkin_data::configured_feature::ConfiguredFeature;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockId, BlockStateId, tag};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::xoroshiro128::Xoroshiro;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::RngExt;

use crate::block::{
    BlockBehaviour, BlockMetadata, BonemealArgs, CanPlaceAtArgs, GetStateForNeighborUpdateArgs,
    RandomTickArgs, blocks::plant::PlantBlockBase,
};
use crate::plugin::api::events::world::structure_grow::{StructureGrowEvent, TreeType};
use crate::world::World;
use crate::world::feature_placer::FeatureCache;

pub struct MushroomPlantBlock;

impl BlockMetadata for MushroomPlantBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::BROWN_MUSHROOM, BlockId::RED_MUSHROOM].into()
    }
}

/// `MushroomBlock.canSpread` (`MushroomBlock.java`): at most four blocks of the same mushroom
/// (the spreading one included) inside the `(-4, -1, -4)..=(4, 1, 4)` box.
const MAX_NEARBY_MUSHROOMS: i32 = 4;

/// The huge mushroom feature each mushroom grows into (`Blocks.java` registers
/// `BROWN_MUSHROOM` with `TreeFeatures.HUGE_BROWN_MUSHROOM`, `RED_MUSHROOM` with
/// `TreeFeatures.HUGE_RED_MUSHROOM`) and that feature's configured `foliage_radius`
/// (`TreeFeatures.java`: 3 for brown, 2 for red).
fn huge_mushroom_feature(block: &Block) -> Option<(ConfiguredFeature, i32)> {
    if block == &Block::BROWN_MUSHROOM {
        Some((ConfiguredFeature::HugeBrownMushroom, 3))
    } else if block == &Block::RED_MUSHROOM {
        Some((ConfiguredFeature::HugeRedMushroom, 2))
    } else {
        None
    }
}

impl MushroomPlantBlock {
    /// `MushroomBlock.canSurvive`, as the shared `PlantBlockBase` check.
    pub fn can_survive(block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        <Self as PlantBlockBase>::can_place_at(&MushroomPlantBlock, block_accessor, pos)
    }

    /// `MushroomBlock.growMushroom`: remove the mushroom, run the huge mushroom feature, and put
    /// the mushroom back if the feature declines. Both the removal and the feature are buffered in
    /// one [`FeatureCache`], so a declined placement simply never commits and the mushroom stays.
    pub fn grow_mushroom(
        world: &Arc<World>,
        pos: &BlockPos,
        block: &Block,
        _state_id: BlockStateId,
    ) -> bool {
        let Some((feature, _)) = huge_mushroom_feature(block) else {
            return false;
        };

        let species = if block == &Block::BROWN_MUSHROOM {
            TreeType::BrownMushroom
        } else {
            TreeType::RedMushroom
        };
        let mut event = StructureGrowEvent::new(*pos, species, true);
        if let Some(server) = world.server.upgrade() {
            server.plugin_manager.fire_blocking(&server, &mut event);
        }
        if event.cancelled {
            return false;
        }

        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::rng().random()));
        let mut cache = FeatureCache::new(world);
        cache.set_block(*pos, Block::AIR.default_state.id);
        if cache.place(feature, *pos, &mut random) {
            cache.commit()
        } else {
            false
        }
    }

    /// `MushroomBlock.canSpread`.
    fn can_spread(world: &World, pos: &BlockPos, this_block: &Block) -> bool {
        let mut found = 0;
        for dx in -4..=4 {
            for dy in -1..=1 {
                for dz in -4..=4 {
                    let check_pos = pos.add(dx, dy, dz);
                    if world.is_loaded(&check_pos) && world.get_block(&check_pos) == this_block {
                        found += 1;
                        if found > MAX_NEARBY_MUSHROOMS {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    fn random_spread_offset(pos: BlockPos) -> BlockPos {
        let mut rng = rand::rng();
        let dx = rng.random_range(0..3) - 1;
        let dy = rng.random_range(0..2) - rng.random_range(0..2);
        let dz = rng.random_range(0..3) - 1;
        pos.add(dx, dy, dz)
    }
}

impl BlockBehaviour for MushroomPlantBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        <Self as PlantBlockBase>::can_place_at(self, args.block_accessor, args.position)
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        <Self as PlantBlockBase>::get_state_for_neighbor_update(
            self,
            args.world,
            args.position,
            args.state_id,
        )
    }

    /// `MushroomBlock.randomTick`: a 1-in-25 chance to spread to a nearby spot, taking up to
    /// four random steps through empty positions the mushroom could survive on.
    fn random_tick(&self, args: RandomTickArgs<'_>) {
        if rand::rng().random_range(0..25) != 0 {
            return;
        }
        let world = args.world;
        let mut pos = *args.position;
        if !Self::can_spread(world, &pos, args.block) {
            return;
        }
        let state_id = world.get_block_state_id(&pos);

        let can_spread_to = |target: &BlockPos| {
            world.is_loaded(target)
                && world.get_block_state(target).is_air()
                && Self::can_survive(world.as_ref(), target)
        };

        let mut offset = Self::random_spread_offset(pos);
        for _ in 0..4 {
            if can_spread_to(&offset) {
                pos = offset;
            }
            offset = Self::random_spread_offset(pos);
        }

        if can_spread_to(&offset) {
            // `level.setBlock(offset, state, 2)`.
            world.set_block_state(&offset, state_id, BlockFlags::NOTIFY_LISTENERS);
        }
    }

    /// `MushroomBlock.isValidBonemealTarget`: the position `4 + foliageRadius` above must be
    /// inside the build height.
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        let Some((_, foliage_radius)) = huge_mushroom_feature(args.block) else {
            return false;
        };
        args.world
            .is_in_height_limit(args.position.0.y + 4 + foliage_radius)
    }

    /// `MushroomBlock.isBonemealSuccess`.
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        rand::rng().random::<f32>() < 0.4
    }

    /// `MushroomBlock.performBonemeal`.
    fn perform_bonemeal(&self, args: BonemealArgs<'_>) {
        Self::grow_mushroom(args.world, args.position, args.block, args.state_id);
    }
}

/// `MushroomBlock.MAX_LIGHT` gate: `canSurvive` needs raw brightness *strictly below* 13
/// (`MushroomBlock.java:86`). Note the direction - mushrooms want darkness, crops want light,
/// which is why an accessor with no light engine must skip the gate rather than assume a value.
const MAX_SURVIVE_LIGHT: u8 = 13;

impl PlantBlockBase for MushroomPlantBlock {
    /// `MushroomBlock.canSurvive` (`MushroomBlock.java:83-87`): a block tagged
    /// `overrides_mushroom_light_requirement` below always works; otherwise the light must be
    /// below 13 and the block below must be `mayPlaceOn`, i.e. `isSolidRender`
    /// (`MushroomBlock.java:78-80`).
    fn can_place_at(&self, block_accessor: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
        let below_pos = block_pos.down();
        let (below, below_state) = block_accessor.get_block_and_state(&below_pos);
        if below.has_tag(&tag::Block::MINECRAFT_OVERRIDES_MUSHROOM_LIGHT_REQUIREMENT) {
            return true;
        }
        let dark_enough = block_accessor
            .get_raw_brightness(block_pos, 0)
            .is_none_or(|light| light < MAX_SURVIVE_LIGHT);
        dark_enough && below_state.is_solid_render()
    }
}
