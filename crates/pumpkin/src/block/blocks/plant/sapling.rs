use pumpkin_data::configured_feature::ConfiguredFeature;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockStateId, tag};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::xoroshiro128::Xoroshiro;
use pumpkin_util::random::{RandomGenerator, RandomImpl};
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use std::sync::Arc;

const GROWTH_LIGHT: u8 = 9;

/// `SaplingBlock.isBonemealSuccess` (`SaplingBlock.java:69-72`).
const BONEMEAL_SUCCESS_CHANCE: f32 = 0.45;

use crate::block::blocks::plant::PlantBlockBase;
use crate::block::{
    BlockBehaviour, BlockFuture, BonemealArgs, CanPlaceAtArgs, GetStateForNeighborUpdateArgs,
    RandomTickArgs,
};
use crate::world::World;
use crate::world::feature_placer::FeatureCache;

#[pumpkin_block_from_tag("minecraft:saplings")]
pub struct SaplingBlock;

/// `SaplingBlock.randomTick`'s light gate: the raw brightness directly above the sapling
/// must be at least `GROWTH_LIGHT`.
const fn has_enough_light(light_above: u8) -> bool {
    light_above >= GROWTH_LIGHT
}

/// One entry of vanilla's `TreeGrower` table (`TreeGrower.java:29-69`).
///
/// The trees themselves are configured features, so growing one is the same operation as
/// worldgen placing one - see `crate::world::feature_placer`.
struct TreeGrower {
    secondary_chance: f32,
    mega: Option<ConfiguredFeature>,
    secondary_mega: Option<ConfiguredFeature>,
    tree: Option<ConfiguredFeature>,
    secondary_tree: Option<ConfiguredFeature>,
    flowers: Option<ConfiguredFeature>,
    secondary_flowers: Option<ConfiguredFeature>,
}

impl TreeGrower {
    /// `TreeGrower(name, megaTree, tree, flowers)` (`TreeGrower.java:79-86`).
    const fn simple(
        mega: Option<ConfiguredFeature>,
        tree: Option<ConfiguredFeature>,
        flowers: Option<ConfiguredFeature>,
    ) -> Self {
        Self {
            secondary_chance: 0.0,
            mega,
            secondary_mega: None,
            tree,
            secondary_tree: None,
            flowers,
            secondary_flowers: None,
        }
    }

    /// `TreeGrower.getConfiguredMegaFeature` (`TreeGrower.java:123-125`). Only rolls when a
    /// secondary mega tree exists, which keeps the random stream aligned with vanilla.
    fn mega_feature(&self, random: &mut RandomGenerator) -> Option<ConfiguredFeature> {
        if self.secondary_mega.is_some() && random.next_f32() < self.secondary_chance {
            self.secondary_mega
        } else {
            self.mega
        }
    }

    /// `TreeGrower.getConfiguredFeature` (`TreeGrower.java:109-121`).
    fn feature(
        &self,
        random: &mut RandomGenerator,
        has_flowers: bool,
    ) -> Option<ConfiguredFeature> {
        if random.next_f32() < self.secondary_chance {
            if has_flowers && self.secondary_flowers.is_some() {
                return self.secondary_flowers;
            }
            if self.secondary_tree.is_some() {
                return self.secondary_tree;
            }
        }
        if has_flowers && self.flowers.is_some() {
            self.flowers
        } else {
            self.tree
        }
    }
}

/// The `TreeGrower` each sapling block carries, from its `Blocks.java` registration.
///
/// `mangrove_propagule` is in the saplings tag, but `plant/mangrove_propagule.rs` is registered
/// after this block and wins for it, so it never reaches here. It is left out of the table
/// deliberately: `MangrovePropaguleBlock` grows only when standing and not hanging
/// (`MangrovePropaguleBlock.java:103-113`), and wiring `TreeGrower.MANGROVE` into that state
/// machine belongs in that file.
fn grower_for(block: &Block) -> Option<TreeGrower> {
    Some(match block {
        b if b == &Block::OAK_SAPLING => TreeGrower {
            secondary_chance: 0.1,
            mega: None,
            secondary_mega: None,
            tree: Some(ConfiguredFeature::Oak),
            secondary_tree: Some(ConfiguredFeature::FancyOak),
            flowers: Some(ConfiguredFeature::OakBees005),
            secondary_flowers: Some(ConfiguredFeature::FancyOakBees005),
        },
        b if b == &Block::SPRUCE_SAPLING => TreeGrower {
            secondary_chance: 0.5,
            mega: Some(ConfiguredFeature::MegaSpruce),
            secondary_mega: Some(ConfiguredFeature::MegaPine),
            tree: Some(ConfiguredFeature::Spruce),
            secondary_tree: None,
            flowers: None,
            secondary_flowers: None,
        },
        b if b == &Block::BIRCH_SAPLING => TreeGrower::simple(
            None,
            Some(ConfiguredFeature::Birch),
            Some(ConfiguredFeature::BirchBees005),
        ),
        b if b == &Block::JUNGLE_SAPLING => TreeGrower::simple(
            Some(ConfiguredFeature::MegaJungleTree),
            Some(ConfiguredFeature::JungleTreeNoVine),
            None,
        ),
        b if b == &Block::ACACIA_SAPLING => {
            TreeGrower::simple(None, Some(ConfiguredFeature::Acacia), None)
        }
        b if b == &Block::CHERRY_SAPLING => TreeGrower::simple(
            None,
            Some(ConfiguredFeature::Cherry),
            Some(ConfiguredFeature::CherryBees005),
        ),
        b if b == &Block::DARK_OAK_SAPLING => {
            TreeGrower::simple(Some(ConfiguredFeature::DarkOak), None, None)
        }
        b if b == &Block::PALE_OAK_SAPLING => {
            TreeGrower::simple(Some(ConfiguredFeature::PaleOakBonemeal), None, None)
        }
        // `AzaleaBlock.performBonemeal` (`AzaleaBlock.java:57-60`) grows `TreeGrower.AZALEA`.
        b if b == &Block::AZALEA || b == &Block::FLOWERING_AZALEA => {
            TreeGrower::simple(None, Some(ConfiguredFeature::AzaleaTree), None)
        }
        _ => return None,
    })
}

/// Reads the vanilla `STAGE` property generically, so blocks in the saplings tag that do not
/// carry one (the azaleas) simply report `None` and grow straight away.
fn stage_of(block: &Block, state_id: BlockStateId) -> Option<u8> {
    block.properties(state_id).and_then(|props| {
        props
            .to_props()
            .iter()
            .find(|(key, _)| *key == "stage")
            .and_then(|(_, value)| value.parse::<u8>().ok())
    })
}

fn with_stage(block: &Block, state_id: BlockStateId, stage: &str) -> Option<BlockStateId> {
    let props = block.properties(state_id)?;
    let updated: Vec<(&str, &str)> = props
        .to_props()
        .iter()
        .map(|(key, value)| {
            if *key == "stage" {
                (*key, stage)
            } else {
                (*key, *value)
            }
        })
        .collect();
    Some(block.from_properties(&updated).to_state_id(block))
}

/// `TreeGrower.isTwoByTwoSapling` (`TreeGrower.java:181-187`): compares blocks, not states.
fn is_two_by_two(world: &World, pos: &BlockPos, block: &Block, dx: i32, dz: i32) -> bool {
    [(dx, dz), (dx + 1, dz), (dx, dz + 1), (dx + 1, dz + 1)]
        .into_iter()
        .all(|(ox, oz)| world.get_block(&pos.offset(Vector3::new(ox, 0, oz))) == block)
}

/// `TreeGrower.hasFlowers` (`TreeGrower.java:189-197`): any flower in the 5x3x5 box around the
/// sapling selects the bee-nest variant.
fn has_flowers(world: &World, pos: &BlockPos) -> bool {
    for dx in -2..=2 {
        for dy in -1..=1 {
            for dz in -2..=2 {
                if world
                    .get_block(&pos.offset(Vector3::new(dx, dy, dz)))
                    .has_tag(&tag::Block::MINECRAFT_FLOWERS)
                {
                    return true;
                }
            }
        }
    }
    false
}

/// `TreeGrower.growTree` (`TreeGrower.java:127-179`).
///
/// The sapling is cleared before the feature runs and restored if the feature declines. Here
/// both the clear and the tree are buffered in one [`FeatureCache`], so declining simply means
/// never committing - see `crate::world::feature_placer`.
///
/// Divergence: vanilla replaces the sapling with `getFluidState(pos).createLegacyBlock()`, which
/// only differs from air for a waterlogged propagule, and propagules are not grown here.
async fn grow_tree(world: &Arc<World>, pos: &BlockPos, block: &Block) -> bool {
    let Some(grower) = grower_for(block) else {
        return false;
    };
    let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::rng().random()));
    let air = Block::AIR.default_state.id;

    if let Some(mega) = grower.mega_feature(&mut random) {
        for dx in [0, -1] {
            for dz in [0, -1] {
                if !is_two_by_two(world, pos, block, dx, dz) {
                    continue;
                }
                let mut cache = FeatureCache::new(world);
                for (ox, oz) in [(dx, dz), (dx + 1, dz), (dx, dz + 1), (dx + 1, dz + 1)] {
                    cache.set_block(pos.offset(Vector3::new(ox, 0, oz)), air);
                }
                let base = pos.offset(Vector3::new(dx, 0, dz));
                if cache.place(mega, base, &mut random) {
                    return cache.commit().await;
                }
                return false;
            }
        }
    }

    let Some(feature) = grower.feature(&mut random, has_flowers(world, pos)) else {
        return false;
    };
    let mut cache = FeatureCache::new(world);
    cache.set_block(*pos, air);
    if cache.place(feature, *pos, &mut random) {
        cache.commit().await
    } else {
        false
    }
}

/// `SaplingBlock.advanceTree` (`SaplingBlock.java:51-57`): stage 0 only ticks the stage up; from
/// stage 1 the tree grows.
async fn advance_tree(world: &Arc<World>, pos: &BlockPos, block: &Block, state_id: BlockStateId) {
    if stage_of(block, state_id) == Some(0) {
        if let Some(new_state_id) = with_stage(block, state_id, "1") {
            world
                .set_block_state(pos, new_state_id, BlockFlags::NOTIFY_ALL)
                .await;
        }
        return;
    }
    grow_tree(world, pos, block).await;
}

impl SaplingBlock {
    async fn generate(&self, world: &Arc<World>, pos: &BlockPos) {
        use crate::plugin::api::events::world::structure_grow::{StructureGrowEvent, TreeType};
        let mut event = StructureGrowEvent::new(*pos, TreeType::Oak, false);
        if let Some(server) = world.server.upgrade() {
            server.plugin_manager.fire(&server, &mut event).await;
        }
        if event.cancelled {
            return;
        }

        let (block, state_id) = world.get_block_and_state_id(pos);
        advance_tree(world, pos, block, state_id).await;
    }
}

impl BlockBehaviour for SaplingBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        <Self as PlantBlockBase>::can_place_at(self, args.block_accessor, args.position)
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            <Self as PlantBlockBase>::get_state_for_neighbor_update(
                self,
                args.world,
                args.position,
                args.state_id,
            )
            .await
        })
    }

    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if !has_enough_light(args.world.get_raw_brightness(&args.position.up(), 0)) {
                return;
            }
            if rand::rng().random_range(0..7) != 0 {
                return;
            }
            self.generate(args.world, args.position).await;
        })
    }

    /// `SaplingBlock.isValidBonemealTarget` (`SaplingBlock.java:59-67`) checks that the column up
    /// to the trunk placer's base height is inside the build height. That height is not reachable
    /// from the configured-feature table here, so only the block above is checked; a sapling right
    /// under the build ceiling can therefore consume bone meal for a tree that then fails to
    /// place, which is the conservative direction.
    ///
    /// A tag member with no grower declines, leaving the generic stage-advance in
    /// `item/items/bone_meal.rs` to handle it exactly as before.
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        grower_for(args.block).is_some() && args.world.is_in_build_limit(args.position.up())
    }

    /// `SaplingBlock.isBonemealSuccess` (`SaplingBlock.java:69-72`).
    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        rand::rng().random::<f32>() < BONEMEAL_SUCCESS_CHANCE
    }

    /// `SaplingBlock.performBonemeal` (`SaplingBlock.java:74-77`).
    fn perform_bonemeal<'a>(&'a self, args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            advance_tree(args.world, args.position, args.block, args.state_id).await;
        })
    }
}

impl PlantBlockBase for SaplingBlock {}

#[cfg(test)]
mod tests {
    use super::{GROWTH_LIGHT, grower_for, has_enough_light};
    use pumpkin_data::Block;
    use pumpkin_data::configured_feature::ConfiguredFeature;

    #[test]
    fn blocks_growth_below_threshold() {
        assert!(!has_enough_light(GROWTH_LIGHT - 1));
    }

    #[test]
    fn allows_growth_at_and_above_threshold() {
        assert!(has_enough_light(GROWTH_LIGHT));
        assert!(has_enough_light(15));
    }

    /// Dark oak and pale oak carry only a mega tree (`TreeGrower.java:68-69`), so a lone sapling
    /// legitimately grows nothing.
    #[test]
    fn dark_oak_only_grows_as_a_two_by_two() {
        let grower = grower_for(&Block::DARK_OAK_SAPLING).expect("dark oak has a grower");
        assert!(grower.tree.is_none());
        assert_eq!(grower.mega, Some(ConfiguredFeature::DarkOak));
    }

    /// Mangrove propagules are in the saplings tag but are a separate block class here.
    #[test]
    fn mangrove_propagule_has_no_grower() {
        assert!(grower_for(&Block::MANGROVE_PROPAGULE).is_none());
    }

    /// `AzaleaBlock` has no `.randomTicks()` in its `Blocks.java` registration
    /// (`Blocks.java:5387-5397`): azaleas grow only from bone meal. The tree path here is reached
    /// from `random_tick` too, so it stays correct only while the dispatcher's data flag says the
    /// azaleas are not random-ticked, and the oak-like saplings are.
    #[test]
    fn azaleas_are_not_random_ticked() {
        use pumpkin_data::block_properties::has_random_ticks;
        assert!(!has_random_ticks(Block::AZALEA.default_state.id));
        assert!(!has_random_ticks(Block::FLOWERING_AZALEA.default_state.id));
        assert!(has_random_ticks(Block::OAK_SAPLING.default_state.id));
    }

    /// Every configured feature named by the table must exist, or growth silently does nothing.
    #[test]
    fn every_grower_feature_is_registered() {
        use pumpkin_world::generation::feature::configured_features::CONFIGURED_FEATURES;
        for block in [
            &Block::OAK_SAPLING,
            &Block::SPRUCE_SAPLING,
            &Block::BIRCH_SAPLING,
            &Block::JUNGLE_SAPLING,
            &Block::ACACIA_SAPLING,
            &Block::CHERRY_SAPLING,
            &Block::DARK_OAK_SAPLING,
            &Block::PALE_OAK_SAPLING,
            &Block::AZALEA,
            &Block::FLOWERING_AZALEA,
        ] {
            let grower = grower_for(block).expect("registered sapling");
            for feature in [
                grower.mega,
                grower.secondary_mega,
                grower.tree,
                grower.secondary_tree,
                grower.flowers,
                grower.secondary_flowers,
            ]
            .into_iter()
            .flatten()
            {
                assert!(
                    CONFIGURED_FEATURES.contains_key(&feature),
                    "{feature:?} missing for {}",
                    block.name
                );
            }
        }
    }
}
