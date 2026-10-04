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
use rustc_hash::FxHashSet;

use spruce::SpruceFoliagePlacer;

use super::{TreeFeature, TreeNode};
use crate::generation::block_state_provider::BlockStateProvider;
use crate::generation::proto_chunk::GenerationCache;
use crate::world::WorldPortalExt;

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

/// Vanilla `FoliagePlacer.FoliageSetter` as implemented in `TreeFeature.place`
/// (`TreeFeature.java:127,137-148`): one set of foliage positions shared by every
/// attachment of a tree, plus the tree's foliage provider sampled per leaf.
pub struct FoliageSetter<'a> {
    provider: &'a BlockStateProvider,
    block_registry: &'a dyn WorldPortalExt,
    /// First-insertion order; re-adding a position keeps its slot, like `HashSet.add`.
    order: Vec<BlockPos>,
    seen: FxHashSet<BlockPos>,
}

impl<'a> FoliageSetter<'a> {
    pub fn new(provider: &'a BlockStateProvider, block_registry: &'a dyn WorldPortalExt) -> Self {
        Self {
            provider,
            block_registry,
            order: Vec::new(),
            seen: FxHashSet::default(),
        }
    }

    /// Vanilla `FoliageSetter.isSet` (`TreeFeature.java:143-146`) checks the positions
    /// written by this tree, not the block state currently in the level.
    #[must_use]
    pub fn is_set(&self, pos: BlockPos) -> bool {
        self.seen.contains(&pos)
    }

    fn record(&mut self, pos: BlockPos) {
        if self.seen.insert(pos) {
            self.order.push(pos);
        }
    }

    /// The tree's deduplicated foliage positions, handed to the tree decorators.
    #[must_use]
    pub fn into_positions(self) -> Vec<BlockPos> {
        self.order
    }
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
    /// `Direction.Plane.HORIZONTAL` (`Direction.java:577`), the edge order of
    /// `placeLeavesRowWithHangingLeavesBelow` (`FoliagePlacer.java:133`). Each edge can draw
    /// RNG, so the order decides which perimeter position consumes which value.
    const HANGING_LEAVES_EDGE_ORDER: [BlockDirection; 4] = [
        BlockDirection::North,
        BlockDirection::East,
        BlockDirection::South,
        BlockDirection::West,
    ];

    #[expect(clippy::too_many_arguments)]
    pub fn generate_square<T: LeaveValidator, T2: GenerationCache>(
        validator: &T,
        chunk: &mut T2,
        random: &mut RandomGenerator,
        center_pos: BlockPos,
        radius: i32,
        y: i32,
        giant_trunk: bool,
        setter: &mut FoliageSetter<'_>,
    ) {
        let i = i32::from(giant_trunk);

        for x in -radius..=(radius + i) {
            for z in -radius..=(radius + i) {
                if validator.is_position_invalid(random, x, y, z, radius, giant_trunk) {
                    continue;
                }
                let pos = BlockPos(center_pos.0.add(&Vector3::new(x, y, z)));
                Self::place_foliage_block(chunk, random, setter, pos);
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
        setter: &mut FoliageSetter<'_>,
    ) {
        let offset = self.offset.get(random);
        self.r#type
            .generate(chunk, random, node, foliage_height, radius, offset, setter);
    }

    pub fn get_random_radius(&self, random: &mut RandomGenerator, base_height: i32) -> i32 {
        match &self.r#type {
            FoliageType::Pine(_) => PineFoliagePlacer::get_random_radius(self, random, base_height),
            _ => self.radius.get(random),
        }
    }

    /// Vanilla `FoliagePlacer.tryPlaceLeaf` (`FoliagePlacer.java:170-187`).
    pub fn place_foliage_block<T: GenerationCache>(
        chunk: &mut T,
        random: &mut RandomGenerator,
        setter: &mut FoliageSetter<'_>,
        pos: BlockPos,
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

        // The provider is sampled per leaf, and only once both checks above passed.
        let block_state = setter
            .provider
            .get(random, pos, &*chunk, setter.block_registry);
        // Vanilla `FoliagePlacer.tryPlaceLeaf` (`FoliagePlacer.java:173-183`) sets
        // `waterlogged` to whether the target contains a water source.
        let (fluid, fluid_state) = GenerationCache::get_fluid_and_fluid_state(chunk, &pos.0);
        let foliage_state = Self::set_waterlogged(
            block_state,
            fluid_state.is_source && fluid.matches_type(&Fluid::WATER),
        );
        chunk.set_block_state(&pos.0, foliage_state);
        setter.record(pos);
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

    fn try_place_extension<T: GenerationCache>(
        chunk: &mut T,
        random: &mut RandomGenerator,
        setter: &mut FoliageSetter<'_>,
        chance: f32,
        log_pos: BlockPos,
        pos: BlockPos,
    ) -> bool {
        if pos.manhattan_distance(log_pos) >= 7 || random.next_f32() > chance {
            false
        } else {
            Self::place_foliage_block(chunk, random, setter, pos)
        }
    }

    #[expect(clippy::too_many_arguments)]
    pub fn generate_square_with_hanging_leaves<T: LeaveValidator, T2: GenerationCache>(
        validator: &T,
        chunk: &mut T2,
        random: &mut RandomGenerator,
        center_pos: BlockPos,
        radius: i32,
        y: i32,
        giant_trunk: bool,
        setter: &mut FoliageSetter<'_>,
        hanging_leaves_chance: f32,
        hanging_leaves_extension_chance: f32,
    ) {
        Self::generate_square(
            validator,
            chunk,
            random,
            center_pos,
            radius,
            y,
            giant_trunk,
            setter,
        );

        let i = i32::from(giant_trunk);
        let log_pos = center_pos.down();

        for along_edge in Self::HANGING_LEAVES_EDGE_ORDER {
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
                let leaves_above = setter.is_set(pos.up());
                if leaves_above
                    && Self::try_place_extension(
                        chunk,
                        random,
                        setter,
                        hanging_leaves_chance,
                        log_pos,
                        pos,
                    )
                {
                    Self::try_place_extension(
                        chunk,
                        random,
                        setter,
                        hanging_leaves_extension_chance,
                        log_pos,
                        pos.down(),
                    );
                }
                pos = pos.offset_dir(along_edge.to_offset(), 1);
            }
        }
    }
}

#[cfg(test)]
mod foliage_setter_tests {
    use pumpkin_data::chunk::Biome;
    use pumpkin_data::{
        Block, BlockDirection, BlockState, BlockStateId, HorizontalFacingExt, Mirror, Rotation,
    };
    use pumpkin_util::math::pool::Weighted;
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_util::random::{RandomGenerator, RandomImpl, legacy_rand::LegacyRand};

    use super::{FoliagePlacer, FoliageSetter};
    use crate::generation::block_state_provider::{
        BlockStateProvider, SimpleStateProvider, WeightedBlockStateProvider,
    };
    use crate::generation::proto_chunk::{GenerationCache, test_cache::FlatWorld};
    use crate::world::{BlockAccessor, WorldPortalExt};

    struct TestWorldPortal;

    impl WorldPortalExt for TestWorldPortal {
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
            state_id: BlockStateId,
            mirror: Mirror,
        ) -> &'static BlockState {
            block.mirror(state_id, mirror)
        }

        fn rotate(
            &self,
            block: &Block,
            state_id: BlockStateId,
            rotation: Rotation,
        ) -> &'static BlockState {
            block.rotate(state_id, rotation)
        }

        fn spawn_mobs_for_chunk_generation(
            &self,
            _cache: &mut dyn GenerationCache,
            _biome: &'static Biome,
            _chunk_x: i32,
            _chunk_z: i32,
        ) {
        }
    }

    fn oak_leaves() -> BlockStateProvider {
        BlockStateProvider::Simple(SimpleStateProvider {
            state: Block::OAK_LEAVES.default_state,
        })
    }

    #[test]
    fn repeated_leaf_is_recorded_once_and_visible_to_later_attachments() {
        // `TreeFeature.place` (`TreeFeature.java:127,137-148`) keeps one foliage `HashSet`
        // per tree: a leaf placed twice is one entry, and `isSet` sees earlier attachments.
        let registry = TestWorldPortal;
        let provider = oak_leaves();
        let mut setter = FoliageSetter::new(&provider, &registry);
        let mut world = FlatWorld::default();
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        let first = BlockPos::new(3, 8, -2);
        let second = BlockPos::new(3, 8, -1);

        assert!(FoliagePlacer::place_foliage_block(
            &mut world,
            &mut random,
            &mut setter,
            first
        ));
        assert!(FoliagePlacer::place_foliage_block(
            &mut world,
            &mut random,
            &mut setter,
            second
        ));
        // Leaves are `#replaceable_by_trees`, so the overlapping placement succeeds again.
        assert!(FoliagePlacer::place_foliage_block(
            &mut world,
            &mut random,
            &mut setter,
            first
        ));

        assert!(setter.is_set(first));
        assert!(!setter.is_set(BlockPos::new(3, 9, -2)));
        assert_eq!(setter.into_positions(), vec![first, second]);
    }

    #[test]
    fn rejected_leaf_does_not_sample_the_provider() {
        // `FoliagePlacer.tryPlaceLeaf` (`FoliagePlacer.java:170-174`) only calls
        // `foliageProvider.getState` once the persistent and `validTreePos` checks pass.
        let registry = TestWorldPortal;
        let provider = BlockStateProvider::Weighted(WeightedBlockStateProvider {
            entries: vec![
                Weighted {
                    data: Block::AZALEA_LEAVES.default_state,
                    weight: 3,
                },
                Weighted {
                    data: Block::FLOWERING_AZALEA_LEAVES.default_state,
                    weight: 1,
                },
            ],
        });
        let mut setter = FoliageSetter::new(&provider, &registry);
        let mut world = FlatWorld::default();
        let pos = BlockPos::new(0, 0, 0);
        world.put(0, 0, 0, Block::STONE.default_state);
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(7));
        let mut untouched = RandomGenerator::Legacy(LegacyRand::from_seed(7));

        assert!(!FoliagePlacer::place_foliage_block(
            &mut world,
            &mut random,
            &mut setter,
            pos
        ));
        assert_eq!(random.next_i32(), untouched.next_i32());
        assert!(!setter.is_set(pos));
    }

    #[test]
    fn weighted_provider_is_sampled_per_leaf() {
        // A fresh `foliageProvider.getState` per leaf (`FoliagePlacer.java:174`) mixes the
        // weighted azalea leaves within one tree instead of choosing one state per tree.
        let registry = TestWorldPortal;
        let provider = BlockStateProvider::Weighted(WeightedBlockStateProvider {
            entries: vec![
                Weighted {
                    data: Block::AZALEA_LEAVES.default_state,
                    weight: 1,
                },
                Weighted {
                    data: Block::FLOWERING_AZALEA_LEAVES.default_state,
                    weight: 1,
                },
            ],
        });
        let mut setter = FoliageSetter::new(&provider, &registry);
        let mut world = FlatWorld::default();
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(3));
        for x in 0..32 {
            FoliagePlacer::place_foliage_block(
                &mut world,
                &mut random,
                &mut setter,
                BlockPos::new(x, 0, 0),
            );
        }

        let placed: Vec<_> = (0..32)
            .map(|x| world.raw(&BlockPos::new(x, 0, 0).0).to_block().id)
            .collect();
        assert!(placed.contains(&Block::AZALEA_LEAVES.id));
        assert!(placed.contains(&Block::FLOWERING_AZALEA_LEAVES.id));
    }

    #[test]
    fn hanging_leaves_edges_follow_the_horizontal_plane_order() {
        // `placeLeavesRowWithHangingLeavesBelow` (`FoliagePlacer.java:133`) iterates
        // `Direction.Plane.HORIZONTAL`: NORTH, EAST, SOUTH, WEST (`Direction.java:577`).
        let expected: Vec<BlockDirection> = BlockDirection::horizontal_worldgen()
            .iter()
            .map(HorizontalFacingExt::to_block_direction)
            .collect();
        assert_eq!(FoliagePlacer::HANGING_LEAVES_EDGE_ORDER.to_vec(), expected);
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
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        node: &TreeNode,
        foliage_height: i32,
        radius: i32,
        offset: i32,
        setter: &mut FoliageSetter<'_>,
    ) {
        match self {
            Self::Blob(blob) => {
                blob.generate(chunk, random, node, foliage_height, radius, offset, setter);
            }
            Self::Spruce(spruce) => {
                spruce.generate(chunk, random, node, foliage_height, radius, offset, setter);
            }
            Self::Pine(pine) => {
                pine.generate(chunk, random, node, foliage_height, radius, offset, setter);
            }
            Self::Acacia(acacia) => {
                acacia.generate(chunk, random, node, foliage_height, radius, offset, setter);
            }
            Self::Bush(bush) => {
                bush.generate(chunk, random, node, foliage_height, radius, offset, setter);
            }
            Self::Fancy(fancy) => {
                fancy.generate(chunk, random, node, foliage_height, radius, offset, setter);
            }
            Self::Jungle(jungle) => {
                jungle.generate(chunk, random, node, foliage_height, radius, offset, setter);
            }
            Self::MegaPine(mega_pine) => {
                mega_pine.generate(chunk, random, node, foliage_height, radius, offset, setter);
            }
            Self::DarkOak(dark_oak) => {
                dark_oak.generate(chunk, random, node, foliage_height, radius, offset, setter);
            }
            Self::RandomSpread(random_spread) => {
                random_spread.generate(chunk, random, node, foliage_height, radius, offset, setter);
            }
            Self::Cherry(cherry) => {
                cherry.generate(chunk, random, node, foliage_height, radius, offset, setter);
            }
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
