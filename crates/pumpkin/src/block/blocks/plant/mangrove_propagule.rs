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
    BlockBehaviour, BonemealArgs, CanPlaceAtArgs, GetStateForNeighborUpdateArgs, OnPlaceArgs,
    RandomTickArgs,
};
use crate::world::World;
use crate::world::feature_placer::FeatureCache;

type PropaguleProperties = MangrovePropaguleLikeProperties;

/// `mangrove_propagule`. Vanilla: `net.minecraft.world.level.block.MangrovePropaguleBlock`.
/// The propagule owns the sapling growth path because its registration wins over the generic
/// sapling tag registration.
#[pumpkin_block("minecraft:mangrove_propagule")]
pub struct MangrovePropaguleBlock;

/// `TreeGrower.MANGROVE`'s `secondaryChance` (`TreeGrower.java:51`).
const SECONDARY_CHANCE: f32 = 0.85;

/// `MangrovePropaguleBlock.MAX_AGE` (`MangrovePropaguleBlock.java:33`).
pub const MAX_AGE: u8 = 4;

/// `SaplingBlock.isBonemealSuccess` (`SaplingBlock.java:69-72`).
const BONEMEAL_SUCCESS_CHANCE: f32 = 0.45;

impl MangrovePropaguleBlock {
    #[must_use]
    pub const fn is_hanging(props: &PropaguleProperties) -> bool {
        props.hanging
    }

    #[must_use]
    pub const fn is_fully_grown(props: &PropaguleProperties) -> bool {
        props.age >= MAX_AGE
    }

    #[must_use]
    pub fn can_survive(
        world: &dyn BlockAccessor,
        pos: &BlockPos,
        props: &PropaguleProperties,
    ) -> bool {
        can_survive(props.hanging, world, pos)
    }

    #[must_use]
    pub fn create_new_hanging_propagule(age: u8) -> BlockStateId {
        let mut props = PropaguleProperties::default(&Block::MANGROVE_PROPAGULE);
        props.hanging = true;
        props.age = age.min(MAX_AGE);
        props.to_state_id(&Block::MANGROVE_PROPAGULE)
    }
}

/// `TreeGrower.getConfiguredFeature` (`TreeGrower.java:109-121`) for the mangrove entry.
fn mangrove_feature(random: &mut RandomGenerator) -> ConfiguredFeature {
    if random.next_f32() < SECONDARY_CHANCE {
        ConfiguredFeature::TallMangrove
    } else {
        ConfiguredFeature::Mangrove
    }
}

/// `TreeGrower.growTree` (`TreeGrower.java:127-179`). Both the clear and feature placement are
/// buffered so a declined feature leaves the propagule intact.
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

/// `SaplingBlock.advanceTree` (`SaplingBlock.java:51-57`). Stage zero advances immediately;
/// feature generation is spawned because the feature cache commits asynchronously.
fn advance_tree(world: &Arc<World>, pos: &BlockPos, bone_meal: bool) {
    let (block, state_id) = world.get_block_and_state_id(pos);
    if block != &Block::MANGROVE_PROPAGULE {
        return;
    }
    let mut props = PropaguleProperties::from_state_id(state_id, block);
    if props.stage == 0 {
        props.stage = 1;
        world.set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
        return;
    }

    use crate::plugin::api::events::world::structure_grow::{StructureGrowEvent, TreeType};
    let mut event = StructureGrowEvent::new(*pos, TreeType::Mangrove, bone_meal);
    if let Some(server) = world.server.upgrade() {
        server.plugin_manager.fire_blocking(&server, &mut event);
    }
    if event.cancelled {
        return;
    }
    let world = world.clone();
    let pos = *pos;
    let waterlogged = props.waterlogged;
    tokio::spawn(async move {
        let _ = grow_tree(&world, &pos, waterlogged).await;
    });
}

fn can_survive(hanging: bool, block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
    if hanging {
        block_accessor
            .get_block(&pos.up())
            .has_tag(&tag::Block::MINECRAFT_SUPPORTS_HANGING_MANGROVE_PROPAGULE)
    } else {
        block_accessor
            .get_block(&pos.down())
            .has_tag(&tag::Block::MINECRAFT_SUPPORTS_MANGROVE_PROPAGULE)
    }
}

impl BlockBehaviour for MangrovePropaguleBlock {
    /// A player places only the standing variant; hanging propagules come from generation.
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_survive(false, args.block_accessor, args.position)
    }

    /// `MangrovePropaguleBlock.getStateForPlacement`: fully grown standing propagules start at
    /// age four and are not hanging.
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = PropaguleProperties::default(args.block);
        props.waterlogged = args.replacing.water_source();
        props.age = MAX_AGE;
        props.hanging = false;
        props.to_state_id(args.block)
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
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
    }

    /// `MangrovePropaguleBlock.randomTick` (`MangrovePropaguleBlock.java:104-113`). Standing
    /// propagules intentionally use this override without the generic sapling light gate.
    fn random_tick(&self, args: RandomTickArgs<'_>) {
        let (block, state_id) = args.world.get_block_and_state_id(args.position);
        let mut props = PropaguleProperties::from_state_id(state_id, block);
        if props.hanging {
            if !Self::is_fully_grown(&props) {
                props.age += 1;
                args.world.set_block_state(
                    args.position,
                    props.to_state_id(block),
                    BlockFlags::NOTIFY_LISTENERS,
                );
            }
        } else if rand::rng().random_range(0..7) == 0 {
            advance_tree(args.world, args.position, false);
        }
    }

    /// `MangrovePropaguleBlock.isValidBonemealTarget` (`MangrovePropaguleBlock.java:116-118`).
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        let props = PropaguleProperties::from_state_id(args.state_id, args.block);
        !props.hanging || !Self::is_fully_grown(&props)
    }

    /// `MangrovePropaguleBlock.isBonemealSuccess` (`MangrovePropaguleBlock.java:121-123`).
    fn is_bonemeal_success(&self, args: BonemealArgs<'_>) -> bool {
        let props = PropaguleProperties::from_state_id(args.state_id, args.block);
        if props.hanging {
            !Self::is_fully_grown(&props)
        } else {
            rand::rng().random::<f32>() < BONEMEAL_SUCCESS_CHANCE
        }
    }

    /// `MangrovePropaguleBlock.performBonemeal` (`MangrovePropaguleBlock.java:126-132`).
    fn perform_bonemeal(&self, args: BonemealArgs<'_>) {
        let mut props = PropaguleProperties::from_state_id(args.state_id, args.block);
        if props.hanging && !Self::is_fully_grown(&props) {
            props.age += 1;
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_LISTENERS,
            );
        } else {
            advance_tree(args.world, args.position, true);
        }
    }
}

impl PlantBlockBase for MangrovePropaguleBlock {
    fn can_place_at(&self, block_accessor: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
        let (block, state) = block_accessor.get_block_and_state(block_pos);
        let hanging = block == &Block::MANGROVE_PROPAGULE
            && PropaguleProperties::from_state_id(state.id, block).hanging;
        can_survive(hanging, block_accessor, block_pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mangrove_features_are_registered() {
        use pumpkin_world::generation::feature::configured_features::CONFIGURED_FEATURES;
        assert!(CONFIGURED_FEATURES.contains_key(&ConfiguredFeature::Mangrove));
        assert!(CONFIGURED_FEATURES.contains_key(&ConfiguredFeature::TallMangrove));
    }

    /// `TreeGrower.MANGROVE` (`TreeGrower.java:49-58`) selects tall mangrove below 0.85.
    #[test]
    fn secondary_roll_picks_both_mangrove_features() {
        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(1));
        let mut tall = false;
        let mut ordinary = false;
        for _ in 0..200 {
            match mangrove_feature(&mut random) {
                ConfiguredFeature::TallMangrove => tall = true,
                ConfiguredFeature::Mangrove => ordinary = true,
                other => panic!("unexpected feature {other:?}"),
            }
        }
        assert!(tall && ordinary);
    }

    #[test]
    fn max_age_matches_vanilla() {
        let mut props = PropaguleProperties::default(&Block::MANGROVE_PROPAGULE);
        props.age = MAX_AGE;
        assert!(MangrovePropaguleBlock::is_fully_grown(&props));
        props.age = MAX_AGE - 1;
        assert!(!MangrovePropaguleBlock::is_fully_grown(&props));
    }
}
