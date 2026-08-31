use acacia::AcaciaFoliagePlacer;
use blob::BlobFoliagePlacer;
use bush::BushFoliagePlacer;
use cherry::CherryFoliagePlacer;
use dark_oak::DarkOakFoliagePlacer;
use fancy::LargeOakFoliagePlacer;
use jungle::JungleFoliagePlacer;
use mega_pine::MegaPineFoliagePlacer;
use pine::PineFoliagePlacer;
use pumpkin_data::Block;
use pumpkin_data::BlockDirection;
use pumpkin_data::BlockState;
use pumpkin_data::fluid::Fluid;
use pumpkin_util::{
    math::{int_provider::IntProvider, position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};
use random_spread::RandomSpreadFoliagePlacer;

use spruce::SpruceFoliagePlacer;

use super::{TreeFeature, TreeNode};
use crate::generation::proto_chunk::GenerationCache;

pub mod acacia;
pub mod blob;
pub mod bush;
pub mod cherry;
pub mod dark_oak;
pub mod fancy;
pub mod jungle;
pub mod mega_pine;
pub mod pine;
pub mod random_spread;
pub mod spruce;

pub struct FoliagePlacer {
    pub radius: IntProvider,
    pub offset: IntProvider,
    pub r#type: FoliageType,
}

pub trait LeaveValidator {
    fn is_position_invalid(
        &self,
        random: &mut RandomGenerator,
        dx: i32,
        y: i32,
        dz: i32,
        radius: i32,
        giant_trunk: bool,
    ) -> bool {
        let x = if giant_trunk {
            dx.abs().min((dx - 1).abs())
        } else {
            dx.abs()
        };
        let z = if giant_trunk {
            dz.abs().min((dz - 1).abs())
        } else {
            dz.abs()
        };
        self.is_invalid_for_leaves(random, x, y, z, radius, giant_trunk)
    }

    fn is_invalid_for_leaves(
        &self,
        random: &mut RandomGenerator,
        dx: i32,
        y: i32,
        dz: i32,
        radius: i32,
        giant_trunk: bool,
    ) -> bool;
}

impl FoliagePlacer {
    #[expect(clippy::too_many_arguments)]
    pub fn generate_square<T: LeaveValidator, T2: GenerationCache>(
        foliage_positions: &mut Vec<BlockPos>,
        validator: &T,
        chunk: &mut T2,
        random: &mut RandomGenerator,
        center_pos: BlockPos,
        radius: i32,
        y: i32,
        giant_trunk: bool,
        foliage_provider: &BlockState,
    ) {
        let i = i32::from(giant_trunk);

        for x in -radius..=(radius + i) {
            for z in -radius..=(radius + i) {
                if validator.is_position_invalid(random, x, y, z, radius, giant_trunk) {
                    continue;
                }
                let pos = BlockPos(center_pos.0.add(&Vector3::new(x, y, z)));
                if Self::place_foliage_block(chunk, pos, foliage_provider) {
                    foliage_positions.push(pos);
                }
            }
        }
    }

    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        node: &TreeNode,
        foliage_height: i32,
        radius: i32,
        foliage_provider: &BlockState,
    ) -> Vec<BlockPos> {
        let offset = self.offset.get(random);
        self.r#type.generate(
            chunk,
            random,
            node,
            foliage_height,
            radius,
            offset,
            foliage_provider,
        )
    }

    pub fn get_random_radius(&self, random: &mut RandomGenerator, base_height: i32) -> i32 {
        match &self.r#type {
            FoliageType::Pine(_) => PineFoliagePlacer::get_random_radius(self, random, base_height),
            _ => self.radius.get(random),
        }
    }

    pub fn place_foliage_block<T: GenerationCache>(
        chunk: &mut T,
        pos: BlockPos,
        block_state: &BlockState,
    ) -> bool {
        let existing = GenerationCache::get_block_state(chunk, &pos.0);
        if existing
            .to_block()
            .properties(existing)
            .is_some_and(|props| {
                props
                    .to_props()
                    .iter()
                    .any(|(key, value)| *key == "persistent" && *value == "true")
            })
            || !TreeFeature::can_replace(existing.to_state(), existing.to_block_id())
        {
            return false;
        }

        // Vanilla `FoliagePlacer.tryPlaceLeaf` (`FoliagePlacer.java:173-183`) sets
        // `waterlogged` to whether the target contains a water source.
        let (fluid, fluid_state) = GenerationCache::get_fluid_and_fluid_state(chunk, &pos.0);
        let foliage_state = Self::set_waterlogged(
            block_state,
            fluid_state.is_source && fluid.matches_type(&Fluid::WATER),
        );
        chunk.set_block_state(&pos.0, foliage_state);
        true
    }

    // Mirrors `FoliagePlacer.tryPlaceLeaf` (`FoliagePlacer.java:175-179`), including
    // clearing a provider state that was already waterlogged outside a water source.
    fn set_waterlogged(block_state: &BlockState, waterlogged: bool) -> &BlockState {
        let block = Block::from_state_id(block_state.id);
        let Some(properties) = block.properties(block_state.id) else {
            return block_state;
        };
        let mut properties = properties.to_props();
        let Some(index) = properties.iter().position(|(key, _)| *key == "waterlogged") else {
            return block_state;
        };
        properties[index] = ("waterlogged", if waterlogged { "true" } else { "false" });
        BlockState::from_id(block.from_properties(&properties).to_state_id(block))
    }

    pub fn is_set(foliage_positions: &[BlockPos], pos: BlockPos) -> bool {
        // Vanilla `FoliageSetter.isSet` (`TreeFeature.java:137-146`) checks the
        // positions written by this tree, not the block state currently in the chunk.
        foliage_positions.contains(&pos)
    }

    fn try_place_extension<T: GenerationCache>(
        foliage_positions: &mut Vec<BlockPos>,
        chunk: &mut T,
        random: &mut RandomGenerator,
        chance: f32,
        log_pos: BlockPos,
        pos: BlockPos,
        foliage_provider: &BlockState,
    ) -> bool {
        if pos.manhattan_distance(log_pos) >= 7 || random.next_f32() > chance {
            false
        } else {
            let placed = Self::place_foliage_block(chunk, pos, foliage_provider);
            if placed {
                foliage_positions.push(pos);
            }
            placed
        }
    }

    #[expect(clippy::too_many_arguments)]
    pub fn generate_square_with_hanging_leaves<T: LeaveValidator, T2: GenerationCache>(
        foliage_positions: &mut Vec<BlockPos>,
        validator: &T,
        chunk: &mut T2,
        random: &mut RandomGenerator,
        center_pos: BlockPos,
        radius: i32,
        y: i32,
        giant_trunk: bool,
        foliage_provider: &BlockState,
        hanging_leaves_chance: f32,
        hanging_leaves_extension_chance: f32,
    ) {
        Self::generate_square(
            foliage_positions,
            validator,
            chunk,
            random,
            center_pos,
            radius,
            y,
            giant_trunk,
            foliage_provider,
        );

        let i = i32::from(giant_trunk);
        let log_pos = center_pos.down();

        let directions = [
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::East,
            BlockDirection::West,
        ];

        for along_edge in directions {
            let to_edge = along_edge.rotate_clockwise();

            let offset_to_edge = if to_edge.positive() {
                radius + i
            } else {
                radius
            };

            let mut pos = center_pos
                .add(0, y - 1, 0)
                .offset_dir(to_edge.to_offset(), offset_to_edge)
                .offset_dir(along_edge.to_offset(), -radius);

            for _ in -radius..(radius + i) {
                let leaves_above = Self::is_set(foliage_positions, pos.up());
                if leaves_above
                    && Self::try_place_extension(
                        foliage_positions,
                        chunk,
                        random,
                        hanging_leaves_chance,
                        log_pos,
                        pos,
                        foliage_provider,
                    )
                {
                    Self::try_place_extension(
                        foliage_positions,
                        chunk,
                        random,
                        hanging_leaves_extension_chance,
                        log_pos,
                        pos.down(),
                        foliage_provider,
                    );
                }
                pos = pos.offset_dir(along_edge.to_offset(), 1);
            }
        }
    }
}

#[cfg(test)]
mod is_set_tests {
    use pumpkin_util::math::position::BlockPos;

    use super::FoliagePlacer;

    #[test]
    fn is_set_only_matches_positions_written_by_this_tree() {
        // `FoliageSetter.isSet` (`TreeFeature.java:137-146`) reads the current
        // tree's foliage set, independently of the block state stored in the level.
        let written = [BlockPos::new(3, 8, -2)];

        assert!(FoliagePlacer::is_set(&written, BlockPos::new(3, 8, -2)));
        assert!(!FoliagePlacer::is_set(&written, BlockPos::new(3, 8, -1)));
    }
}

pub enum FoliageType {
    Blob(BlobFoliagePlacer),
    Spruce(SpruceFoliagePlacer),
    Pine(PineFoliagePlacer),
    Acacia(AcaciaFoliagePlacer),
    Bush(BushFoliagePlacer),
    Fancy(LargeOakFoliagePlacer),
    Jungle(JungleFoliagePlacer),
    MegaPine(MegaPineFoliagePlacer),
    DarkOak(DarkOakFoliagePlacer),
    RandomSpread(RandomSpreadFoliagePlacer),
    Cherry(CherryFoliagePlacer),
}

impl FoliageType {
    #[expect(clippy::too_many_arguments)]
    #[expect(clippy::too_many_lines)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        node: &TreeNode,
        foliage_height: i32,
        radius: i32,
        offset: i32,
        foliage_provider: &BlockState,
    ) -> Vec<BlockPos> {
        match self {
            Self::Blob(blob) => blob.generate(
                chunk,
                random,
                node,
                foliage_height,
                radius,
                offset,
                foliage_provider,
            ),
            Self::Spruce(spruce) => spruce.generate(
                chunk,
                random,
                node,
                foliage_height,
                radius,
                offset,
                foliage_provider,
            ),
            Self::Pine(pine) => pine.generate(
                chunk,
                random,
                node,
                foliage_height,
                radius,
                offset,
                foliage_provider,
            ),
            Self::Acacia(acacia) => acacia.generate(
                chunk,
                random,
                node,
                foliage_height,
                radius,
                offset,
                foliage_provider,
            ),
            Self::Bush(bush) => bush.generate(
                chunk,
                random,
                node,
                foliage_height,
                radius,
                offset,
                foliage_provider,
            ),
            Self::Fancy(fancy) => fancy.generate(
                chunk,
                random,
                node,
                foliage_height,
                radius,
                offset,
                foliage_provider,
            ),
            Self::Jungle(jungle) => jungle.generate(
                chunk,
                random,
                node,
                foliage_height,
                radius,
                offset,
                foliage_provider,
            ),
            Self::MegaPine(mega_pine) => mega_pine.generate(
                chunk,
                random,
                node,
                foliage_height,
                radius,
                offset,
                foliage_provider,
            ),
            Self::DarkOak(dark_oak) => dark_oak.generate(
                chunk,
                random,
                node,
                foliage_height,
                radius,
                offset,
                foliage_provider,
            ),
            Self::RandomSpread(random_spread) => random_spread.generate(
                chunk,
                random,
                node,
                foliage_height,
                radius,
                offset,
                foliage_provider,
            ),
            Self::Cherry(cherry) => cherry.generate(
                chunk,
                random,
                node,
                foliage_height,
                radius,
                offset,
                foliage_provider,
            ),
        }
    }

    pub fn get_random_height(&self, random: &mut RandomGenerator, trunk_height: i32) -> i32 {
        match self {
            Self::Blob(blob) => blob.get_random_height(random),
            Self::Spruce(spruce) => spruce.get_random_height(random, trunk_height),
            Self::Pine(pine) => pine.get_random_height(random, trunk_height),
            Self::Acacia(_acacia) => AcaciaFoliagePlacer::get_random_height(random),
            Self::Bush(bush) => bush.get_random_height(random),
            Self::Fancy(fancy) => fancy.get_random_height(random),
            Self::Jungle(jungle) => jungle.get_random_height(random, trunk_height),
            Self::MegaPine(mega_pine) => mega_pine.get_random_height(random, trunk_height),
            Self::DarkOak(_dark_oak) => DarkOakFoliagePlacer::get_random_height(random),
            Self::RandomSpread(random_spread) => {
                random_spread.get_random_height(random, trunk_height)
            }
            Self::Cherry(cherry) => cherry.get_random_height(random),
        }
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::Block;

    use super::FoliagePlacer;

    #[test]
    fn try_place_leaf_matches_vanilla_waterlogged_selection() {
        // Vanilla `FoliagePlacer.tryPlaceLeaf` (`FoliagePlacer.java:175-179`) writes both
        // outcomes of the water-source predicate to a waterloggable foliage state.
        let wet = FoliagePlacer::set_waterlogged(Block::OAK_LEAVES.default_state, true);
        let dry = FoliagePlacer::set_waterlogged(wet, false);
        assert!(!dry.is_waterlogged());
        assert!(wet.is_waterlogged());
    }
}
