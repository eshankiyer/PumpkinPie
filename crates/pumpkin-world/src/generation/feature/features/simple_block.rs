use pumpkin_data::{
    Block, BlockState,
    block_properties::{DoubleBlockHalf, EnumVariants},
    fluid::Fluid,
};
use pumpkin_util::{math::position::BlockPos, random::RandomGenerator};

use crate::generation::proto_chunk::GenerationCache;
use crate::{
    generation::block_state_provider::BlockStateProvider,
    world::{BlockAccessor, WorldPortalExt},
};

pub struct SimpleBlockFeature {
    pub to_place: BlockStateProvider,
    pub schedule_tick: Option<bool>,
}

impl SimpleBlockFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        block_registry: &dyn WorldPortalExt,
        chunk: &mut T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let state = self.to_place.get(random, pos, chunk, block_registry);
        let block = Block::from_state_id(state.id);
        let block_accessor: &dyn BlockAccessor = chunk;
        if !block_registry.can_place_at(block, state, block_accessor, &pos) {
            return false;
        }

        if is_double_plant(block) {
            let upper_pos = pos.up();
            if !GenerationCache::get_block_state(chunk, &upper_pos.0)
                .to_state()
                .is_air()
            {
                return false;
            }

            // `DoublePlantBlock.placeAt`: the regular placement callback is not
            // invoked during worldgen, so normalize and write both halves here.
            let lower_state = double_plant_state(
                block,
                state,
                DoubleBlockHalf::Lower,
                is_water_at(chunk, &pos),
            );
            let upper_state = double_plant_state(
                block,
                state,
                DoubleBlockHalf::Upper,
                is_water_at(chunk, &upper_pos),
            );
            chunk.set_block_state(&pos.0, lower_state);
            chunk.set_block_state(&upper_pos.0, upper_state);
        } else {
            chunk.set_block_state(&pos.0, state);
        }
        // TODO: schedule tick when needed
        true
    }
}

fn is_double_plant(block: &Block) -> bool {
    block.id == Block::TALL_GRASS.id
        || block.id == Block::LARGE_FERN.id
        || block.id == Block::TALL_SEAGRASS.id
        || block.id == Block::SMALL_DRIPLEAF.id
        || block.id == Block::PITCHER_CROP.id
        || block.id == Block::PITCHER_PLANT.id
        || block.id == Block::SUNFLOWER.id
        || block.id == Block::LILAC.id
        || block.id == Block::PEONY.id
        || block.id == Block::ROSE_BUSH.id
}

fn double_plant_state(
    block: &Block,
    state: &'static BlockState,
    half: DoubleBlockHalf,
    waterlogged: bool,
) -> &'static BlockState {
    let props = block
        .properties(state.id)
        .expect("all DoublePlantBlock states have a half property")
        .to_props();
    let props = props
        .into_iter()
        .map(|(name, value)| match name {
            "half" => (name, half.to_value()),
            "waterlogged" => (name, if waterlogged { "true" } else { "false" }),
            _ => (name, value),
        })
        .collect::<Vec<_>>();
    BlockState::from_id(block.from_properties(&props).to_state_id(block))
}

fn is_water_at<T: GenerationCache>(chunk: &T, pos: &BlockPos) -> bool {
    let fluid = GenerationCache::get_fluid_and_fluid_state(chunk, &pos.0).0;
    let state = GenerationCache::get_block_state(chunk, &pos.0).to_state();
    let block = Block::from_state_id(state.id);
    fluid == Fluid::WATER
        || fluid == Fluid::FLOWING_WATER
        || state.is_waterlogged()
        || block.id == Block::WATER.id
        || block.id == Block::BUBBLE_COLUMN.id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProtoChunk;
    use crate::generation::generator::{GeneratorInit, VanillaGenerator, WorldGenerator};
    use pumpkin_data::{Mirror, Rotation, dimension::Dimension};
    use pumpkin_util::random::legacy_rand::LegacyRand;
    use pumpkin_util::world_seed::Seed;

    struct AllowsPlacement;

    impl WorldPortalExt for AllowsPlacement {
        fn can_place_at(
            &self,
            _block: &Block,
            _state: &BlockState,
            _block_accessor: &dyn BlockAccessor,
            _block_pos: &BlockPos,
        ) -> bool {
            true
        }

        fn mirror(
            &self,
            block: &Block,
            state_id: pumpkin_data::BlockStateId,
            mirror: Mirror,
        ) -> &'static BlockState {
            block.mirror(state_id, mirror)
        }

        fn rotate(
            &self,
            block: &Block,
            state_id: pumpkin_data::BlockStateId,
            rotation: Rotation,
        ) -> &'static BlockState {
            block.rotate(state_id, rotation)
        }

        fn spawn_mobs_for_chunk_generation(
            &self,
            _cache: &mut dyn GenerationCache,
            _biome: &'static pumpkin_data::chunk::Biome,
            _chunk_x: i32,
            _chunk_z: i32,
        ) {
        }
    }

    #[test]
    fn places_the_upper_half_of_a_tall_plant() {
        let world_gen = WorldGenerator::Noise(Box::new(VanillaGenerator::new(
            Seed(1),
            Dimension::OVERWORLD,
        )));
        let WorldGenerator::Noise(generator) = &world_gen else {
            unreachable!()
        };
        let mut chunk = ProtoChunk::new(0, 0, &world_gen);
        let mut random =
            RandomGenerator::Legacy(LegacyRand::from_seed(generator.random_config.seed));
        let feature = SimpleBlockFeature {
            to_place: BlockStateProvider::Simple(
                crate::generation::block_state_provider::SimpleStateProvider {
                    state: Block::SUNFLOWER.default_state,
                },
            ),
            schedule_tick: None,
        };
        let pos = BlockPos::new(4, 64, 4);

        assert!(feature.generate(&AllowsPlacement, &mut chunk, &mut random, pos));
        assert_eq!(
            GenerationCache::get_block_state(&chunk, &pos.0),
            Block::SUNFLOWER.default_state.id
        );

        assert_eq!(
            GenerationCache::get_block_state(&chunk, &pos.up().0),
            double_plant_state(
                &Block::SUNFLOWER,
                Block::SUNFLOWER.default_state,
                DoubleBlockHalf::Upper,
                false,
            )
            .id
        );
    }

    #[test]
    fn does_not_replace_an_occupied_upper_block() {
        let world_gen = WorldGenerator::Noise(Box::new(VanillaGenerator::new(
            Seed(1),
            Dimension::OVERWORLD,
        )));
        let WorldGenerator::Noise(generator) = &world_gen else {
            unreachable!()
        };
        let mut chunk = ProtoChunk::new(0, 0, &world_gen);
        let mut random =
            RandomGenerator::Legacy(LegacyRand::from_seed(generator.random_config.seed));
        let feature = SimpleBlockFeature {
            to_place: BlockStateProvider::Simple(
                crate::generation::block_state_provider::SimpleStateProvider {
                    state: Block::SUNFLOWER.default_state,
                },
            ),
            schedule_tick: None,
        };
        let pos = BlockPos::new(4, 64, 4);
        GenerationCache::set_block_state(&mut chunk, &pos.up().0, Block::STONE.default_state);

        assert!(!feature.generate(&AllowsPlacement, &mut chunk, &mut random, pos));
        assert_eq!(
            GenerationCache::get_block_state(&chunk, &pos.0),
            Block::AIR.default_state.id
        );
        assert_eq!(
            GenerationCache::get_block_state(&chunk, &pos.up().0),
            Block::STONE.default_state.id
        );
    }

    #[test]
    fn copies_waterlogged_state_for_each_half() {
        let lower = double_plant_state(
            &Block::SMALL_DRIPLEAF,
            Block::SMALL_DRIPLEAF.default_state,
            DoubleBlockHalf::Lower,
            true,
        );
        let upper = double_plant_state(
            &Block::SMALL_DRIPLEAF,
            Block::SMALL_DRIPLEAF.default_state,
            DoubleBlockHalf::Upper,
            false,
        );

        for (state, half, waterlogged) in [(lower, "lower", "true"), (upper, "upper", "false")] {
            let props = Block::SMALL_DRIPLEAF
                .properties(state.id)
                .unwrap()
                .to_props();
            assert!(props.contains(&("half", half)));
            assert!(props.contains(&("waterlogged", waterlogged)));
        }
    }

    #[test]
    fn recognizes_bubble_columns_when_the_proto_fluid_accessor_is_empty() {
        let world_gen = WorldGenerator::Noise(Box::new(VanillaGenerator::new(
            Seed(1),
            Dimension::OVERWORLD,
        )));
        let mut chunk = ProtoChunk::new(0, 0, &world_gen);
        let pos = BlockPos::new(4, 64, 4);
        GenerationCache::set_block_state(
            &mut chunk,
            &pos.0,
            Block::BUBBLE_COLUMN.states.last().unwrap(),
        );

        assert!(is_water_at(&chunk, &pos));
    }
}
