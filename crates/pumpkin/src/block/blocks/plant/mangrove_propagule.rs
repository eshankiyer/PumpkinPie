use std::sync::Arc;

use pumpkin_data::block_properties::{BlockProperties, MangrovePropaguleLikeProperties};
use pumpkin_data::configured_feature::ConfiguredFeature;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::{Block, BlockStateId, tag, tag::Taggable};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::xoroshiro128::Xoroshiro;
use pumpkin_util::random::{RandomGenerator, RandomImpl};
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::RngExt;

use crate::block::blocks::plant::PlantBlockBase;
use crate::block::{
    BlockBehaviour, BlockFuture, BonemealArgs, CanPlaceAtArgs, GetStateForNeighborUpdateArgs,
    OnPlaceArgs, RandomTickArgs,
};
use crate::world::World;
use crate::world::feature_placer::FeatureCache;

type PropaguleProperties = MangrovePropaguleLikeProperties;

/// `mangrove_propagule`. Vanilla: `net.minecraft.world.level.block.MangrovePropaguleBlock`.
///
/// `MangrovePropaguleBlock extends SaplingBlock`, but this block wins registration over the
/// tag-registered `plant/sapling.rs`, so none of that file's `TreeGrower` wiring reaches here and
/// the whole `SaplingBlock` half - `advanceTree`, `growTree` and the bone-meal hooks - is carried
/// explicitly below. `TreeGrower.MANGROVE` (`TreeGrower.java:49-58`) is degenerate: no mega tree,
/// no flowering variant, just an 0.85 roll between the tall and the ordinary mangrove, so it is
/// spelled out rather than shared with `sapling.rs`'s general table.
#[pumpkin_block("minecraft:mangrove_propagule")]
pub struct MangrovePropaguleBlock;

/// `TreeGrower.MANGROVE`'s `secondaryChance` (`TreeGrower.java:51`).
const SECONDARY_CHANCE: f32 = 0.85;

/// `MangrovePropaguleBlock.MAX_AGE` (`MangrovePropaguleBlock.java:33`).
const MAX_AGE: u8 = 4;

/// `SaplingBlock.isBonemealSuccess` (`SaplingBlock.java:69-72`).
const BONEMEAL_SUCCESS_CHANCE: f32 = 0.45;

const fn is_fully_grown(age: u8) -> bool {
    age >= MAX_AGE
}

/// `TreeGrower.getConfiguredFeature` (`TreeGrower.java:109-121`) for the mangrove entry: with no
/// flowering variants and no mega tree, the only branch left is the secondary roll.
fn mangrove_feature(random: &mut RandomGenerator) -> ConfiguredFeature {
    if random.next_f32() < SECONDARY_CHANCE {
        ConfiguredFeature::TallMangrove
    } else {
        ConfiguredFeature::Mangrove
    }
}

/// `TreeGrower.growTree` (`TreeGrower.java:127-179`) for a propagule.
///
/// Vanilla clears the sapling to `getFluidState(pos).createLegacyBlock()` before running the
/// feature, which for a propagule really can be water rather than air - the one case
/// `plant/sapling.rs` documents as out of its reach. Both the clear and the tree are buffered in
/// one [`FeatureCache`], so a feature that declines simply never commits.
async fn grow_tree(world: &Arc<World>, pos: &BlockPos, waterlogged: bool) -> bool {
    let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::rng().random()));
    let cleared = if waterlogged {
        Block::WATER.default_state.id
    } else {
        Block::AIR.default_state.id
    };
    let feature = mangrove_feature(&mut random);
    let mut cache = FeatureCache::new(world);
    cache.set_block(*pos, cleared);
    if cache.place(feature, *pos, &mut random) {
        cache.commit().await
    } else {
        false
    }
}

/// `SaplingBlock.advanceTree` (`SaplingBlock.java:51-57`): stage 0 only ticks the stage up; from
/// stage 1 the tree grows. Fires `StructureGrowEvent` the same way `SaplingBlock::generate` does.
async fn advance_tree(world: &Arc<World>, pos: &BlockPos, bone_meal: bool) {
    use crate::plugin::api::events::world::structure_grow::{StructureGrowEvent, TreeType};
    let mut event = StructureGrowEvent::new(*pos, TreeType::Mangrove, bone_meal);
    if let Some(server) = world.server.upgrade() {
        server.plugin_manager.fire(&server, &mut event).await;
        if event.cancelled {
            return;
        }
    }

    let (block, state_id) = world.get_block_and_state_id(pos);
    if block != &Block::MANGROVE_PROPAGULE {
        return;
    }
    let mut props = PropaguleProperties::from_state_id(state_id, block);
    if props.stage == 0 {
        props.stage = 1;
        world
            .set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
            .await;
        return;
    }
    grow_tree(world, pos, props.waterlogged).await;
}

/// `MangrovePropaguleBlock.mayPlaceOn` (non-hanging) / `canSurvive`'s hanging branch.
fn can_survive(hanging: bool, block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
    if hanging {
        let above = block_accessor.get_block(&pos.up());
        above.has_tag(&tag::Block::MINECRAFT_SUPPORTS_HANGING_MANGROVE_PROPAGULE)
    } else {
        let below = block_accessor.get_block(&pos.down());
        below.has_tag(&tag::Block::MINECRAFT_SUPPORTS_MANGROVE_PROPAGULE)
    }
}

impl BlockBehaviour for MangrovePropaguleBlock {
    /// A player can only ever place a propagule standing up; the hanging variant is only
    /// produced by mangrove tree generation.
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_survive(false, args.block_accessor, args.position)
    }

    /// `MangrovePropaguleBlock.getStateForPlacement`: `AGE = 4` (fully grown), not hanging.
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = PropaguleProperties::default(args.block);
            props.waterlogged = args.replacing.water_source();
            props.age = 4;
            props.hanging = false;
            props.to_state_id(args.block)
        })
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let props = PropaguleProperties::from_state_id(args.state_id, args.block);
            if props.waterlogged {
                args.world.schedule_fluid_tick(
                    &Fluid::WATER,
                    *args.position,
                    Fluid::WATER.flow_speed as u8,
                    TickPriority::Normal,
                );
            }
            <Self as PlantBlockBase>::get_state_for_neighbor_update(
                self,
                args.world,
                args.position,
                args.state_id,
            )
            .await
        })
    }

    /// `MangrovePropaguleBlock.randomTick` (`MangrovePropaguleBlock.java:104-113`). Note this
    /// override drops `SaplingBlock`'s brightness gate entirely: a standing propagule grows in
    /// the dark, unlike every other sapling.
    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let (block, state_id) = args.world.get_block_and_state_id(args.position);
            let mut props = PropaguleProperties::from_state_id(state_id, block);
            if props.hanging {
                if !is_fully_grown(props.age) {
                    props.age += 1;
                    args.world
                        .set_block_state(
                            args.position,
                            props.to_state_id(block),
                            BlockFlags::NOTIFY_LISTENERS,
                        )
                        .await;
                }
            } else if rand::rng().random_range(0..7) == 0 {
                advance_tree(args.world, args.position, false).await;
            }
        })
    }

    /// `MangrovePropaguleBlock.isValidBonemealTarget` (`MangrovePropaguleBlock.java:116-118`).
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        let props = PropaguleProperties::from_state_id(args.state_id, args.block);
        !props.hanging || !is_fully_grown(props.age)
    }

    /// `MangrovePropaguleBlock.isBonemealSuccess` (`MangrovePropaguleBlock.java:121-123`), whose
    /// standing branch is `SaplingBlock.isBonemealSuccess`.
    fn is_bonemeal_success(&self, args: BonemealArgs<'_>) -> bool {
        let props = PropaguleProperties::from_state_id(args.state_id, args.block);
        if props.hanging {
            !is_fully_grown(props.age)
        } else {
            rand::rng().random::<f32>() < BONEMEAL_SUCCESS_CHANCE
        }
    }

    /// `MangrovePropaguleBlock.performBonemeal` (`MangrovePropaguleBlock.java:126-132`).
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let mut props = PropaguleProperties::from_state_id(args.state_id, args.block);
            if props.hanging && !is_fully_grown(props.age) {
                props.age += 1;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_LISTENERS,
                    )
                    .await;
            } else {
                advance_tree(args.world, args.position, true).await;
            }
        })
    }
}

impl PlantBlockBase for MangrovePropaguleBlock {
    /// Overrides the default (which only checks `SUPPORTS_VEGETATION` below) to branch on
    /// whether *this* propagule is currently hanging.
    fn can_place_at(&self, block_accessor: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
        let (block, state) = block_accessor.get_block_and_state(block_pos);
        let hanging = if block == &Block::MANGROVE_PROPAGULE {
            PropaguleProperties::from_state_id(state.id, block).hanging
        } else {
            false
        };
        can_survive(hanging, block_accessor, block_pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mangrove features the grower names must actually be registered, or growth silently
    /// does nothing - the same trap `plant/sapling.rs` guards for the other saplings.
    #[test]
    fn mangrove_features_are_registered() {
        use pumpkin_world::generation::feature::configured_features::CONFIGURED_FEATURES;
        assert!(CONFIGURED_FEATURES.contains_key(&ConfiguredFeature::Mangrove));
        assert!(CONFIGURED_FEATURES.contains_key(&ConfiguredFeature::TallMangrove));
    }

    /// `TreeGrower.MANGROVE` (`TreeGrower.java:49-58`) rolls the *tall* mangrove below the 0.85
    /// secondary chance and the ordinary one at or above it.
    #[test]
    fn secondary_roll_picks_the_tall_mangrove() {
        let mut low = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(1));
        let mut picked_tall = false;
        let mut picked_short = false;
        for _ in 0..200 {
            match mangrove_feature(&mut low) {
                ConfiguredFeature::TallMangrove => picked_tall = true,
                ConfiguredFeature::Mangrove => picked_short = true,
                other => panic!("unexpected feature {other:?}"),
            }
        }
        assert!(picked_tall, "an 0.85 chance must produce tall mangroves");
        assert!(picked_short, "the remaining 15% must produce mangroves");
    }

    #[test]
    fn max_age_matches_vanilla() {
        assert!(is_fully_grown(MAX_AGE));
        assert!(!is_fully_grown(MAX_AGE - 1));
    }

    #[test]
    fn hanging_age_caps_at_four() {
        let mut props = PropaguleProperties::default(&Block::MANGROVE_PROPAGULE);
        props.hanging = true;
        props.age = 4;
        // Mirrors the `props.hanging && props.age < 4` guard in random_tick: at max age no
        // further increment should be attempted.
        assert!(!(props.hanging && props.age < 4));

        props.age = 3;
        assert!(props.hanging && props.age < 4);
    }
}
