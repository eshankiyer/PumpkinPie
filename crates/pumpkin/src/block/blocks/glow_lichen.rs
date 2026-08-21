//! `GlowLichenBlock` (`net/minecraft/world/level/block/GlowLichenBlock.java:14`) and the plain
//! `MultifaceBlock` registration used by `resin_clump`
//! (`net/minecraft/world/level/block/Blocks.java:2425-2435`, `MultifaceBlock::new`).
//!
//! Both share `MultifaceBlock`'s placement/support/neighbour behaviour
//! (`net/minecraft/world/level/block/MultifaceBlock.java:30`); only glow lichen adds
//! `BonemealableBlock`, driven by a `MultifaceSpreader` with the default config
//! (`MultifaceSpreader.DefaultSpreaderConfig`, `MultifaceSpreader.java:117-143`).
//!
//! This is the second consumer of `abstract_multiface.rs`/`multiface_spreader.rs`, after
//! `sculk_vein.rs`, and follows that file's structure.

use std::sync::Arc;

use pumpkin_data::block_properties::{
    BlockProperties, GlowLichenLikeProperties, WaterLikeProperties,
};
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState, BlockStateId, FacingExt};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::{RandomGenerator, get_seed, xoroshiro128::Xoroshiro};
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::{BlockAccessor, BlockFlags};

use crate::block::blocks::abstract_multiface::{
    FaceSet, MultifaceBlockBase, MultifaceProperties, has_any_vacant_face,
};
use crate::block::blocks::multiface_spreader::{
    self, SpreadConfig, SpreadPos, SpreadTarget, can_spread_in_any_direction,
};
use crate::block::{
    BlockBehaviour, BlockFuture, BlockIsReplacing, BlockMetadata, BonemealArgs, CanPlaceAtArgs,
    CanUpdateAtArgs, GetStateForNeighborUpdateArgs, OnPlaceArgs,
};
use crate::entity::EntityBase;
use crate::world::World;

/// Serves both `glow_lichen` (`GlowLichenBlock`) and `resin_clump` (a bare `MultifaceBlock`).
/// Everything except the bonemeal hooks is identical between them, and the bonemeal hooks are
/// gated on the block id below, so a single behaviour can own both ids.
pub struct MultifaceGrowthBlock;

impl BlockMetadata for MultifaceGrowthBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::GLOW_LICHEN, BlockId::RESIN_CLUMP].into()
    }
}

/// Neither `GlowLichenBlock` nor `resin_clump`'s `MultifaceBlock` overrides `isFaceSupported`
/// (MultifaceBlock.java:105-107 returns true for all six), so the default impl is correct.
impl MultifaceBlockBase for MultifaceGrowthBlock {}

/// `existingState.is(Blocks.WATER) && existingState.getFluidState().isSource()`
/// (`MultifaceSpreader.DefaultSpreaderConfig#stateCanBeReplaced`, MultifaceSpreader.java:134).
fn is_water_source(state: &BlockState) -> bool {
    let block = Block::from_state_id(state.id);
    block == &Block::WATER && WaterLikeProperties::from_state_id(state.id, block).level == 0
}

/// `MultifaceSpreader.DefaultSpreaderConfig` (MultifaceSpreader.java:117-143), bound to one
/// concrete multiface block id.
struct DefaultSpreaderConfig {
    block: &'static Block,
}

impl SpreadConfig for DefaultSpreaderConfig {
    /// `DefaultSpreaderConfig#canSpreadInto` (MultifaceSpreader.java:137-142).
    fn can_spread_into(
        &self,
        accessor: &dyn BlockAccessor,
        _source_pos: &BlockPos,
        spread_pos: SpreadPos,
    ) -> bool {
        let existing_state = accessor.get_block_state(&spread_pos.pos);
        // `stateCanBeReplaced` (MultifaceSpreader.java:131-135).
        let replaceable = existing_state.is_air()
            || Block::from_state_id(existing_state.id) == self.block
            || is_water_source(existing_state);
        if !replaceable {
            return false;
        }
        MultifaceGrowthBlock.is_valid_state_for_placement(
            accessor,
            existing_faces(existing_state, self.block),
            &spread_pos.pos,
            spread_pos.face,
        )
    }
}

/// Vanilla's repeated `oldState.is(this)` guard before reading face properties off a state.
fn existing_faces(state: &BlockState, block: &'static Block) -> Option<FaceSet> {
    (Block::from_state_id(state.id) == block)
        .then(|| GlowLichenLikeProperties::from_state_id(state.id, block).faces())
}

/// `MultifaceBlock#getStateForPlacement(BlockState, BlockGetter, BlockPos, Direction)`
/// (MultifaceBlock.java:200-216), reduced to a state id.
fn state_for_placement(
    accessor: &dyn BlockAccessor,
    block: &'static Block,
    old_state: &BlockState,
    placement_pos: &BlockPos,
    placement_direction: BlockDirection,
) -> Option<BlockStateId> {
    let old_faces = existing_faces(old_state, block);
    let new_faces = MultifaceGrowthBlock.faces_for_placement(
        accessor,
        old_faces,
        placement_pos,
        placement_direction,
    )?;

    let mut props = if old_faces.is_some() {
        GlowLichenLikeProperties::from_state_id(old_state.id, block)
    } else {
        let mut default_props = GlowLichenLikeProperties::default(block);
        default_props.r#waterlogged = is_water_source(old_state);
        default_props
    };
    props.set_faces(new_faces);
    Some(props.to_state_id(block))
}

/// A `SpreadTarget` writing into a live `World`, mirroring `DefaultSpreaderConfig#placeBlock`'s
/// inherited `SpreadConfig.placeBlock` default (`level.setBlock(spreadPos.pos(), state, 2)`,
/// i.e. `NOTIFY_LISTENERS` only).
struct WorldSpreadTarget<'a> {
    world: &'a Arc<World>,
    block: &'static Block,
}

impl SpreadTarget for WorldSpreadTarget<'_> {
    fn accessor(&self) -> &dyn BlockAccessor {
        self.world.as_ref()
    }

    fn place(&self, spread_pos: SpreadPos) -> BlockFuture<'_, bool> {
        Box::pin(async move {
            let existing_state = self.world.get_block_state(&spread_pos.pos);
            let Some(new_state_id) = state_for_placement(
                self.world.as_ref(),
                self.block,
                existing_state,
                &spread_pos.pos,
                spread_pos.face,
            ) else {
                return false;
            };
            self.world
                .set_block_state(&spread_pos.pos, new_state_id, BlockFlags::NOTIFY_LISTENERS)
                .await;
            true
        })
    }
}

impl BlockBehaviour for MultifaceGrowthBlock {
    /// `MultifaceBlock#getStateForPlacement(BlockPlaceContext)` (MultifaceBlock.java:179-189):
    /// try the player's nearest looking directions in order and take the first that works.
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let old_state = args.world.get_block_state(args.position);
            let existing = match args.replacing {
                BlockIsReplacing::Itself(state_id) => {
                    Some(GlowLichenLikeProperties::from_state_id(state_id, args.block).faces())
                }
                _ => None,
            };

            let mut candidates: Vec<BlockDirection> = args
                .player
                .get_entity()
                .get_entity_facing_order()
                .into_iter()
                .map(|f| f.to_block_direction())
                .collect();
            if let Some(idx) = candidates.iter().position(|&d| d == args.direction) {
                candidates.remove(idx);
            }
            candidates.insert(0, args.direction);

            for direction in candidates {
                if let Some(new_faces) =
                    self.faces_for_placement(args.world, existing, args.position, direction)
                {
                    let mut props = if existing.is_some() {
                        GlowLichenLikeProperties::from_state_id(old_state.id, args.block)
                    } else {
                        let mut default_props = GlowLichenLikeProperties::default(args.block);
                        default_props.r#waterlogged = args.replacing.water_source();
                        default_props
                    };
                    props.set_faces(new_faces);
                    return props.to_state_id(args.block);
                }
            }

            Block::AIR.default_state.id
        })
    }

    /// `MultifaceBlock#canSurvive` (MultifaceBlock.java:154-167).
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        if let Some(direction) = args.direction
            && self.is_valid_state_for_placement(
                args.block_accessor,
                None,
                args.position,
                direction,
            )
        {
            return true;
        }
        BlockDirection::all().into_iter().any(|direction| {
            self.is_valid_state_for_placement(args.block_accessor, None, args.position, direction)
        })
    }

    /// `MultifaceBlock#canBeReplaced` (MultifaceBlock.java:169-172). This hook only fires when
    /// the clicked block already is this block, so the `!itemInHand.is(this)` half is always
    /// true here and it reduces to a vacant-face check - same reasoning as `sculk_vein.rs`.
    fn can_update_at(&self, args: CanUpdateAtArgs<'_>) -> bool {
        let faces = GlowLichenLikeProperties::from_state_id(args.state_id, args.block).faces();
        has_any_vacant_face(faces)
    }

    /// `MultifaceBlock#updateShape` (MultifaceBlock.java:118-141).
    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = GlowLichenLikeProperties::from_state_id(args.state_id, args.block);
            if props.r#waterlogged {
                args.world.schedule_fluid_tick(
                    &pumpkin_data::fluid::Fluid::WATER,
                    *args.position,
                    pumpkin_data::fluid::Fluid::WATER.flow_speed as u8,
                    TickPriority::Normal,
                );
            }

            let faces = props.faces();
            let neighbor_state = args.neighbor_state_id.to_state();
            self.update_faces_for_neighbor(faces, neighbor_state, args.direction)
                .map_or(Block::AIR.default_state.id, |new_faces| {
                    props.set_faces(new_faces);
                    props.to_state_id(args.block)
                })
        })
    }

    /// `GlowLichenBlock#isValidBonemealTarget` (GlowLichenBlock.java:31-34). `resin_clump` is a
    /// bare `MultifaceBlock` and implements no `BonemealableBlock`, so it is never a target.
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        if args.block != &Block::GLOW_LICHEN {
            return false;
        }
        let faces = GlowLichenLikeProperties::from_state_id(args.state_id, args.block).faces();
        let config = DefaultSpreaderConfig {
            block: &Block::GLOW_LICHEN,
        };
        BlockDirection::all().into_iter().any(|face| {
            can_spread_in_any_direction(
                &config,
                args.world.as_ref(),
                faces,
                *args.position,
                face.opposite(),
            )
        })
    }

    /// `GlowLichenBlock#performBonemeal` (GlowLichenBlock.java:41-44):
    /// `spreader.spreadFromRandomFaceTowardRandomDirection(state, level, pos, random)`.
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.block != &Block::GLOW_LICHEN {
                return;
            }
            let faces = GlowLichenLikeProperties::from_state_id(args.state_id, args.block).faces();
            let config = DefaultSpreaderConfig {
                block: &Block::GLOW_LICHEN,
            };
            let target = WorldSpreadTarget {
                world: args.world,
                block: &Block::GLOW_LICHEN,
            };
            let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(get_seed()));
            multiface_spreader::spread_from_random_face_toward_random_direction(
                &config,
                &target,
                faces,
                *args.position,
                &mut random,
            )
            .await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MultifaceBlock#createBlockStateDefinition` (MultifaceBlock.java:110-118) adds all six
    /// face properties plus `WATERLOGGED`: 2^7 = 128 states for both ids.
    #[test]
    fn both_ids_have_the_full_multiface_state_space() {
        assert_eq!(Block::GLOW_LICHEN.states.len(), 128);
        assert_eq!(Block::RESIN_CLUMP.states.len(), 128);
    }

    /// `Blocks.GLOW_LICHEN` is registered with `.lightLevel(GlowLichenBlock.emission(7))`
    /// (Blocks.java:2412-2424): light 7 when any face is present, 0 otherwise. `RESIN_CLUMP`
    /// (Blocks.java:2425-2435) has no `lightLevel`. Both are data-driven here, which is why the
    /// behaviour above does not implement `emission`.
    #[test]
    fn glow_lichen_emission_is_data_driven_and_resin_clump_is_dark() {
        let mut lit = GlowLichenLikeProperties::default(&Block::GLOW_LICHEN);
        lit.set_faces(FaceSet::from_directions([BlockDirection::Down]));
        let lit_state = lit.to_state_id(&Block::GLOW_LICHEN).to_state();
        assert_eq!(lit_state.luminance, 7);

        let dark = GlowLichenLikeProperties::default(&Block::GLOW_LICHEN);
        assert_eq!(dark.faces(), FaceSet::EMPTY);
        assert_eq!(
            dark.to_state_id(&Block::GLOW_LICHEN).to_state().luminance,
            0
        );

        let mut resin = GlowLichenLikeProperties::default(&Block::RESIN_CLUMP);
        resin.set_faces(FaceSet::from_directions([BlockDirection::Down]));
        assert_eq!(
            resin.to_state_id(&Block::RESIN_CLUMP).to_state().luminance,
            0
        );
    }

    /// `DefaultSpreaderConfig#stateCanBeReplaced` (MultifaceSpreader.java:131-135) accepts air,
    /// the block itself, and a water source - and nothing else.
    #[test]
    fn default_spreader_replaceability_matches_vanilla() {
        assert!(Block::AIR.default_state.is_air());
        assert!(is_water_source(Block::WATER.default_state));
        assert!(!is_water_source(Block::STONE.default_state));
    }
}
